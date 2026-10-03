use hyper_boring::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use rustymiddle_tls::{TrustError, UpstreamTlsContextFactory};

/// Protocols advertised by one Hyper origin connector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyperEgressMode {
    /// Require HTTP/1.1 ALPN.
    Http1Only,
    /// Require HTTP/2 ALPN.
    Http2Only,
    /// Permit HTTP/2 or HTTP/1.1, preferring HTTP/2.
    Auto,
}

impl HyperEgressMode {
    fn alpn(self) -> &'static [u8] {
        match self {
            Self::Http1Only => b"\x08http/1.1",
            Self::Http2Only => b"\x02h2",
            Self::Auto => b"\x02h2\x08http/1.1",
        }
    }
}

/// Creates a Hyper connector from the shared upstream TLS factory.
///
/// # Errors
///
/// Returns [`TrustError`] when the common TLS policy, ALPN list, or
/// `hyper-boring` session configuration cannot be applied.
pub fn build_https_connector(
    factory: &UpstreamTlsContextFactory,
    mode: HyperEgressMode,
) -> Result<HttpsConnector<HttpConnector>, TrustError> {
    let mut tcp = HttpConnector::new();
    tcp.enforce_http(false);
    let tls = factory.hyper_connector_builder(mode.alpn())?;
    Ok(HttpsConnector::with_connector(tcp, tls)?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustymiddle_tls::{
        SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };

    use super::*;

    #[test]
    fn shared_snapshot_builds_each_hyper_alpn_mode() {
        let snapshot = Arc::new(TrustSnapshot::load(&SystemTrustSource, 7).unwrap());
        let factory = UpstreamTlsContextFactory::new(snapshot, UpstreamTlsPolicy::default());
        for mode in [
            HyperEgressMode::Http1Only,
            HyperEgressMode::Http2Only,
            HyperEgressMode::Auto,
        ] {
            build_https_connector(&factory, mode).unwrap();
        }
    }
}
