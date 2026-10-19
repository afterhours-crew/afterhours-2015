# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Portable, offline regressions. Mock only the Git/GitHub boundary; exercise real
# package files, hashes, identity checks and the production publication script.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Join-Path ([IO.Path]::GetTempPath()) "afterhours-draft-tests-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $root | Out-Null
$global:AfterhoursDraftTestState = @{}
$revision = '1234567890123456789012345678901234567890'
$global:AfterhoursDraftTestState.tagLines = @("$revision`trefs/tags/v0.3.1")
$global:AfterhoursDraftTestState.gitExit = 0
$global:AfterhoursDraftTestState.ghExit = 0
$global:AfterhoursDraftTestState.ghCalls = @()
$global:AfterhoursDraftTestState.lookupExit = 0
$global:AfterhoursDraftTestState.existingRelease = $null
function git {
    $global:LASTEXITCODE = $global:AfterhoursDraftTestState.gitExit
    $global:AfterhoursDraftTestState.tagLines
}
function gh {
    if ($args[0] -eq 'api') {
        $global:LASTEXITCODE = $global:AfterhoursDraftTestState.lookupExit
        @{ data = @{ repository = @{ release = $global:AfterhoursDraftTestState.existingRelease } } } | ConvertTo-Json -Depth 6
        return
    }
    $global:AfterhoursDraftTestState.ghCalls += ,@($args)
    $global:LASTEXITCODE = $global:AfterhoursDraftTestState.ghExit
}
function Write-Package {
    [IO.File]::WriteAllText((Join-Path $root 'Afterhours-windows-x64.zip'), 'constructed archive bytes')
    [IO.File]::WriteAllText((Join-Path $root 'Afterhours-contract.zip'), 'constructed contract bytes')
    $hash = (Get-FileHash -LiteralPath (Join-Path $root 'Afterhours-windows-x64.zip')).Hash.ToLowerInvariant()
    $contractHash = (Get-FileHash -LiteralPath (Join-Path $root 'Afterhours-contract.zip')).Hash.ToLowerInvariant()
    "$hash  Afterhours-windows-x64.zip`n$contractHash  Afterhours-contract.zip" | Set-Content -LiteralPath (Join-Path $root 'SHA256SUMS.txt')
    @{
        release = 'v0.3.1'; revision = $revision; dirty_source = $false
        executable_version = "Afterhours 0.3.1 (v0.3.1 / $revision)"
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'build-info.json')
}
function Assert-Refused([string]$Name, [scriptblock]$Action, [int]$ExpectedCalls = 0) {
    $global:AfterhoursDraftTestState.ghCalls = @()
    $refused = $false
    try { & $Action } catch { $refused = $true }
    if (!$refused -or $global:AfterhoursDraftTestState.ghCalls.Count -ne $ExpectedCalls) { throw "Guard failed: $Name" }
    Write-Output "Passed: $Name"
}
$publisher = Join-Path $PSScriptRoot 'Publish-TerminalDraft.ps1'
$publish = { & $publisher -PackageDirectory $root -Release v0.3.1 -Revision $revision -Repository test-owner/test-repo }
try {
    Write-Package
    & $publish
    if ($global:AfterhoursDraftTestState.ghCalls.Count -ne 1) { throw 'Expected one draft creation.' }
    $call = $global:AfterhoursDraftTestState.ghCalls[0]
    if ($call[0] -ne 'release' -or $call[1] -ne 'create' -or $call[2] -ne 'v0.3.1' -or
        $call -notcontains '--draft' -or $call -notcontains '--verify-tag' -or
        $call -contains '--clobber' -or $call -contains '--latest') { throw 'Unexpected publication command.' }
    foreach ($asset in @('Afterhours-windows-x64.zip', 'SHA256SUMS.txt', 'build-info.json', 'Afterhours-contract.zip')) {
        if ($call -notcontains (Join-Path $root $asset)) { throw "Missing asset: $asset" }
    }
    Write-Output 'Passed: complete package creates an unpublished draft with four assets'

    $global:AfterhoursDraftTestState.tagLines = @("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`trefs/tags/v0.3.1", "$revision`trefs/tags/v0.3.1^{}")
    & $publish
    Write-Output 'Passed: annotated tag resolves to the built commit'
    $global:AfterhoursDraftTestState.tagLines = @("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`trefs/tags/v0.3.1")
    Assert-Refused 'moved remote tag' $publish
    $global:AfterhoursDraftTestState.tagLines = @()
    Assert-Refused 'missing remote tag' $publish
    $global:AfterhoursDraftTestState.tagLines = @("$revision`trefs/tags/v0.3.1")
    $global:AfterhoursDraftTestState.gitExit = 1
    Assert-Refused 'remote verification failure' $publish
    $global:AfterhoursDraftTestState.gitExit = 0
    Assert-Refused 'invalid release input' { & $publisher -PackageDirectory $root -Release '--draft' -Revision $revision -Repository test-owner/test-repo }
    Assert-Refused 'wrong release identity' { & $publisher -PackageDirectory $root -Release v0.3.2 -Revision $revision -Repository test-owner/test-repo }
    Assert-Refused 'wrong built revision' { & $publisher -PackageDirectory $root -Release v0.3.1 -Revision ('a' * 40) -Repository test-owner/test-repo }
    Add-Content -LiteralPath (Join-Path $root 'Afterhours-windows-x64.zip') -Value changed
    Assert-Refused 'changed archive' $publish
    Write-Package
    Add-Content -LiteralPath (Join-Path $root 'Afterhours-contract.zip') -Value changed
    Assert-Refused 'changed contract archive' $publish
    Write-Package
    Remove-Item -LiteralPath (Join-Path $root 'Afterhours-contract.zip')
    Assert-Refused 'missing contract archive' $publish
    Write-Package
    $manifest = Get-Content -LiteralPath (Join-Path $root 'build-info.json') -Raw | ConvertFrom-Json
    $manifest.dirty_source = $true
    $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'build-info.json')
    Assert-Refused 'dirty source' $publish
    Write-Package
    $manifest.dirty_source = $false
    $manifest.executable_version = 'Afterhours 0.3.0 (development / local build)'
    $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'build-info.json')
    Assert-Refused 'wrong executable version' $publish
    Write-Package
    $global:AfterhoursDraftTestState.lookupExit = 1
    Assert-Refused 'release lookup failure' $publish
    $global:AfterhoursDraftTestState.lookupExit = 0
    $global:AfterhoursDraftTestState.existingRelease = @{ id = 'existing'; isDraft = $true }
    Assert-Refused 'existing draft' $publish
    $global:AfterhoursDraftTestState.existingRelease = @{ id = 'existing'; isDraft = $false }
    Assert-Refused 'existing published release' $publish
    $global:AfterhoursDraftTestState.existingRelease = $null
    $global:AfterhoursDraftTestState.ghExit = 1
    Assert-Refused 'failed creation/upload is not edited/retried' $publish 1
} finally {
    # Only these four known temporary files belong to this test; no recursive deletion.
    foreach ($name in @('Afterhours-windows-x64.zip', 'SHA256SUMS.txt', 'build-info.json', 'Afterhours-contract.zip')) {
        Remove-Item -LiteralPath (Join-Path $root $name) -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $root
    Remove-Variable -Name AfterhoursDraftTestState -Scope Global
}

$global:LASTEXITCODE = 0
