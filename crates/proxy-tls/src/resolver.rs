//! Replaceable downstream certificate resolution.

use std::{
    sync::{Mutex, MutexGuard},
    time::SystemTime,
};

use thiserror::Error;

use crate::{EndpointIdentity, IssuedLeaf, LeafCache, LeafCacheError, ProxyCa};

/// Resolves certificate material for one validated downstream identity.
///
/// Implementations must return a certificate whose subject alternative name
/// exactly covers `identity`. The runtime validates the returned identity again
/// before constructing a TLS acceptor. Resolution runs on the connection task,
/// so implementations must return promptly and must not perform blocking I/O.
pub trait DownstreamCertificateResolver: Send + Sync {
    /// Resolves certificate and private-key material at `now`.
    ///
    /// # Errors
    ///
    /// Returns a redacted resolution failure. Implementations must not include
    /// private key data in the error message.
    fn resolve(
        &self,
        identity: &EndpointIdentity,
        now: SystemTime,
    ) -> Result<IssuedLeaf, CertificateResolverError>;
}

/// Bounded MITM certificate resolver backed by [`LeafCache`].
pub struct CachedMitmCertificateResolver {
    cache: Mutex<LeafCache>,
}

impl std::fmt::Debug for CachedMitmCertificateResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CachedMitmCertificateResolver")
            .finish_non_exhaustive()
    }
}

impl CachedMitmCertificateResolver {
    /// Creates a resolver with explicit cache and certificate-lifetime bounds.
    ///
    /// # Errors
    ///
    /// Returns [`LeafCacheError::InvalidConfiguration`] for an invalid bound.
    pub fn new(ca: ProxyCa, capacity: usize, validity_days: u32) -> Result<Self, LeafCacheError> {
        Ok(Self {
            cache: Mutex::new(LeafCache::new(ca, capacity, validity_days)?),
        })
    }

    fn cache(&self) -> Result<MutexGuard<'_, LeafCache>, CertificateResolverError> {
        self.cache
            .lock()
            .map_err(|_| CertificateResolverError::Unavailable)
    }
}

impl DownstreamCertificateResolver for CachedMitmCertificateResolver {
    fn resolve(
        &self,
        identity: &EndpointIdentity,
        now: SystemTime,
    ) -> Result<IssuedLeaf, CertificateResolverError> {
        self.cache()?
            .get_or_issue(identity.clone(), now)
            .map_err(CertificateResolverError::from)
    }
}

/// Downstream certificate-provider failure.
#[derive(Debug, Error)]
pub enum CertificateResolverError {
    /// The resolver's bounded cache or issuance policy failed.
    #[error(transparent)]
    Leaf(#[from] LeafCacheError),
    /// The resolver is unavailable after an internal synchronization failure.
    #[error("downstream certificate resolver is unavailable")]
    Unavailable,
    /// A custom resolver returned material for a different identity.
    #[error("downstream certificate resolver returned a mismatched identity")]
    IdentityMismatch,
    /// A custom resolver failed without exposing sensitive details.
    #[error("downstream certificate resolution failed: {0}")]
    Provider(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_resolver_reuses_exact_identity_and_keeps_identities_separate() {
        let resolver = CachedMitmCertificateResolver::new(
            ProxyCa::generate("Transmog resolver test", 2).unwrap(),
            2,
            1,
        )
        .unwrap();
        let first = EndpointIdentity::Dns("one.example".to_owned());
        let second = EndpointIdentity::Dns("two.example".to_owned());
        let now = SystemTime::now();
        let first_leaf = resolver.resolve(&first, now).unwrap();
        let cached_leaf = resolver.resolve(&first, now).unwrap();
        let second_leaf = resolver.resolve(&second, now).unwrap();

        assert_eq!(first_leaf.identity, first);
        assert_eq!(cached_leaf.identity, first);
        assert_eq!(
            first_leaf.certificate.to_der().unwrap(),
            cached_leaf.certificate.to_der().unwrap()
        );
        assert_eq!(second_leaf.identity, second);
        assert_ne!(
            first_leaf.certificate.to_der().unwrap(),
            second_leaf.certificate.to_der().unwrap()
        );
    }
}
