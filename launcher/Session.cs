// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Afterhours.Windows
{
    /// Owned only by the dedicated supervisor, never by an interactive shell.
    /// Children inherit membership atomically when Windows creates them. The
    /// non-inheritable job handle disappears on forced supervisor termination.
    public sealed class SessionJob
    {
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] private static extern SafeFileHandle CreateJobObject(IntPtr security, string name);
        [DllImport("kernel32.dll", SetLastError=true)] private static extern bool SetInformationJobObject(SafeFileHandle job, int infoClass, byte[] info, uint size);
        [DllImport("kernel32.dll", SetLastError=true)] private static extern bool QueryInformationJobObject(SafeFileHandle job, int infoClass, byte[] info, uint size, out uint returned);
        [DllImport("kernel32.dll", SetLastError=true)] private static extern bool AssignProcessToJobObject(SafeFileHandle job, IntPtr process);
        private readonly SafeFileHandle handle;
        private SessionJob(SafeFileHandle value) { handle=value; }
        public static SessionJob AttachSupervisor()
        {
            Api.Require(IntPtr.Size==8 && !EnvironmentCheck.InJob(), "Run the script from an ordinary PowerShell terminal, outside an IDE or agent process job");
            var job=CreateJobObject(IntPtr.Zero,null);
            try {
                Api.Check(!job.IsInvalid,"Create session job");
                // x64 JOBOBJECT_EXTENDED_LIMIT_INFORMATION: LimitFlags at 16.
                var limits=new byte[144]; BitConverter.GetBytes(0x2000u).CopyTo(limits,16);
                Api.Check(SetInformationJobObject(job,9,limits,144),"Set session crash cleanup");
                using(var process=Process.GetCurrentProcess()) Api.Check(AssignProcessToJobObject(job,process.Handle),"Own supervisor process tree");
                return new SessionJob(job);
            } catch { job.Dispose(); throw; }
        }
        public bool HasOwnedWindow(string executable)
        {
            // Observe only an installed helper in this session's own job. Never
            // interact with its authentication UI or adopt an unrelated process.
            foreach(var process in Process.GetProcessesByName(Path.GetFileNameWithoutExtension(executable))) {
                using(process) try {
                    bool owned;
                    Api.Check(Api.IsProcessInJob(process.Handle,handle.DangerousGetHandle(),out owned),"Query owned helper");
                    if(owned && process.MainWindowHandle!=IntPtr.Zero &&
                       String.Equals(Path.GetFullPath(process.MainModule.FileName),Path.GetFullPath(executable),StringComparison.OrdinalIgnoreCase)) return true;
                } catch(InvalidOperationException) { } // The helper may exit during observation.
                  catch(System.ComponentModel.Win32Exception) { }
            }
            return false;
        }
        /// Disarm only after Windows reports that every child has exited. On
        /// error retain the armed handle; process exit performs crash cleanup.
        public void Finish()
        {
            var ids=new byte[8+128*8]; uint returned;
            // A signalled process handle can precede removal from the job's
            // accounting list. Keep the job armed throughout that short tail.
            var deadline=Stopwatch.StartNew();
            while(true) {
                Api.Check(QueryInformationJobObject(handle,3,ids,(uint)ids.Length,out returned),"Verify session children exited");
                uint count=BitConverter.ToUInt32(ids,4);
                if(count==1 && BitConverter.ToUInt64(ids,8)==(ulong)Environment.ProcessId) break;
                Api.Require(deadline.Elapsed < TimeSpan.FromSeconds(5),"Session still has live children");
                System.Threading.Thread.Sleep(20);
            }
            Api.Check(SetInformationJobObject(handle,9,new byte[144],144),"Finish session job");
            handle.Dispose();
        }
    }

    public sealed class Child : IDisposable
    {
        private readonly Process process;
        private readonly Task output, errors;
        private readonly TaskCompletionSource<string> first = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        private bool outputTruncated;
        private readonly ConcurrentQueue<string> diagnostics = new ConcurrentQueue<string>();
        private int diagnosticsDropped;
        public int Id => process.Id;
        public long Created => process.StartTime.ToUniversalTime().ToFileTimeUtc();
        public bool Exited => process.HasExited;
        public int ExitCode => process.ExitCode;
        public Task<string> FirstLine => first.Task;
        public bool OutputTruncated => outputTruncated;
        public bool LogsFinished => output.IsCompleted && errors.IsCompleted;
        public string ReadDiagnostics()
        {
            var text = new StringBuilder();
            if(System.Threading.Interlocked.Exchange(ref diagnosticsDropped,0)!=0)
                text.AppendLine("[Afterhours: console output skipped while busy; inspect the bounded session log.]");
            for(int i=0;i<16 && diagnostics.TryDequeue(out var chunk);i++) text.Append(chunk);
            return text.ToString();
        }
        public void CheckDiagnostics()
        {
            if(output.IsFaulted || errors.IsFaulted)
                throw new IOException("Could not drain child diagnostics", (output.Exception ?? errors.Exception).GetBaseException());
        }
        private Child(Process value,string outPath,string errPath)
        {
            process=value;
            output=Pump(process.StandardOutput,outPath,true);
            errors=Pump(process.StandardError,errPath,false);
            output.ContinueWith(t => first.TrySetException(t.Exception.GetBaseException()),TaskContinuationOptions.OnlyOnFaulted);
        }
        private async Task Pump(StreamReader reader,string path,bool captureFirst)
        {
            const int MaxChars=1024*1024;
            using(var file=new FileStream(path,FileMode.CreateNew,FileAccess.Write,FileShare.Read))
            using(var writer=new StreamWriter(file,new UTF8Encoding(false))) {
                int kept=0; var line=new StringBuilder(); bool found=!captureFirst;
                var buffer=new char[4096]; int count;
                while((count=await reader.ReadAsync(buffer,0,buffer.Length).ConfigureAwait(false))>0) {
                    // Console speed must not block pipe draining or retain an
                    // unbounded backlog. Only diagnostics, never JSON payloads.
                    if(!captureFirst) {
                        if(diagnostics.Count<128) diagnostics.Enqueue(new string(buffer,0,count));
                        else System.Threading.Interlocked.Exchange(ref diagnosticsDropped,1);
                    }
                    if(!found) for(int i=0;i<count;i++) {
                        if(buffer[i]=='\n') { first.TrySetResult(line.ToString().TrimEnd('\r')); found=true; break; }
                        if(line.Length>=65536) {first.TrySetException(new InvalidOperationException("Readiness line exceeded its bound"));found=true;break;}
                        line.Append(buffer[i]);
                    }
                    int write=Math.Min(count,MaxChars-kept);
                    if(write>0) {await writer.WriteAsync(buffer,0,write).ConfigureAwait(false);kept+=write;await writer.FlushAsync().ConfigureAwait(false);}
                    if(write<count && captureFirst) outputTruncated=true;
                    // Keep draining after the limit so diagnostics cannot block traffic.
                }
                if(!found) first.TrySetException(new InvalidOperationException("Process exited without readiness output"));
            }
        }
        public static Child Start(string executable,string[] arguments,string directory,string stdout,string stderr,string marker)
        {
            var info=new ProcessStartInfo(executable){WorkingDirectory=directory,UseShellExecute=false,CreateNoWindow=true,RedirectStandardOutput=true,RedirectStandardError=true,
                StandardOutputEncoding=new UTF8Encoding(false,true),StandardErrorEncoding=new UTF8Encoding(false,true)};
            foreach(var argument in arguments) info.ArgumentList.Add(argument);
            if(marker!=null) info.Environment["EARtPLaunchCode"]=marker;
            var process=new Process{StartInfo=info}; bool started=false;
            try {started=process.Start();Api.Require(started,"Process failed to start");return new Child(process,stdout,stderr);}
            catch {if(started && !process.HasExited) process.Kill(true);process.Dispose();throw;}
        }
        public bool CloseWindow() { return process.CloseMainWindow(); }
        public void Kill() { if(!process.HasExited)process.Kill(true); }
        public void Dispose() { process.Dispose(); }
    }
}
