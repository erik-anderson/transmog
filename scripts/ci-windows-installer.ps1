[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$reportRoot = Join-Path $repositoryRoot 'artifacts\windows-ci'
New-Item -ItemType Directory -Force -Path $reportRoot | Out-Null
$phaseMeasurements = [System.Collections.Generic.List[object]]::new()
$startedAt = [DateTimeOffset]::UtcNow
$buildClock = [System.Diagnostics.Stopwatch]::StartNew()
$buildDrive = [System.IO.DriveInfo]::new([System.IO.Path]::GetPathRoot($repositoryRoot))
$freeBytesAtStart = $buildDrive.AvailableFreeSpace
$status = 'failed'
$failure = $null
$installerInfo = $null

function Invoke-BuildPhase {
    param([string]$Name, [scriptblock]$Action)
    Write-Host "::group::$Name"
    $phaseClock = [System.Diagnostics.Stopwatch]::StartNew()
    $phaseStatus = 'failed'
    try {
        & $Action
        $phaseStatus = 'passed'
    } finally {
        $phaseClock.Stop()
        $phaseMeasurements.Add([pscustomobject]@{
            Name = $Name
            Status = $phaseStatus
            Seconds = [math]::Round($phaseClock.Elapsed.TotalSeconds, 2)
        })
        Write-Host '::endgroup::'
    }
}

Push-Location $repositoryRoot
try {
    Invoke-BuildPhase 'Native toolchain' {
        & (Join-Path $PSScriptRoot 'setup-windows-ci.ps1')
    }
    Invoke-BuildPhase 'Rust toolchain' {
        $toolchainFile = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'rust-toolchain.toml')
        $toolchainMatch = [regex]::Match($toolchainFile, '(?m)^channel\s*=\s*"([^"]+)"\s*$')
        if (-not $toolchainMatch.Success) { throw 'rust-toolchain.toml has no channel pin.' }
        & rustup toolchain install $toolchainMatch.Groups[1].Value --profile minimal --component clippy --component rustfmt --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw "Rust toolchain installation failed: $LASTEXITCODE" }
        . (Join-Path $PSScriptRoot 'dev-env.ps1') -Check
        $versionReport = @(
            "Runner image: $env:ImageOS $env:ImageVersion"
            "Rust: $(& rustc --version)"
            "Cargo: $(& cargo --version)"
            "Node: $(& node --version)"
            "npm: $(& npm --version)"
            "LLVM: $((& clang --version | Select-Object -First 1))"
            "CMake: $((& cmake --version | Select-Object -First 1))"
            "Ninja: $(& ninja --version)"
            "NASM: $(& nasm -v)"
            "PE linker: $((Get-Command link).Source)"
        )
        $versionReport | Set-Content -LiteralPath (Join-Path $reportRoot 'tool-versions.txt') -Encoding utf8NoBOM
        $versionReport | ForEach-Object { Write-Host $_ }
    }
    Invoke-BuildPhase 'Locked UI dependencies' {
        Push-Location (Join-Path $repositoryRoot 'apps\desktop\ui')
        try {
            & npm ci
            if ($LASTEXITCODE -ne 0) { throw "npm ci failed: $LASTEXITCODE" }
        } finally { Pop-Location }
    }
    Invoke-BuildPhase 'Verified V8 archive' {
        & (Join-Path $PSScriptRoot 'cache-v8.ps1') | Out-Host
    }
    Invoke-BuildPhase 'Tests and unsigned NSIS packaging' {
        $packageOutput = & (Join-Path $PSScriptRoot 'package-windows.ps1') -UnsignedDevelopment |
            ForEach-Object {
                if ($null -ne $_.PSObject.Properties['Installer']) { $_ } else { $_ | Out-Host }
            }
        if ($null -eq $packageOutput -or @($packageOutput).Count -ne 1) {
            throw 'Packaging did not return exactly one installer result.'
        }
        if ($packageOutput.Signature -ne 'NotSigned') {
            throw "Expected an unsigned development installer, got: $($packageOutput.Signature)"
        }
        $installerDirectory = Join-Path $reportRoot 'installer'
        New-Item -ItemType Directory -Force -Path $installerDirectory | Out-Null
        $installerName = Split-Path -Leaf $packageOutput.Installer
        Copy-Item -LiteralPath $packageOutput.Installer -Destination (Join-Path $installerDirectory $installerName) -Force
        "$($packageOutput.Sha256.ToLowerInvariant())  $installerName" |
            Set-Content -LiteralPath (Join-Path $installerDirectory 'SHA256SUMS') -Encoding utf8NoBOM
        $script:installerInfo = [pscustomobject]@{
            Name = $installerName
            Bytes = $packageOutput.Bytes
            Sha256 = $packageOutput.Sha256
            Signature = [string]$packageOutput.Signature
        }
    }
    $status = 'passed'
} catch {
    $failure = $_.Exception.Message
    throw
} finally {
    $buildClock.Stop()
    $targetRoot = Join-Path $repositoryRoot 'target'
    $targetBytes = if (Test-Path -LiteralPath $targetRoot) {
        (Get-ChildItem -LiteralPath $targetRoot -File -Recurse -ErrorAction SilentlyContinue |
            Measure-Object -Property Length -Sum).Sum
    } else { 0 }
    $freeBytesAtEnd = $buildDrive.AvailableFreeSpace
    $metrics = [pscustomobject]@{
        Status = $status
        Failure = $failure
        StartedAtUtc = $startedAt.ToString('o')
        FinishedAtUtc = [DateTimeOffset]::UtcNow.ToString('o')
        BuildSeconds = [math]::Round($buildClock.Elapsed.TotalSeconds, 2)
        Commit = $env:GITHUB_SHA
        RunnerImage = "$env:ImageOS $env:ImageVersion".Trim()
        DependencyCacheRequested = $env:TRANSMOG_CI_USE_CACHE -eq 'true'
        FreeDiskBytesAtStart = $freeBytesAtStart
        FreeDiskBytesAtEnd = $freeBytesAtEnd
        TargetBytesAtEnd = $targetBytes
        Phases = @($phaseMeasurements.ToArray())
        Installer = $installerInfo
    }
    $metrics | ConvertTo-Json -Depth 6 |
        Set-Content -LiteralPath (Join-Path $reportRoot 'build-metrics.json') -Encoding utf8NoBOM
    $summary = @(
        '# Windows unsigned installer build'
        ''
        "Status: **$status**"
        "Commit: $env:GITHUB_SHA"
        "Dependency downloads cache requested: $($metrics.DependencyCacheRequested)"
        "Build duration: $($metrics.BuildSeconds) seconds (excludes checkout and Node setup)."
        "Free disk: $([math]::Round($freeBytesAtStart / 1GB, 2)) GiB before; $([math]::Round($freeBytesAtEnd / 1GB, 2)) GiB after."
        "Cargo target size after build: $([math]::Round($targetBytes / 1GB, 2)) GiB."
        ''
        '| Phase | Result | Seconds |'
        '| --- | --- | ---: |'
    )
    $summary += $phaseMeasurements | ForEach-Object { "| $($_.Name) | $($_.Status) | $($_.Seconds) |" }
    if ($null -ne $installerInfo) {
        $summary += @('', "Installer: $($installerInfo.Name) ($($installerInfo.Bytes) bytes), unsigned.", "SHA-256: $($installerInfo.Sha256)")
    }
    if ($failure) { $summary += @('', "Failure: $failure") }
    $summary | Set-Content -LiteralPath (Join-Path $reportRoot 'build-summary.md') -Encoding utf8NoBOM
    if ($env:GITHUB_STEP_SUMMARY) {
        $summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Encoding utf8NoBOM
    }
    Pop-Location
}
