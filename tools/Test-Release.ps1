# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Portable, offline tests of the release automation: the version and notes
# helpers, and Prepare-Release.ps1, Select-ReleaseTag.ps1 and
# Find-ReleaseNotes.ps1 against constructed repositories. Only the Git and
# GitHub boundary (git, gh) is replaced.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'AfterhoursRelease.psm1') -Force

$root = Join-Path ([IO.Path]::GetTempPath()) "afterhours-release-tests-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path (Join-Path $root 'crates/bundle') | Out-Null
$savedOutput = $env:GITHUB_OUTPUT
$outputFile = Join-Path $root 'github-output.txt'
$bodyFile = Join-Path $root 'release-pr.md'
$notesFile = Join-Path $root 'notes.md'
$repository = 'test-owner/test-repo'

function Assert-Equal($Actual, $Expected, [string]$Name) {
    if ([string]$Actual -cne [string]$Expected) { throw "$Name`: expected <$Expected>, got <$Actual>" }
}
function Assert-True($Condition, [string]$Name) { if (!$Condition) { throw "Failed: $Name" } }
function Assert-Refused([string]$Name, [scriptblock]$Action, [string]$Pattern) {
    $message = $null
    try { & $Action | Out-Null } catch { $message = $_.Exception.Message }
    if ($null -eq $message) { throw "Not refused: $Name" }
    if ($Pattern -and $message -notmatch $Pattern) { throw "$Name refused for another reason: $message" }
    Write-Output "Passed: $Name"
}

# --- Constructed repository state behind the git and gh replacements. ---
$sha = @{
    v040 = 'a' * 40; m132 = 'b' * 40; m135 = 'c' * 40; rebase = 'd' * 40; direct = 'e' * 40
    release = 'f' * 40; other = '1' * 40
}
function Pull([int]$Number, [string]$Title, [string]$Head, [string[]]$Labels = @(), [string]$Body = '') {
    @{
        number = $Number; title = $Title; body = $Body; head = @{ ref = $Head }
        html_url = "https://github.com/$repository/pull/$Number"
        labels = @($Labels | ForEach-Object { @{ name = $_ } }); merged_at = '2026-10-20T00:00:00Z'
    }
}
$previousNotes = @"
Afterhours 0.4.0 fills free roam with traffic.

## Added

- **Traffic.** It drives. ([#132](https://example.invalid/132))

## Known limits

- **Traffic stops at 3x.** The game crashed at 5x.
  - Nested detail.
- **No cops yet.**

  ``````
  .\Afterhours.exe settings
  ``````

This is an early tester release, not the complete game.

For feedback, include the release version.
"@

function Reset-State {
    $global:ReleaseTestState = @{
        Remote = @{ 'refs/tags/v0.4.0' = $sha.v040 }
        LocalTags = @('v0.4.0')
        Ancestor = $true
        Describe = 'v0.4.0'
        Log = @(
            "$($sha.m132)`tMerge pull request #132 from test-owner/codex/traffic-ai"
            "$($sha.m135)`tKeep the newest output when a session log is full (#135)"
            "$($sha.rebase)`tRebased commit without a number"
            "$($sha.direct)`tDirect commit"
            "$($sha.release)`tMerge pull request #137 from test-owner/feature/release-0.4.0"
        )
        Pulls = @{
            132 = Pull 132 'Traffic and AI rivals in solo free roam' 'codex/traffic-ai' @('kind:feature') "## Summary`nCode.`n`n## Release note`nAmbient cars drive Ventura Bay.`n`n## Validation`nTests."
            135 = Pull 135 'Keep the newest output when a session log is full' 'bugfix/logs-keep-newest'
            136 = Pull 136 'Roaming rivals as a count up to 15' 'feature/ai-amounts'
            137 = Pull 137 'Release 0.4.0: bump the bundle version' 'feature/release-0.4.0'
        }
        CommitPulls = @{ $sha.rebase = (ConvertTo-Json -Depth 6 -InputObject @(@{ number = 136; merged_at = '2026-10-20T00:00:00Z' })) }
        Releases = @{ 'v0.4.0' = $previousNotes }
        Latest = 'v0.4.0'
        Files = @{}
        CiRuns = '1'
        FailGh = $null
        FailGit = $null
    }
}
function global:git {
    $s = $global:ReleaseTestState
    $global:LASTEXITCODE = 0
    if ($s.FailGit -and $args[0] -eq $s.FailGit) { $global:LASTEXITCODE = 128; return }
    switch ($args[0]) {
        'ls-remote' {
            $found = @($args | Where-Object { $_ -like 'refs/*' -and $s.Remote.ContainsKey($_) } |
                ForEach-Object { "$($s.Remote[$_])`t$_" })
            if ($found.Count -eq 0 -and $args -contains '--exit-code') { $global:LASTEXITCODE = 2 }
            return $found
        }
        'rev-parse' {
            $tag = ($args[-1] -replace '^refs/tags/', '') -replace '\^\{commit\}$', ''
            if ($s.LocalTags -notcontains $tag) { $global:LASTEXITCODE = 1 }
            return
        }
        'merge-base' { if (!$s.Ancestor) { $global:LASTEXITCODE = 1 }; return }
        'rev-list' {
            if ($s.Ancestor) { return @($sha.other, $sha.release, $sha.v040) }
            return @($sha.other, $sha.v040)
        }
        'describe' { return $s.Describe }
        'log' { return $s.Log }
        'show' {
            if ($s.Files.ContainsKey($args[1])) { return ($s.Files[$args[1]] -split "`n") }
            $global:LASTEXITCODE = 128
            return
        }
    }
    throw "Unexpected git $args"
}
function global:gh {
    $s = $global:ReleaseTestState
    $global:LASTEXITCODE = 0
    if ($args[0] -ne 'api') { throw "Unexpected gh $args" }
    $path = $args[1]
    if ($s.FailGh -and $path -match $s.FailGh) { $global:LASTEXITCODE = 1; return }
    if ($path -match '/commits/([0-9a-f]{40})/pulls$') {
        if ($s.CommitPulls.ContainsKey($Matches[1])) { return $s.CommitPulls[$Matches[1]] }
        return '[]'
    }
    if ($path -match '/pulls/([0-9]+)$') {
        $number = [int]$Matches[1]
        if (!$s.Pulls.ContainsKey($number)) { $global:LASTEXITCODE = 1; return }
        return (ConvertTo-Json -Depth 6 -InputObject $s.Pulls[$number])
    }
    if ($path -match '/releases/tags/(.+)$') {
        if (!$s.Releases.ContainsKey($Matches[1])) { $global:LASTEXITCODE = 1; return }
        return ($s.Releases[$Matches[1]] -split "`r?`n")
    }
    if ($path -match '/releases/latest$') {
        if (!$s.Latest) { $global:LASTEXITCODE = 1; return }
        return $s.Latest
    }
    if ($path -match '/actions/workflows/ci\.yml/runs\?') { return $s.CiRuns }
    throw "Unexpected gh $args"
}

$toml = "[package]`nname = `"nfs-afterhours-bundle`"`nversion = `"0.4.0`"`nedition.workspace = true`n`n[dependencies]`nserde = { version = `"1.0.0`" }`n"
$lock = "version = 4`n`n[[package]]`nname = `"nfs-afterhours`"`nversion = `"0.4.0`"`n`n[[package]]`nname = `"nfs-afterhours-bundle`"`nversion = `"0.4.0`"`ndependencies = [`n `"nfs-afterhours`",`n]`n"
function Write-Repository {
    [IO.File]::WriteAllText((Join-Path $root 'crates/bundle/Cargo.toml'), $toml)
    [IO.File]::WriteAllText((Join-Path $root 'Cargo.lock'), $lock)
    foreach ($file in @($bodyFile, $outputFile, $notesFile)) { Remove-Item -LiteralPath $file -ErrorAction SilentlyContinue }
}
function Read-Text([string]$Path) { [IO.File]::ReadAllText($Path) }

$prepare = Join-Path $PSScriptRoot 'Prepare-Release.ps1'
$select = Join-Path $PSScriptRoot 'Select-ReleaseTag.ps1'
$find = Join-Path $PSScriptRoot 'Find-ReleaseNotes.ps1'

try {
    # --- Versions ---
    foreach ($valid in @('0.4.0', '1.20.300', '0.5.0-rc.1')) { Assert-True (Test-ReleaseVersion $valid) "valid $valid" }
    foreach ($invalid in @('', 'v0.4.0', '0.4', '01.2.3', '0.4.0.1', '0.4.0 ', '--draft')) { Assert-True (!(Test-ReleaseVersion $invalid)) "invalid '$invalid'" }
    $ordered = @('0.3.9', '0.4.0-rc.1', '0.4.0-rc.2', '0.4.0-rc.10', '0.4.0', '0.4.1', '0.10.0', '1.0.0')
    for ($i = 1; $i -lt $ordered.Count; $i++) {
        Assert-Equal (Compare-ReleaseVersion $ordered[$i - 1] $ordered[$i]) -1 "$($ordered[$i - 1]) < $($ordered[$i])"
        Assert-Equal (Compare-ReleaseVersion $ordered[$i] $ordered[$i - 1]) 1 "$($ordered[$i]) > $($ordered[$i - 1])"
    }
    Assert-Equal (Compare-ReleaseVersion 'v0.4.0' '0.4.0') 0 'a leading v is ignored'
    Assert-Equal (Get-NextPatchVersion '0.4.0') '0.4.1' 'next patch'
    Assert-Equal (Get-NextPatchVersion '0.5.0-rc.1') '0.5.0' 'a pre-release is followed by its release'
    Write-Output 'Passed: versions validate, order the semver way and suggest the next patch'

    # --- Bundle version files ---
    Assert-Equal (Get-BundleVersion $toml) '0.4.0' 'bundle version'
    $bumped = Set-BundleVersion -CargoToml $toml -CargoLock $lock -Version '0.5.0'
    Assert-Equal (Get-BundleVersion $bumped.CargoToml) '0.5.0' 'bumped manifest'
    Assert-True ($bumped.CargoToml.Contains('serde = { version = "1.0.0" }')) 'dependency versions stay'
    Assert-True ($bumped.CargoLock.Contains("name = `"nfs-afterhours-bundle`"`nversion = `"0.5.0`"")) 'bumped lock entry'
    Assert-True ($bumped.CargoLock.Contains("name = `"nfs-afterhours`"`nversion = `"0.4.0`"")) 'other lock entries stay'
    $crlf = Set-BundleVersion -CargoToml ($toml -replace "`n", "`r`n") -CargoLock ($lock -replace "`n", "`r`n") -Version '0.5.0'
    Assert-True ($crlf.CargoLock.Contains("name = `"nfs-afterhours-bundle`"`r`nversion = `"0.5.0`"")) 'CRLF lock'
    Write-Output 'Passed: only the bundle version changes, with either line ending'
    Assert-Refused 'lock without the bundle' { Set-BundleVersion -CargoToml $toml -CargoLock ($lock -replace 'nfs-afterhours-bundle', 'other') -Version '0.5.0' } 'nfs-afterhours-bundle'
    Assert-Refused 'manifest without a package version' { Set-BundleVersion -CargoToml '[workspace]' -CargoLock $lock -Version '0.5.0' } 'Cargo.toml'
    Assert-Refused 'invalid new version' { Set-BundleVersion -CargoToml $toml -CargoLock $lock -Version '0.5' } 'release version'

    # --- Pull requests into sections ---
    Assert-Equal (Get-PullRequestNumber 'Merge pull request #137 from afterhours-crew/feature/release-0.4.0') 137 'merge subject'
    Assert-Equal (Get-PullRequestNumber 'Keep the newest output (#135)') 135 'squash subject'
    Assert-Equal (Get-PullRequestNumber 'Traffic (#54) and more') $null 'an issue reference mid-title is not a merge'
    $section = { param($Head, $Labels = @()) Get-ReleaseSection ([pscustomobject]@{ Head = $Head; Labels = @($Labels) }) }
    Assert-Equal (& $section 'codex/x' @('kind:bug', 'kind:feature')) 'Fixed' 'kind:bug label'
    Assert-Equal (& $section 'bugfix/x' @('kind:feature')) 'Added' 'a label beats the branch'
    Assert-Equal (& $section 'feature/x' @('kind:change')) 'Changed' 'kind:change label'
    Assert-Equal (& $section 'feature/x') 'Added' 'feature branch'
    Assert-Equal (& $section 'bugfix/x') 'Fixed' 'bugfix branch'
    Assert-Equal (& $section 'codex/x' @('kind:research')) 'Changed' 'anything else'
    Assert-Equal (& $section 'feature/release-0.4.0' @('kind:feature')) 'Skip' 'release pull request'
    Assert-Equal (& $section 'feature/releases-page') 'Added' 'a feature merely named release'
    Write-Output 'Passed: pull requests sort by kind: label, then branch prefix; release pull requests are left out'

    $body = "## Summary`r`nInternal.`r`n`r`n## Release note`r`nCars appear.`r`nThey leave.`r`n`r`n## Validation`r`nTests."
    Assert-Equal (Get-ReleaseNoteText $body) "Cars appear.`nThey leave." 'release note section'
    Assert-Equal (Get-ReleaseNoteText "## Summary`nNo note.") $null 'no release note'
    $entry = Format-ReleaseEntry ([pscustomobject]@{ Number = 9; Title = 'Add traffic'; Body = $body; Url = 'u9' })
    Assert-Equal $entry "- **Add traffic.** Cars appear.`n  They leave. ([#9](u9))" 'entry with a note'
    $entry = Format-ReleaseEntry ([pscustomobject]@{ Number = 8; Title = 'Fix it!'; Body = ''; Url = 'u8' })
    Assert-Equal $entry '- **Fix it!** ([#8](u8))' 'entry from the title'
    Write-Output 'Passed: entries use a pull request''s release note section, else its title'

    $limits = Get-LeadingList (Get-MarkdownSection $previousNotes 'Known limits')
    Assert-True ($limits.StartsWith('- **Traffic stops at 3x.**') -and $limits.Contains('  - Nested detail.') -and
        $limits.Contains('  .\Afterhours.exe settings') -and $limits.EndsWith('  ```')) 'known limits list'
    Assert-True (!$limits.Contains('early tester release')) 'closing paragraphs are not limits'
    Assert-Equal (Get-LeadingList 'A paragraph first.') $null 'no leading list'
    Assert-Equal (Get-MarkdownSection $previousNotes 'Fixed') $null 'missing section'
    Write-Output 'Passed: the previous Known limits list is taken without the closing paragraphs'

    $prs = @(
        [pscustomobject]@{ Number = 136; Title = 'Rivals'; Body = ''; Url = 'u136'; Labels = @(); Head = 'feature/ai' }
        [pscustomobject]@{ Number = 135; Title = 'Logs'; Body = ''; Url = 'u135'; Labels = @(); Head = 'bugfix/logs' }
        [pscustomobject]@{ Number = 132; Title = 'Traffic'; Body = ''; Url = 'u132'; Labels = @(); Head = 'feature/traffic' }
        [pscustomobject]@{ Number = 137; Title = 'Release'; Body = ''; Url = 'u137'; Labels = @(); Head = 'feature/release-0.4.0' }
    )
    $notes = New-ServerReleaseNotes -Version '0.5.0' -Repository $repository -PullRequests $prs -KnownLimits '- **No cops yet.**'
    $added = $notes.IndexOf('## Added'); $fixed = $notes.IndexOf('## Fixed')
    Assert-True ($added -gt 0 -and $fixed -gt $added -and $notes.IndexOf('## Changed') -lt 0) 'sections in order, empty ones left out'
    Assert-True ($notes.IndexOf('([#132](u132))') -lt $notes.IndexOf('([#136](u136))')) 'entries by number'
    Assert-True (!$notes.Contains('#137')) 'release pull request left out'
    Assert-True ($notes.Contains("## Known limits`n`n- **No cops yet.**")) 'known limits'
    Assert-True ($notes.Contains("https://github.com/$repository/blob/v0.5.0/docs/TESTER-QUICKSTART.md")) 'links name the tag'
    Assert-True ($notes.Contains('Download **Afterhours-windows-x64.zip**')) 'download instructions'
    $empty = New-ServerReleaseNotes -Version '0.5.0' -Repository $repository
    Assert-True ($empty.Contains("## Known limits`n`n- <!--")) 'placeholder limits'
    Write-Output 'Passed: notes follow the tester release layout'

    $prBody = New-ReleasePullRequestBody -Notes "Line one`n`n## Added" -Review 'Check it.'
    Assert-Equal (Get-MarkedReleaseNotes $prBody) "Line one`n`n## Added" 'marked notes'
    Assert-Equal (Get-MarkedReleaseNotes ($prBody -replace "`n", "`r`n")) "Line one`n`n## Added" 'notes edited in a browser'
    Assert-True (!(Get-MarkedReleaseNotes $prBody).Contains('Check it.')) 'review text stays out'
    Assert-Equal (Get-MarkedReleaseNotes 'No markers.') $null 'no markers'
    Assert-Equal (Get-MarkedReleaseNotes "<!-- release-notes:start -->`n `n<!-- release-notes:end -->") $null 'empty notes'
    Write-Output 'Passed: the notes between the markers survive editing; the review section stays out'

    # --- Prepare-Release.ps1 ---
    $env:GITHUB_OUTPUT = $outputFile
    Reset-State
    Write-Repository
    & $prepare -Repository $repository -BodyFile $bodyFile -Root $root | Out-Null
    Assert-Equal (Get-BundleVersion (Read-Text (Join-Path $root 'crates/bundle/Cargo.toml'))) '0.4.1' 'blank version is the next patch'
    Assert-True ((Read-Text (Join-Path $root 'Cargo.lock')).Contains("name = `"nfs-afterhours-bundle`"`nversion = `"0.4.1`"")) 'lock bumped'
    $written = Read-Text $bodyFile
    $notes = Get-MarkedReleaseNotes $written
    Assert-True ($notes.Contains('Afterhours 0.4.1 for Windows testers.')) 'summary line'
    Assert-True ($notes.Contains("## Added`n`n- **Traffic and AI rivals in solo free roam.** Ambient cars drive Ventura Bay. ([#132]")) 'label and release note'
    Assert-True ($notes.Contains("/pull/132))`n- **Roaming rivals as a count up to 15.** ([#136]")) 'rebase merge found through the API, sorted by branch'
    Assert-True ($notes.Contains("## Fixed`n`n- **Keep the newest output when a session log is full.** ([#135]")) 'squash merge'
    Assert-True (!$notes.Contains('#137') -and !$notes.Contains('Direct commit')) 'release pull request and direct commits left out'
    Assert-Equal (Get-LeadingList (Get-MarkdownSection $notes 'Known limits')) (Get-LeadingList (Get-MarkdownSection $previousNotes 'Known limits')) 'previous known limits'
    Assert-True ($written.Contains('0.4.0 → 0.4.1') -and $written.Contains('**Left out** (release pull requests): #137.') -and
        $written.Contains("copied from v0.4.0's release notes")) 'review section'
    Assert-Equal ((Get-Content -LiteralPath $outputFile) -join ',') 'version=0.4.1,tag=v0.4.1,branch=feature/release-0.4.1,previous=v0.4.0' 'workflow outputs'
    Write-Output 'Passed: Prepare-Release bumps the bundle and drafts notes from merged pull requests'

    Write-Repository
    & $prepare -Version 'v0.5.0' -Repository $repository -BodyFile $bodyFile -Root $root | Out-Null
    Assert-Equal (Get-BundleVersion (Read-Text (Join-Path $root 'crates/bundle/Cargo.toml'))) '0.5.0' 'named version'
    Write-Output 'Passed: a named version, with or without its v'

    Reset-State
    $global:ReleaseTestState.Remote['refs/heads/feature/release-0.4.1'] = $sha.other
    Write-Repository
    & $prepare -Repository $repository -BodyFile $bodyFile -Root $root -Preview | Out-Null
    Assert-Equal (Read-Text (Join-Path $root 'crates/bundle/Cargo.toml')) $toml 'a preview leaves the manifest'
    Assert-Equal (Read-Text (Join-Path $root 'Cargo.lock')) $lock 'a preview leaves the lock'
    Assert-True ((Get-MarkedReleaseNotes (Read-Text $bodyFile)).Contains('Afterhours 0.4.1 for Windows testers.')) 'preview notes'
    Write-Output 'Passed: a preview writes only the description, even with a release branch open'

    Reset-State
    $global:ReleaseTestState.LocalTags = @()
    $global:ReleaseTestState.Describe = 'v0.3.0'
    $global:ReleaseTestState.Releases = @{}
    Write-Repository
    & $prepare -Repository $repository -BodyFile $bodyFile -Root $root 3>$null | Out-Null
    Assert-True ((Read-Text $outputFile).Contains('previous=v0.3.0')) 'nearest tag when the current one is missing'
    Assert-True ((Read-Text $bodyFile).Contains("## Known limits`n`n- <!--")) 'no previous release'
    Assert-True ((Read-Text $bodyFile).Contains("v0.3.0's release notes have none to copy")) 'review says the limits are missing'
    Write-Output 'Passed: without the current version''s tag, the nearest version tag starts the notes'

    $refusals = [ordered]@{
        'version that is not newer' = @{ Version = '0.4.0'; Pattern = 'not newer' }
        'older version' = @{ Version = '0.3.9'; Pattern = 'not newer' }
        'malformed version' = @{ Version = '0.5'; Pattern = 'version such as' }
        'option-like version' = @{ Version = '--draft'; Pattern = 'version such as' }
        'existing tag' = @{ Version = '0.4.2'; Remote = 'refs/tags/v0.4.2'; Pattern = 'already exists' }
        'existing release branch' = @{ Version = '0.4.2'; Remote = 'refs/heads/feature/release-0.4.2'; Pattern = 'close its pull request' }
        'remote check failure' = @{ Version = '0.4.2'; FailGit = 'ls-remote'; Pattern = 'Cannot check' }
        'pull request lookup failure' = @{ Version = '0.4.2'; FailGh = '/pulls/'; Pattern = 'Cannot' }
    }
    foreach ($name in $refusals.Keys) {
        $case = $refusals[$name]
        Reset-State
        if ($case.ContainsKey('Remote')) { $global:ReleaseTestState.Remote[$case.Remote] = $sha.other }
        if ($case.ContainsKey('FailGit')) { $global:ReleaseTestState.FailGit = $case.FailGit }
        if ($case.ContainsKey('FailGh')) { $global:ReleaseTestState.FailGh = $case.FailGh }
        Write-Repository
        Assert-Refused "Prepare-Release refuses: $name" { & $prepare -Version $case.Version -Repository $repository -BodyFile $bodyFile -Root $root } $case.Pattern
        Assert-Equal (Read-Text (Join-Path $root 'crates/bundle/Cargo.toml')) $toml "$name leaves the manifest"
        Assert-True (!(Test-Path -LiteralPath $bodyFile)) "$name writes no description"
    }

    # --- Select-ReleaseTag.ps1 ---
    function Set-Versions([string]$Now, [string]$Before) {
        $global:ReleaseTestState.Files = @{
            "$($sha.release):crates/bundle/Cargo.toml" = $toml.Replace('0.4.0', $Now)
            "$($sha.release)^1:crates/bundle/Cargo.toml" = $toml.Replace('0.4.0', $Before)
        }
    }
    Reset-State
    Set-Versions '0.4.1' '0.4.0'
    Remove-Item -LiteralPath $outputFile -ErrorAction SilentlyContinue
    & $select -Commit $sha.release -Repository $repository | Out-Null
    Assert-Equal (Read-Text $outputFile).Trim() 'tag=v0.4.1' 'tag output'
    Write-Output 'Passed: a commit that changes the bundle version is tagged'

    Remove-Item -LiteralPath $outputFile
    & $select -Commit $sha.release -Repository $repository -RequireCi | Out-Null
    Assert-Equal (Read-Text $outputFile).Trim() 'tag=v0.4.1' 'manual run with passing CI'
    Write-Output 'Passed: a manual run tags after CI passed on the commit'

    $quiet = [ordered]@{
        'unchanged version' = { Set-Versions '0.4.0' '0.4.0' }
        'tag already on the commit' = { Set-Versions '0.4.1' '0.4.0'; $global:ReleaseTestState.Remote['refs/tags/v0.4.1'] = 'a' * 40; $global:ReleaseTestState.Remote['refs/tags/v0.4.1^{}'] = $sha.release }
    }
    foreach ($name in $quiet.Keys) {
        Reset-State
        & $quiet[$name]
        Remove-Item -LiteralPath $outputFile -ErrorAction SilentlyContinue
        & $select -Commit $sha.release -Repository $repository | Out-Null
        Assert-True (!(Test-Path -LiteralPath $outputFile)) "$name tags nothing"
        Write-Output "Passed: Select-ReleaseTag tags nothing for $name"
    }

    $refusals = [ordered]@{
        'abbreviated commit' = @{ Commit = 'abc123'; Pattern = 'full commit' }
        'commit off main''s first-parent history' = @{ Setup = { $global:ReleaseTestState.Ancestor = $false }; Pattern = "not on main's first-parent history" }
        'tag on another commit' = @{ Setup = { $global:ReleaseTestState.Remote['refs/tags/v0.4.1'] = $sha.other }; Pattern = 'already names' }
        'version older than the published one' = @{ Setup = { $global:ReleaseTestState.Latest = 'v0.5.0' }; Pattern = 'not newer' }
        'manual run without passing CI' = @{ Setup = { $global:ReleaseTestState.CiRuns = '0' }; RequireCi = $true; Pattern = 'CI has not passed' }
        'unreadable manifest' = @{ Setup = { $global:ReleaseTestState.Files = @{} }; Pattern = 'Cannot read' }
    }
    foreach ($name in $refusals.Keys) {
        $case = $refusals[$name]
        Reset-State
        Set-Versions '0.4.1' '0.4.0'
        if ($case.ContainsKey('Setup')) { & $case.Setup }
        $commit = if ($case.ContainsKey('Commit')) { $case.Commit } else { $sha.release }
        $requireCi = $case.ContainsKey('RequireCi')
        Remove-Item -LiteralPath $outputFile -ErrorAction SilentlyContinue
        Assert-Refused "Select-ReleaseTag refuses: $name" { & $select -Commit $commit -Repository $repository -RequireCi:$requireCi } $case.Pattern
        Assert-True (!(Test-Path -LiteralPath $outputFile)) "$name outputs no tag"
    }

    # --- Find-ReleaseNotes.ps1 ---
    $merged = @{ number = 140; merged_at = '2026-10-21T00:00:00Z'; merge_commit_sha = $sha.release; body = (New-ReleasePullRequestBody -Notes "Afterhours 0.4.1.`r`n`r`n## Fixed" -Review 'Reviewer only.') }
    Reset-State
    $global:ReleaseTestState.CommitPulls[$sha.release] = ConvertTo-Json -Depth 6 -InputObject @($merged)
    Write-Repository
    & $find -Revision $sha.release -Repository $repository -Output $notesFile | Out-Null
    Assert-Equal (Read-Text $notesFile) "Afterhours 0.4.1.`n`n## Fixed`n" 'notes from the merged pull request'
    Write-Output 'Passed: the draft takes the notes of the pull request that merged the built commit'

    $cases = [ordered]@{
        'no pull request' = { }
        'a pull request merged elsewhere' = { $global:ReleaseTestState.CommitPulls[$sha.release] = ConvertTo-Json -Depth 6 -InputObject @(@{ number = 141; merged_at = 'x'; merge_commit_sha = $sha.other; body = $merged.body }) }
        'a pull request without notes' = { $global:ReleaseTestState.CommitPulls[$sha.release] = ConvertTo-Json -Depth 6 -InputObject @(@{ number = 142; merged_at = 'x'; merge_commit_sha = $sha.release; body = 'Bump.' }) }
        'a failed lookup' = { $global:ReleaseTestState.FailGh = '/pulls$' }
    }
    foreach ($name in $cases.Keys) {
        Reset-State
        & $cases[$name]
        Write-Repository
        & $find -Revision $sha.release -Repository $repository -Output $notesFile 3>$null | Out-Null
        Assert-True (!(Test-Path -LiteralPath $notesFile)) "$name writes no notes"
        Write-Output "Passed: Find-ReleaseNotes leaves the default text for $name"
    }
    Assert-Refused 'Find-ReleaseNotes refuses an abbreviated commit' { & $find -Revision 'abc' -Repository $repository -Output $notesFile } 'commit SHA'
} finally {
    $env:GITHUB_OUTPUT = $savedOutput
    foreach ($file in @('crates/bundle/Cargo.toml', 'Cargo.lock', 'github-output.txt', 'release-pr.md', 'notes.md')) {
        Remove-Item -LiteralPath (Join-Path $root $file) -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath (Join-Path $root 'crates/bundle'), (Join-Path $root 'crates'), $root
    Remove-Item -Path Function:\git, Function:\gh
    Remove-Variable -Name ReleaseTestState -Scope Global -ErrorAction SilentlyContinue
}
$global:LASTEXITCODE = 0
