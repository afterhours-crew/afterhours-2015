# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#Requires -Version 7.5
param([ValidateSet('Test','Crash','Finish')][string]$Mode='Test',[string]$Directory)
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Afterhours.psm1') -Force
if($Mode -ne 'Test') {
    try {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    try {if([Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Desktop worker must not be elevated.'}}finally{$identity.Dispose()}
    Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs'),(Join-Path $PSScriptRoot 'Session.cs')
    $job=[Afterhours.Windows.SessionJob]::AttachSupervisor()
    $child=[Afterhours.Windows.Child]::Start((Join-Path $PSHOME 'pwsh.exe'),[string[]]@('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60'),$Directory,(Join-Path $Directory 'out'),(Join-Path $Directory 'err'),$null)
    if($child.Exited){throw 'Disposable child exited before test.'}
    @{worker=$PID;worker_created=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToFileTimeUtc();child=$child.Id;child_created=$child.Created}|ConvertTo-Json -Compress|Set-Content -LiteralPath (Join-Path $Directory 'ready.json')
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(30)
    while(-not (Test-Path -LiteralPath (Join-Path $Directory 'go'))){if([DateTimeOffset]::UtcNow -gt $deadline){throw 'Controller did not resume test.'};Start-Sleep -Milliseconds 50}
    if($Mode -eq 'Crash'){[Diagnostics.Process]::GetCurrentProcess().Kill();return}
    $child.Kill()
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(10)
    while(-not $child.Exited){if([DateTimeOffset]::UtcNow -gt $deadline){throw 'Child did not exit.'};Start-Sleep -Milliseconds 50}
    $child.Dispose();$job.Finish()
    [IO.File]::WriteAllText((Join-Path $Directory 'finished'),'verified')
    return
    } catch { [IO.File]::WriteAllText((Join-Path $Directory 'failure'),$_.Exception.ToString()); throw }
}
$temporary=Join-Path ([IO.Path]::GetTempPath()) ('afterhours-session-tests-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $temporary)
try {
    foreach($modeName in @('Crash','Finish')) {
        $directory=Join-Path $temporary "$modeName space å's"
        [void](New-Item -ItemType Directory -Path $directory)
        $command="& '$($PSCommandPath.Replace("'","''"))' -Mode $modeName -Directory '$($directory.Replace("'","''"))'"
        $info=[Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
        $info.UseShellExecute=$false;$info.CreateNoWindow=$true
        foreach($arg in @('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command)))){[void]$info.ArgumentList.Add($arg)}
        $launched=[Diagnostics.Process]::Start($info)
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(30)
        while(-not (Test-Path -LiteralPath (Join-Path $directory 'ready.json'))){if($launched.HasExited){throw (Get-Content -LiteralPath (Join-Path $directory 'failure') -Raw)};if([DateTimeOffset]::UtcNow -gt $deadline){$launched.Kill();throw 'Test worker did not start.'};Start-Sleep -Milliseconds 100}
        $receipt=Get-Content -LiteralPath (Join-Path $directory 'ready.json') -Raw|ConvertFrom-Json
        $worker=[Diagnostics.Process]::GetProcessById($receipt.worker)
        $child=[Diagnostics.Process]::GetProcessById($receipt.child)
        # Pin handles and creation times before the intentional failure.
        if($worker.Handle -eq 0 -or $child.Handle -eq 0 -or $worker.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $receipt.worker_created -or $child.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $receipt.child_created){throw 'Test process identity changed.'}
        try {
            [IO.File]::WriteAllText((Join-Path $directory 'go'),'go')
            if(-not $worker.WaitForExit(15000) -or -not $child.WaitForExit(10000)){throw 'Session job failed to clean its owned process tree.'}
            if($modeName -eq 'Finish' -and -not (Test-Path -LiteralPath (Join-Path $directory 'finished'))){
                if(Test-Path -LiteralPath (Join-Path $directory 'failure')){throw (Get-Content -LiteralPath (Join-Path $directory 'failure') -Raw)}
                throw 'Graceful job disarm failed.'
            }
            Write-Output "$modeName : owned child exited and process handles confirmed cleanup."
        } finally {
            if(-not $worker.HasExited){$worker.Kill();[void]$worker.WaitForExit(10000)}
            $worker.Dispose();$child.Dispose();$launched.Dispose()
        }
    }
} finally {
    $resolved=[IO.Path]::GetFullPath($temporary)
    $tempRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
    if($resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved).StartsWith('afterhours-session-tests-')){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
exit 0
