param(
    [string]$CertificatePath,
    [string]$PrivateKeyPath,
    [string]$BinaryPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$localDirectory = Join-Path $repositoryRoot '.local'
if (-not $CertificatePath) {
    $CertificatePath = Join-Path $localDirectory 'live-test-ca.pem'
}
if (-not $PrivateKeyPath) {
    $PrivateKeyPath = Join-Path $localDirectory 'live-test-ca.key'
}
if (-not $BinaryPath) {
    $BinaryPath = Join-Path $repositoryRoot 'target\release\transmog-cli.exe'
}

$certificateExists = Test-Path -LiteralPath $CertificatePath
$keyExists = Test-Path -LiteralPath $PrivateKeyPath
if ($certificateExists -xor $keyExists) {
    throw 'The live-test CA certificate and key must either both exist or both be absent.'
}
if (-not $certificateExists) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $CertificatePath) | Out-Null
    & "$PSScriptRoot\new-proxy-ca.ps1" `
        -CertificatePath $CertificatePath `
        -PrivateKeyPath $PrivateKeyPath `
        -Name 'Transmog durable Playwright test CA' `
        -BinaryPath $BinaryPath
}

& "$PSScriptRoot\install-ca-user.ps1" -CertificatePath $CertificatePath
& "$PSScriptRoot\verify-ca-user.ps1" -CertificatePath $CertificatePath
Write-Output "CA_CERT=$([System.IO.Path]::GetFullPath($CertificatePath))"
Write-Output "CA_KEY=$([System.IO.Path]::GetFullPath($PrivateKeyPath))"
Write-Output 'The live-test CA remains installed until scripts/remove-live-test-ca.ps1 is run.'
