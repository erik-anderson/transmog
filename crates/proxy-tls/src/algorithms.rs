//! Shared algorithm preferences for TCP TLS and QUIC.
use boring::{
    error::ErrorStack,
    ssl::{SslContextBuilder, SslSignatureAlgorithm},
};

// Keep aligned with Chromium's TLS supported groups. BoringSSL sends initial
// shares for the first group and the first group of the other (PQ/classical)
// kind, giving X25519MLKEM768 and X25519 without a retry round trip.
const PREFERRED_GROUPS: &str = "X25519MLKEM768:X25519:P-256:P-384";

pub(crate) fn configure_key_exchange(context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
    context.set_curves_list(PREFERRED_GROUPS)
}

pub(crate) fn configure_authentication(context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
    // Chromium's PQ verification preferences, followed by the existing
    // BoringSSL classical list so this change preserves origin compatibility.
    context.set_verify_algorithm_prefs(&[
        SslSignatureAlgorithm::ML_DSA_44,
        SslSignatureAlgorithm::ML_DSA_65,
        SslSignatureAlgorithm::ML_DSA_87,
        SslSignatureAlgorithm::ECDSA_SECP256R1_SHA256,
        SslSignatureAlgorithm::RSA_PSS_RSAE_SHA256,
        SslSignatureAlgorithm::RSA_PKCS1_SHA256,
        SslSignatureAlgorithm::ECDSA_SECP384R1_SHA384,
        SslSignatureAlgorithm::RSA_PSS_RSAE_SHA384,
        SslSignatureAlgorithm::RSA_PKCS1_SHA384,
        SslSignatureAlgorithm::RSA_PSS_RSAE_SHA512,
        SslSignatureAlgorithm::RSA_PKCS1_SHA512,
        SslSignatureAlgorithm::RSA_PKCS1_SHA1,
    ])
}
