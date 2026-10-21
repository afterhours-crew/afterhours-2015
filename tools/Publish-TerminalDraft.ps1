# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Called only after the tagged build, tests and packaging have succeeded.
# Creates a draft once; never publishes, replaces assets, or edits a release.
param(
    [Parameter(Mandatory)][string]$PackageDirectory,
    [Parameter(Mandatory)][string]$Release,
    [Parameter(Mandatory)][string]$Revision,
    [Parameter(Mandatory)][string]$Repository,
    # Reviewed notes from the release pull request (Find-ReleaseNotes.ps1).
    # Without them the draft gets the default text plus generated notes.
    [string]$NotesFile = ''
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($Release -notmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$') {
    throw 'Use a version tag such as v0.3.1.'
}
if ($Revision -notmatch '^[0-9a-f]{40}$') { throw 'Expected the exact built commit SHA.' }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Expected owner/repository.' }
$package = (Resolve-Path -LiteralPath $PackageDirectory).Path
$assets = @('Afterhours-windows-x64.zip', 'SHA256SUMS.txt', 'build-info.json', 'Afterhours-contract.zip') |
    ForEach-Object { (Resolve-Path -LiteralPath (Join-Path $package $_)).Path }
$info = Get-Content -LiteralPath $assets[2] -Raw | ConvertFrom-Json
$version = $Release.Substring(1)
if ($info.release -cne $Release -or $info.revision -cne $Revision -or
    $info.dirty_source -isnot [bool] -or $info.dirty_source -or
    $info.executable_version -cne "Afterhours $version ($Release / $Revision)") {
    throw 'Package identity must match the clean, versioned release build.'
}
$archiveHash = (Get-FileHash -LiteralPath $assets[0] -Algorithm SHA256).Hash.ToLowerInvariant()
$contractHash = (Get-FileHash -LiteralPath $assets[3] -Algorithm SHA256).Hash.ToLowerInvariant()
$checksum = (Get-Content -LiteralPath $assets[1] -Raw).Replace("`r`n", "`n").Trim()
if ($checksum -cne "$archiveHash  Afterhours-windows-x64.zip`n$contractHash  Afterhours-contract.zip") { throw 'Archive checksum mismatch.' }

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

# GraphQL resolves both drafts and published releases for this authenticated
# workflow. Fail closed on lookup errors; absence is the only creation path.
$repoParts = $Repository -split '/'
$query = 'query($owner: String!, $name: String!, $tag: String!) { repository(owner: $owner, name: $name) { release(tagName: $tag) { id isDraft } } }'
$lookupText = gh api graphql -f "query=$query" -F "owner=$($repoParts[0])" -F "name=$($repoParts[1])" -F "tag=$Release"
if ($LASTEXITCODE -ne 0) { throw 'Cannot check for an existing release.' }
$lookup = $lookupText | ConvertFrom-Json
if ($null -eq $lookup.data.repository) { throw 'Release repository is unavailable.' }
if ($null -ne $lookup.data.repository.release) { throw 'A draft or published release already exists for this tag; inspect it before retrying.' }

$reviewed = if ($NotesFile) { (Resolve-Path -LiteralPath $NotesFile).Path } else { $null }
$notes = New-TemporaryFile
try {
    $notesArguments = if ($reviewed) { @('--notes-file', $reviewed) } else {
        @"
Download **Afterhours-windows-x64.zip**, extract it, and put **Afterhours.exe** beside **NFS16.exe** in your supported, activated game installation. Close the game and Afterhours before replacing an older executable, then double-click Afterhours.exe as an ordinary user.

The ZIP, SHA256SUMS.txt and build-info.json were prepared from commit $Revision after the build, tests and package smoke checks passed. Afterhours-contract.zip holds the executable's ``version --json`` record and the JSON Schemas of its command-line records for desktop launchers. Review these notes and the assets before publishing this draft.

[Tester quickstart](https://github.com/afterhours-crew/afterhours-2015/blob/$Release/docs/TESTER-QUICKSTART.md)

"@ | Set-Content -LiteralPath $notes.FullName -Encoding utf8
        @('--notes-file', $notes.FullName, '--generate-notes')
    }
    # No --clobber, edit, or publish operation: a failed/partial upload leaves
    # a draft for inspection. The workflow serializes preparations for this tag.
    gh release create $Release @assets --repo $Repository --verify-tag --target $Revision --draft `
        --title "Afterhours $version" @notesArguments
    if ($LASTEXITCODE -ne 0) {
        throw 'Draft preparation failed. Inspect any existing draft and the Actions artifact before retrying; nothing is published or overwritten.'
    }
} finally { Remove-Item -LiteralPath $notes.FullName }
