# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Called before the draft is created. Writes $Output with the notes of the
# pull request that merged $Revision (the text between its release-notes
# markers), so the draft carries the notes reviewed there. Writes nothing when
# there are none, and then the draft gets the default text.
param(
    [Parameter(Mandatory)][string]$Revision,
    [Parameter(Mandatory)][string]$Repository,
    [Parameter(Mandatory)][string]$Output
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'AfterhoursRelease.psm1') -Force

if ($Revision -notmatch '^[0-9a-f]{40}$') { throw 'Expected the exact built commit SHA.' }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Expected owner/repository.' }

# A lookup failure after a long build should not lose the build: the draft
# falls back to the default text, which the owner replaces before publishing.
$text = gh api "repos/$Repository/commits/$Revision/pulls"
if ($LASTEXITCODE -ne 0) {
    Write-Warning "Cannot look up the pull request that merged $Revision; the draft gets the default text."
    return
}
$pulls = @(($text -join "`n") | ConvertFrom-Json) |
    Where-Object { $_.merged_at -and $_.merge_commit_sha -ceq $Revision }
foreach ($pull in $pulls) {
    $notes = Get-MarkedReleaseNotes ([string]$pull.body)
    if ($notes) {
        [IO.File]::WriteAllText($Output, "$notes`n")
        Write-Output "Release notes from #$($pull.number)."
        return
    }
}
Write-Output "No pull request with release notes merged $Revision; the draft gets the default text."
