param(
    [string]$SigningCertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [switch]$UnsignedDevelopment,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if ($UnsignedDevelopment -and $SigningCertificateThumbprint) {
    throw 'Choose either a signed release or -UnsignedDevelopment, not both.'
}
if (-not $UnsignedDevelopment -and $SigningCertificateThumbprint -notmatch '^[0-9A-Fa-f]{40}$') {
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

. (Join-Path $PSScriptRoot 'dev-env.ps1')

Push-Location (Join-Path $desktopRoot 'ui')
try {
    npm run check
} finally {
    Pop-Location
}

$cargoArguments = @('test', '--locked', '-p', 'transmog-app', '-p', 'transmog-app-webui', '-p', 'transmog-host-windows', '-p', 'transmog-desktop')
if ($Offline) { $cargoArguments += '--offline' }
& cargo @cargoArguments
if ($LASTEXITCODE -ne 0) { throw "Cargo release tests failed with exit code $LASTEXITCODE" }

$tauriArguments = @('build', '--bundles', 'nsis', '--ci')
$overridePath = Join-Path $artifactRoot 'signing-config.json'
if (-not $UnsignedDevelopment) {
    @{
        bundle = @{
            windows = @{
                certificateThumbprint = $SigningCertificateThumbprint.ToUpperInvariant()
                digestAlgorithm = 'sha256'
                timestampUrl = $TimestampUrl
                tsp = $false
            }
        }
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $overridePath -Encoding utf8NoBOM
    $tauriArguments += @('--config', $overridePath)
}

Push-Location $desktopRoot
$priorCargoOffline = $env:CARGO_NET_OFFLINE
try {
    if ($Offline) { $env:CARGO_NET_OFFLINE = 'true' }
    & .\ui\node_modules\.bin\tauri.cmd @tauriArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri packaging failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:CARGO_NET_OFFLINE = $priorCargoOffline
    Pop-Location
    Remove-Item -LiteralPath $overridePath -Force -ErrorAction SilentlyContinue
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
