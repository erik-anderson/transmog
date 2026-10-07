param(
    [string]$SigningCertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [switch]$UnsignedDevelopment,
    [switch]$Offline,
    [switch]$BuildOnly,
    [switch]$BundleOnly,
    [switch]$SkipTests,
    [string]$SigningMetadataPath,
    [string]$SigningClientDll,
    [string]$ExpectedPublisher
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if ($BuildOnly -and $BundleOnly) { throw 'BuildOnly and BundleOnly are mutually exclusive.' }
if ($BuildOnly -and -not $UnsignedDevelopment) { throw 'BuildOnly requires UnsignedDevelopment; signing happens during bundling.' }
if ($SkipTests -and (-not $BuildOnly -or -not $UnsignedDevelopment)) { throw 'SkipTests is limited to unsigned BuildOnly after the CI repository gate.' }
if ($SigningMetadataPath -and ($UnsignedDevelopment -or $SigningCertificateThumbprint)) {
    throw 'Choose one signing mode: Artifact Signing, local certificate, or unsigned development.'
}
if ($UnsignedDevelopment -and $SigningCertificateThumbprint) { throw 'Choose either a signed release or -UnsignedDevelopment.' }
if ($SigningMetadataPath -and (-not $SigningClientDll -or -not $ExpectedPublisher)) {
    throw 'Artifact Signing requires a client DLL and the expected publisher.'
}
if (-not $UnsignedDevelopment -and -not $SigningMetadataPath -and $SigningCertificateThumbprint -notmatch '^[0-9A-Fa-f]{40}$') {
    throw 'A release requires the 40-hex SHA-1 thumbprint of a current-user code-signing certificate.'
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = Join-Path $repositoryRoot 'apps\desktop'
$artifactRoot = Join-Path $repositoryRoot 'artifacts\windows-package'
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null

$bundleConfig = Get-Content -Raw -LiteralPath (Join-Path $desktopRoot 'tauri.conf.json') | ConvertFrom-Json
if ($bundleConfig.bundle.windows.webviewInstallMode.type -ne 'skip') {
    throw 'Release packaging relies on the inbox Evergreen WebView2 prerequisite and must not bundle a fixed runtime or installer.'
}

if (-not $BundleOnly) {
    . (Join-Path $PSScriptRoot 'dev-env.ps1')

Push-Location (Join-Path $desktopRoot 'ui')
try {
    npm run check
} finally {
    Pop-Location
}

if (-not $SkipTests) {
    $cargoArguments = @('test', '--locked', '-p', 'transmog-app', '-p', 'transmog-app-webui', '-p', 'transmog-host-windows', '-p', 'transmog-script', '-p', 'transmog-script-host', '-p', 'transmog-script-supervisor', '-p', 'transmog-preview-worker', '-p', 'transmog-desktop')
    if ($Offline) { $cargoArguments += '--offline' }
    & cargo @cargoArguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo release tests failed with exit code $LASTEXITCODE" }
}

if (-not $BuildOnly) {
    $hostBuildArguments = @('build', '--locked', '--release', '-p', 'transmog-script-host', '-p', 'transmog-preview-worker')
    if ($Offline) { $hostBuildArguments += '--offline' }
    & cargo @hostBuildArguments
    if ($LASTEXITCODE -ne 0) { throw "Script host release build failed with exit code $LASTEXITCODE" }
}
}

$binaryDirectory = Join-Path $desktopRoot 'binaries'
$bundledHost = Join-Path $binaryDirectory 'transmog-script-host-x86_64-pc-windows-msvc.exe'
$bundledPreview = Join-Path $binaryDirectory 'transmog-preview-worker-x86_64-pc-windows-msvc.exe'
if (-not $BuildOnly) { New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null }
if ($SigningMetadataPath) {
    $SigningMetadataPath = (Resolve-Path -LiteralPath $SigningMetadataPath).Path
    $SigningClientDll = (Resolve-Path -LiteralPath $SigningClientDll).Path
    foreach ($helperName in @('transmog-script-host.exe', 'transmog-preview-worker.exe')) {
        & (Join-Path $PSScriptRoot 'sign-windows-file.ps1') -FilePath (Join-Path $repositoryRoot "target\release\$helperName") -MetadataPath $SigningMetadataPath -ClientDll $SigningClientDll -ExpectedPublisher $ExpectedPublisher | Out-Host
    }
}
if (-not $BuildOnly) {
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'target\release\transmog-script-host.exe') -Destination $bundledHost -Force
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'target\release\transmog-preview-worker.exe') -Destination $bundledPreview -Force
}

$tauriArguments = if ($BuildOnly) { @('build', '--no-bundle', '--no-sign', '--ci') }
    elseif ($BundleOnly) { @('bundle', '--bundles', 'nsis', '--ci') }
    else { @('build', '--bundles', 'nsis', '--ci') }
if ($UnsignedDevelopment -and -not $BuildOnly) { $tauriArguments += '--no-sign' }
$overridePath = Join-Path $artifactRoot 'signing-config.json'
$override = @{
    bundle = @{
        externalBin = @()
    }
}
if (-not $BuildOnly) { $override.bundle.externalBin = @('binaries/transmog-script-host', 'binaries/transmog-preview-worker') }
if ($SigningMetadataPath) {
    $override.bundle.windows = @{
        digestAlgorithm = 'sha256'
        signCommand = @{
            cmd = (Get-Command pwsh).Source
            args = @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'sign-windows-file.ps1'), '-FilePath', '%1', '-MetadataPath', $SigningMetadataPath, '-ClientDll', $SigningClientDll, '-ExpectedPublisher', $ExpectedPublisher)
        }
    }
} elseif (-not $UnsignedDevelopment) {
    $override.bundle.windows = @{
        certificateThumbprint = $SigningCertificateThumbprint.ToUpperInvariant()
        digestAlgorithm = 'sha256'
        timestampUrl = $TimestampUrl
        tsp = $false
    }
}
$override | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $overridePath -Encoding utf8NoBOM
$tauriArguments += @('--config', $overridePath)
if (-not $BundleOnly) {
    $tauriArguments += @('--', '--locked')
    if ($BuildOnly) {
        # One Cargo release graph builds all three shipped binaries. Sidecars are
        # staged only when bundling, after these outputs exist.
        $tauriArguments += @('-p', 'transmog-desktop', '-p', 'transmog-script-host', '-p', 'transmog-preview-worker')
    }
}

Push-Location $desktopRoot
$priorCargoOffline = $env:CARGO_NET_OFFLINE
$priorTemp = $env:TEMP
$priorTmp = $env:TMP
try {
    if ($Offline) { $env:CARGO_NET_OFFLINE = 'true' }
    if ($SigningMetadataPath) {
        $signingTemp = Join-Path $artifactRoot 'signing-temp'
        New-Item -ItemType Directory -Force -Path $signingTemp | Out-Null
        $env:TEMP = $signingTemp
        $env:TMP = $signingTemp
    }
    & .\ui\node_modules\.bin\tauri.cmd @tauriArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri packaging failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:CARGO_NET_OFFLINE = $priorCargoOffline
    $env:TEMP = $priorTemp
    $env:TMP = $priorTmp
    Pop-Location
    Remove-Item -LiteralPath $overridePath -Force -ErrorAction SilentlyContinue
    if (-not $BuildOnly) {
        Remove-Item -LiteralPath $bundledHost -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $bundledPreview -Force -ErrorAction SilentlyContinue
    }
}

if ($BuildOnly) {
    [pscustomobject]@{ BuildDirectory = (Join-Path $repositoryRoot 'target\release'); Version = $bundleConfig.version; Target = 'x86_64-pc-windows-msvc' }
    return
}

$installer = Get-ChildItem (Join-Path $repositoryRoot 'target\release\bundle\nsis\Transmog_*-setup.exe') |
    Sort-Object LastWriteTimeUtc -Descending |
    Select-Object -First 1
if ($null -eq $installer) {
    throw 'The NSIS installer was not produced.'
}

$signature = Get-AuthenticodeSignature -LiteralPath $installer.FullName
if (-not $UnsignedDevelopment -and $signature.Status -ne 'Valid') {
    throw "Installer signature is not valid: $($signature.Status)"
}

$hash = Get-FileHash -Algorithm SHA256 -LiteralPath $installer.FullName
[pscustomobject]@{
    Installer = $installer.FullName
    Bytes = $installer.Length
    Sha256 = $hash.Hash
    Signature = $signature.Status
    WebView2Mode = 'Evergreen prerequisite (not bundled)'
}
