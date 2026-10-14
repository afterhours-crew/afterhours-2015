# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#Requires -Version 7.5
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'Afterhours.psm1') -Force
Add-Type -Path @((Join-Path $PSScriptRoot 'Native.cs'), (Join-Path $PSScriptRoot 'Session.cs'), (Join-Path $PSScriptRoot 'Native.Tests.cs'))
[Afterhours.Windows.Tests.Transactions]::Run()
function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Refuses([scriptblock]$Action) { try { & $Action | Out-Null } catch { return }; throw 'Expected refusal.' }
$sample = [DateTimeOffset]::Parse('2026-10-13T12:00:00Z')
Assert ((Get-AfterhoursLaunchMarker $sample) -eq '1497848135') 'Supported UTC marker vector'
Assert ((Get-AfterhoursLaunchMarker ([DateTimeOffset]::Parse('2026-10-14T01:00:00+02:00'))) -eq '1497848135') 'Marker must use UTC, not wall date'
Assert ((Get-AfterhoursLaunchMarker ([DateTimeOffset]::Parse('2026-10-14T12:00:00Z'))) -eq '14876899') 'Next UTC day marker vector'
Assert ((Get-AfterhoursCompatibilityName 'C:\Program Files\Electronic Arts\EADesktop.exe') -eq 'EADesktopDebug.exe') 'EA compatibility name'
Assert ((Get-AfterhoursCompatibilityName 'D:\Origin\Origin.exe') -eq 'OriginDebug.exe') 'Origin compatibility name'
Refuses { Get-AfterhoursCompatibilityName 'EADesktop.exe' }
Refuses { Get-AfterhoursCompatibilityName 'C:\Windows\System32\cmd.exe' }
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('afterhours-launcher-tests-' + [Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $temporary)
try {
    $game = Join-Path $temporary 'game'
    $data = Join-Path $temporary 'data space å'
    [void](New-Item -ItemType Directory -Path $game)
    Refuses { Assert-AfterhoursDataPath $game $game }
    Refuses { Assert-AfterhoursDataPath (Join-Path $game 'saves') $game }
    Assert ((Assert-AfterhoursDataPath $data $game) -eq $data) 'Path with spaces/non-ASCII'
    $source = Join-Path $temporary 'server.exe'
    [IO.File]::WriteAllBytes($source, [byte[]]@(1,2,3,4))
    $preflight = [pscustomobject]@{data_directory=$data;game_directory=$game;server_path=$source;server_sha256=(Get-FileHash $source).Hash.ToLowerInvariant();registered_client_path='C:\EA\EADesktop.exe'}
    $copy = Install-AfterhoursCompatibilityServer $preflight
    Assert ($copy -eq (Install-AfterhoursCompatibilityServer $preflight)) 'Repeat preparation'
    [IO.File]::WriteAllBytes($copy, [byte[]]@(9))
    Refuses { Install-AfterhoursCompatibilityServer $preflight }
    Assert (([IO.File]::ReadAllBytes($copy))[0] -eq 9) 'Never overwrite a changed existing file'
    [IO.File]::WriteAllText((Join-Path $temporary 'ordinary-file'), 'sentinel')
    Refuses { Assert-AfterhoursDataPath (Join-Path $temporary 'ordinary-file/child') $game }
    Refuses { Assert-AfterhoursDataPath ([IO.Path]::GetPathRoot($data)) $game }
    $junction=Join-Path $data 'linked-content'
    [void](New-Item -ItemType Junction -Path $junction -Target $game)
    try { Refuses { Assert-AfterhoursDataTree $data $game } }
    finally { Remove-Item -LiteralPath $junction -Force }
    $id=[Guid]::NewGuid().ToString('N')
    $run=Join-Path $data "artifacts/launcher/$id"
    [void](New-Item -ItemType Directory -Path $run -Force)
    @{version=1;id=$id}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $data 'current-session.json')
    @{version=1;id=$id;state='ready';message='test'}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $run 'status.json')
    $lockPath=Join-Path $data 'launcher.lock'
    $lock=[IO.File]::Open($lockPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try { Assert ((Get-AfterhoursSessionStatus $data).state -eq 'ready') 'Active supervisor lock must preserve ready state' } finally {$lock.Dispose()}
    Assert ((Get-AfterhoursSessionStatus $data).state -eq 'interrupted') 'Stale status must not claim a live session'
    $marker=Get-AfterhoursLaunchMarker ([DateTimeOffset]::Parse('2026-10-14T12:00:00Z'))
    $markerChild=[Afterhours.Windows.Child]::Start((Join-Path $PSHOME 'pwsh.exe'),[string[]]@('-NoProfile','-NonInteractive','-Command','[Console]::WriteLine([Environment]::GetEnvironmentVariable("EARtPLaunchCode"))'),$data,(Join-Path $data 'marker.stdout'),(Join-Path $data 'marker.stderr'),$marker)
    try {
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(10)
        while(-not $markerChild.Exited -or -not $markerChild.LogsFinished){if([DateTimeOffset]::UtcNow -gt $deadline){$markerChild.Kill();throw 'Marker child timeout'};Start-Sleep -Milliseconds 50}
        Assert ($markerChild.FirstLine.GetAwaiter().GetResult() -eq $marker) 'Exact marker must reach the child process environment'
        $markerChild.CheckDiagnostics()
    }finally{$markerChild.Dispose()}
    @{version=1;id='../outside'}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $data 'current-session.json')
    Refuses { Get-AfterhoursSessionStatus $data }
    $stdout=Join-Path $data 'utf8.stdout.log'
    $stderr=Join-Path $data 'utf8.stderr.log'
    $command="[Console]::OutputEncoding=[Text.UTF8Encoding]::new(`$false); [Console]::WriteLine('å-path'); [Console]::Write([string]::new('x',1048600)); [Console]::Error.WriteLine('visible-diagnostic')"
    $child=[Afterhours.Windows.Child]::Start((Join-Path $PSHOME 'pwsh.exe'),[string[]]@('-NoProfile','-NonInteractive','-Command',$command),$data,$stdout,$stderr,$null)
    try {
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(20)
        while(-not $child.Exited -or -not $child.LogsFinished){if([DateTimeOffset]::UtcNow -gt $deadline){$child.Kill();throw 'Child output test timeout'};Start-Sleep -Milliseconds 50}
        Assert ($child.FirstLine.GetAwaiter().GetResult() -eq 'å-path') 'UTF-8 must survive redirected Windows stdout'
        Assert ($child.ExitCode -eq 0 -and $child.OutputTruncated) 'Bounded sink must drain the child without blocking it'
        Assert ($child.ReadDiagnostics() -like '*visible-diagnostic*') 'Console diagnostics available without exposing stdout payloads'
        Assert ((Get-Item -LiteralPath $stdout).Length -le 4*1024*1024) 'Diagnostic sink byte bound'
        $child.CheckDiagnostics()
    } finally {$child.Dispose()}
    # A failed sink must become an actionable error rather than silent success.
    $child=[Afterhours.Windows.Child]::Start((Join-Path $PSHOME 'pwsh.exe'),[string[]]@('-NoProfile','-NonInteractive','-Command',"[Console]::WriteLine('test')"),$data,$stdout,(Join-Path $data 'failure.stderr.log'),$null)
    try {
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(10)
        while(-not $child.Exited -or -not $child.LogsFinished){if([DateTimeOffset]::UtcNow -gt $deadline){$child.Kill();throw 'Failed-sink test timeout'};Start-Sleep -Milliseconds 50}
        Refuses { $child.CheckDiagnostics() }
    }finally{$child.Dispose()}
    # Recovery only publishes a pointer. The real server validates SQLite later.
    $accountId='0123456789abcdef0123456789abcdef'
    $accountDir=Join-Path $data 'artifacts/account'
    [void](New-Item -ItemType Directory -Path $accountDir)
    $database=Join-Path $accountDir "$accountId.sqlite"
    [IO.File]::WriteAllText($database,'preserved-test-database')
    $before=(Get-FileHash -LiteralPath $database).Hash
    $launcher=Join-Path $PSScriptRoot 'Start-Afterhours.ps1'
    foreach($attempt in 1..2){
        & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -NonInteractive -File $launcher -Action Recover -DataDirectory $data -AccountId $accountId | Out-Null
        Assert ($LASTEXITCODE -eq 0) 'Recovery and idempotent retry'
    }
    Assert ((Read-AfterhoursJson (Join-Path $data 'account.json')).local_account -eq $accountId) 'Recovered pointer identity'
    Assert ((Get-FileHash -LiteralPath $database).Hash -eq $before) 'Recovery must not modify the database'
    $otherId='fedcba9876543210fedcba9876543210'
    [IO.File]::WriteAllText((Join-Path $accountDir "$otherId.sqlite"),'another-test-database')
    & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -NonInteractive -File $launcher -Action Recover -DataDirectory $data -AccountId $otherId *> (Join-Path $data 'recovery-refusal.log')
    Assert ($LASTEXITCODE -ne 0) 'Recovery must refuse replacing an existing different account'
    Assert ((Read-AfterhoursJson (Join-Path $data 'account.json')).local_account -eq $accountId) 'Existing pointer survived refusal'
} finally {
    $resolved = [IO.Path]::GetFullPath($temporary)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved).StartsWith('afterhours-launcher-tests-')) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
Write-Output 'Launcher transaction, partial-write recovery, marker, naming, path and copy-integrity tests passed.'
exit 0
