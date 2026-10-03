param(
    [Parameter(Mandatory = $true)]
    [string]$CertificatePath
)

$ErrorActionPreference = 'Stop'
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

$store = [System.Security.Cryptography.X509Certificates.X509Store]::new(
    [System.Security.Cryptography.X509Certificates.StoreName]::Root,
    [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser
)
try {
    $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
    $matches = @($store.Certificates | Where-Object {
        $_.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256) -eq $sha256
    })
    if ($matches.Count -ne 1) {
        throw "Expected exactly one current-user root with SHA-256 $sha256; found $($matches.Count). Run scripts/setup-live-test-ca.ps1."
    }
} finally {
    $store.Close()
}

Write-Output "CA_SHA256=$sha256"
Write-Output 'CA_STORE=Cert:\CurrentUser\Root'
Write-Output 'CA_TRUST_VERIFIED=true'
