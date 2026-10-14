// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace Afterhours.Windows
{
    // OS calls stay here; the server and its protocol/state libraries remain
    // safe Rust. No debugger, protection changes, remote code or registry writes.
    internal static class Api
    {
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern SafeProcessHandle OpenProcess(uint access, bool inherit, uint pid);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern SafeWaitHandle OpenThread(uint access, bool inherit, uint tid);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern bool GetProcessTimes(SafeProcessHandle handle, out long created, out long exited, out long kernel, out long user);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] internal static extern bool QueryFullProcessImageName(SafeProcessHandle handle, uint flags, StringBuilder path, ref uint size);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern uint WaitForSingleObject(SafeProcessHandle handle, uint milliseconds);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern bool ReadProcessMemory(SafeProcessHandle handle, IntPtr address, byte[] bytes, UIntPtr size, out UIntPtr read);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern bool WriteProcessMemory(SafeProcessHandle handle, IntPtr address, byte[] bytes, UIntPtr size, out UIntPtr written);
        [DllImport("kernel32.dll", SetLastError = true)] internal static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
        [DllImport("psapi.dll", SetLastError = true)] internal static extern bool EnumProcessModulesEx(SafeProcessHandle process, IntPtr[] modules, uint bytes, out uint needed, uint filter);
        [DllImport("psapi.dll", SetLastError = true)] internal static extern bool GetModuleInformation(SafeProcessHandle process, IntPtr module, out ModuleInfo info, uint size);
        [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)] internal static extern uint GetModuleFileNameEx(SafeProcessHandle process, IntPtr module, StringBuilder path, uint size);
        [DllImport("ntdll.dll")] internal static extern int NtQueryInformationThread(SafeWaitHandle thread, uint kind, out ThreadInfo info, uint size, out uint returned);
        [DllImport("iphlpapi.dll")] internal static extern uint GetExtendedTcpTable(byte[] buffer, ref uint size, bool sorted, uint family, uint kind, uint reserved);
        [StructLayout(LayoutKind.Sequential)] internal struct ModuleInfo { internal IntPtr Base; internal uint Size; internal IntPtr Entry; }
        [StructLayout(LayoutKind.Sequential)] internal struct ThreadInfo { internal int Exit; internal IntPtr Teb, Pid, Tid, Affinity; internal int Priority, BasePriority; }
        internal static void Check(bool valid, string operation) { if (!valid) throw new Win32Exception(Marshal.GetLastWin32Error(), operation); }
        internal static void Require(bool valid, string message) { if (!valid) throw new InvalidOperationException(message); }
    }

    public static class EnvironmentCheck
    {
        public static bool InJob()
        {
            using (var process = Process.GetCurrentProcess()) {
                bool value;
                Api.Check(Api.IsProcessInJob(process.Handle, IntPtr.Zero, out value), "Query process job");
                return value;
            }
        }
    }

    internal sealed class ProcessPin : IDisposable
    {
        internal readonly SafeProcessHandle Handle;
        internal readonly uint Id;
        internal readonly long Created;
        internal readonly string Path;
        internal ProcessPin(uint id, long created, string path, string hash, bool write)
        {
            Api.Require(IntPtr.Size == 8 && id != 0 && created > 0, "Invalid process identity or platform");
            Id = id; Created = created; Path = System.IO.Path.GetFullPath(path);
            Handle = Api.OpenProcess(0x100410u | (write ? 0x28u : 0u), false, id);
            try {
                Api.Check(!Handle.IsInvalid, "Open owned process");
                Verify();
                using (var file = File.OpenRead(Path)) {
                    Api.Require(Convert.ToHexString(SHA256.HashData(file)).Equals(hash, StringComparison.OrdinalIgnoreCase), "Executable digest changed");
                }
            } catch { Handle.Dispose(); throw; }
        }
        internal bool Exited {
            get {
                uint status = Api.WaitForSingleObject(Handle, 0);
                Api.Require(status == 0 || status == 258, "Process wait failed");
                return status == 0;
            }
        }
        internal void Verify()
        {
            Api.Require(!Exited, "Owned process exited");
            long created, exited, kernel, user;
            Api.Check(Api.GetProcessTimes(Handle, out created, out exited, out kernel, out user), "Read process creation time");
            var path = new StringBuilder(32768); uint size = 32768;
            Api.Check(Api.QueryFullProcessImageName(Handle, 0, path, ref size), "Read process image path");
            Api.Require(created == Created && String.Equals(System.IO.Path.GetFullPath(path.ToString()), Path, StringComparison.OrdinalIgnoreCase), "Process lifetime/path changed");
        }
        internal (long, uint) Module()
        {
            var modules = new IntPtr[1]; uint needed;
            Api.Check(Api.EnumProcessModulesEx(Handle, modules, 8, out needed, 3), "Read main module");
            Api.Require(needed >= 8 && modules[0] != IntPtr.Zero, "Main module missing");
            Api.ModuleInfo info;
            Api.Check(Api.GetModuleInformation(Handle, modules[0], out info, (uint)Marshal.SizeOf<Api.ModuleInfo>()), "Read module extent");
            var path = new StringBuilder(32768);
            uint length = Api.GetModuleFileNameEx(Handle, modules[0], path, 32768);
            Api.Require(length > 0 && length < 32768 && String.Equals(System.IO.Path.GetFullPath(path.ToString()), Path, StringComparison.OrdinalIgnoreCase), "Main module path mismatch");
            return (info.Base.ToInt64(), info.Size);
        }
        public void Dispose() { Handle.Dispose(); }
    }

    /// Fixed 13-byte data transaction, also exercised against constructed memory.
    /// Read/check/write cannot make the game's concurrent reads atomic.
    public sealed class EndpointTransaction
    {
        private readonly Func<int, byte[]> read;
        private readonly Action<int, byte[]> write;
        private readonly Action guard;
        private readonly byte[][] original;
        private readonly byte[][] replacement;
        public EndpointTransaction(Func<int, byte[]> read, Action<int, byte[]> write, Action guard, ushort port)
        {
            Api.Require(port >= 1024, "Expected an unprivileged loopback port");
            this.read = read; this.write = write; this.guard = guard;
            original = Current();
            Api.Require(original[0][0] == 0 && original[1][0] == 1 && original[2].All(b => b == 0), "Endpoint is not at its startup defaults");
            replacement = new[] { Encoding.ASCII.GetBytes("127.0.0.1\0"), new byte[] {0}, BitConverter.GetBytes(port) };
        }
        private byte[][] Current()
        {
            guard(); var state = new[] { read(0), read(1), read(2) };
            Api.Require(state[0].Length == 10 && state[1].Length == 1 && state[2].Length == 2, "Partial endpoint read");
            return state;
        }
        private static bool Equal(byte[][] a, byte[][] b) { return Enumerable.Range(0, 3).All(i => a[i].SequenceEqual(b[i])); }
        private byte[][] Change(int field, byte[] value, byte[][] expected)
        {
            Api.Require(Equal(Current(), expected), "Endpoint changed before write");
            write(field, (byte[])value.Clone());
            var next = expected.Select(b => (byte[])b.Clone()).ToArray(); next[field] = (byte[])value.Clone();
            Api.Require(Equal(Current(), next), "Endpoint write did not verify"); return next;
        }
        public void Apply()
        {
            var current = Current(); Api.Require(Equal(current, original), "Startup bytes changed");
            foreach (int field in new[] {2, 1, 0}) current = Change(field, replacement[field], current);
        }
        public void Restore()
        {
            var current = Current();
            Api.Require(Enumerable.Range(0, 3).All(i => Enumerable.Range(0, current[i].Length)
                .All(j => current[i][j] == original[i][j] || current[i][j] == replacement[i][j])), "Unrelated endpoint change; refusing overwrite");
            foreach (int field in new[] {0, 1, 2}) if (!current[field].SequenceEqual(original[field])) current = Change(field, original[field], current);
            Api.Require(Equal(Current(), original), "Endpoint rollback did not verify");
        }
        public bool IsOriginal() { return Equal(Current(), original); }
    }

    /// One game lifetime, one root thread, one local server. No arbitrary-address
    /// write API is exposed. This object must be held by the launch supervisor.
    public sealed class EndpointLease : IDisposable
    {
        private const string Build = "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df";
        private readonly ProcessPin game, server;
        private readonly ushort port;
        private SafeWaitHandle thread;
        private long teb, tid, root, provider, moduleBase;
        private uint moduleSize, tlsIndex;
        private EndpointTransaction transaction;
        private bool touched, applying;
        private readonly Stopwatch budget = new Stopwatch();
        private int reads, bytesRead;
        private static readonly int[] Offsets = {0x546, 0x646, 0x648};
        private static readonly int[] Sizes = {10, 1, 2};
        private static readonly (int, int, string)[] Code = {
            (0x03ecca30,37,"9814166c5e8c200613a6239d40707f02ca649953fcb5abd749457e949d902a12"),
            (0x04363fd0,33,"cb794f9ae8dc1b3074eb76b8a56f67fef316e315a7ddff9343d997b6e2c0394f"),
            (0x04c43ab0,24,"dfe377bcb9eac63d66e12ef5a02dce32e24847de0bae004706aebbf6cccd49eb")};

        public EndpointLease(uint gameId, long gameCreated, string gamePath, uint serverId, long serverCreated, string serverPath, string serverHash, ushort port)
        {
            Api.Require(port >= 1024, "Unexpected local port"); this.port = port;
            game = new ProcessPin(gameId, gameCreated, gamePath, Build, true);
            try { server = new ProcessPin(serverId, serverCreated, serverPath, serverHash, false); }
            catch { game.Dispose(); throw; }
        }
        private void ResetBudget() { budget.Restart(); reads = 0; bytesRead = 0; }
        private byte[] Read(long address, int count)
        {
            Api.Require(address >= 0x10000 && address < 0x00007fffffff0000 && count > 0 && count <= 256
                && address + count <= 0x00007fffffff0000 && reads++ < 1500 && (bytesRead += count) <= 32768 && budget.Elapsed.TotalSeconds < 5, "Memory observation exceeds bounds");
            var bytes = new byte[count]; UIntPtr read;
            Api.Check(Api.ReadProcessMemory(game.Handle, new IntPtr(address), bytes, (UIntPtr)count, out read), "Read startup data");
            Api.Require(read.ToUInt64() == (ulong)count, "Partial startup read"); return bytes;
        }
        private long Q(long address) { return BitConverter.ToInt64(Read(address, 8)); }
        private uint U(long address) { return BitConverter.ToUInt32(Read(address, 4)); }
        private Api.ThreadInfo ThreadInfo(SafeWaitHandle handle)
        {
            Api.ThreadInfo value; uint returned;
            Api.Require(Marshal.SizeOf<Api.ThreadInfo>() == 48, "Unsupported thread ABI");
            int status = Api.NtQueryInformationThread(handle, 0, out value, 48, out returned);
            Api.Require(status == 0 && returned == 48 && value.Exit == 259 && value.Pid.ToInt64() == game.Id, "Root thread exited or changed"); return value;
        }
        private void CheckCode()
        {
            foreach (var item in Code) Api.Require(Convert.ToHexString(SHA256.HashData(Read(moduleBase + item.Item1, item.Item2))).Equals(item.Item3, StringComparison.OrdinalIgnoreCase), "Unsupported live code guard");
        }
        private long ThreadRoot(Api.ThreadInfo info)
        {
            long address = info.Teb.ToInt64();
            Api.Require(Q(address + 0x30) == address && Q(address + 0x40) == game.Id && Q(address + 0x48) == info.Tid.ToInt64(), "Thread environment identity mismatch");
            long array = Q(address + 0x58); if (array == 0) return 0;
            long block = Q(array + tlsIndex * 8); return block == 0 ? 0 : Q(block + 0x290);
        }
        /// Call during startup before the first backend request. A false result
        /// means the guarded startup root is not ready; it never authorizes input.
        public bool Prepare()
        {
            Api.Require(transaction == null && thread == null, "Lease already prepared");
            game.Verify(); server.Verify(); ResetBudget();
            (moduleBase, moduleSize) = game.Module();
            Api.Require(moduleBase >= 0x10000 && moduleSize > 0x04c43ab0 + 24 && moduleSize <= 0x10000000, "Unsupported module extent");
            CheckCode(); tlsIndex = U(moduleBase + 0x02c9e2b4); Api.Require(tlsIndex < 64, "Unsupported TLS index");
            SafeWaitHandle selected = null; long found = 0;
            try {
                using (var process = Process.GetProcessById((int)game.Id)) {
                    Api.Require(process.StartTime.ToUniversalTime().ToFileTimeUtc() == game.Created && process.Threads.Count <= 128, "Thread inventory lifetime/bound");
                    foreach (ProcessThread entry in process.Threads) {
                        var candidate = Api.OpenThread(0x40, false, (uint)entry.Id);
                        try {
                            Api.Check(!candidate.IsInvalid, "Open game thread");
                            var info = ThreadInfo(candidate); long value = ThreadRoot(info);
                            if (value == 0) continue;
                            Api.Require(found == 0 || found == value, "Conflicting startup roots"); found = value;
                            if (selected == null) { selected = candidate; candidate = null; teb = info.Teb.ToInt64(); tid = info.Tid.ToInt64(); }
                        } finally { if (candidate != null) candidate.Dispose(); }
                    }
                }
                if (selected == null) return false;
                root = found; provider = Q(root + 0x750); thread = selected; selected = null;
                Guard();
                long map = Q(provider + 0xd48); long begin = Q(map + 0x18), end = Q(map + 0x20), capacity = Q(map + 0x28);
                Api.Require(begin >= 0 && begin == end && (begin != 0 || capacity == 0) && capacity >= end && (capacity - begin) % 48 == 0 && (capacity - begin) / 48 <= 4096 && capacity < 0x00007fffffff0000, "Backend startup has already begun or configuration is invalid");
                transaction = new EndpointTransaction(i => Read(root + Offsets[i], Sizes[i]), Write, Guard, port);
                CheckListener(); return true;
            } catch { if (thread != null) { thread.Dispose(); thread = null; } transaction = null; throw; }
            finally { if (selected != null) selected.Dispose(); }
        }
        private void Guard()
        {
            game.Verify(); ResetBudget();
            Api.Require(game.Module() == (moduleBase, moduleSize), "Main module changed"); CheckCode();
            var info = ThreadInfo(thread);
            Api.Require(info.Teb.ToInt64() == teb && info.Tid.ToInt64() == tid && U(moduleBase + 0x02c9e2b4) == tlsIndex && ThreadRoot(info) == root, "Startup root changed");
            Api.Require(Q(root) == moduleBase + 0x02394fb8 && Q(root + 8) == moduleBase + 0x02394fd8 && Q(root + 0x750) == provider
                && Q(provider) == moduleBase + 0x023db740 && Q(provider + 8) == root && U(root + 0x528) == 0 && U(root + 0x530) == 3, "Startup object identity changed");
        }
        private void CheckListener()
        {
            server.Verify(); uint size = 0;
            uint code = Api.GetExtendedTcpTable(null, ref size, false, 2, 3, 0);
            Api.Require(code == 122 && size <= 1 << 20 && size >= 4, "TCP ownership table unavailable");
            for (int attempt = 0; attempt < 3; attempt++) {
                var table = new byte[size]; code = Api.GetExtendedTcpTable(table, ref size, false, 2, 3, 0);
                if (code == 122) { Api.Require(size <= 1 << 20, "TCP table bound"); continue; }
                Api.Require(code == 0, "TCP ownership query failed");
                uint count = BitConverter.ToUInt32(table, 0); Api.Require(count <= 10000 && 4L + count * 24L <= table.Length, "TCP table length mismatch");
                for (int i = 0; i < count; i++) {
                    int p = 4 + i * 24;
                    if (BitConverter.ToUInt32(table, p) == 2 && table[p+4] == 127 && table[p+5] == 0 && table[p+6] == 0 && table[p+7] == 1
                        && (table[p+8] * 256 + table[p+9]) == port && BitConverter.ToUInt32(table, p+20) == server.Id) return;
                }
                throw new InvalidOperationException("Expected server no longer owns the loopback listener");
            }
            throw new InvalidOperationException("TCP table changed repeatedly");
        }
        private void Write(int field, byte[] value)
        {
            Api.Require(field >= 0 && field < 3 && value.Length == Sizes[field], "Endpoint write extent mismatch");
            if (applying) CheckListener();
            UIntPtr count;
            Api.Check(Api.WriteProcessMemory(game.Handle, new IntPtr(root + Offsets[field]), value, (UIntPtr)value.Length, out count), "Write local endpoint");
            Api.Require(count.ToUInt64() == (ulong)value.Length, "Partial endpoint write");
        }
        public void Apply()
        {
            Api.Require(transaction != null && !touched, "Lease not prepared or already applied");
            CheckListener(); touched = true;
            try { applying = true; transaction.Apply(); }
            catch { applying = false; Restore(); throw; }
            finally { applying = false; }
        }
        public void Restore()
        {
            if (!touched) return;
            if (!game.Exited) { transaction.Restore(); Api.Require(transaction.IsOriginal(), "Independent rollback check failed"); }
            touched = false;
        }
        /// Shutdown can retire the root thread before the process handle signals.
        /// Only the held process handle proves that the temporary address space
        /// is gone. Never chase a replacement thread during process teardown.
        public void CompleteAfterExit()
        {
            Api.Require(Api.WaitForSingleObject(game.Handle, 10000) == 0, "Game lifetime has not ended");
            touched = false;
        }
        public void Dispose()
        {
            try { Restore(); }
            finally { if (thread != null) thread.Dispose(); server.Dispose(); game.Dispose(); }
        }
    }
}
