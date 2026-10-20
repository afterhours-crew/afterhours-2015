# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Release automation helpers for Prepare-Release.ps1, Select-ReleaseTag.ps1
# and Find-ReleaseNotes.ps1, tested offline by Test-Release.ps1. The text
# functions are pure; Get-MergedPullRequest and Get-PreviousKnownLimit ask
# git and gh, which the tests replace.
Set-StrictMode -Version Latest

$script:VersionPattern = '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$'
$script:NotesStart = '<!-- release-notes:start -->'
$script:NotesEnd = '<!-- release-notes:end -->'

function Test-ReleaseVersion {
    param([AllowEmptyString()][string]$Version)
    return $Version -match $script:VersionPattern
}

function Split-ReleaseVersion {
    param([Parameter(Mandatory)][string]$Version)
    $text = $Version -replace '^v', ''
    if ($text -notmatch $script:VersionPattern) { throw "Not a release version: $Version" }
    $pre = @()
    if ($Matches[4]) { $pre = @($Matches[4].Substring(1) -split '\.') }
    [pscustomobject]@{
        Core = @([long]$Matches[1], [long]$Matches[2], [long]$Matches[3])
        Pre = $pre
    }
}

# Negative, zero or positive like a sort comparator, following semver:
# a release outranks its pre-releases, numeric identifiers compare as numbers.
function Compare-ReleaseVersion {
    param([Parameter(Mandatory)][string]$Left, [Parameter(Mandatory)][string]$Right)
    $a = Split-ReleaseVersion $Left
    $b = Split-ReleaseVersion $Right
    for ($i = 0; $i -lt 3; $i++) {
        if ($a.Core[$i] -ne $b.Core[$i]) { return [Math]::Sign($a.Core[$i] - $b.Core[$i]) }
    }
    if ($a.Pre.Count -eq 0 -or $b.Pre.Count -eq 0) { return [Math]::Sign($b.Pre.Count - $a.Pre.Count) }
    for ($i = 0; $i -lt [Math]::Min($a.Pre.Count, $b.Pre.Count); $i++) {
        $x = $a.Pre[$i]; $y = $b.Pre[$i]
        $numericX = $x -match '^[0-9]+$'; $numericY = $y -match '^[0-9]+$'
        if ($numericX -and $numericY) {
            if ([long]$x -ne [long]$y) { return [Math]::Sign([long]$x - [long]$y) }
        } elseif ($numericX) { return -1 }
        elseif ($numericY) { return 1 }
        else {
            $order = [string]::CompareOrdinal($x, $y)
            if ($order -ne 0) { return [Math]::Sign($order) }
        }
    }
    return [Math]::Sign($a.Pre.Count - $b.Pre.Count)
}

# The release after $Version when none is named: the next patch, or the
# release itself for a pre-release.
function Get-NextPatchVersion {
    param([Parameter(Mandatory)][string]$Version)
    $parts = Split-ReleaseVersion $Version
    $patch = if ($parts.Pre.Count -gt 0) { $parts.Core[2] } else { $parts.Core[2] + 1 }
    return "$($parts.Core[0]).$($parts.Core[1]).$patch"
}

$script:BundleTomlPattern = '(?m)(^\[package\][^\[]*?^version\s*=\s*")([^"]+)(")'
$script:BundleLockPattern = '(?m)(^\[\[package\]\]\r?\nname = "nfs-afterhours-bundle"\r?\nversion = ")([^"]+)(")'

function Get-BundleVersion {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$CargoToml)
    $found = [regex]::Matches($CargoToml, $script:BundleTomlPattern)
    if ($found.Count -ne 1) { throw 'crates/bundle/Cargo.toml has no single [package] version.' }
    return $found[0].Groups[2].Value
}

# The bundle version lives in crates/bundle/Cargo.toml and its Cargo.lock
# entry; internal library versions stay as they are.
function Set-BundleVersion {
    param(
        [Parameter(Mandatory)][string]$CargoToml,
        [Parameter(Mandatory)][string]$CargoLock,
        [Parameter(Mandatory)][string]$Version
    )
    if (!(Test-ReleaseVersion $Version)) { throw "Not a release version: $Version" }
    $replacement = '${1}' + $Version + '${3}'
    if ([regex]::Matches($CargoToml, $script:BundleTomlPattern).Count -ne 1) {
        throw 'crates/bundle/Cargo.toml has no single [package] version.'
    }
    if ([regex]::Matches($CargoLock, $script:BundleLockPattern).Count -ne 1) {
        throw 'Cargo.lock has no single nfs-afterhours-bundle entry.'
    }
    [pscustomobject]@{
        CargoToml = [regex]::Replace($CargoToml, $script:BundleTomlPattern, $replacement)
        CargoLock = [regex]::Replace($CargoLock, $script:BundleLockPattern, $replacement)
    }
}

# GitHub's merge commit subject, or a squash merge's "Title (#N)".
function Get-PullRequestNumber {
    param([AllowEmptyString()][string]$Subject)
    if ($Subject -match '^Merge pull request #([0-9]+) from ') { return [int]$Matches[1] }
    if ($Subject -match '\(#([0-9]+)\)\s*$') { return [int]$Matches[1] }
    return $null
}

function ConvertTo-ReleasePullRequest {
    param([Parameter(Mandatory)]$Pull)
    [pscustomobject]@{
        Number = [int]$Pull.number
        Title = [string]$Pull.title
        Body = if ($null -eq $Pull.body) { '' } else { [string]$Pull.body }
        Url = [string]$Pull.html_url
        Labels = @($Pull.labels | ForEach-Object { [string]$_.name })
        Head = [string]$Pull.head.ref
    }
}

# Where a pull request goes in the notes: its kind: label first, then its
# branch prefix. Release pull requests themselves are left out.
function Get-ReleaseSection {
    param([Parameter(Mandatory)]$PullRequest)
    if ($PullRequest.Head -match '^(feature/)?release[-/]') { return 'Skip' }
    if ($PullRequest.Labels -contains 'kind:bug') { return 'Fixed' }
    if ($PullRequest.Labels -contains 'kind:feature') { return 'Added' }
    if ($PullRequest.Labels -contains 'kind:change') { return 'Changed' }
    if ($PullRequest.Head -match '^(bugfix|fix)/') { return 'Fixed' }
    if ($PullRequest.Head -match '^feature/') { return 'Added' }
    return 'Changed'
}

# The text under "## Heading" (any level 2 heading, case-insensitive) up to
# the next level 1 or 2 heading, or $null.
function Get-MarkdownSection {
    param([AllowEmptyString()][string]$Markdown, [Parameter(Mandatory)][string]$Heading)
    $lines = ($Markdown -replace "`r`n", "`n") -split "`n"
    $inside = $false
    $out = [Collections.Generic.List[string]]::new()
    foreach ($line in $lines) {
        if ($line -match '^#{1,2}\s') {
            if ($inside) { break }
            $inside = $line -match ('^##\s+' + [regex]::Escape($Heading) + '\s*$')
            continue
        }
        if ($inside) { $out.Add($line) }
    }
    $text = ($out -join "`n").Trim()
    if ($text) { return $text } else { return $null }
}

# The list a section opens with, without the paragraphs that may follow it
# (a release's "Known limits" is followed by its closing paragraphs).
function Get-LeadingList {
    param([AllowEmptyString()][string]$Text)
    $out = [Collections.Generic.List[string]]::new()
    $blanks = 0
    foreach ($line in (($Text -replace "`r`n", "`n") -split "`n")) {
        if ($out.Count -eq 0) {
            if ($line -match '^\s*$') { continue }
            if ($line -notmatch '^[-*+] ') { return $null }
            $out.Add($line)
            continue
        }
        if ($line -match '^\s*$') { $blanks++; continue }
        $continues = $line -match '^[-*+] ' -or $line -match '^\s+\S' -or $blanks -eq 0
        if (!$continues) { break }
        for (; $blanks -gt 0; $blanks--) { $out.Add('') }
        $out.Add($line)
    }
    if ($out.Count -eq 0) { return $null }
    return ($out -join "`n")
}

# A pull request may carry its own tester-facing text under a
# "## Release note" heading; otherwise its title is the entry.
function Get-ReleaseNoteText {
    param([AllowEmptyString()][string]$Body)
    foreach ($heading in @('Release note', 'Release notes', 'For testers')) {
        $text = Get-MarkdownSection $Body $heading
        if ($text) { return $text }
    }
    return $null
}

function Format-ReleaseEntry {
    param([Parameter(Mandatory)]$PullRequest)
    $title = $PullRequest.Title.Trim()
    if ($title -notmatch '[.!?]$') { $title += '.' }
    $line = "- **$title**"
    $note = Get-ReleaseNoteText $PullRequest.Body
    if ($note) { $line += ' ' + ($note -replace "`n", "`n  ") }
    return "$line ([#$($PullRequest.Number)]($($PullRequest.Url)))"
}

# Tester-facing notes in the shape of the hand-written v0.4.0 notes. The
# owner edits the summary line and the entries in the release pull request.
function New-ServerReleaseNotes {
    param(
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$Repository,
        [AllowEmptyCollection()][object[]]$PullRequests = @(),
        [AllowEmptyString()][string]$KnownLimits = ''
    )
    $tag = "v$Version"
    $site = "https://github.com/$Repository"
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('<!-- Replace the next line with one sentence on what this release brings. -->')
    $lines.Add("Afterhours $Version for Windows testers.")
    $lines.Add('')
    $lines.Add('Download **Afterhours-windows-x64.zip** below, extract it and place **Afterhours.exe** beside **NFS16.exe**, replacing the old one. Close the game and Afterhours first. Your saves in `%LOCALAPPDATA%\Afterhours2015` carry over; back that folder up before trying a new version.')
    foreach ($section in @('Added', 'Changed', 'Fixed')) {
        $entries = @($PullRequests | Where-Object { (Get-ReleaseSection $_) -eq $section } | Sort-Object Number)
        if ($entries.Count -eq 0) { continue }
        $lines.Add('')
        $lines.Add("## $section")
        $lines.Add('')
        foreach ($entry in $entries) { $lines.Add((Format-ReleaseEntry $entry)) }
    }
    $lines.Add('')
    $lines.Add('## Known limits')
    $lines.Add('')
    if ($KnownLimits) { $lines.Add($KnownLimits) }
    else { $lines.Add('- <!-- List what testers should not expect yet. -->') }
    $lines.Add('')
    $lines.Add('This is an early tester release, not the complete game. It needs the supported, previously activated 64-bit Windows installation (EA or Steam) and does not activate the game. No game content is distributed.')
    $lines.Add('')
    $lines.Add('For feedback, include the release version, what you did, what happened and what you expected. Check logs for personal information before sharing them, and never upload game files, saves or activation credentials.')
    $lines.Add('')
    $lines.Add("[Quickstart]($site/blob/$tag/docs/TESTER-QUICKSTART.md) · [Command guide]($site/blob/$tag/docs/LOCAL-LAUNCHER.md) · [Gameplay roadmap]($site/issues/62)")
    return ($lines -join "`n")
}

# The release pull request's description: the notes between markers, then
# a review section the draft leaves out.
function New-ReleasePullRequestBody {
    param([Parameter(Mandatory)][string]$Notes, [AllowEmptyString()][string]$Review = '')
    $body = "$script:NotesStart`n$($Notes.Trim())`n$script:NotesEnd"
    if ($Review) { $body += "`n`n---`n`n$($Review.Trim())" }
    return "$body`n"
}

# The notes between the markers, with line endings as the browser editor
# may have changed them normalized; $null when there are none.
function Get-MarkedReleaseNotes {
    param([AllowEmptyString()][string]$Body)
    $pattern = '(?s)' + [regex]::Escape($script:NotesStart) + '(.*?)' + [regex]::Escape($script:NotesEnd)
    $found = [regex]::Match(($Body -replace "`r`n", "`n"), $pattern)
    if (!$found.Success) { return $null }
    $notes = $found.Groups[1].Value.Trim()
    if ($notes) { return $notes } else { return $null }
}

# Pull requests merged into the first-parent history after $Since up to
# $Head, oldest first. A merge subject names its pull request. A squash
# subject's "(#N)" counts when N is a merged pull request; otherwise N is the
# issue a commit refers to (a stacked pull request's base branch, a rebase
# merge) and the commit is looked up like any other. Commits of no merged
# pull request (direct, or still unmerged) are skipped.
function Get-MergedPullRequest {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$Since,
        [string]$Head = 'HEAD'
    )
    $log = @(git log --first-parent --reverse '--format=%H%x09%s' "$Since..$Head")
    if ($LASTEXITCODE -ne 0) { throw "Cannot list the commits after $Since." }
    $seen = [Collections.Generic.HashSet[int]]::new()
    foreach ($entry in $log) {
        if (!$entry) { continue }
        $sha, $subject = $entry -split "`t", 2
        $number = Get-PullRequestNumber $subject
        $pull = $null
        if ($subject -match '^Merge pull request #') {
            if ($seen.Contains($number)) { continue }
            $text = gh api "repos/$Repository/pulls/$number"
            if ($LASTEXITCODE -ne 0) { throw "Cannot read pull request #$number." }
            $pull = ($text -join "`n") | ConvertFrom-Json
        } elseif ($null -ne $number) {
            if ($seen.Contains($number)) { continue }
            # Not found is an answer here: the number may name an issue.
            $text = gh api "repos/$Repository/pulls/$number" 2>$null
            if ($LASTEXITCODE -eq 0) {
                $pull = ($text -join "`n") | ConvertFrom-Json
                if (!$pull.merged_at) { $pull = $null }
            }
        }
        if ($null -eq $pull) {
            $text = gh api "repos/$Repository/commits/$sha/pulls"
            if ($LASTEXITCODE -ne 0) { throw "Cannot look up the pull request of $sha." }
            $merged = @(($text -join "`n") | ConvertFrom-Json) | Where-Object { $_.merged_at } | Select-Object -First 1
            if ($null -eq $merged) { continue }
            $number = [int]$merged.number
            if ($seen.Contains($number)) { continue }
            $text = gh api "repos/$Repository/pulls/$number"
            if ($LASTEXITCODE -ne 0) { throw "Cannot read pull request #$number." }
            $pull = ($text -join "`n") | ConvertFrom-Json
        }
        [void]$seen.Add($number)
        ConvertTo-ReleasePullRequest $pull
    }
}

# The previous release's "Known limits" list, which mostly still holds.
function Get-PreviousKnownLimit {
    param([Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$Tag)
    $text = gh api "repos/$Repository/releases/tags/$Tag" --jq .body
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "No release found for $Tag; the notes start with an empty Known limits."
        return $null
    }
    return Get-LeadingList (Get-MarkdownSection ($text -join "`n") 'Known limits')
}

# The nearest version tag reachable from $Head, or $null. All of git's output
# is collected: `Select-Object -First` would stop the pipeline before git's
# exit code is recorded and leave the previous command's in $LASTEXITCODE.
function Get-NearestVersionTag {
    param([Parameter(Mandatory)][string]$Head)
    $found = @(git describe --tags --abbrev=0 --match 'v[0-9]*' $Head 2>$null)
    if ($LASTEXITCODE -ne 0 -or $found.Count -eq 0 -or !$found[0]) { return $null }
    return ([string]$found[0]).Trim()
}

Export-ModuleMember -Function Test-ReleaseVersion, Compare-ReleaseVersion, Get-NextPatchVersion,
    Get-BundleVersion, Set-BundleVersion, Get-PullRequestNumber, ConvertTo-ReleasePullRequest,
    Get-ReleaseSection, Get-MarkdownSection, Get-LeadingList, Get-ReleaseNoteText,
    Format-ReleaseEntry, New-ServerReleaseNotes, New-ReleasePullRequestBody,
    Get-MarkedReleaseNotes, Get-MergedPullRequest, Get-PreviousKnownLimit, Get-NearestVersionTag
