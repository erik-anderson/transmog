//! TCP HTTPS adapter with Chromium's externally managed client ticket cache.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{Ssl, SslContext, SslContextBuilder, SslRef, SslSession, SslSessionCacheMode},
};
use http::Uri;
use hyper_boring::MaybeHttpsStream;
use hyper_util::{client::legacy::connect::Connection, rt::TokioIo};
use std::{
    error::Error,
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tower_service::Service;
use transmog_tls::{
    UpstreamTlsContextFactory,
    local_trust::TrustEpoch,
    session_cache::{SessionCache, SessionLifetime},
};

type BoxError = Box<dyn Error + Send + Sync>;
// The pinned boring wrapper's Ssl Debug getter still assumes legacy X.509.
// Preserve its source chain, but keep error Debug formatting buffer-compatible.
struct HandshakeFailure(BoxError);
impl std::fmt::Display for HandshakeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl std::fmt::Debug for HandshakeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl Error for HandshakeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}
type Callback = Arc<dyn Fn(&mut SslRef, &Uri) -> Result<(), ErrorStack> + Send + Sync>;
#[derive(Clone, Debug, Eq, PartialEq)]
struct Key {
    host: String,
    port: u16,
    trust: TrustEpoch,
    authentication: u64,
    ech: Option<Vec<u8>>,
}
#[derive(Clone)]
struct TicketOwner {
    key: Key,
    generation: u64,
}
static OWNER: OnceLock<Result<Index<Ssl, TicketOwner>, ErrorStack>> = OnceLock::new();
fn owner_index() -> Result<Index<Ssl, TicketOwner>, ErrorStack> {
    OWNER.get_or_init(Ssl::new_ex_index).clone()
}

/// HTTPS over an existing TCP service, with one bounded caller/run ticket partition.
#[derive(Clone)]
pub struct ChromiumHttpsConnector<S> {
    tcp: S,
    context: SslContext,
    factory: UpstreamTlsContextFactory,
    cache: Arc<SessionCache<Key, SslSession>>,
    callback: Option<Callback>,
    automatic_ech_retry: bool,
    offered_protocols: Vec<String>,
}
impl<S> ChromiumHttpsConnector<S> {
    pub(crate) fn new(
        tcp: S,
        mut builder: SslContextBuilder,
        factory: UpstreamTlsContextFactory,
        alpn: &[u8],
    ) -> Result<Self, ErrorStack> {
        let cache = Arc::new(SessionCache::default());
        let index = owner_index()?;
        let tickets = cache.clone();
        // Request-driven identity selection requires a fresh CertificateRequest.
        let enabled = !factory.has_client_certificate_resolver();
        builder
            .set_session_cache_mode(SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL);
        builder.set_new_session_callback(move |ssl, session| {
            if enabled && let Some(owner) = ssl.ex_data(index) {
                let lifetime = SessionLifetime::from_session(&session);
                tickets.insert(owner.generation, owner.key.clone(), session, lifetime);
            }
        });
        Ok(Self {
            tcp,
            context: builder.build(),
            factory,
            cache,
            callback: None,
            automatic_ech_retry: true,
            offered_protocols: decode_alpn(alpn),
        })
    }
    /// Installs per-connection policy before the first `ClientHello` is sent.
    pub fn set_ssl_callback<F>(&mut self, callback: F)
    where
        F: Fn(&mut SslRef, &Uri) -> Result<(), ErrorStack> + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(callback));
    }
    pub(crate) fn capture_each_ech_attempt(&mut self) {
        self.automatic_ech_retry = false;
    }
}
impl<S, T> Service<Uri> for ChromiumHttpsConnector<S>
where
    S: Service<Uri, Response = TokioIo<T>> + Send + Clone + 'static,
    S::Error: Into<BoxError>,
    S::Future: Send + 'static,
    T: AsyncRead + AsyncWrite + Connection + Unpin + Send + Sync + std::fmt::Debug + 'static,
{
    type Response = MaybeHttpsStream<T>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, BoxError>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.tcp.poll_ready(cx).map_err(Into::into)
    }
    #[allow(clippy::too_many_lines)] // Keeps endpoint selection and the single pre-HTTP retry in one owned future.
    fn call(&mut self, uri: Uri) -> Self::Future {
        let mut tcp = self.tcp.clone();
        let context = self.context.clone();
        let factory = self.factory.clone();
        let cache = self.cache.clone();
        let callback = self.callback.clone();
        let automatic_ech_retry = self.automatic_ech_retry;
        let mut alpn = self.offered_protocols.clone();
        Box::pin(async move {
            let https = matches!(uri.scheme_str(), Some("https" | "wss"));
            factory.prepare_trust().await?;
            let host = uri
                .host()
                .ok_or("URI missing host")?
                .trim_matches(['[', ']']);
            let allowed = crate::metrics::allowed_for_setup()
                .or_else(|| factory.network_profile().map(|p| p.settings.protocols))
                .unwrap_or_default();
            alpn.retain(|protocol| match protocol.as_str() {
                "h2" => allowed.http2,
                "http/1.1" => allowed.http1,
                _ => false,
            });
            let discovery_start = std::time::Instant::now();
            let mut offer = if https {
                factory
                    .ech_offer(host, uri.port_u16().unwrap_or(443), alpn)
                    .await
            } else {
                None
            };
            if https && host.contains('.') && host.parse::<std::net::IpAddr>().is_err() {
                crate::metrics::dns_finished(discovery_start, true);
            }
            for attempt in 0..2 {
                let routed = if let Some(offer) = &offer {
                    Uri::builder()
                        .scheme(uri.scheme_str().unwrap_or("https"))
                        .authority(if offer.endpoint.parse::<std::net::Ipv6Addr>().is_ok() {
                            format!("[{}]:{}", offer.endpoint, offer.port)
                        } else {
                            format!("{}:{}", offer.endpoint, offer.port)
                        })
                        .path_and_query("/")
                        .build()?
                } else {
                    uri.clone()
                };
                std::future::poll_fn(|cx| tcp.poll_ready(cx))
                    .await
                    .map_err(Into::into)?;
                let socket = tcp.call(routed).await.map_err(Into::into)?.into_inner();
                if !https {
                    return Ok(MaybeHttpsStream::Http(socket));
                }
                let key = Key {
                    host: host.to_ascii_lowercase(),
                    port: uri.port_u16().unwrap_or(443),
                    trust: factory.trust_epoch(),
                    authentication: factory.client_certificate_admission_generation(),
                    ech: offer
                        .as_ref()
                        .filter(|o| !o.config.is_empty())
                        .map(|o| {
                            boring::hash::hash(boring::hash::MessageDigest::sha256(), &o.config)
                                .map(|hash| hash.to_vec())
                        })
                        .transpose()?,
                };
                let mut ssl = Ssl::new(&context)?;
                if host.parse::<std::net::IpAddr>().is_err() {
                    ssl.set_hostname(host)?;
                }
                if let Some(offer) = &offer
                    && !offer.config.is_empty()
                {
                    transmog_tls::configure_ech(&mut ssl, &offer.config)?;
                }
                ssl.set_ex_data(
                    owner_index()?,
                    TicketOwner {
                        key: key.clone(),
                        generation: cache.generation(),
                    },
                );
                if !factory.has_client_certificate_resolver()
                    && let Some(session) = cache.take(&key)
                {
                    transmog_tls::restore_client_session(&mut ssl, &session)?;
                }
                if let Some(callback) = &callback {
                    callback(&mut ssl, &uri)?;
                }
                if let Some(offer) = &offer {
                    transmog_tls::ech::record_ech_service(&ssl, offer);
                }
                match tokio_boring::SslStreamBuilder::new(ssl, socket)
                    .connect()
                    .await
                {
                    Ok(stream) => return Ok(MaybeHttpsStream::Https(stream)),
                    Err(error) => {
                        let retry = error.ssl().and_then(transmog_tls::authenticated_ech_retry);
                        if automatic_ech_retry
                            && attempt == 0
                            && let Some(config) = retry
                            && let Some(offer) = &mut offer
                        {
                            offer.config = config.into();
                            continue;
                        }
                        return Err(Box::new(HandshakeFailure(Box::new(error))) as BoxError);
                    }
                }
            }
            Err("ECH retry limit reached".into())
        })
    }
}
fn decode_alpn(mut wire: &[u8]) -> Vec<String> {
    let mut protocols = Vec::new();
    while let Some((length, rest)) = wire.split_first() {
        let length = usize::from(*length);
        if rest.len() < length {
            break;
        }
        if let Ok(protocol) = std::str::from_utf8(&rest[..length]) {
            protocols.push(protocol.to_owned());
        }
        wire = &rest[length..];
    }
    protocols
}
