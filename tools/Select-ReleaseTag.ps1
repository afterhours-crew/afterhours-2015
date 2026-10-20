# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Called by the Tag release workflow after CI passed on a main commit. Prints
# (and writes to GITHUB_OUTPUT) the tag to create when that commit is the one
# that changed the bundle version, such as a merged release pull request;
# prints nothing to tag otherwise. Creates nothing itself.
param(
    [Parameter(Mandatory)][string]$Commit,
    [Parameter(Mandatory)][string]$Repository,
    # For a manual run: require a successful CI push run on the commit.
    [switch]$RequireCi,
    [string]$Branch = 'main'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'AfterhoursRelease.psm1') -Force

if ($Commit -notmatch '^[0-9a-f]{40}$') { throw 'Expected the full commit SHA.' }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Expected owner/repository.' }
if ($Branch -notmatch '^[A-Za-z0-9_./-]+$') { throw 'Expected a branch name.' }

# Main's own history: a merge commit, not a commit inside a merged branch.
$history = @(git rev-list --first-parent "origin/$Branch")
if ($LASTEXITCODE -ne 0) { throw "Cannot list the history of $Branch." }
if ($history -notcontains $Commit) { throw "$Commit is not on $Branch's first-parent history." }

function Get-VersionAt([string]$Revision) {
    $text = @(git show "${Revision}:crates/bundle/Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "Cannot read crates/bundle/Cargo.toml at $Revision." }
    Get-BundleVersion ($text -join "`n")
}
$version = Get-VersionAt $Commit
$before = Get-VersionAt "$Commit^1"
if ($version -ceq $before) {
    Write-Output "$Commit keeps version $version; nothing to tag."
    return
}
if (!(Test-ReleaseVersion $version)) { throw "Bundle version $version is not a release version." }
$tag = "v$version"

# An existing tag is never moved. If it already names this commit, a previous
# run tagged it; a retry of the build is manual (docs/RELEASING.md).
$tagRef = "refs/tags/$tag"
$tagLines = @(git ls-remote --tags origin $tagRef "$tagRef^{}")
if ($LASTEXITCODE -ne 0) { throw 'Cannot check the remote tags.' }
$refs = @{}
foreach ($line in $tagLines) {
    $parts = $line -split '\s+', 2
    if ($parts.Count -eq 2) { $refs[$parts[1]] = $parts[0] }
}
if ($refs.Count -gt 0) {
    $tagged = if ($refs.ContainsKey("$tagRef^{}")) { $refs["$tagRef^{}"] } else { $refs[$tagRef] }
    if ($tagged -ceq $Commit) {
        Write-Output "$tag already names $Commit; nothing to tag."
        return
    }
    throw "$tag already names $tagged, not $Commit. Prepare a newer version."
}

# Never tag a version older than one testers already have.
$latest = @(gh api "repos/$Repository/releases/latest" --jq .tag_name)
if ($LASTEXITCODE -eq 0 -and $latest.Count -gt 0 -and (Test-ReleaseVersion ($latest[0] -replace '^v', ''))) {
    if ((Compare-ReleaseVersion $version $latest[0]) -le 0) {
        throw "$tag is not newer than the published $($latest[0])."
    }
} else {
    Write-Warning 'No published release found to compare with.'
}

if ($RequireCi) {
    $runs = @(gh api "repos/$Repository/actions/workflows/ci.yml/runs?head_sha=$Commit&event=push&status=success&per_page=1" --jq .total_count)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot check CI for the commit.' }
    if ($runs.Count -eq 0 -or [int]$runs[0] -lt 1) { throw "CI has not passed on $Commit." }
}

if ($env:GITHUB_OUTPUT) { Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "tag=$tag" }
Write-Output "Tag $Commit as $tag ($before → $version)."
