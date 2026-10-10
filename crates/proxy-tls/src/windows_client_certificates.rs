//! Windows software keys stay in CurrentUser/My and CNG. Transport signing
//! hashes the complete TLS input once, uses digest-length RSA-PSS salt, and
//! converts raw CNG ECDSA r||s to DER, as Chromium's platform key adapter does.
use crate::client_authentication::{
    ClientCredential, ClientCredentialInventoryFuture, ClientCredentialSource, ClientSigningKey,
    PolicyClientAuthenticationResolver,
};
use boring::{
    bn::BigNum,
    ecdsa::EcdsaSig,
    hash::{MessageDigest, hash},
    x509::X509,
};
use std::{
    sync::{Arc, Weak, atomic::Ordering},
    time::Duration,
};
use transmog_client_certificates::{
    DigestAlgorithm, KeyAlgorithm, SignaturePadding, SigningIdentity,
};

/// Read-only Microsoft software CNG provider. Hardware and legacy CAPI keys are
/// excluded before key acquisition; this provider never invokes native UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsCurrentUserCertificateSource;
impl ClientCredentialSource for WindowsCurrentUserCertificateSource {
    fn credentials(&self) -> ClientCredentialInventoryFuture {
        Box::pin(async {
            crate::blocking_job::run(|cancelled| {
                transmog_client_certificates::current_user_certificates_cancellable(|| {
                    cancelled.load(Ordering::Acquire)
                })
                .map_err(|_| "Windows client certificate inventory is unavailable".to_owned())?
                .into_iter()
                .map(|identity| {
                    let chain = identity
                        .chain
                        .into_iter()
                        .map(|der| X509::from_der(&der))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| {
                            "Windows client certificate inventory is unavailable".to_owned()
                        })?;
                    Ok(Arc::new(ClientCredential {
                        id: identity.key.id(),
                        chain,
                        key: Arc::new(WindowsSigningKey(identity.key)),
                    }))
                })
                .collect::<Result<Vec<_>, String>>()
            })
            .await
            .map_err(|_| "Windows client certificate inventory is unavailable".to_owned())?
        })
    }
}
struct WindowsSigningKey(SigningIdentity);
impl ClientSigningKey for WindowsSigningKey {
    fn algorithms(&self) -> &[u16] {
        match self.0.algorithm() {
            // Match Chromium's ordinary software RSA preference: PKCS#1 first
            // for TLS 1.2, PSS for TLS 1.3. TLS-version filtering is shared.
            KeyAlgorithm::Rsa => &[0x0401, 0x0501, 0x0601, 0x0201, 0x0804, 0x0805, 0x0806],
            // TLS 1.2 permits other digests with these curves; TLS 1.3 binds
            // the signature scheme to the exact curve in shared eligibility.
            KeyAlgorithm::EcdsaP256 | KeyAlgorithm::EcdsaP384 | KeyAlgorithm::EcdsaP521 => {
                &[0x0403, 0x0503, 0x0603, 0x0203]
            }
        }
    }
    fn available(&self) -> bool {
        self.0.available()
    }
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        let failure = || "Windows client certificate signing is unavailable".to_owned();
        if !self.algorithms().contains(&algorithm) {
            return Err(failure());
        }
        let (digest, md) = match algorithm {
            0x0201 | 0x0203 => (DigestAlgorithm::Sha1, MessageDigest::sha1()),
            0x0401 | 0x0403 | 0x0804 => (DigestAlgorithm::Sha256, MessageDigest::sha256()),
            0x0501 | 0x0503 | 0x0805 => (DigestAlgorithm::Sha384, MessageDigest::sha384()),
            0x0601 | 0x0603 | 0x0806 => (DigestAlgorithm::Sha512, MessageDigest::sha512()),
            _ => return Err(failure()),
        };
        let padding = if self.0.algorithm() != KeyAlgorithm::Rsa {
            SignaturePadding::Ecdsa
        } else if (0x0804..=0x0806).contains(&algorithm) {
            SignaturePadding::RsaPss
        } else {
            SignaturePadding::RsaPkcs1
        };
        let digest_bytes = hash(md, input).map_err(|_| failure())?;
        let signature = self
            .0
            .sign_hash(digest, padding, &digest_bytes)
            .map_err(|_| failure())?;
        if padding != SignaturePadding::Ecdsa {
            return Ok(signature);
        }
        let width = match self.0.algorithm() {
            KeyAlgorithm::EcdsaP256 => 32,
            KeyAlgorithm::EcdsaP384 => 48,
            KeyAlgorithm::EcdsaP521 => 66,
            KeyAlgorithm::Rsa => return Err(failure()),
        };
        if signature.len() != width * 2 {
            return Err(failure());
        }
        EcdsaSig::from_private_components(
            BigNum::from_slice(&signature[..width]).map_err(|_| failure())?,
            BigNum::from_slice(&signature[width..]).map_err(|_| failure())?,
        )
        .and_then(|signature| signature.to_der())
        .map_err(|_| failure())
    }
}

/// Owns a periodic inventory monitor. Dropping it aborts the monitor; no source
/// or resolver ownership cycle survives application shutdown.
pub struct WindowsCertificateInventoryMonitor(tokio::task::JoinHandle<()>);
impl Drop for WindowsCertificateInventoryMonitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl WindowsCurrentUserCertificateSource {
    /// Starts a two-second change monitor after reading its initial snapshot.
    /// Observes certificate, chain and key disappearance, including key deletion
    /// that produces no certificate-store event. A changed or failed inventory
    /// invalidates the resolver generation once per transition. Native work is
    /// performed on workers; the resolver is retained only while notifying it.
    /// # Errors
    /// Returns a generic failure if the initial inventory cannot be read.
    pub async fn monitor(
        &self,
        resolver: Weak<PolicyClientAuthenticationResolver>,
    ) -> Result<WindowsCertificateInventoryMonitor, String> {
        let mut previous = snapshot(self.credentials().await?);
        let source = *self;
        Ok(WindowsCertificateInventoryMonitor(tokio::spawn(
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if resolver.strong_count() == 0 {
                        break;
                    }
                    let current = source.credentials().await.ok().and_then(snapshot);
                    if current != previous {
                        let Some(resolver) = resolver.upgrade() else {
                            break;
                        };
                        if resolver.inventory_changed().is_err() {
                            break;
                        }
                        previous = current;
                    }
                }
            },
        )))
    }
}
fn snapshot(credentials: Vec<Arc<ClientCredential>>) -> Option<Vec<(String, Vec<Vec<u8>>)>> {
    let mut entries = credentials
        .into_iter()
        .map(|credential| {
            let chain = credential
                .chain
                .iter()
                .map(|certificate| certificate.to_der())
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some((credential.id.clone(), chain))
        })
        .collect::<Option<Vec<_>>>()?;
    entries.sort();
    Some(entries)
}
