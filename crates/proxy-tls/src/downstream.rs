use boring::ssl::{AlpnError, SslAcceptor, SslMethod, SslVersion, select_next_proto};
use thiserror::Error;

use crate::IssuedLeaf;

const DOWNSTREAM_ALPN: &[u8] = b"\x02h2\x08http/1.1";

/// Browser-facing TLS policy used after an accepted CONNECT.
#[derive(Clone, Copy, Debug)]
pub struct DownstreamTlsPolicy {
    /// Minimum accepted TLS version.
    pub minimum_version: SslVersion,
    /// Maximum accepted TLS version.
    pub maximum_version: SslVersion,
}

impl Default for DownstreamTlsPolicy {
    fn default() -> Self {
        Self {
            minimum_version: SslVersion::TLS1_2,
            maximum_version: SslVersion::TLS1_3,
        }
    }
}

/// Builds browser-facing `BoringSSL` acceptors from issued leaf material.
#[derive(Clone, Copy, Debug)]
pub struct DownstreamTlsContextFactory {
    policy: DownstreamTlsPolicy,
}

impl DownstreamTlsContextFactory {
    /// Creates a factory with an explicit browser-facing policy.
    pub fn new(policy: DownstreamTlsPolicy) -> Self {
        Self { policy }
    }

    /// Creates an acceptor offering HTTP/2 and HTTP/1.1 through ALPN.
    ///
    /// # Errors
    ///
    /// Returns [`DownstreamTlsError`] when the certificate/key pair or TLS
    /// policy cannot be installed in a `BoringSSL` context.
    pub fn acceptor(&self, leaf: &IssuedLeaf) -> Result<SslAcceptor, DownstreamTlsError> {
        let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;
        builder.set_min_proto_version(Some(self.policy.minimum_version))?;
        builder.set_max_proto_version(Some(self.policy.maximum_version))?;
        builder.set_certificate(&leaf.certificate)?;
        builder.set_private_key(&leaf.private_key)?;
        builder.check_private_key()?;
        builder.set_alpn_select_callback(|_, offered| {
            select_next_proto(DOWNSTREAM_ALPN, offered).ok_or(AlpnError::NOACK)
        });
        Ok(builder.build())
    }
}

/// Browser-facing TLS context construction failure.
#[derive(Debug, Error)]
pub enum DownstreamTlsError {
    /// `BoringSSL` rejected the policy or issued leaf.
    #[error("BoringSSL downstream TLS context failed: {0}")]
    Boring(#[from] boring::error::ErrorStack),
}

#[cfg(test)]
mod tests {
    use crate::{EndpointIdentity, ProxyCa};

    use super::*;

    #[test]
    fn issued_leaf_builds_h1_h2_acceptor() {
        let ca = ProxyCa::generate("Transmog downstream test", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("example.test").unwrap(), 1)
            .unwrap();
        DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&leaf)
            .unwrap();
    }
}
