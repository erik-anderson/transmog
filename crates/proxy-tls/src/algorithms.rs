//! Shared algorithm preferences for TCP TLS and QUIC.
use boring::{error::ErrorStack, ssl::SslContextBuilder};

// Keep aligned with Chromium's TLS supported groups. BoringSSL sends initial
// shares for the first group and the first group of the other (PQ/classical)
// kind, giving X25519MLKEM768 and X25519 without a retry round trip.
const PREFERRED_GROUPS: &str = "X25519MLKEM768:X25519:P-256:P-384";

pub(crate) fn configure_key_exchange(context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
    context.set_curves_list(PREFERRED_GROUPS)
}
