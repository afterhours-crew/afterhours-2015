# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#Requires -Version 7.5
[CmdletBinding()]
param(
    [ValidateSet('Check','Setup','Start','Status','Stop','Recover')][string]$Action='Start',
    [string]$GameDirectory,
    [string]$DataDirectory=(Join-Path $env:LOCALAPPDATA 'Afterhours2015'),
    [string]$ServerPath=(Join-Path $PSScriptRoot '../target/release/nfs-server.exe'),
    [string]$PlayerName='Driver',
    [string]$AccountId,
    [switch]$RecordProtocol
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Afterhours.psm1') -Force
$data=[IO.Path]::GetFullPath($DataDirectory)
if($Action -eq 'Recover') {
    # Restore only the small launcher pointer; never create/import/reset a DB.
    if($AccountId -notmatch '^[a-f0-9]{32}$'){throw 'Recovery requires -AccountId: the 32-character ID in the successful create-account output or saved SQLite filename.'}
    Assert-AfterhoursDataTree $data (Join-Path $data '__not_a_game_installation__')
    $lock=[IO.File]::Open((Join-Path $data 'launcher.lock'),[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try {
        $database=Get-Item -LiteralPath (Join-Path $data "artifacts/account/$AccountId.sqlite")
        if($database.PSIsContainer -or $database.Length -eq 0 -or $database.Length -gt 64MB){throw 'Account database is absent or exceeds its bound.'}
        $target=Join-Path $data 'account.json'
        $record=@{version=1;local_account=$AccountId;progression='post-intro'}|ConvertTo-Json -Compress
        if(Test-Path -LiteralPath $target){
            $existing=Read-AfterhoursJson $target
            if($existing.version -ne 1 -or $existing.local_account -ne $AccountId){throw 'An existing account record will not be replaced.'}
        } else {
            $file=[IO.File]::Open($target,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
            try{$bytes=[Text.Encoding]::UTF8.GetBytes($record);$file.Write($bytes);$file.Flush($true)}finally{$file.Dispose()}
        }
        Write-Output 'Account pointer recovered. The server validates database ownership and schema on next start; no account data was modified.'
    }finally{$lock.Dispose()}
    exit
}
if($Action -in @('Status','Stop')) {
    $session=Get-AfterhoursSessionStatus $data
    if(-not $session){Write-Output 'No Afterhours session has been started.';exit}
    $run=Join-Path $data "artifacts/launcher/$($session.id)"
    if($Action -eq 'Status') {$session|ConvertTo-Json -Compress;exit}
    if($session.state -in @('finished','failed','interrupted')){Write-Output $session.message;exit}
    # A stop request never kills an arbitrary PID. The live supervisor holding
    # the per-directory lock consumes this file and owns all process handles.
    [void](Assert-AfterhoursDataPath $run (Join-Path $data '__not_a_game_installation__'))
    $stop=Join-Path $run 'stop'
    if(-not (Test-Path -LiteralPath $stop)){$file=[IO.File]::Open($stop,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read);$file.Dispose()}
    Write-Output 'Requested game shutdown, followed by server shutdown.'
    exit
}
if(-not $GameDirectory){throw 'Pass -GameDirectory with the installed Need for Speed directory.'}
$preflight=Get-AfterhoursPreflight $GameDirectory $data $ServerPath
if($Action -eq 'Check'){$preflight;exit}
[void](New-Item -ItemType Directory -Path $data -Force)
$lock=$null
try {
    $lock=[IO.File]::Open((Join-Path $data 'launcher.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    $id=[Guid]::NewGuid().ToString('N')
    $run=Join-Path $data "artifacts/launcher/$id"
    [void](Assert-AfterhoursDataPath $run $preflight.game_directory)
    [void](New-Item -ItemType Directory -Path $run)
    [void](Assert-AfterhoursDataPath $run $preflight.game_directory)
    $request=Join-Path $run 'request.json'
    [IO.File]::WriteAllText($request,(@{version=1;id=$id;action=$Action;game_directory=$preflight.game_directory;data_directory=$data;server_path=$preflight.server_path;server_sha256=$preflight.server_sha256;player_name=$PlayerName;record_protocol=[bool]$RecordProtocol} | ConvertTo-Json -Compress))
    # Keep the session in a dedicated foreground PowerShell child, so the
    # interactive shell stays usable after this session's job is finished.
    $worker=(Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'Supervise.ps1')).ProviderPath
    $lock.Dispose();$lock=$null
    Write-Host "Afterhours $Action — data: $data"
    Write-Host "Session logs: $run"
    & (Join-Path $PSHOME 'pwsh.exe') -NoLogo -NoProfile -NonInteractive -File $worker -Request $request
    if($LASTEXITCODE -ne 0){throw "Afterhours session failed. Inspect $run."}
} finally {if($lock){$lock.Dispose()}}
