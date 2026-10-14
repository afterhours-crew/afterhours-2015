# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#Requires -Version 7.5
param([Parameter(Mandatory)][string]$Request)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Afterhours.psm1') -Force
Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs'),(Join-Path $PSScriptRoot 'Session.cs')
$requestData=Read-AfterhoursJson $Request
if($requestData.version -ne 1 -or $requestData.id -notmatch '^[a-f0-9]{32}$' -or $requestData.action -notin @('Start','Setup')) {throw 'Invalid launch request.'}
$data=Assert-AfterhoursDataPath $requestData.data_directory $requestData.game_directory
$run=Join-Path $data "artifacts/launcher/$($requestData.id)"
if([IO.Path]::GetFullPath($Request) -ne (Join-Path $run 'request.json')){throw 'Launch request directory mismatch.'}
[void](Assert-AfterhoursDataPath $run $requestData.game_directory)
$lock=$null;$job=$null;$server=$null;$game=$null;$lease=$null;$failure=$null
$gameId=$null;$serverId=$null
function Set-SessionStatus([string]$State,[string]$Message) {
    Write-Host "[$([DateTimeOffset]::UtcNow.ToString('HH:mm:ss'))] $State : $Message"
    $record=@{version=1;id=$requestData.id;utc=[DateTimeOffset]::UtcNow.ToString('o');state=$State;message=$Message;worker_pid=$PID;game_pid=$gameId;server_pid=$serverId}
    $temporary=Join-Path $run 'status.tmp'
    [IO.File]::WriteAllText($temporary,($record|ConvertTo-Json -Compress))
    [IO.File]::Move($temporary,(Join-Path $run 'status.json'),$true)
}
function Show-Diagnostics($Child) {
    if($Child){$text=$Child.ReadDiagnostics();if($text){Write-Host -NoNewline $text}}
}
function Invoke-Preparation([string]$Name,[string[]]$Arguments) {
    $stdout=Join-Path $run "$Name.stdout.json"
    $child=[Afterhours.Windows.Child]::Start($preflight.server_path,$Arguments,$data,$stdout,(Join-Path $run "$Name.stderr.log"),$null)
    try {
        $deadline=[DateTimeOffset]::UtcNow.AddMinutes(10)
        while(-not $child.Exited -or -not $child.LogsFinished){
            Show-Diagnostics $child
            $child.CheckDiagnostics()
            if([DateTimeOffset]::UtcNow -gt $deadline -or (Test-Path -LiteralPath (Join-Path $run 'stop'))){$child.Kill();throw 'Preparation cancelled or exceeded ten minutes.'}
            Start-Sleep -Milliseconds 100
        }
        $child.CheckDiagnostics()
        Show-Diagnostics $child
        if($child.ExitCode -ne 0 -or $child.OutputTruncated){throw "$Name failed; see its local error log."}
        return Get-Content -LiteralPath $stdout -Raw | ConvertFrom-Json
    } finally {$child.Dispose()}
}
function Wait-ChildExit($Child,[int]$Seconds) {
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds($Seconds)
    while(-not $Child.Exited -and [DateTimeOffset]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 100}
    if(-not $Child.Exited){
        $Child.Kill();$deadline=[DateTimeOffset]::UtcNow.AddSeconds(10)
        while(-not $Child.Exited -and [DateTimeOffset]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 100}
        if(-not $Child.Exited){throw 'Owned process did not exit; session job remains armed.'}
    }
}
try {
    $lock=[IO.File]::Open((Join-Path $data 'launcher.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    if(Test-Path -LiteralPath (Join-Path $run 'stop')){throw 'Start cancelled before preparation.'}
    $preflight=Get-AfterhoursPreflight $requestData.game_directory $data $requestData.server_path
    if($preflight.server_sha256 -ne $requestData.server_sha256){throw 'Server changed between request and preflight.'}
    $pointerTemporary=Join-Path $data ("current-session.$($requestData.id).tmp")
    $pointer=[IO.File]::Open($pointerTemporary,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try {$bytes=[Text.Encoding]::UTF8.GetBytes((@{version=1;id=$requestData.id}|ConvertTo-Json -Compress));$pointer.Write($bytes);$pointer.Flush($true)}finally{$pointer.Dispose()}
    [IO.File]::Move($pointerTemporary,(Join-Path $data 'current-session.json'),$true)
    $job=[Afterhours.Windows.SessionJob]::AttachSupervisor()
    Set-SessionStatus 'preparing' 'Preparing local content and account.'
    $prepared=Invoke-Preparation 'prepare' @('prepare-local','--root',$data,'--game-dir',$preflight.game_directory)
    $accountPath=Join-Path $data 'account.json'
    $state=Join-Path $data 'artifacts/account'
    if(Test-Path -LiteralPath $accountPath){
        $account=Read-AfterhoursJson $accountPath
        if($account.version -ne 1 -or $account.local_account -notmatch '^[a-f0-9]{32}$' -or -not (Test-Path -LiteralPath $state)){throw 'Existing account record is invalid; no state was replaced.'}
    }else{
        if(Test-Path -LiteralPath $state){throw 'An account directory exists without its launcher record. Preserve it for recovery; it will not be overwritten.'}
        $created=Invoke-Preparation 'create-account' @('create-account','--name',$requestData.player_name,'--state-directory',$state,'--account-policy',$prepared.files.'account-policy','--game-dir',$preflight.game_directory,'--content-cache',(Join-Path $data 'artifacts/content'),'--progression','post-intro')
        $account=@{version=1;local_account=$created.local_account;progression='post-intro'}
        $file=[IO.File]::Open($accountPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        try {$bytes=[Text.Encoding]::UTF8.GetBytes(($account|ConvertTo-Json -Compress));$file.Write($bytes);$file.Flush($true)}finally{$file.Dispose()}
    }
    if($requestData.action -eq 'Setup'){Set-SessionStatus 'finished' 'Setup complete. Local account starts after the unsupported scripted prologue.';return}
    $alias=Install-AfterhoursCompatibilityServer $preflight
    $output=Join-Path $data "artifacts/runs/$($requestData.id)"
    [void](New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force)
    $stopServer=Join-Path $run 'stop-server'
    $arguments=[Collections.Generic.List[string]]::new()
    foreach($s in @('--root',$data,'--output',$output,'--game-dir',$preflight.game_directory,'--state-directory','artifacts/account','--local-account',$account.local_account,'--lsx-port','3216','--stop-file',$stopServer,'--owned-local-social','--owned-menu-awards','--owned-world-readiness','--owned-world-attributes','--owned-world-connection','--owned-only')){$arguments.Add($s)}
    foreach($p in $prepared.files.PSObject.Properties){if($p.Name -ne 'account-policy'){$arguments.Add('--'+$p.Name);$arguments.Add([string]$p.Value)}}
    if($requestData.record_protocol){$arguments.Add('--record-protocol')}
    $server=[Afterhours.Windows.Child]::Start($alias,$arguments.ToArray(),$data,(Join-Path $run 'server.stdout.jsonl'),(Join-Path $run 'server.stderr.log'),$null)
    $serverId=$server.Id
    $deadline=[DateTimeOffset]::UtcNow.AddMinutes(2)
    while(-not $server.FirstLine.IsCompleted){Show-Diagnostics $server;$server.CheckDiagnostics();if($server.Exited -or [DateTimeOffset]::UtcNow -gt $deadline){throw 'Server did not become ready.'};Start-Sleep -Milliseconds 100}
    $ready=$server.FirstLine.GetAwaiter().GetResult() | ConvertFrom-Json
    if(-not $ready.ready -or $ready.pid -ne $server.Id -or $ready.lsx -ne '127.0.0.1:3216'){throw 'Server readiness identity mismatch.'}
    $endpoint=[Net.IPEndPoint]::Parse($ready.redirector)
    if(-not $endpoint.Address.Equals([Net.IPAddress]::Loopback)){throw 'Server redirector is not loopback.'}
    $gamePath=Join-Path $preflight.game_directory 'NFS16.exe'
    if((Get-FileHash -LiteralPath $gamePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $preflight.game_sha256){throw 'Game changed after preflight.'}
    Set-SessionStatus 'starting' 'Starting game; wait for ready status before pressing Space.'
    $launchDate=[DateTimeOffset]::UtcNow
    $launchMarker=Get-AfterhoursLaunchMarker $launchDate
    # This date-derived value is launch context, not an account credential.
    Write-Host "Launch context: UTC $($launchDate.ToString('yyyy-MM-dd')), marker $launchMarker."
    $game=[Afterhours.Windows.Child]::Start($gamePath,[string[]]@(),$preflight.game_directory,(Join-Path $run 'game.stdout.log'),(Join-Path $run 'game.stderr.log'),$launchMarker)
    $gameId=$game.Id
    $lease=[Afterhours.Windows.EndpointLease]::new($game.Id,$game.Created,$gamePath,$server.Id,$server.Created,$alias,$preflight.server_sha256,$endpoint.Port)
    $deadline=[DateTimeOffset]::UtcNow.AddMinutes(2)
    $preparedLease=$false
    $waitingActivation=$false
    while(-not $preparedLease){
        Show-Diagnostics $server;Show-Diagnostics $game
        if(Test-Path -LiteralPath (Join-Path $run 'stop')){throw 'Start cancelled.'}
        if($server.Exited){throw 'Local server exited during startup.'}
        if($job.HasOwnedWindow((Join-Path $preflight.game_directory 'Core/ActivationUI.exe'))){
            if(-not $waitingActivation){Set-SessionStatus 'activation' 'Waiting for the installed activation dialog. Complete it yourself, or use Stop to cancel. Local play is not ready.'}
            $waitingActivation=$true
            # Human sign-in has no game-start timer. Keep the owned tree and
            # cancellation available; do not read or submit credentials.
            $deadline=[DateTimeOffset]::UtcNow.AddMinutes(2)
            Start-Sleep -Milliseconds 250
            continue
        }
        if($waitingActivation){Set-SessionStatus 'starting' 'Activation dialog closed; checking the original game process.';$waitingActivation=$false}
        if($game.Exited){throw ('Game exited before local routing (code 0x'+$game.ExitCode.ToString('X8')+'). Any activation handoff is outside this local session; cleanup closes its owned helpers. Inspect the installed activation log before retrying.')}
        if($server.Exited -or [DateTimeOffset]::UtcNow -gt $deadline){throw 'Server exited or startup routing did not become ready.'}
        try {$preparedLease=$lease.Prepare()} catch {
            # Protected startup code is not unpacked at process creation. Only
            # this exact pre-write condition may be retried; identity/shape
            # failures and configuration already in use fail closed.
            if($_.Exception.ToString() -notlike '*Unsupported live code guard*'){throw}
        }
        if(-not $preparedLease){Start-Sleep -Milliseconds 250}
    }
    $lease.Apply()
    Set-SessionStatus 'ready' 'Local routing is ready. Press Space in the game to connect.'
    while(-not $game.Exited){
        Show-Diagnostics $server;Show-Diagnostics $game
        $server.CheckDiagnostics();$game.CheckDiagnostics()
        if($server.Exited){throw 'Local server exited unexpectedly.'}
        if(Test-Path -LiteralPath (Join-Path $run 'stop')){Set-SessionStatus 'stopping' 'Closing game before server.';[void]$game.CloseWindow();Wait-ChildExit $game 15;break}
        Start-Sleep -Milliseconds 250
    }
    if($game.ExitCode -ne 0){throw ('Game exited with code 0x'+$game.ExitCode.ToString('X8')+'. See Windows Application events and this session directory.')}
} catch {
    $failure=$_.Exception.Message
    Set-SessionStatus 'failed' $failure
} finally {
    try {
        if($game -and -not $game.Exited){[void]$game.CloseWindow();Wait-ChildExit $game 15}
        if($lease){$lease.CompleteAfterExit();$lease.Dispose()}
        if($server -and -not $server.Exited){[IO.File]::WriteAllText((Join-Path $run 'stop-server'),'stop');Wait-ChildExit $server 15}
        # A process exit can precede the last asynchronous diagnostic write.
        # Finish both owned processes before waiting for their bounded drains.
        $drainDeadline=[DateTimeOffset]::UtcNow.AddSeconds(5)
        foreach($child in @($game,$server)) {
            if($child) {
                while(-not $child.LogsFinished -and [DateTimeOffset]::UtcNow -lt $drainDeadline){Start-Sleep -Milliseconds 50}
                $child.CheckDiagnostics()
                if(-not $child.LogsFinished){throw 'Child diagnostics did not finish after process exit.'}
                # Drain the bounded console queue completely after process exit.
                while($text=$child.ReadDiagnostics()){Write-Host -NoNewline $text}
            }
        }
        if($game){$game.Dispose()};if($server){$server.Dispose()}
        if($job){$job.Finish()}
        if(-not $failure){
            if($requestData.action -eq 'Setup'){Set-SessionStatus 'finished' 'Setup complete. Local account starts after the unsupported scripted prologue.'}
            else{Set-SessionStatus 'finished' 'Game and server stopped; account data is preserved.'}
        }
    } catch {
        $cleanupFailure=$_.Exception.Message+' The session job remains armed for cleanup on supervisor exit.'
        if($failure){$cleanupFailure=$failure+' Cleanup: '+$cleanupFailure}
        $failure=$cleanupFailure
        Set-SessionStatus 'failed' $cleanupFailure
    }
    if($lock){$lock.Dispose()}
    if($failure){exit 1}
}
