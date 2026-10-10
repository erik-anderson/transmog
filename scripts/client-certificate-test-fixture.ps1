# Only called by the explicitly ignored Windows CNG acceptance test. Creates
# untrusted, non-exportable fixture keys and removes only their exact identities.
param(
    [Parameter(Mandatory)]
    [ValidateSet('Create', 'DeleteKey', 'DeleteCertificate', 'Cleanup')]
    [string]$Action,
    [ValidateSet('RSA', 'ECDSA_nistP256', 'ECDSA_nistP384', 'ECDSA_nistP521', 'ECDSA_genericP256', 'ECDSA_genericP384', 'ECDSA_genericP521', 'ECDSA_genericSecP256r1', 'ECDSA_genericX962P256v1', 'ECDSA_genericSecP384r1', 'ECDSA_genericSecP521r1')]
    [string]$Algorithm = 'RSA',
    [ValidatePattern('^[A-Fa-f0-9]{40}$')]
    [string]$Thumbprint,
    [ValidatePattern('^[A-Fa-f0-9]{40}$')]
    [string]$IssuerThumbprint
)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$prefix = 'CN=Transmog client authentication test '

function Get-FixturePrivateKey($certificate) {
    if ($certificate.PublicKey.Oid.Value -eq '1.2.840.113549.1.1.1') {
        return [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($certificate)
    }
    return [System.Security.Cryptography.X509Certificates.ECDsaCertificateExtensions]::GetECDsaPrivateKey($certificate)
}

function Remove-FixtureKey($certificate) {
    $privateKey = $null
    try {
        $privateKey = Get-FixturePrivateKey $certificate
        if ($privateKey) {
            if ($privateKey.Key.Provider.Provider -ne 'Microsoft Software Key Storage Provider') {
                throw 'Fixture provider mismatch.'
            }
            $privateKey.Key.Delete()
        }
    } finally { if ($privateKey) { $privateKey.Dispose() } }
}

if ($Action -eq 'Create') {
    $certificate = $null
    $privateKey = $null
    $cngKey = $null
    $issuerKey = $null
    $issuerCertificate = $null
    try {
        # Associate the persisted CNG key directly. Framework CopyWithPrivateKey
        # can compare curve aliases by representation rather than their identity.
        # The production pairing check and live peer verify the actual public key.
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
public static class TransmogClientCertificateFixture {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct KeyInfo {
        [MarshalAs(UnmanagedType.LPWStr)] public string Container;
        [MarshalAs(UnmanagedType.LPWStr)] public string Provider;
        public uint ProviderType;
        public uint Flags;
        public uint ParameterCount;
        public IntPtr Parameters;
        public uint KeySpec;
    }
    [DllImport("crypt32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CertSetCertificateContextProperty(IntPtr context, uint property, uint flags, ref KeyInfo info);
    public static void Associate(X509Certificate2 certificate, CngKey key) {
        var info = new KeyInfo {
            Container = key.KeyName,
            Provider = key.Provider.Provider,
            // ProviderType/Flags/KeySpec zero mean the current-user CNG key.
        };
        if (!CertSetCertificateContextProperty(certificate.Handle, 2, 0, ref info))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        GC.KeepAlive(certificate);
        GC.KeepAlive(key);
    }
}
'@
        $identifier = [Guid]::NewGuid().ToString('N')
        $parameters = [System.Security.Cryptography.CngKeyCreationParameters]::new()
        $parameters.Provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
        $parameters.ExportPolicy = [System.Security.Cryptography.CngExportPolicies]::None
        $parameters.KeyUsage = [System.Security.Cryptography.CngKeyUsages]::Signing
        if ($Algorithm -eq 'RSA') {
            $parameters.Parameters.Add([System.Security.Cryptography.CngProperty]::new('Length', [BitConverter]::GetBytes(2048), [System.Security.Cryptography.CngPropertyOptions]::None))
            $cngAlgorithm = [System.Security.Cryptography.CngAlgorithm]::Rsa
        } elseif ($Algorithm.StartsWith('ECDSA_generic', [StringComparison]::Ordinal)) {
            $curveName = switch ($Algorithm) {
                ECDSA_genericP256 { 'nistP256' }
                ECDSA_genericP384 { 'nistP384' }
                ECDSA_genericP521 { 'nistP521' }
                ECDSA_genericSecP256r1 { 'secP256r1' }
                ECDSA_genericX962P256v1 { 'x962P256v1' }
                ECDSA_genericSecP384r1 { 'secP384r1' }
                ECDSA_genericSecP521r1 { 'secP521r1' }
            }
            $parameters.Parameters.Add([System.Security.Cryptography.CngProperty]::new('ECCCurveName', [Text.Encoding]::Unicode.GetBytes($curveName + [char]0), [System.Security.Cryptography.CngPropertyOptions]::None))
            $cngAlgorithm = [System.Security.Cryptography.CngAlgorithm]::new('ECDSA')
        } else {
            $cngAlgorithm = switch ($Algorithm) {
                ECDSA_nistP256 { [System.Security.Cryptography.CngAlgorithm]::ECDsaP256 }
                ECDSA_nistP384 { [System.Security.Cryptography.CngAlgorithm]::ECDsaP384 }
                ECDSA_nistP521 { [System.Security.Cryptography.CngAlgorithm]::ECDsaP521 }
            }
        }
        $cngKey = [System.Security.Cryptography.CngKey]::Create($cngAlgorithm, ('Transmog-client-cert-test-' + $identifier), $parameters)
        if ($Algorithm.StartsWith('ECDSA_generic', [StringComparison]::Ordinal) -and $cngKey.Algorithm.Algorithm -ne 'ECDSA') {
            throw 'Fixture did not exercise the generic named-curve identifier.'
        }
        if ($Algorithm -eq 'RSA') {
            $privateKey = [System.Security.Cryptography.RSACng]::new($cngKey)
            $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(($prefix + $identifier), $privateKey, [System.Security.Cryptography.HashAlgorithmName]::SHA256, [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
        } else {
            $privateKey = [System.Security.Cryptography.ECDsaCng]::new($cngKey)
            $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(($prefix + $identifier), $privateKey, [System.Security.Cryptography.HashAlgorithmName]::SHA256)
        }
        $request.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new([System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true))
        $usage = [System.Security.Cryptography.OidCollection]::new()
        $usage.Add([System.Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.2')) | Out-Null
        $request.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($usage, $true))
        # An untrusted public issuer in My exercises real local chain building.
        # Only the leaf has a persisted, non-exportable signing identity.
        $issuerKey = [System.Security.Cryptography.RSA]::Create(2048)
        $issuerRequest = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(($prefix + $identifier + ' issuer'), $issuerKey, [System.Security.Cryptography.HashAlgorithmName]::SHA256, [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $issuerRequest.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($true, $false, 0, $true))
        $issuerRequest.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new([System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyCertSign, $true))
        $issuerCertificate = $issuerRequest.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-2), [DateTimeOffset]::UtcNow.AddDays(2))
        $issuerGenerator = [System.Security.Cryptography.X509Certificates.X509SignatureGenerator]::CreateForRSA($issuerKey, [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $issued = $request.Create($issuerCertificate.SubjectName, $issuerGenerator, [DateTimeOffset]::UtcNow.AddMinutes(-1), [DateTimeOffset]::UtcNow.AddDays(1), [Guid]::NewGuid().ToByteArray())
        try {
            $certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($issued.RawData)
            [TransmogClientCertificateFixture]::Associate($certificate, $cngKey)
        } finally { $issued.Dispose() }
        $store = [System.Security.Cryptography.X509Certificates.X509Store]::new('My', 'CurrentUser')
        $publicIssuer = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($issuerCertificate.RawData)
        try {
            $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
            $store.Add($publicIssuer)
            $store.Add($certificate)
        } finally { $store.Dispose(); $publicIssuer.Dispose() }
        if (-not $privateKey -or $privateKey.Key.ExportPolicy -ne [System.Security.Cryptography.CngExportPolicies]::None) {
            throw 'Fixture key is not non-exportable CNG.'
        }
        $exportRejected = $false
        try {
            $unexpectedExport = $privateKey.Key.Export([System.Security.Cryptography.CngKeyBlobFormat]::Pkcs8PrivateBlob)
            [Array]::Clear($unexpectedExport, 0, $unexpectedExport.Length)
        } catch [System.Security.Cryptography.CryptographicException] { $exportRejected = $true }
        if (-not $exportRejected) { throw 'Fixture private-key export unexpectedly succeeded.' }
        $hash = [System.Security.Cryptography.SHA256]::Create()
        try { $fingerprint = [BitConverter]::ToString($hash.ComputeHash($certificate.RawData)).Replace('-', '').ToLowerInvariant() }
        finally { $hash.Dispose() }
        @{ thumbprint = $certificate.Thumbprint; issuerThumbprint = $issuerCertificate.Thumbprint; id = 'windows-current-user:' + $fingerprint } | ConvertTo-Json -Compress
    } catch {
        if ($privateKey) { $privateKey.Dispose(); $privateKey = $null }
        if ($certificate) {
            try { Remove-FixtureKey $certificate } finally {
                Remove-Item -LiteralPath ('Cert:\CurrentUser\My\' + $certificate.Thumbprint) -ErrorAction SilentlyContinue
            }
        }
        if ($cngKey) { $cngKey.Delete() }
        if ($issuerCertificate) { Remove-Item -LiteralPath ('Cert:\CurrentUser\My\' + $issuerCertificate.Thumbprint) -ErrorAction SilentlyContinue }
        throw
    } finally {
        if ($privateKey) { $privateKey.Dispose() }; if ($certificate) { $certificate.Dispose() }; if ($cngKey) { $cngKey.Dispose() }
        if ($issuerCertificate) { $issuerCertificate.Dispose() }; if ($issuerKey) { $issuerKey.Dispose() }
    }
    exit
}
if (-not $Thumbprint) { throw 'An exact fixture thumbprint is required.' }
if ($Action -eq 'Cleanup' -and $IssuerThumbprint) {
    $issuerPath = 'Cert:\CurrentUser\My\' + $IssuerThumbprint
    $issuer = Get-Item -LiteralPath $issuerPath -ErrorAction SilentlyContinue
    if ($issuer) {
        try {
            if (-not $issuer.Subject.StartsWith($prefix, [StringComparison]::Ordinal) -or $issuer.HasPrivateKey) { throw 'Refusing to remove a non-fixture public issuer.' }
            Remove-Item -LiteralPath $issuerPath
        } finally { $issuer.Dispose() }
    }
}
$path = 'Cert:\CurrentUser\My\' + $Thumbprint
$certificate = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
if (-not $certificate) {
    if ($Action -eq 'Cleanup') { exit }
    throw 'Fixture certificate is missing.'
}
try {
    if (-not $certificate.Subject.StartsWith($prefix, [StringComparison]::Ordinal)) {
        throw 'Refusing to modify a certificate outside the test fixture namespace.'
    }
    if ($Action -in @('DeleteKey', 'Cleanup')) {
        try { Remove-FixtureKey $certificate }
        catch {
            # A prior DeleteKey is an expected state during final cleanup. Only
            # the exact owned certificate is removed; no key names are enumerated.
            if ($Action -ne 'Cleanup') { throw }
        }
    }
    if ($Action -in @('DeleteCertificate', 'Cleanup')) { Remove-Item -LiteralPath $path }
} finally { $certificate.Dispose() }
