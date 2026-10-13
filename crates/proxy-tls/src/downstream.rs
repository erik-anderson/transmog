use boring::ssl::{
    AlpnError, SslAcceptor, SslAcceptorBuilder, SslMethod, SslVersion, select_next_proto,
};
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
        self.acceptor_captured(leaf, None)
    }

    /// Creates an acceptor collecting public handshake facts before native state is released.
    /// # Errors
    /// Returns an error when the certificate, TLS policy or evidence callback cannot be installed.
    pub fn acceptor_captured(
        &self,
        leaf: &IssuedLeaf,
        capture: Option<&crate::TlsCapture>,
    ) -> Result<SslAcceptor, DownstreamTlsError> {
        let mut builder = self.builder(capture)?;
        builder.set_certificate(&leaf.certificate)?;
        builder.set_private_key(&leaf.private_key)?;
        builder.check_private_key()?;
        Ok(builder.build())
    }

    /// Offers ordered alternatives; `BoringSSL` selects only a peer-compatible credential.
    /// # Errors
    /// Returns an error for empty/mismatched identities or invalid credential material.
    pub fn acceptor_credentials_captured(
        &self,
        leaves: &[IssuedLeaf],
        capture: Option<&crate::TlsCapture>,
    ) -> Result<SslAcceptor, DownstreamTlsError> {
        let fallback = leaves
            .last()
            .ok_or(DownstreamTlsError::InvalidCredentials)?;
        if leaves.iter().any(|leaf| leaf.identity != fallback.identity) {
            return Err(DownstreamTlsError::InvalidCredentials);
        }
        let mut builder = self.builder(capture)?;
        // The legacy credential is always last in BoringSSL's selection list.
        builder.set_certificate(&fallback.certificate)?;
        builder.set_private_key(&fallback.private_key)?;
        builder.check_private_key()?;
        for leaf in &leaves[..leaves.len() - 1] {
            let chain: Vec<_> = std::iter::once(leaf.certificate.clone())
                .chain(leaf.issuer_chain.iter().cloned())
                .collect();
            transmog_tls_native::add_server_credential(&mut builder, &chain, &leaf.private_key)?;
        }
        Ok(builder.build())
    }

    fn builder(
        self,
        capture: Option<&crate::TlsCapture>,
    ) -> Result<SslAcceptorBuilder, DownstreamTlsError> {
        let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;
        builder.set_min_proto_version(Some(self.policy.minimum_version))?;
        builder.set_max_proto_version(Some(self.policy.maximum_version))?;
        crate::algorithms::configure_key_exchange(&mut builder)?;
        if let Some(capture) = capture {
            capture.attach_context(&mut builder)?;
        }
        builder.set_alpn_select_callback(|_, offered| {
            select_next_proto(DOWNSTREAM_ALPN, offered).ok_or(AlpnError::NOACK)
        });
        Ok(builder)
    }
}

/// Browser-facing TLS context construction failure.
#[derive(Debug, Error)]
pub enum DownstreamTlsError {
    /// No credentials, or credentials for different target identities, were supplied.
    #[error("downstream TLS credentials must cover one nonempty identity set")]
    InvalidCredentials,
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
