use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    vec,
};

use hyper_boring::HttpsConnector;
use hyper_util::client::legacy::connect::{
    HttpConnector,
    dns::{GaiResolver, Name},
};
use tower_service::Service;
use transmog_network::{HappyEyeballsConfig, interleave_candidates};
use transmog_tls::{TrustError, UpstreamTlsContextFactory};

/// System DNS resolver that bounds and family-interleaves origin candidates.
///
/// Hyper remains responsible for racing TCP connections. This resolver makes
/// its address input obey the same finite candidate policy as Transmog's QUIC
/// transport.
#[derive(Clone, Debug)]
pub struct HappyEyeballsResolver {
    inner: GaiResolver,
    happy_eyeballs: HappyEyeballsConfig,
}

impl HappyEyeballsResolver {
    /// Creates a resolver governed by `happy_eyeballs`.
    pub fn new(happy_eyeballs: HappyEyeballsConfig) -> Self {
        Self {
            inner: GaiResolver::new(),
            happy_eyeballs,
        }
    }
}

impl Service<Name> for HappyEyeballsResolver {
    type Response = vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let resolution = self.inner.call(name);
        let limit = self.happy_eyeballs.max_candidates();
        Box::pin(async move {
            let addresses = resolution.await?;
            Ok(interleave_candidates(addresses, limit).into_iter())
        })
    }
}

/// HTTPS origin connector using Transmog's bounded DNS and connection policy.
pub type HttpsOriginConnector = HttpsConnector<HttpConnector<HappyEyeballsResolver>>;

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
) -> Result<HttpsOriginConnector, TrustError> {
    build_https_connector_with_happy_eyeballs(factory, mode, HappyEyeballsConfig::default())
}

/// Creates a Hyper connector with explicit ALPN and Happy Eyeballs policy.
///
/// # Errors
///
/// Returns [`TrustError`] when the common TLS policy, ALPN list, or
/// `hyper-boring` session configuration cannot be applied.
pub fn build_https_connector_with_happy_eyeballs(
    factory: &UpstreamTlsContextFactory,
    mode: HyperEgressMode,
    happy_eyeballs: HappyEyeballsConfig,
) -> Result<HttpsOriginConnector, TrustError> {
    let resolver = HappyEyeballsResolver::new(happy_eyeballs);
    let mut tcp = HttpConnector::new_with_resolver(resolver);
    tcp.enforce_http(false);
    tcp.set_happy_eyeballs_timeout(Some(happy_eyeballs.attempt_delay()));
    let tls = factory.hyper_connector_builder(mode.alpn())?;
    Ok(HttpsConnector::with_connector(tcp, tls)?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use transmog_tls::{
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
            build_https_connector_with_happy_eyeballs(
                &factory,
                mode,
                HappyEyeballsConfig::new(std::time::Duration::from_millis(10), 4).unwrap(),
            )
            .unwrap();
        }
    }
}
