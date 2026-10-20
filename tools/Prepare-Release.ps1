# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Called by the Prepare release workflow (and dry-run by CI). Sets the bundle
# version in crates/bundle/Cargo.toml and Cargo.lock and writes the release
# pull request's description: tester-facing notes drafted from the pull
# requests merged since the previous release tag. Commits, pushes and opens
# nothing; the workflow does that after this succeeds.
param(
    # Blank: the next patch version.
    [AllowEmptyString()][string]$Version = '',
    [Parameter(Mandatory)][string]$Repository,
    [Parameter(Mandatory)][string]$BodyFile,
    # The commit whose history the notes cover (CI's dry run uses the base of
    # a pull request's merge commit).
    [string]$Head = 'HEAD',
    # CI's preview: write only the description, and allow an open release
    # branch for the version.
    [switch]$Preview,
    [string]$Root = (Join-Path $PSScriptRoot '..')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'AfterhoursRelease.psm1') -Force

if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Expected owner/repository.' }
$tomlPath = Join-Path $Root 'crates/bundle/Cargo.toml'
$lockPath = Join-Path $Root 'Cargo.lock'
$toml = [IO.File]::ReadAllText($tomlPath)
$lock = [IO.File]::ReadAllText($lockPath)
$current = Get-BundleVersion $toml

$Version = ($Version.Trim()) -replace '^v', ''
if (!$Version) { $Version = Get-NextPatchVersion $current }
if (!(Test-ReleaseVersion $Version)) { throw "Use a version such as 0.5.0 (got '$Version')." }
if ((Compare-ReleaseVersion $Version $current) -le 0) {
    throw "$Version is not newer than the current bundle version $current."
}
$tag = "v$Version"
$branch = "feature/release-$Version"

# Tags are immutable and a release branch may already carry an open pull
# request: refuse both rather than reuse or overwrite them.
git ls-remote --exit-code --tags origin "refs/tags/$tag" | Out-Null
if ($LASTEXITCODE -eq 0) { throw "$tag already exists; choose a newer version." }
if ($LASTEXITCODE -ne 2) { throw 'Cannot check the remote tags.' }
if (!$Preview) {
    git ls-remote --exit-code --heads origin "refs/heads/$branch" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "$branch already exists; close its pull request and delete the branch first." }
    if ($LASTEXITCODE -ne 2) { throw 'Cannot check the remote branches.' }
}

# The notes start after the release the current version came from, or the
# nearest version tag when that one is missing.
$previous = "v$current"
git rev-parse --quiet --verify "refs/tags/$previous^{commit}" | Out-Null
$hasPrevious = $LASTEXITCODE -eq 0
if ($hasPrevious) {
    git merge-base --is-ancestor $previous $Head
    $hasPrevious = $LASTEXITCODE -eq 0
}
if (!$hasPrevious) {
    $previous = (git describe --tags --abbrev=0 --match 'v[0-9]*' $Head | Select-Object -First 1)
    if ($LASTEXITCODE -ne 0 -or !$previous) { throw 'No earlier version tag to start the notes from.' }
    $previous = $previous.Trim()
}

$pulls = @(Get-MergedPullRequest -Repository $Repository -Since $previous -Head $Head)
$limits = Get-PreviousKnownLimit -Repository $Repository -Tag $previous
$notes = New-ServerReleaseNotes -Version $Version -Repository $Repository -PullRequests $pulls -KnownLimits ([string]$limits)

$included = @($pulls | Where-Object { (Get-ReleaseSection $_) -ne 'Skip' })
$skipped = @($pulls | Where-Object { (Get-ReleaseSection $_) -eq 'Skip' })
$sorted = if ($included.Count -gt 0) {
    ($included | ForEach-Object {
        $why = if ($_.Labels -match '^kind:') { ($_.Labels -match '^kind:') -join ', ' } else { "``$($_.Head)``" }
        "#$($_.Number) → $(Get-ReleaseSection $_) ($why)"
    }) -join '; '
} else { 'none' }
$review = @(
    '### Before merging'
    ''
    "- **Version:** $current → $Version in ``crates/bundle/Cargo.toml`` and ``Cargo.lock``. Merging tags the merge commit ``$tag`` once CI passes on it, and **Windows tester bundle** then builds the unpublished draft."
    "- **Notes:** drafted from the pull requests merged after ``$previous``. Edit the text between the ``release-notes`` markers in this description: the draft release takes it from here when it is built. Everything below the line stays out of the notes."
    "- **Sorted** by ``kind:feature`` / ``kind:bug`` / ``kind:change`` label, otherwise by branch (``feature/`` → Added, ``bugfix/`` → Fixed, others → Changed): $sorted."
    $(if ($skipped.Count -gt 0) { "- **Left out** (release pull requests): $(($skipped | ForEach-Object { "#$($_.Number)" }) -join ', ')." })
    $(if ($limits) { "- **Known limits** are copied from $previous's release notes; remove what no longer holds." }
      else { "- **Known limits:** $previous's release notes have none to copy; fill them in." })
    '- If more pull requests merge before this one, close it, delete its branch and run **Prepare release** again so the notes include them.'
) -join "`n"

$bumped = Set-BundleVersion -CargoToml $toml -CargoLock $lock -Version $Version
if (!$Preview) {
    [IO.File]::WriteAllText($tomlPath, $bumped.CargoToml)
    [IO.File]::WriteAllText($lockPath, $bumped.CargoLock)
}
[IO.File]::WriteAllText($BodyFile, (New-ReleasePullRequestBody -Notes $notes -Review $review))

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value @("version=$Version", "tag=$tag", "branch=$branch", "previous=$previous")
}
Write-Output "Prepared $tag ($current → $Version) with notes on $($included.Count) pull requests since $previous."
