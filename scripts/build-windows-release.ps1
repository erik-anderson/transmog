[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$payloadRoot = Join-Path $repositoryRoot 'artifacts\windows-build'
if (Test-Path -LiteralPath $payloadRoot) { throw 'Build payload directory already exists; use a fresh checkout.' }
New-Item -ItemType Directory -Force -Path (Join-Path $payloadRoot 'target\release'), (Join-Path $payloadRoot 'apps\desktop\ui'), (Join-Path $payloadRoot 'evidence') | Out-Null
Push-Location $repositoryRoot
try {
    . (Join-Path $PSScriptRoot 'dev-env.ps1') -Check
    # UI credits read locked Cargo metadata and dependency license files offline.
    # Populate the cache before the first UI build on a fresh release runner.
    & cargo fetch --locked
    # The complete deterministic repository gate runs before signing credentials exist.
    Push-Location (Join-Path $repositoryRoot 'apps\desktop\ui')
    try {
        & npm run check
        Write-Host '::group::Browser workspace checks before Rust compilation'
        try { & npm run test:workspaces } finally { Write-Host '::endgroup::' }
    } finally { Pop-Location }
    Write-Host '::group::Repository analysis and tests (development profile)'
    try {
        & cargo fmt --all -- --check
        & cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
        & cargo test --workspace --all-features --locked
        & (Join-Path $PSScriptRoot 'test-saz-interop.ps1')
        & cargo deny check
        & cargo deny --manifest-path fuzz/Cargo.toml --config fuzz/deny.toml --locked check
        & (Join-Path $PSScriptRoot 'check-crypto-graph.ps1')
        & (Join-Path $PSScriptRoot 'generate-supply-chain-artifacts.ps1') -NoticePath (Join-Path $payloadRoot 'evidence\THIRD_PARTY_NOTICES.md') -SbomPath (Join-Path $payloadRoot 'evidence\sbom.cdx.json')
    } finally { Write-Host '::endgroup::' }
    Write-Host '::group::Optimized application and helpers (one release build)'
    try {
        $buildResult = & (Join-Path $PSScriptRoot 'package-windows.ps1') -UnsignedDevelopment -BuildOnly -SkipTests |
            ForEach-Object { if ($null -ne $_.PSObject.Properties['BuildDirectory']) { $_ } else { $_ | Out-Host } }
        if (@($buildResult).Count -ne 1) { throw 'The release build did not produce one build result.' }
    } finally { Write-Host '::endgroup::' }
    . (Join-Path $PSScriptRoot 'windows-release-symbols.ps1')
    New-WindowsReleaseSymbolArchive -BuildDirectory $buildResult.BuildDirectory -Version $buildResult.Version -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID -OutputPath (Join-Path $payloadRoot "evidence/Transmog_$($buildResult.Version)_windows-x64-symbols.zip")
    Write-Host '::group::Release WebView validation and 3-minute soak'
    try {
        $desktopGate = & (Join-Path $PSScriptRoot 'test-windows-desktop.ps1') -SkipReleaseBuild -SoakMinutes 3 -HostedRunnerDevToolsPolicy |
            ForEach-Object { if ($null -ne $_.PSObject.Properties['StartupVerified']) { $_ } else { $_ | Out-Host } }
    } finally { Write-Host '::endgroup::' }
    $desktopGate | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $payloadRoot 'evidence\desktop-gate.json') -Encoding utf8NoBOM
    foreach ($binary in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'transmog-cli.exe')) {
        $sourceBinary = Join-Path $buildResult.BuildDirectory $binary
        if ((Get-AuthenticodeSignature -LiteralPath $sourceBinary).Status -ne 'NotSigned') { throw "Build output is unexpectedly signed: $binary" }
        Copy-Item -LiteralPath $sourceBinary -Destination (Join-Path $payloadRoot "target\release\$binary") -Force
    }
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'apps\desktop\ui\dist') -Destination (Join-Path $payloadRoot 'apps\desktop\ui') -Recurse -Force
    $files = @(Get-ChildItem -LiteralPath $payloadRoot -File -Recurse | ForEach-Object {
        [pscustomobject]@{ Path = [System.IO.Path]::GetRelativePath($payloadRoot, $_.FullName).Replace('\', '/'); Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
    })
    $manifest = [ordered]@{ Commit = $env:GITHUB_SHA; RunId = $env:GITHUB_RUN_ID; SourceBranch = $env:GITHUB_REF_NAME; Version = $buildResult.Version; Channel = $buildResult.Channel; ReleaseType = $env:RELEASE_TYPE; Target = $buildResult.Target; BuiltAtUtc = [DateTimeOffset]::UtcNow.ToString('o'); Files = $files }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $payloadRoot 'build-manifest.json') -Encoding utf8NoBOM
    if ($env:GITHUB_OUTPUT) { "version=$($buildResult.Version)" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8NoBOM }
    Write-Host "Prepared tested unsigned build payload for Transmog $($buildResult.Version)."
} finally { Pop-Location }
