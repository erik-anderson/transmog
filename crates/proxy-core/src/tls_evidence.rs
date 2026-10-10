//! Public TLS evidence; certificate bytes are carried privately and stored once.
use crate::VerificationResult;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, sync::Arc};

/// One public X.509 certificate, addressed by its SHA-256 digest.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicCertificate {
    /// Lowercase SHA-256 digest of the original DER certificate.
    pub sha256: String,
    /// Exact public DER. Native captures store it in a shared certificate definition,
    /// while message and transport records carry only the digest reference.
    #[serde(skip)]
    pub der: Arc<[u8]>,
}
impl fmt::Debug for PublicCertificate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicCertificate")
            .field("sha256", &self.sha256)
            .field("bytes", &self.der.len())
            .finish()
    }
}
impl PublicCertificate {
    /// Checks the bounded reference shape; importers validate DER and its digest.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.der.len() <= 1024 * 1024
    }
}

/// Recorded connection facts needed to understand certificates and TLS failures.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsEvidence {
    /// Server name used for the connection, when available.
    pub server_name: Option<String>,
    /// Server certificate chain, leaf first. No private keys or session secrets.
    pub server_chain: Vec<PublicCertificate>,
    /// Client authentication certificate chain, if one was configured or supplied.
    pub client_chain: Vec<PublicCertificate>,
    /// How the server chain was obtained: verification path, presented chain or configured.
    pub chain_source: Option<String>,
    /// Local minimum/maximum TLS version bounds at connection creation.
    pub offered_versions: Vec<String>,
    /// Local cipher suites available at connection creation.
    pub offered_ciphers: Vec<String>,
    /// Local application protocols offered at connection creation.
    pub offered_alpn: Vec<String>,
    /// Source of the trust snapshot, independent of the current machine's trust.
    pub trust_source: Option<String>,
    /// Immutable trust snapshot generation, if this adapter exposes it.
    pub trust_generation: Option<u64>,
    /// Recorded verification result; absent if verification was not reached.
    pub verification: Option<VerificationResult>,
    /// Recorded verification or negotiation explanations without key material.
    pub errors: Vec<String>,
    /// Original summary-only TLS fields from formats which omitted certificate DER.
    #[serde(default)]
    pub details: BTreeMap<String, String>,
}
impl TlsEvidence {
    /// Checks finite shape before accepting saved evidence.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.server_chain.len() <= 32
            && self.client_chain.len() <= 32
            && self
                .server_chain
                .iter()
                .chain(&self.client_chain)
                .all(PublicCertificate::valid)
            && self.offered_versions.len() <= 8
            && self.offered_ciphers.len() <= 256
            && self.offered_alpn.len() <= 16
            && self
                .offered_versions
                .iter()
                .chain(&self.offered_ciphers)
                .chain(&self.offered_alpn)
                .all(|value| value.len() <= 256)
            && [&self.server_name, &self.chain_source, &self.trust_source]
                .into_iter()
                .all(|value| value.as_ref().is_none_or(|value| value.len() <= 1024))
            && self.errors.len() <= 16
            && self.errors.iter().all(|value| value.len() <= 2048)
            && self.details.len() <= 64
            && self
                .details
                .iter()
                .all(|(key, value)| key.len() <= 256 && value.len() <= 4096)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_references_never_serialize_certificate_bytes_or_arbitrary_extra_fields() {
        let certificate = PublicCertificate {
            sha256: "a".repeat(64),
            der: Arc::from(b"public certificate DER".as_slice()),
        };
        let json = serde_json::to_value(&certificate).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 1);
        assert_eq!(json["sha256"], "a".repeat(64));
        assert!(
            serde_json::from_value::<PublicCertificate>(
                serde_json::json!({"sha256":"a".repeat(64),"privateKey":"must not be accepted"})
            )
            .is_err()
        );
        let reopened: PublicCertificate = serde_json::from_value(json).unwrap();
        assert!(reopened.der.is_empty());
        assert!(reopened.valid());
    }
}
