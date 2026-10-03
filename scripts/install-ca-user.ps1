param(
    [Parameter(Mandatory = $true)]
    [string]$CertificatePath
)

$ErrorActionPreference = 'Stop'
$certificatePath = (Resolve-Path -LiteralPath $CertificatePath).Path
$certificatePem = Get-Content -Raw -LiteralPath $certificatePath
$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::CreateFromPem(
    $certificatePem
)
$basicConstraints = $certificate.Extensions |
    Where-Object { $_.Oid.Value -eq '2.5.29.19' } |
    Select-Object -First 1
if ($null -eq $basicConstraints) {
    throw 'Certificate has no Basic Constraints extension.'
}
$decoded = [System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new(
    $basicConstraints,
    $basicConstraints.Critical
)
if (-not $decoded.CertificateAuthority) {
    throw 'Refusing to install a certificate that is not a CA.'
}

$sha256 = $certificate.GetCertHashString(
    [System.Security.Cryptography.HashAlgorithmName]::SHA256
)
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new(
    [System.Security.Cryptography.X509Certificates.StoreName]::Root,
    [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser
)
try {
    $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
    $alreadyPresent = $store.Certificates |
        Where-Object {
            $_.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256) -eq $sha256
        }
    if (-not $alreadyPresent) {
        $store.Add($certificate)
    }
} finally {
    $store.Close()
    $certificate.Dispose()
}

$verifyStore = [System.Security.Cryptography.X509Certificates.X509Store]::new('Root', 'CurrentUser')
try {
    $verifyStore.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
    $verified = $verifyStore.Certificates |
        Where-Object {
            $_.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256) -eq $sha256
        }
    if (@($verified).Count -ne 1) {
        throw "Expected exactly one installed certificate with SHA-256 $sha256."
    }
} finally {
    $verifyStore.Close()
}
Write-Output "CA_SHA256=$sha256"
Write-Output 'CA_STORE=Cert:\CurrentUser\Root'
