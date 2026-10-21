//! Frozen untrusted CTLs. Windows owns their subject-identifier matching rules.
use crate::LocalTrustError;

/// Owned, immutable untrusted CTLs. Matching performs no store or network IO.
#[derive(Debug, Default)]
pub struct CtlDistrust {
    #[cfg(windows)]
    lists: Vec<windows::Ctl>,
    entries: usize,
}
impl CtlDistrust {
    /// Decodes copied OS CTLs without granting trust or verifying a peer chain.
    /// # Errors
    /// Rejects malformed, oversized or unsupported platform inputs.
    pub fn new(encoded: &[Vec<u8>]) -> Result<Self, LocalTrustError> {
        #[cfg(not(windows))]
        {
            if !encoded.is_empty() {
                return Err(LocalTrustError::UnsupportedCtl);
            }
            Ok(Self::default())
        }
        #[cfg(windows)]
        {
            let mut result = Self::default();
            for bytes in encoded {
                let ctl = windows::Ctl::new(bytes)?;
                result.entries += ctl.entries();
                result.lists.push(ctl);
            }
            Ok(result)
        }
    }
    /// Number of untrusted subject identifiers, including identifiers without DER.
    pub fn len(&self) -> usize {
        self.entries
    }
    /// Whether the snapshot has no untrusted identifiers.
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }
    /// Matches DER using the CTL's subject algorithm, including Windows signature
    /// hashes. No certificate properties from a store are used.
    /// # Errors
    /// Returns decoding/matching failures instead of silently omitting distrust.
    pub fn contains(&self, der: &[u8]) -> Result<bool, LocalTrustError> {
        #[cfg(windows)]
        {
            windows::contains(&self.lists, der)
        }
        #[cfg(not(windows))]
        {
            let _ = der;
            Ok(false)
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::LocalTrustError;
    use std::{ffi::CStr, sync::Mutex};
    use windows_sys::Win32::Security::Cryptography::{
        CERT_MD5_HASH_PROP_ID, CERT_SHA1_HASH_PROP_ID, CERT_SHA256_HASH_PROP_ID,
        CERT_SIGNATURE_HASH_PROP_ID, CRYPT_INTEGER_BLOB, CTL_ANY_SUBJECT_INFO,
        CTL_ANY_SUBJECT_TYPE, CTL_CONTEXT, CertCreateCTLContext, CertCreateCertificateContext,
        CertFindSubjectInCTL, CertFreeCTLContext, CertFreeCertificateContext,
        CertGetCertificateContextProperty, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };

    #[derive(Debug)]
    pub(super) struct Ctl {
        context: Mutex<*const CTL_CONTEXT>,
        property: u32,
    }
    // SAFETY: owns a read-only Crypt32 context; serialized matching keeps even
    // Crypt32's lazy lookup caches confined to one caller at a time.
    unsafe impl Send for Ctl {}
    unsafe impl Sync for Ctl {}
    impl Drop for Ctl {
        fn drop(&mut self) {
            // SAFETY: final owner releases its native context exactly once.
            unsafe {
                CertFreeCTLContext(
                    *self
                        .context
                        .get_mut()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                )
            };
        }
    }
    impl Ctl {
        pub(super) fn new(bytes: &[u8]) -> Result<Self, LocalTrustError> {
            if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
                return Err(LocalTrustError::Limit);
            }
            // SAFETY: bounded bytes remain live; Crypt32 copies their content.
            let context = unsafe {
                CertCreateCTLContext(
                    X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                    bytes.as_ptr(),
                    u32::try_from(bytes.len()).map_err(|_| LocalTrustError::Limit)?,
                )
            };
            if context.is_null() {
                return Err(crate::windows::error("untrusted CTL decoding"));
            }
            let mut result = Self {
                context: Mutex::new(context),
                property: 0,
            };
            // SAFETY: successful Crypt32 decoder owns a valid CTL_INFO.
            let info = unsafe { &*(*context).pCtlInfo };
            if info.cCTLEntry > 131_072 || (info.cCTLEntry != 0 && info.rgCTLEntry.is_null()) {
                return Err(LocalTrustError::Limit);
            }
            if info.SubjectAlgorithm.pszObjId.is_null() {
                return Err(LocalTrustError::UnsupportedCtl);
            }
            // SAFETY: the decoder owns a terminated OID string in CTL_INFO.
            let oid = unsafe { CStr::from_ptr(info.SubjectAlgorithm.pszObjId.cast()) }.to_bytes();
            result.property = match oid {
                b"1.3.14.3.2.26" | b"1.3.6.1.4.1.311.10.11.3" => CERT_SHA1_HASH_PROP_ID,
                b"1.2.840.113549.2.5" | b"1.3.6.1.4.1.311.10.11.4" => CERT_MD5_HASH_PROP_ID,
                b"2.16.840.1.101.3.4.2.1" | b"1.3.6.1.4.1.311.10.11.107" => {
                    CERT_SHA256_HASH_PROP_ID
                }
                b"1.3.6.1.4.1.311.10.11.15" => CERT_SIGNATURE_HASH_PROP_ID,
                _ => return Err(LocalTrustError::UnsupportedCtl),
            };
            Ok(result)
        }
        pub(super) fn entries(&self) -> usize {
            let context = self
                .context
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // SAFETY: owned, successfully decoded immutable native context.
            unsafe { (**context).pCtlInfo.as_ref().unwrap().cCTLEntry as usize }
        }
    }
    pub(super) fn contains(lists: &[Ctl], der: &[u8]) -> Result<bool, LocalTrustError> {
        if lists.is_empty() {
            return Ok(false);
        }
        // SAFETY: input lives through the call; returned context is owned.
        let cert = unsafe {
            CertCreateCertificateContext(
                X509_ASN_ENCODING,
                der.as_ptr(),
                u32::try_from(der.len()).map_err(|_| LocalTrustError::Limit)?,
            )
        };
        if cert.is_null() {
            return Err(crate::windows::error("untrusted certificate decoding"));
        }
        let result = (|| {
            for list in lists {
                let context = list
                    .context
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut hash = [0_u8; 128];
                let mut length = u32::try_from(hash.len()).unwrap();
                // SAFETY: fresh DER-only context and fixed output buffer. Crypt32
                // computes the appropriate signature digest, including MD5/SHA-384.
                if unsafe {
                    CertGetCertificateContextProperty(
                        cert,
                        list.property,
                        hash.as_mut_ptr().cast(),
                        &raw mut length,
                    )
                } == 0
                {
                    return Err(crate::windows::error("untrusted CTL subject hashing"));
                }
                // CertFindSubjectInCTL's CERT mode only recognizes SHA-1/MD5
                // OIDs, not the property OID used by Windows' untrusted CTL.
                // Compute that property explicitly and use exact ANY-mode lookup.
                let subject = CTL_ANY_SUBJECT_INFO {
                    SubjectAlgorithm: unsafe {
                        (**context).pCtlInfo.as_ref().unwrap().SubjectAlgorithm
                    },
                    SubjectIdentifier: CRYPT_INTEGER_BLOB {
                        cbData: length,
                        pbData: hash.as_mut_ptr(),
                    },
                };
                // SAFETY: both contexts are live; matching only computes properties
                // of the supplied DER and searches the copied CTL, without IO.
                let entry = unsafe {
                    CertFindSubjectInCTL(
                        0,
                        CTL_ANY_SUBJECT_TYPE,
                        (&raw const subject).cast(),
                        *context,
                        0,
                    )
                };
                if !entry.is_null() {
                    return Ok(true);
                }
                let error = crate::windows::error("untrusted CTL matching");
                if !matches!(
                    error,
                    LocalTrustError::Windows {
                        code: crate::windows::NOT_FOUND,
                        ..
                    }
                ) {
                    return Err(error);
                }
            }
            Ok(false)
        })();
        // SAFETY: owned context, released once after all searches have completed.
        unsafe { CertFreeCertificateContext(cert) };
        result
    }
}
