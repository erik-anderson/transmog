use serde::{Deserialize, Serialize};

/// HTTP version used on one leg of an intercepted exchange.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum HttpLegVersion {
    /// HTTP/1.1.
    Http1,
    /// HTTP/2.
    Http2,
    /// HTTP/3.
    Http3,
}

/// Result of upstream certificate verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VerificationResult {
    /// `BoringSSL` accepted the chain and endpoint identity.
    Verified,
    /// `BoringSSL` rejected the peer.
    Rejected {
        /// `BoringSSL` verification error code.
        error_code: i32,
        /// Depth at which verification failed, when known.
        depth: Option<u32>,
    },
}

/// Metadata captured from one TLS connection without sensitive key material.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TlsSummary {
    /// Server name sent to the peer, if any.
    pub sni: Option<String>,
    /// Negotiated ALPN bytes rendered as a lossless hexadecimal string when needed.
    pub alpn: Option<String>,
    /// Negotiated cipher name.
    pub cipher: Option<String>,
    /// SHA-256 fingerprints for the peer chain, leaf first.
    pub peer_chain_sha256: Vec<String>,
    /// Immutable trust snapshot generation used for the connection.
    pub trust_generation: u64,
    /// Verification result.
    pub verification: VerificationResult,
}
