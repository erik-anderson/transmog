[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target = 'x86_64-pc-windows-msvc',
    [ValidateSet('All','UI','Browser','Analysis','Tests','Native','Archives','Build','Qualify','Assemble')][string]$Phase = 'All',
    [ValidateSet('All','Frontend','Validation','Package')][string]$Stage = 'All',
    [string]$PolicySnapshot,
    [string]$CompiledBuild,
    [string]$CompiledBuildId,
    [switch]$HostedRunnerDevToolsPolicy
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ($Stage -eq 'Frontend' -and ($Target -cne 'x86_64-pc-windows-msvc' -or -not $IsWindows -or
    [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64 -or
    [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne [Runtime.InteropServices.Architecture]::X64)) {
    throw 'Shared frontend acceptance requires native Windows x64.'
}
$evidenceTarget = if ($Stage -eq 'Frontend') { 'shared' } else { $Target }
if ($CompiledBuild -and ($Stage -ne 'Package' -or $Phase -ne 'Build')) { throw 'Compiled inputs are restricted to the Package Build gate.' }
if ([bool]$CompiledBuild -ne [bool]$CompiledBuildId) { throw 'Compiled inputs require their immutable artifact ID.' }
$repositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'windows-release-target.ps1')
. (Join-Path $PSScriptRoot 'dev-env.ps1') -Target $Target
$platform = Get-WindowsReleaseTarget $Target
$payloadRoot = Join-Path $repositoryRoot 'artifacts/windows-build'
$statePath = Join-Path $repositoryRoot 'artifacts/windows-build-state.json'
$gateRoot = Join-Path $repositoryRoot 'artifacts/windows-build-gates'
$phases = @('UI','Browser','Analysis','Tests','Native','Archives','Build','Qualify','Assemble')
if ($Stage -eq 'Frontend') { $phases = @('UI','Browser') }
if ($Stage -eq 'Validation') { $phases = @('Analysis','Tests','Native','Archives') }
if ($Stage -eq 'Package') { $phases = @('Build','Qualify','Assemble') }
if ($Phase -ne 'All' -and $Phase -notin $phases) { throw "Phase $Phase does not belong to stage $Stage." }

$sourceCommit = (git -C $repositoryRoot rev-parse HEAD).Trim()
if ($env:GITHUB_SHA -and $env:GITHUB_SHA -cne $sourceCommit) { throw 'Checkout differs from the workflow source.' }
if (git -C $repositoryRoot status --porcelain) { throw 'Release builds require a clean committed source.' }
if ($env:GITHUB_STEP_SUMMARY) { @('| Gate | Result | Duration |','| --- | --- | --- |') | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
if ($Phase -eq 'All' -or $Phase -eq $phases[0]) {
    if (Test-Path -LiteralPath $payloadRoot) { throw 'Build payload directory already exists; use a fresh checkout.' }
    New-Item -ItemType Directory -Force -Path (Join-Path $payloadRoot 'target/release'), (Join-Path $payloadRoot 'apps/desktop/ui'), (Join-Path $payloadRoot 'evidence/gates') | Out-Null
    New-Item -ItemType Directory -Force -Path $gateRoot | Out-Null
    $state = @{ Commit=$sourceCommit; Target=$Target; Stage=$Stage; RunId=$env:GITHUB_RUN_ID; RunAttempt=$env:GITHUB_RUN_ATTEMPT; Phases=@{} }
} else {
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json -AsHashtable
    if ($state.Commit -cne $sourceCommit -or $state.Target -cne $Target -or $state.Stage -cne $Stage -or $state.RunId -cne $env:GITHUB_RUN_ID -or $state.RunAttempt -cne $env:GITHUB_RUN_ATTEMPT) { throw 'Build phase evidence belongs to a different source or target.' }
}
function Invoke-BuildPhase([string]$Name, [scriptblock]$Action) {
    if ($Name -notin $phases -or ($Phase -cne 'All' -and $Phase -cne $Name)) { return }
    foreach ($required in $phases[0..([array]::IndexOf($phases,$Name))] | Where-Object { $_ -cne $Name }) {
        if ($state.Phases[$required].Result -cne 'passed') { throw "Build phase $Name requires a successful $required phase." }
    }
    $phaseIndex = [array]::IndexOf($phases,$Name)
    foreach ($invalidated in $phases[$phaseIndex..($phases.Count-1)]) { [void]$state.Phases.Remove($invalidated) }
    if ('Build' -in $phases -and $phaseIndex -le [array]::IndexOf($phases,'Build')) { [void]$state.Remove('Build') }
    $started = [DateTimeOffset]::UtcNow
    $result = 'failed'
    Write-Host "::group::Release gate: $Name"
    $global:LASTEXITCODE = 0
    try {
        & $Action | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "$Name exited with $LASTEXITCODE" }
        $result = 'passed'
    } finally {
        $record = [ordered]@{ Schema=2; Name=$Name; Commit=$sourceCommit; Target=$evidenceTarget; Stage=$Stage; RunId=$env:GITHUB_RUN_ID; RunAttempt=$env:GITHUB_RUN_ATTEMPT; Result=$result; StartedAtUtc=$started.ToString('o'); DurationSeconds=[math]::Round(([DateTimeOffset]::UtcNow-$started).TotalSeconds,2) }
        if ($Stage -eq 'Frontend') { $record.HostTarget = 'x86_64-pc-windows-msvc' }
        $state.Phases[$Name] = $record
        $state | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
        $record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $gateRoot "$Name.json") -Encoding utf8NoBOM
        if ($env:GITHUB_STEP_SUMMARY) { "| $Name | $result | $($record.DurationSeconds) s |" | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
        Write-Host '::endgroup::'
    }
}
function Get-QualifiedBuild {
    if (-not $state.Build -or $state.Phases.Build.Result -cne 'passed') { throw 'No successful optimized build is recorded.' }
    return [pscustomobject]$state.Build
}
Push-Location $repositoryRoot
try {
    Invoke-BuildPhase 'UI' {
        Invoke-TransmogPython 'set-release-version.py' @('--check')
        & cargo fmt --all -- --check
        Invoke-TransmogPython 'vendor-boringssl-pki.py' @('--check')
        Invoke-TransmogPython 'test-chromium-psl.py' @()
        Invoke-TransmogPython 'check-crypto-graph.py' @()
        $snapshot = if ($PolicySnapshot) { $PolicySnapshot } else { Join-Path $repositoryRoot 'artifacts/release-policy/local-snapshot' }
        $policyArguments = @('--snapshot',$snapshot,'--output',(Join-Path $payloadRoot 'evidence/policy'))
        if (-not $PolicySnapshot) { $policyArguments += '--prepare' }
        Invoke-TransmogPython 'release-policy.py' $policyArguments
        & npm run check
        & npm --prefix apps/desktop/ui run check
    }
    Invoke-BuildPhase 'Browser' { & npm --prefix apps/desktop/ui run test:workspaces }
    Invoke-BuildPhase 'Analysis' { & cargo clippy --workspace --all-targets --all-features --locked -- -D warnings }
    Invoke-BuildPhase 'Tests' { & cargo test --workspace --all-features --locked }
    Invoke-BuildPhase 'Native' {
        # Keep one feature graph for workspace tests and the opt-in native fixtures.
        & cargo test --workspace --all-features --locked --test windows_client_certificates -- --ignored
        & cargo test --workspace --all-features --locked --test client_authentication -- --ignored
        & cargo build --workspace --all-features --locked
        & (Join-Path $PSScriptRoot 'test-windows-client-certificate-ui.ps1') -SkipBuild -HostedRunnerDevToolsPolicy:$HostedRunnerDevToolsPolicy
    }
    Invoke-BuildPhase 'Archives' { Invoke-TransmogPython 'test-saz-interop.py' @('--workspace-features') }
    Invoke-BuildPhase 'Build' {
        if ($CompiledBuild) {
            $restored = Invoke-TransmogPython 'windows_compiled.py' @('restore','--target',$Target,'--directory',$CompiledBuild,'--artifact-id',$CompiledBuildId)
            $state.Build = ($restored -join "`n") | ConvertFrom-Json -AsHashtable
            return
        }
        Invoke-TransmogPython 'generate-supply-chain-artifacts.py' @('--notice-path',(Join-Path $payloadRoot 'evidence/THIRD_PARTY_NOTICES.md'),'--sbom-path',(Join-Path $payloadRoot 'evidence/sbom.cdx.json'))
        $build = & (Join-Path $PSScriptRoot 'package-windows.ps1') -Target $Target -UnsignedDevelopment -BuildOnly -SkipTests |
            ForEach-Object { if ($null -ne $_.PSObject.Properties['BuildDirectory']) { $_ } else { $_ | Out-Host } }
        if (@($build).Count -ne 1) { throw 'Expected one optimized build result.' }
        $state.Build = $build
    }
    Invoke-BuildPhase 'Qualify' {
        $buildResult = Get-QualifiedBuild
        $cliGate = Invoke-TransmogPython 'test-cli-support.py' @('--executable',(Join-Path $buildResult.BuildDirectory 'transmog-cli.exe'),'--artifacts',(Join-Path $repositoryRoot 'artifacts/cli-build-validation'))
        ($cliGate -join "`n") | ConvertFrom-Json | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $payloadRoot 'evidence/cli-gate.json') -Encoding utf8NoBOM
        . (Join-Path $PSScriptRoot 'windows-release-symbols.ps1')
        New-WindowsReleaseSymbolArchive -Target $Target -BuildDirectory $buildResult.BuildDirectory -Version $buildResult.Version -Commit $sourceCommit -RunId $env:GITHUB_RUN_ID -OutputPath (Join-Path $payloadRoot "evidence/Transmog_$($buildResult.Version)_windows-$($platform.Architecture)-symbols.zip")
        $desktopGate = & (Join-Path $PSScriptRoot 'test-windows-desktop.ps1') -ExecutablePath (Join-Path $buildResult.BuildDirectory 'transmog.exe') -SkipReleaseBuild -SoakMinutes 3 -HostedRunnerDevToolsPolicy:$HostedRunnerDevToolsPolicy |
            ForEach-Object { if ($null -ne $_.PSObject.Properties['StartupVerified']) { $_ } else { $_ | Out-Host } }
        $desktopGate | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $payloadRoot 'evidence/desktop-gate.json') -Encoding utf8NoBOM
    }
    Invoke-BuildPhase 'Assemble' {
        $buildResult = Get-QualifiedBuild
        foreach ($completed in $phases | Where-Object { $_ -cne 'Assemble' }) {
            Copy-Item -LiteralPath (Join-Path $gateRoot "$completed.json") -Destination (Join-Path $payloadRoot 'evidence/gates') -Force
        }
        foreach ($binary in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'transmog-cli.exe')) {
            $sourceBinary = Join-Path $buildResult.BuildDirectory $binary
            Assert-WindowsBinaryTarget -Path $sourceBinary -Target $Target
            if ((Get-AuthenticodeSignature -LiteralPath $sourceBinary).Status -ne 'NotSigned') { throw "Build output is unexpectedly signed: $binary" }
            Copy-Item -LiteralPath $sourceBinary -Destination (Join-Path $payloadRoot "target\release\$binary") -Force
        }
        Copy-Item -LiteralPath (Join-Path $repositoryRoot 'apps\desktop\ui\dist') -Destination (Join-Path $payloadRoot 'apps\desktop\ui') -Recurse -Force
        $files = @(Get-ChildItem -LiteralPath $payloadRoot -File -Recurse | ForEach-Object {
            [pscustomobject]@{ Path = [System.IO.Path]::GetRelativePath($payloadRoot, $_.FullName).Replace('\', '/'); Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
        })
        $manifest = [ordered]@{ Commit = $sourceCommit; RunId = $env:GITHUB_RUN_ID; SourceBranch = $env:GITHUB_REF_NAME; Version = $buildResult.Version; Channel = $buildResult.Channel; ReleaseType = $env:RELEASE_TYPE; Target = $buildResult.Target; BuiltAtUtc = [DateTimeOffset]::UtcNow.ToString('o'); Files = $files }
        . (Join-Path $PSScriptRoot 'windows-release-common.ps1')
        Assert-ExplicitReleaseType $manifest
        $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $payloadRoot 'build-manifest.json') -Encoding utf8NoBOM
        if ($Stage -eq 'All') {
            Assert-ReleasePayload -PayloadRoot $payloadRoot -Commit $sourceCommit -RunId $env:GITHUB_RUN_ID | Out-Null
        } else {
            # This is a partial producer, never a signable build payload. The
            # aggregation job must verify all three immutable stage artifacts.
            Remove-Item -LiteralPath (Join-Path $payloadRoot 'build-manifest.json')
            $manifest.Remove('Files')
            $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $payloadRoot 'evidence/package-metadata.json') -Encoding utf8NoBOM
        }
        if ($env:GITHUB_OUTPUT) { "version=$($buildResult.Version)" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8NoBOM }
        Write-Host "Prepared tested unsigned build payload for Transmog $($buildResult.Version)."
    }
    if ($Stage -ne 'All' -and ($Phase -eq 'All' -or $Phase -eq $phases[-1])) {
        foreach ($gate in $phases | Where-Object { $_ -ne 'Assemble' }) {
            Copy-Item -LiteralPath (Join-Path $gateRoot "$gate.json") -Destination (Join-Path $payloadRoot 'evidence/gates') -Force
        }
        if ($Stage -ne 'Frontend') {
            if (-not $env:TRANSMOG_V8_ARTIFACT -or $env:V8_ARTIFACT_ID -notmatch '^\d+$') { throw 'A verified immutable V8 artifact is required.' }
            $engineManifest = Join-Path $env:TRANSMOG_V8_ARTIFACT 'manifest.json'
            $engine = [ordered]@{ Commit=$sourceCommit; RunId=$env:GITHUB_RUN_ID; Target=$Target; ArtifactId=$env:V8_ARTIFACT_ID; ManifestSha256=(Get-FileHash -LiteralPath $engineManifest -Algorithm SHA256).Hash; Engine=(Get-Content -Raw -LiteralPath $engineManifest | ConvertFrom-Json) }
            $engine | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $payloadRoot "evidence/v8-engine-$($Stage.ToLowerInvariant()).json") -Encoding utf8NoBOM
        }
        Invoke-TransmogPython 'release_stages.py' @('export','--stage',$Stage,'--root',$payloadRoot,'--target',$Target,'--commit',$sourceCommit)
    }
} finally { Pop-Location }
