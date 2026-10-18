# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Called only after the tagged build, tests and packaging have succeeded.
# Creates a draft once; never publishes, replaces assets, or edits a release.
param(
    [Parameter(Mandatory)][string]$PackageDirectory,
    [Parameter(Mandatory)][string]$Release,
    [Parameter(Mandatory)][string]$Revision
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($Release -notmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$') {
    throw 'Use a version tag such as v0.3.1.'
}
if ($Revision -notmatch '^[0-9a-f]{40}$') { throw 'Expected the exact built commit SHA.' }
$package = (Resolve-Path -LiteralPath $PackageDirectory).Path
$assets = @('Afterhours-windows-x64.zip', 'SHA256SUMS.txt', 'build-info.json') |
    ForEach-Object { (Resolve-Path -LiteralPath (Join-Path $package $_)).Path }
$info = Get-Content -LiteralPath $assets[2] -Raw | ConvertFrom-Json
$version = $Release.Substring(1)
if ($info.release -cne $Release -or $info.revision -cne $Revision -or
    $info.dirty_source -isnot [bool] -or $info.dirty_source -or
    $info.executable_version -cne "Afterhours $version ($Release / $Revision)") {
    throw 'Package identity must match the clean, versioned release build.'
}
$archiveHash = (Get-FileHash -LiteralPath $assets[0] -Algorithm SHA256).Hash.ToLowerInvariant()
$checksum = (Get-Content -LiteralPath $assets[1] -Raw).Trim()
if ($checksum -cne "$archiveHash  Afterhours-windows-x64.zip") { throw 'Archive checksum mismatch.' }

# Check the remote again after the long build. Support annotated and lightweight
# tags, and refuse a tag moved to a different commit while compilation ran.
$tagRef = "refs/tags/$Release"
$tagLines = @(git ls-remote --exit-code --tags origin $tagRef "$tagRef^{}")
if ($LASTEXITCODE -ne 0) { throw 'Cannot verify the remote release tag.' }
$refs = @{}
foreach ($line in $tagLines) {
    $parts = $line -split '\s+', 2
    if ($parts.Count -eq 2) { $refs[$parts[1]] = $parts[0] }
}
$tagRevision = if ($refs.ContainsKey("$tagRef^{}")) { $refs["$tagRef^{}"] } else { $refs[$tagRef] }
if ($tagRevision -cne $Revision) { throw 'Remote tag no longer matches the built commit.' }

$notes = New-TemporaryFile
try {
    @"
Download **Afterhours-windows-x64.zip**, extract it, and put **Afterhours.exe** beside **NFS16.exe** in your supported, activated game installation. Close the game and Afterhours before replacing an older executable, then double-click Afterhours.exe as an ordinary user.

The ZIP, SHA256SUMS.txt and build-info.json were prepared from commit $Revision after the build, tests and package smoke checks passed. Review these notes and the assets before publishing this draft.

[Tester quickstart](https://github.com/afterhours-crew/afterhours-2015/blob/$Release/docs/TESTER-QUICKSTART.md)

"@ | Set-Content -LiteralPath $notes.FullName -Encoding utf8
    # gh refuses an existing release. No --clobber, edit, or publish operation is
    # used: a failed/partial upload leaves a draft for the maintainer to inspect.
    gh release create $Release @assets --verify-tag --target $Revision --draft `
        --title "Afterhours $version" --notes-file $notes.FullName --generate-notes
    if ($LASTEXITCODE -ne 0) {
        throw 'Draft preparation failed. Inspect any existing draft and the Actions artifact before retrying; nothing is published or overwritten.'
    }
} finally { Remove-Item -LiteralPath $notes.FullName }
