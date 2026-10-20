# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Packages only the built executable, its JSON contract and an explicit
# documentation allowlist. Nothing is read from a game installation or a local
# account. The contract (the executable's `version --json` and the committed
# JSON Schemas, verified against the executable) is also packaged separately as
# Afterhours-contract.zip so a desktop launcher can generate types from it.
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Release = 'development'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repo = Split-Path -Parent $PSScriptRoot
$binary = (Resolve-Path -LiteralPath $Executable).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw 'Choose a new, empty output directory. Existing release files are never overwritten.' }
$stage = Join-Path $output 'package'
New-Item -ItemType Directory -Path $stage -Force | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'Afterhours.exe')
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $repo 'docs/TESTER-QUICKSTART.md') -Destination (Join-Path $stage 'README.md')

# Test the copied executable in isolation. Neither a source checkout nor a
# sibling nfs-server is in its working directory.
Push-Location $stage
try {
    $version = (& './Afterhours.exe' --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or !$version.StartsWith('Afterhours ')) { throw 'Invalid bundled executable version.' }
    $checks = @(
        @{ Arguments = @('--help'); Expected = 'Prepare, start and supervise' },
        @{ Arguments = @('--afterhours-server', '--help'); Expected = 'nfs-server: owned local' },
        @{ Arguments = @('--afterhours-server', 'prepare-local', '--help'); Expected = 'prepare-local' }
    )
    foreach ($check in $checks) {
        $arguments = $check.Arguments
        $text = (& './Afterhours.exe' @arguments 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "Packaged executable smoke check failed: $arguments`n$text" }
        if (!$text.Contains($check.Expected)) { throw "Packaged executable selected the wrong role: $arguments" }
    }
    $versionJson = (& './Afterhours.exe' version --json | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Packaged executable did not report its JSON contract.' }
    $contract = $versionJson | ConvertFrom-Json
    if ($contract.program -cne 'Afterhours' -or $contract.release -cne $Release) {
        throw 'Contract identity does not match the packaged release.'
    }
    # Every record's schema as the executable prints it must equal the
    # committed file; nothing else may be in the schema directory.
    $schemaSource = Join-Path $repo 'crates/launcher/schema'
    $records = @($contract.records.PSObject.Properties.Name)
    $committed = @(Get-ChildItem -LiteralPath $schemaSource -File | ForEach-Object Name)
    $expected = @($records | ForEach-Object { "$_.schema.json" })
    if ($records.Count -eq 0 -or $committed.Count -ne $expected.Count -or
        @(Compare-Object -CaseSensitive $committed $expected).Count -ne 0) {
        throw 'Committed schemas do not match the records the executable reports.'
    }
    $schemaStage = Join-Path $stage 'schema'
    New-Item -ItemType Directory -Path $schemaStage | Out-Null
    foreach ($record in $records) {
        $printed = (& './Afterhours.exe' schema $record | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "Packaged executable did not print the $record schema." }
        $file = Join-Path $schemaSource "$record.schema.json"
        $text = [IO.File]::ReadAllText($file).Replace("`r`n", "`n")
        if ($printed.Replace("`r`n", "`n").TrimEnd() -cne $text.TrimEnd()) { throw "The $record schema differs from the executable." }
        [IO.File]::WriteAllText((Join-Path $schemaStage "$record.schema.json"), $text, [Text.UTF8Encoding]::new($false))
    }
} finally { Pop-Location }

Push-Location $repo
try {
    $revision = (git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source revision.' }
    $changes = @(git status --porcelain --untracked-files=normal)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source status.' }
    if ($Release -ne 'development' -and ($changes.Count -gt 0 -or !$version.Contains("($Release / $revision)"))) {
        throw 'Release packaging requires clean source and matching embedded release/revision.'
    }
    $metadataText = cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve locked dependency metadata.' }
    $metadata = $metadataText | ConvertFrom-Json
    $toolchain = (rustc --version).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Rust version.' }
} finally { Pop-Location }

$notices = [Text.StringBuilder]::new()
[void]$notices.AppendLine("Afterhours third-party notices. Source: https://github.com/afterhours-crew/afterhours-2015/tree/$revision")
foreach ($package in ($metadata.packages | Where-Object source | Sort-Object name, version)) {
    [void]$notices.AppendLine("`n--- $($package.name) $($package.version) ---`nLicense: $($package.license)`nSource: https://crates.io/crates/$($package.name)/$($package.version)")
    $directory = Split-Path -Parent $package.manifest_path
    $licenseFiles = @(Get-ChildItem -LiteralPath $directory -File | Where-Object Name -Match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([._-]|$)')
    if ($package.name -eq 'openssl-src') {
        $licenseFiles += Get-Item -LiteralPath (Join-Path $directory 'openssl/LICENSE.txt')
    }
    foreach ($file in ($licenseFiles | Sort-Object FullName -Unique)) {
        [void]$notices.AppendLine("`n$($file.Name):`n$([IO.File]::ReadAllText($file.FullName))")
    }
}
[IO.File]::WriteAllText((Join-Path $stage 'THIRD-PARTY-NOTICES.txt'), $notices.ToString())
$info = [ordered]@{
    release = $Release
    revision = $revision
    target = 'x86_64-pc-windows-msvc'
    toolchain = $toolchain
    executable_version = $version
    dirty_source = $changes.Count -gt 0
    executable_sha256 = (Get-FileHash -LiteralPath (Join-Path $stage 'Afterhours.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    source = "https://github.com/afterhours-crew/afterhours-2015/tree/$revision"
    records = $contract.records
    capabilities = @($contract.capabilities)
}
$info | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'build-info.json') -Encoding utf8
Copy-Item -LiteralPath (Join-Path $output 'build-info.json') -Destination $stage
$archive = Join-Path $output 'Afterhours-windows-x64.zip'
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive
$contractStage = Join-Path $output 'contract'
New-Item -ItemType Directory -Path $contractStage | Out-Null
Copy-Item -LiteralPath $schemaStage -Destination $contractStage -Recurse
[IO.File]::WriteAllText((Join-Path $contractStage 'version.json'), "$versionJson`n", [Text.UTF8Encoding]::new($false))
$contractArchive = Join-Path $output 'Afterhours-contract.zip'
Compress-Archive -Path (Join-Path $contractStage '*') -DestinationPath $contractArchive
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
$contractHash = (Get-FileHash -LiteralPath $contractArchive -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $output 'SHA256SUMS.txt'), "$hash  Afterhours-windows-x64.zip`n$contractHash  Afterhours-contract.zip`n")
Write-Output "Packaged and smoke-tested $archive ($hash) and $contractArchive ($contractHash)."
