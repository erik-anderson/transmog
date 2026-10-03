param(
    [string]$CertificatePath,
    [string]$PrivateKeyPath
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $CertificatePath) {
    $CertificatePath = Join-Path $repositoryRoot '.local\live-test-ca.pem'
}
if (-not $PrivateKeyPath) {
    $PrivateKeyPath = Join-Path $repositoryRoot '.local\live-test-ca.key'
}

$certificatePem = Get-Content -Raw -LiteralPath (Resolve-Path -LiteralPath $CertificatePath).Path
$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::CreateFromPem(
    $certificatePem
)
try {
    $sha256 = $certificate.GetCertHashString(
        [System.Security.Cryptography.HashAlgorithmName]::SHA256
    )
} finally {
    $certificate.Dispose()
}

& "$PSScriptRoot\uninstall-ca-user.ps1" -Sha256 $sha256
Remove-Item -LiteralPath $CertificatePath -Force
Remove-Item -LiteralPath $PrivateKeyPath -Force
Write-Output 'Removed the durable live-test CA certificate and private key.'
