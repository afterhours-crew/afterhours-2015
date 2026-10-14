# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#Requires -Version 7.5
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:GameHash = '92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df'
$script:ActivationHash = '5901b0ad269e12deecc2bcd2eaa56f8964538a8d6b1a265651d56dbd636daefd'

function Get-AfterhoursLaunchMarker {
    param([Parameter(Mandatory)][DateTimeOffset]$Date)
    $utc = $Date.UtcDateTime
    $mixed = ([long]$utc.Day * 0x558a9) -bxor ([long]$utc.Year * 0x19919) -bxor ([long]$utc.Month * 0x36de1)
    return (($mixed -bxor ($mixed -shr 16) -bxor ($mixed -shl 16)) -band 0xffffffffL).ToString([Globalization.CultureInfo]::InvariantCulture)
}

function Get-AfterhoursCompatibilityName {
    param([Parameter(Mandatory)][string]$RegisteredPath)
    # The supported SDK checks either basename or basename + Debug. Use the
    # latter so the compatibility server is distinguishable from the EA app.
    if (-not [IO.Path]::IsPathFullyQualified($RegisteredPath)) { throw 'Launcher registration is not an absolute path.' }
    switch ([IO.Path]::GetFileName($RegisteredPath).ToLowerInvariant()) {
        'eadesktop.exe' { return 'EADesktopDebug.exe' }
        'origin.exe' { return 'OriginDebug.exe' }
        default { throw 'Unrecognized launcher registration; no machine setting was changed.' }
    }
}

function Get-AfterhoursRegistryPath {
    if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'Afterhours requires 64-bit Windows and PowerShell.' }
    $root = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryView]::Registry32)
    $key = $null
    try {
        $key = $root.OpenSubKey('SOFTWARE\Origin', $false)
        if (-not $key -or $key.GetValueKind('ClientPath') -ne [Microsoft.Win32.RegistryValueKind]::String) {
            throw 'Existing game launcher registration is missing or unsupported. Afterhours does not install or modify it.'
        }
        return [string]$key.GetValue('ClientPath', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally { if ($key) { $key.Dispose() }; $root.Dispose() }
}

function Assert-AfterhoursDataPath {
    param([Parameter(Mandatory)][string]$DataDirectory, [Parameter(Mandatory)][string]$GameDirectory)
    $data = [IO.Path]::GetFullPath($DataDirectory).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $game = [IO.Path]::GetFullPath($GameDirectory).TrimEnd([IO.Path]::DirectorySeparatorChar)
    if ([IO.Path]::GetPathRoot($data).TrimEnd('\') -eq $data -or $data.StartsWith('\\')) { throw 'Choose a dedicated local data directory, not a drive root or network share.' }
    if ($data -eq $game -or $data.StartsWith($game + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Choose a data directory outside the game installation.'
    }
    # Refuse junctions/symlinks in the writable path rather than following one
    # into an installation, a shared directory or another user's saved state.
    $cursor = $data
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $entry = Get-Item -LiteralPath $cursor -Force
            if (-not $entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Data path must contain only ordinary directories.'
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    return $data
}

function Assert-AfterhoursDataTree {
    param([Parameter(Mandatory)][string]$DataDirectory, [Parameter(Mandatory)][string]$GameDirectory)
    $data = Assert-AfterhoursDataPath $DataDirectory $GameDirectory
    if (-not (Test-Path -LiteralPath $data)) { return }
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($data)
    $count = 0
    while ($pending.Count) {
        foreach ($entry in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
            if (++$count -gt 100000) { throw 'Data directory exceeds the 100,000-entry inspection bound.' }
            if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Data directory contains a junction or symbolic link; use ordinary files and directories.' }
            if ($entry.PSIsContainer) { $pending.Push($entry.FullName) }
        }
    }
}

function Read-AfterhoursJson {
    param([Parameter(Mandatory)][string]$Path)
    $entry = Get-Item -LiteralPath $Path -Force
    if ($entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $entry.Length -gt 65536) { throw 'Invalid or oversized launcher record.' }
    return Get-Content -LiteralPath $Path -Raw -Encoding utf8 | ConvertFrom-Json
}

function Get-AfterhoursSessionStatus {
    param([Parameter(Mandatory)][string]$DataDirectory)
    [void](Assert-AfterhoursDataPath $DataDirectory (Join-Path $DataDirectory '__not_a_game_installation__'))
    $pointer = Join-Path $DataDirectory 'current-session.json'
    if (-not (Test-Path -LiteralPath $pointer)) { return $null }
    $session = Read-AfterhoursJson $pointer
    if ($session.version -ne 1 -or $session.id -notmatch '^[a-f0-9]{32}$') { throw 'Invalid session identifier.' }
    $run = Join-Path $DataDirectory "artifacts/launcher/$($session.id)"
    [void](Assert-AfterhoursDataPath $run (Join-Path $DataDirectory '__not_a_game_installation__'))
    $status = Read-AfterhoursJson (Join-Path $run 'status.json')
    if ($status.id -ne $session.id) { throw 'Session status identity mismatch.' }
    if ($status.state -notin @('finished','failed')) {
        $probe = $null
        $lockPath = Join-Path $DataDirectory 'launcher.lock'
        if (-not (Test-Path -LiteralPath $lockPath)) {
            $status.state = 'interrupted'
            $status.message = 'Supervisor lock is missing. Inspect the data directory before restarting.'
            return $status
        }
        try {
            $probe = [IO.File]::Open($lockPath,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
            $status.state = 'interrupted'
            $status.message = 'Supervisor is no longer running. Its session job closes owned processes; saved account data is preserved. Start again to recover.'
        } catch [IO.IOException] { } finally { if ($probe) { $probe.Dispose() } }
    }
    return $status
}

function Get-AfterhoursPreflight {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$GameDirectory,
          [Parameter(Mandatory)][string]$DataDirectory,
          [Parameter(Mandatory)][string]$ServerPath)
    if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'Afterhours requires 64-bit Windows and PowerShell.' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    try {
        if ([Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
            throw 'Run Afterhours from an ordinary, non-administrator PowerShell window.'
        }
    } finally { $identity.Dispose() }
    $game = (Resolve-Path -LiteralPath $GameDirectory).ProviderPath
    $data = Assert-AfterhoursDataPath -DataDirectory $DataDirectory -GameDirectory $game
    Assert-AfterhoursDataTree $data $game
    $server = (Resolve-Path -LiteralPath $ServerPath).ProviderPath
    if ([IO.Path]::GetExtension($server) -ne '.exe') { throw 'Select the built Afterhours server executable.' }
    foreach ($entry in @(@('NFS16.exe', $script:GameHash), @('Core/ActivationUI.exe', $script:ActivationHash))) {
        if ((Get-FileHash -LiteralPath (Join-Path $game $entry[0]) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry[1]) {
            throw "Unsupported installed file: $($entry[0]). Nothing was changed."
        }
    }
    $registration = Get-AfterhoursRegistryPath
    $compatibility = Get-AfterhoursCompatibilityName $registration
    foreach ($name in @('NFS16','NFS16_trial','EADesktop','Origin','EADesktopDebug','OriginDebug')) {
        $processes = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
        if ($processes.Count) { throw "Close the existing $name process before starting Afterhours." }
    }
    $probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 3216)
    $probe.Server.ExclusiveAddressUse = $true
    try { $probe.Start() } catch { throw 'Local port 3216 is occupied; close the application using it and retry.' }
    finally { $probe.Stop() }
    [pscustomobject]@{
        version = 1; game_directory = $game; data_directory = $data; server_path = $server
        server_sha256 = (Get-FileHash -LiteralPath $server -Algorithm SHA256).Hash.ToLowerInvariant()
        game_sha256 = $script:GameHash; activation_sha256 = $script:ActivationHash
        registered_client_path = $registration; compatibility_name = $compatibility
        requires_administrator = $false; machine_changes = @(); directory_write_access = 'checked during setup'
    }
}

function Install-AfterhoursCompatibilityServer {
    param([Parameter(Mandatory)]$Preflight)
    $name = Get-AfterhoursCompatibilityName $Preflight.registered_client_path
    if ($Preflight.server_sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Invalid server digest.' }
    $data = Assert-AfterhoursDataPath $Preflight.data_directory $Preflight.game_directory
    $directory = Join-Path $data "bin/$($Preflight.server_sha256)"
    [void](Assert-AfterhoursDataPath $directory $Preflight.game_directory)
    [void](New-Item -ItemType Directory -Path $directory -Force)
    [void](Assert-AfterhoursDataPath $directory $Preflight.game_directory)
    $target = Join-Path $directory $name
    if (Test-Path -LiteralPath $target) {
        $entry = Get-Item -LiteralPath $target -Force
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked compatibility executable.' }
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Preflight.server_sha256) {
            throw 'Existing compatibility executable has changed; choose a new data directory or inspect it.'
        }
    } else {
        if ((Get-FileHash -LiteralPath $Preflight.server_path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Preflight.server_sha256) {
            throw 'Server changed after preflight.'
        }
        $source = [IO.File]::OpenRead($Preflight.server_path)
        $destination = $null
        $temporary = Join-Path $directory ([Guid]::NewGuid().ToString('N')+'.partial')
        try {
            $destination = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            $source.CopyTo($destination)
            $destination.Flush($true)
        } finally { if ($destination) { $destination.Dispose() }; $source.Dispose() }
        if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Preflight.server_sha256) {
            throw 'Compatibility server copy failed its digest check.'
        }
        [IO.File]::Move($temporary,$target,$false)
    }
    return $target
}

Export-ModuleMember -Function Get-AfterhoursLaunchMarker, Get-AfterhoursCompatibilityName, Get-AfterhoursPreflight, Install-AfterhoursCompatibilityServer, Assert-AfterhoursDataPath, Assert-AfterhoursDataTree, Read-AfterhoursJson, Get-AfterhoursSessionStatus
