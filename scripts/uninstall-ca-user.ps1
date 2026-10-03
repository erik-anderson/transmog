param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f]{64}$')]
    [string]$Sha256
)

$ErrorActionPreference = 'Stop'
$expected = $Sha256.ToUpperInvariant()
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new(
    [System.Security.Cryptography.X509Certificates.StoreName]::Root,
    [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser
)
try {
    $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
    $matches = @($store.Certificates | Where-Object {
        $_.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256) -eq $expected
    })
    if ($matches.Count -gt 1) {
        throw "Refusing ambiguous removal: found $($matches.Count) certificates with SHA-256 $expected."
    }
    if ($matches.Count -eq 1) {
        $store.Remove($matches[0])
    }
} finally {
    $store.Close()
}

$verifyStore = [System.Security.Cryptography.X509Certificates.X509Store]::new('Root', 'CurrentUser')
try {
    $verifyStore.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
    $remaining = @($verifyStore.Certificates | Where-Object {
        $_.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256) -eq $expected
    })
    if ($remaining.Count -ne 0) {
        throw "Certificate SHA-256 $expected remains installed."
    }
} finally {
    $verifyStore.Close()
}
Write-Output "REMOVED_CA_SHA256=$expected"
Write-Output 'CA_STORE=Cert:\CurrentUser\Root'
