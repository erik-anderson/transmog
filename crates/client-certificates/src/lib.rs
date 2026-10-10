//! Public certificate inventory and silent, non-exporting Windows CNG signing.
//! Only CurrentUser/My and Microsoft software keys are supported. Inventory is
//! read-only and never downloads issuers, adds trust or signs a challenge.
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
mod windows;

/// Supported software key families. Curve names describe the actual CNG key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAlgorithm {
    /// RSA with PKCS#1 v1.5 or PSS padding.
    Rsa,
    /// ECDSA on P-256.
    EcdsaP256,
    /// ECDSA on P-384.
    EcdsaP384,
    /// ECDSA on P-521.
    EcdsaP521,
}

/// Hash applied to the complete TLS signing input by the transport adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestAlgorithm {
    /// Legacy TLS 1.2 SHA-1.
    Sha1,
    /// SHA-256.
    Sha256,
    /// SHA-384.
    Sha384,
    /// SHA-512.
    Sha512,
}
impl DigestAlgorithm {
    /// Required prehash byte count.
    pub fn size(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }
}
/// The signature encoding/padding requested from CNG.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignaturePadding {
    /// RSA PKCS#1 v1.5.
    RsaPkcs1,
    /// RSA PSS with a salt exactly as long as the digest.
    RsaPss,
    /// ECDSA; returned as fixed-width, big-endian r followed by s.
    Ecdsa,
}

/// Bounded, redaction-safe native failure; includes no key names or OS diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CertificateError;
impl std::fmt::Display for CertificateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Windows client certificate is unavailable or unsupported")
    }
}
impl std::error::Error for CertificateError {}

/// Public chain plus a reference to the exact current-user identity. Private key
/// bytes and native handles never enter the inventory or this portable object.
#[derive(Clone, Debug)]
pub struct UserCertificate {
    /// Leaf-first, locally available public DER path, at most 32 certificates.
    pub chain: Vec<Vec<u8>>,
    /// Exact identity reference used for fresh acquisition on each operation.
    pub key: SigningIdentity,
}

/// Exact certificate and provider/container association; opaque container names
/// are deliberately excluded from Debug and the settings identifier.
#[derive(Clone)]
pub struct SigningIdentity {
    fingerprint: [u8; 32],
    algorithm: KeyAlgorithm,
    #[cfg(windows)]
    container: Vec<u16>,
    #[cfg(windows)]
    legacy_spec: u32,
    #[cfg(windows)]
    lookup_hash: [u8; 20],
}
impl std::fmt::Debug for SigningIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SigningIdentity")
            .field("id", &self.id())
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}
impl SigningIdentity {
    /// Stable non-secret source identifier and lowercase public SHA-256 digest.
    pub fn id(&self) -> String {
        use std::fmt::Write as _;
        let mut id = String::from("windows-current-user:");
        for byte in self.fingerprint {
            let _ = write!(id, "{byte:02x}");
        }
        id
    }
    /// Actual supported key family.
    pub fn algorithm(&self) -> KeyAlgorithm {
        self.algorithm
    }
    /// Reopens the exact certificate and key silently, without holding a cached
    /// key handle that could outlive deletion. Never performs signing or exports
    /// private material. Callers should keep native inventory work off the reactor.
    pub fn available(&self) -> bool {
        #[cfg(windows)]
        {
            windows::available(self)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    /// Signs an already hashed TLS input with a freshly acquired software key.
    /// No UI is allowed; disappearance, changed associations and mismatched keys
    /// fail closed. Only public key information is exported for pairing checks.
    /// # Errors
    /// Returns a generic failure for unsupported padding/hash or unavailable keys.
    pub fn sign_hash(
        &self,
        digest: DigestAlgorithm,
        padding: SignaturePadding,
        input: &[u8],
    ) -> Result<Vec<u8>, CertificateError> {
        if input.len() != digest.size()
            || (self.algorithm == KeyAlgorithm::Rsa) == (padding == SignaturePadding::Ecdsa)
        {
            return Err(CertificateError);
        }
        #[cfg(windows)]
        {
            windows::sign(self, digest, padding, input)
        }
        #[cfg(not(windows))]
        {
            Err(CertificateError)
        }
    }
}

/// Enumerates supported current-user software CNG identities. This is a blocking
/// native lookup, bounded to 4,096 store entries and 1,024 supported identities.
/// Unsupported/hardware, keyless and inaccessible identities are omitted.
/// # Errors
/// Returns a generic failure for an unavailable/oversized store or public path.
pub fn current_user_certificates() -> Result<Vec<UserCertificate>, CertificateError> {
    current_user_certificates_cancellable(|| false)
}

/// Performs the same bounded lookup, checking cancellation before starting it
/// and between store entries/key acquisitions. An in-flight native call may
/// finish; cancellation prevents admitting subsequent key lookups.
/// # Errors
/// Returns a generic failure on cancellation or an unavailable/oversized store.
pub fn current_user_certificates_cancellable(
    cancelled: impl Fn() -> bool,
) -> Result<Vec<UserCertificate>, CertificateError> {
    #[cfg(windows)]
    {
        windows::inventory(&cancelled)
    }
    #[cfg(not(windows))]
    {
        let _ = cancelled;
        Err(CertificateError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_inventory_stops_before_native_store_access() {
        assert!(current_user_certificates_cancellable(|| true).is_err());
    }

    #[test]
    fn incompatible_padding_and_prehash_lengths_fail_before_acquisition() {
        for algorithm in [KeyAlgorithm::Rsa, KeyAlgorithm::EcdsaP256] {
            let identity = SigningIdentity {
                fingerprint: [0; 32],
                algorithm,
                #[cfg(windows)]
                container: Vec::new(),
                #[cfg(windows)]
                legacy_spec: 0,
                #[cfg(windows)]
                lookup_hash: [0; 20],
            };
            let wrong_padding = if algorithm == KeyAlgorithm::Rsa {
                SignaturePadding::Ecdsa
            } else {
                SignaturePadding::RsaPss
            };
            assert_eq!(
                identity.sign_hash(DigestAlgorithm::Sha256, wrong_padding, &[0; 32]),
                Err(CertificateError)
            );
            assert_eq!(
                identity.sign_hash(DigestAlgorithm::Sha256, SignaturePadding::Ecdsa, &[0; 31]),
                Err(CertificateError)
            );
        }
    }
}
