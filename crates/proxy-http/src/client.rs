use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Version, header::HOST, uri::InvalidUri};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{Client, Error as ClientError},
    rt::TokioExecutor,
};
use thiserror::Error;
use tokio::time::timeout;
use transmog_core::network_profile::chromium_http;
use transmog_core::{
    BodyFrame, BodyStream, BodyStreamError, CanonicalRequest, CanonicalResponse, HeaderBlock,
    HeaderField, HttpLegVersion, MessageKind, RequestHead, ResponseHead, StreamingRequest,
    StreamingResponse, TranslationOptions, prepare_headers,
};
use transmog_network::HappyEyeballsConfig;
use transmog_tls::{TrustError, UpstreamTlsContextFactory};

#[path = "websocket_h2.rs"]
mod websocket_h2;

/// Transport exposed by a successful WebSocket handshake on either HTTP version.
pub trait WebSocketIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> WebSocketIo for T {}
/// Deferred acquisition of a WebSocket byte stream.
pub type WebSocketUpgrade = Pin<
    Box<dyn std::future::Future<Output = Result<Box<dyn WebSocketIo>, HyperOriginError>> + Send>,
>;

use crate::{
    HyperEgressMode,
    metrics::{ConnectionObservation, MeasuredHttpsConnector},
};
use transmog_core::performance::{Milestone, PerformanceRecorder, ProtocolObservation};

#[derive(Clone)]
struct ResponseObservation {
    connection: ConnectionObservation,
    shared: bool,
    performance: PerformanceRecorder,
}
impl Drop for ResponseObservation {
    fn drop(&mut self) {
        self.performance.transport(
            self.connection
                .snapshot(self.shared, Some(&self.performance)),
        );
    }
}

type OriginClient = Client<MeasuredHttpsConnector, CanonicalHttpBody>;

/// Pooled Hyper origin client with distinct ALPN pools for forced H1 and H2.
#[derive(Clone)]
pub struct HyperOriginClient {
    pools: std::sync::Arc<std::sync::Mutex<GenerationPools>>,
    protocols: transmog_core::connection_policy::OutboundProtocolPolicy,
    factory: UpstreamTlsContextFactory,
    happy_eyeballs: HappyEyeballsConfig,
    performance: Option<PerformanceRecorder>,
    websockets: websocket_h2::Pool,
}
#[derive(Clone)]
struct GenerationPools {
    epoch: u64,
    trust_epoch: transmog_tls::local_trust::TrustEpoch,
    h1: OriginClient,
    h2: OriginClient,
    auto: OriginClient,
}
impl GenerationPools {
    fn client(&self, mode: HyperEgressMode) -> OriginClient {
        match mode {
            HyperEgressMode::Http1Only => self.h1.clone(),
            HyperEgressMode::Http2Only => self.h2.clone(),
            HyperEgressMode::Auto => self.auto.clone(),
        }
    }
}

/// Result of an HTTP/1.1 Upgrade or HTTP/2 extended CONNECT attempt.
pub enum HyperUpgradeResponse {
    /// Origin accepted the protocol switch and exposed the upgraded stream.
    Switched {
        /// Canonical successful response head (101 for H1, 2xx for H2).
        head: ResponseHead,
        /// Future resolving to the origin byte stream.
        upgraded: WebSocketUpgrade,
        /// Exact canonical request sent on the selected HTTP leg.
        request: RequestHead,
    },
    /// Origin declined the switch with an ordinary streaming response.
    Rejected(StreamingResponse, RequestHead),
}

impl std::fmt::Debug for HyperUpgradeResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Switched { head, .. } => formatter
                .debug_struct("Switched")
                .field("head", head)
                .finish_non_exhaustive(),
            Self::Rejected(response, _) => {
                formatter.debug_tuple("Rejected").field(response).finish()
            }
        }
    }
}

impl HyperOriginClient {
    /// Enables or disables physical connection sharing for subsequent requests.
    pub fn set_server_connection_reuse(&self, enabled: bool) {
        self.protocols.set_reuse_connections(enabled);
    }
    /// Installs authentication in new pools for an immutable application caller.
    /// # Errors
    /// Returns a TLS connector construction failure.
    pub fn with_client_certificate_resolver(
        &self,
        resolver: std::sync::Arc<
            dyn transmog_tls::client_authentication::ClientAuthenticationResolver,
        >,
        caller: impl Into<String>,
    ) -> Result<Self, HyperOriginError> {
        Self::with_connection_policy(
            &self
                .factory
                .clone()
                .with_client_certificate_resolver(resolver, caller),
            self.happy_eyeballs,
            self.protocols.clone(),
        )
    }
    /// Creates a new pool for an immutable certificate caller partition.
    /// # Errors
    /// Returns a TLS connector construction failure.
    pub fn for_client_certificate_caller(
        &self,
        caller: impl Into<String>,
    ) -> Result<Self, HyperOriginError> {
        Self::with_connection_policy(
            &self.factory.clone().for_client_certificate_caller(caller),
            self.happy_eyeballs,
            self.protocols.clone(),
        )
    }
    async fn authorized_client(
        &self,
        uri: &http::Uri,
        mode: HyperEgressMode,
    ) -> Result<OriginClient, HyperOriginError> {
        if matches!(uri.scheme_str(), Some("https" | "wss")) {
            self.factory.prepare_trust().await?;
        }
        let reuse = self.protocols.reuse_connections()
            && (!matches!(uri.scheme_str(), Some("https" | "wss"))
                || self.factory.permits_client_certificate_reuse(
                    uri.host().unwrap_or("").trim_matches(['[', ']']),
                    uri.port_u16().unwrap_or(443),
                ));
        if !reuse {
            let fresh = Self::build_pools(
                &self.factory,
                self.happy_eyeballs,
                self.protocols.clone(),
                false,
            )?;
            let client = fresh
                .pools
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .client(mode);
            return Ok(client);
        }
        let epoch = self.factory.client_certificate_generation();
        let trust_epoch = self.factory.trust_epoch();
        {
            let pools = self
                .pools
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if epoch == pools.epoch && trust_epoch == pools.trust_epoch {
                return Ok(pools.client(mode));
            }
        }
        let next = Self::build_pools(
            &self.factory,
            self.happy_eyeballs,
            self.protocols.clone(),
            true,
        )?;
        let next = next
            .pools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut pools = self
            .pools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Construction and resolver callbacks happen outside the pool lock. A
        // concurrent newer replacement must never be rolled back by this builder.
        if next.epoch != self.factory.client_certificate_generation()
            || next.trust_epoch != self.factory.trust_epoch()
        {
            return Err(TrustError::LocalPolicy(
                "Certificate trust changed during connection admission; retry the request.".into(),
            )
            .into());
        }
        if next.epoch == pools.epoch && next.trust_epoch == pools.trust_epoch {
            return Ok(pools.client(mode));
        }
        let old = std::mem::replace(&mut *pools, next);
        let client = pools.client(mode);
        drop(pools);
        drop(old);
        Ok(client)
    }
    /// Creates separate physical pools with the same immutable TLS and connector policy.
    ///
    /// # Errors
    /// Returns a connector-construction error without changing the original pools.
    pub fn separate_pool(&self) -> Result<Self, HyperOriginError> {
        Self::with_connection_policy(&self.factory, self.happy_eyeballs, self.protocols.clone())
    }
    /// Shares an exchange-local recorder while keeping the same physical pools.
    #[must_use]
    pub fn with_performance(mut self, performance: Option<PerformanceRecorder>) -> Self {
        self.performance = performance;
        self
    }

    async fn request(
        &self,
        mut outgoing: Request<CanonicalHttpBody>,
        mode: HyperEgressMode,
    ) -> Result<http::Response<Incoming>, HyperOriginError> {
        let client = self.authorized_client(outgoing.uri(), mode).await?;
        let mut captured = hyper_util::client::legacy::connect::capture_connection(&mut outgoing);
        let response = client.request(outgoing);
        tokio::pin!(response);
        let mut assigned = None;
        let early = tokio::select! {
            result = &mut response => Some(result),
            connected = captured.wait_for_connection_metadata() => {
                if let Some(connected) = connected.as_ref() {
                    let mut extras = http::Extensions::new();
                    connected.get_extras(&mut extras);
                    if let Some(connection) = extras.get::<ConnectionObservation>() {
                        let shared = connection.claim();
                        assigned = Some((connection.clone(), shared));
                        if let Some(performance) = &self.performance {
                            performance.transport(connection.snapshot(shared,Some(performance)));
                            performance.mark(Milestone::UpstreamConnected);
                            performance.protocol(ProtocolObservation {boundary: "upstream-request".into(), version: if connected.is_negotiated_h2() || mode == HyperEgressMode::Http2Only {"HTTP/2"} else {"HTTP/1.1"}.into(), reason: None});
                        }
                    }
                }
                None
            }
        };
        let result = match early {
            Some(result) => result,
            None => response.await,
        };
        if let Err(error) = &result
            && let Some(performance) = &self.performance
            && let Some(observation) = crate::metrics::failed_observation(error, performance)
        {
            performance.transport(observation);
        }
        let mut response = result?;
        let connection = assigned.or_else(|| {
            response
                .extensions()
                .get::<ConnectionObservation>()
                .map(|connection| (connection.clone(), connection.claim()))
        });
        if let Some(performance) = &self.performance {
            if let Some((connection, shared)) = connection {
                response.extensions_mut().insert(ResponseObservation {
                    connection: connection.clone(),
                    shared,
                    performance: performance.clone(),
                });
                // The actual response protocol also identifies the protocol used by Hyper for this exchange.
                performance.protocol(ProtocolObservation {
                    boundary: "upstream-request".into(),
                    version: if response.version() == Version::HTTP_2 {
                        "HTTP/2"
                    } else {
                        "HTTP/1.1"
                    }
                    .into(),
                    reason: None,
                });
                performance.transport(connection.snapshot(shared, Some(performance)));
            }
            performance.mark(Milestone::ResponseHeaders);
            performance.protocol(ProtocolObservation {
                boundary: "upstream-response".into(),
                version: format!("{:?}", response.version()),
                reason: response
                    .extensions()
                    .get::<hyper::ext::ReasonPhrase>()
                    .map(|reason| String::from_utf8_lossy(reason.as_bytes()).into_owned()),
            });
        }
        Ok(response)
    }

    /// Builds all Hyper pools from one immutable TLS policy generation.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] if a `BoringSSL` connector cannot be
    /// constructed from the shared context factory.
    pub fn new(factory: &UpstreamTlsContextFactory) -> Result<Self, HyperOriginError> {
        Self::with_happy_eyeballs(factory, HappyEyeballsConfig::default())
    }

    /// Builds all Hyper pools with an explicit shared Happy Eyeballs policy.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] if a `BoringSSL` connector cannot be
    /// constructed from the shared context factory.
    pub fn with_happy_eyeballs(
        factory: &UpstreamTlsContextFactory,
        happy_eyeballs: HappyEyeballsConfig,
    ) -> Result<Self, HyperOriginError> {
        Self::with_connection_policy(
            factory,
            happy_eyeballs,
            transmog_core::connection_policy::OutboundProtocolPolicy::default(),
        )
    }

    /// Creates pools sharing a live protocol policy checked only for new connections.
    ///
    /// # Errors
    /// Returns a TLS connector construction error.
    pub fn with_connection_policy(
        factory: &UpstreamTlsContextFactory,
        happy_eyeballs: HappyEyeballsConfig,
        protocols: transmog_core::connection_policy::OutboundProtocolPolicy,
    ) -> Result<Self, HyperOriginError> {
        Self::build_pools(factory, happy_eyeballs, protocols, true)
    }
    fn build_pools(
        factory: &UpstreamTlsContextFactory,
        happy_eyeballs: HappyEyeballsConfig,
        protocols: transmog_core::connection_policy::OutboundProtocolPolicy,
        reusable: bool,
    ) -> Result<Self, HyperOriginError> {
        let epoch = factory.client_certificate_generation();
        let admitted = factory.clone().for_client_certificate_epoch(epoch);
        let mut builder = Client::builder(TokioExecutor::new());
        builder
            .http2_initial_connection_window_size(chromium_http::CONNECTION_RECEIVE_WINDOW)
            .http2_initial_stream_window_size(chromium_http::STREAM_RECEIVE_WINDOW)
            .http2_header_table_size(chromium_http::HTTP2_HEADER_TABLE_SIZE)
            .http2_max_header_list_size(chromium_http::HEADER_LIST_SIZE)
            // Keep the protocol's default frame size without advertising an
            // extra SETTINGS entry that Chromium omits.
            .http2_max_frame_size(None);
        if factory
            .network_profile()
            .is_none_or(|profile| profile.settings.http2_alps)
        {
            // Wait for the ALPS bootstrap (or the real initial SETTINGS if the
            // peer declines ALPS) before encoding an application request.
            builder.http2_initial_max_send_streams(0);
        }
        if !reusable {
            builder.pool_max_idle_per_host(0);
        }
        if factory.has_client_certificate_resolver() {
            builder.retry_canceled_requests(false);
        }
        let h1 = builder.build(crate::metrics::connector(
            &admitted,
            HyperEgressMode::Http1Only,
            happy_eyeballs,
            protocols.clone(),
        )?);
        let mut h2_builder = builder.clone();
        h2_builder.http2_only(true);
        let h2 = h2_builder.build(crate::metrics::connector(
            &admitted,
            HyperEgressMode::Http2Only,
            happy_eyeballs,
            protocols.clone(),
        )?);
        let auto = builder.build(crate::metrics::connector(
            &admitted,
            HyperEgressMode::Auto,
            happy_eyeballs,
            protocols.clone(),
        )?);
        Ok(Self {
            pools: std::sync::Arc::new(std::sync::Mutex::new(GenerationPools {
                epoch,
                trust_epoch: factory.trust_epoch(),
                h1,
                h2,
                auto,
            })),
            protocols,
            factory: factory.clone(),
            happy_eyeballs,
            performance: None,
            websockets: websocket_h2::Pool::default(),
        })
    }

    /// Sends one explicitly bounded canonical exchange through Hyper.
    ///
    /// The request is already buffered by the caller under policy. Response
    /// buffering is bounded by `max_response_bytes`; trailers remain distinct
    /// frames.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid canonical data, TLS/transport
    /// failure, or a response that exceeds the configured bound.
    pub async fn execute(
        &self,
        request: CanonicalRequest,
        mode: HyperEgressMode,
        max_response_bytes: usize,
        body_idle_timeout: Duration,
    ) -> Result<CanonicalResponse, HyperOriginError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        let mut headers = prepare_headers(
            &request.head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        if destination == HttpLegVersion::Http1 {
            headers.replace_all(HeaderField::try_new(
                HOST.as_str(),
                request.head.target.authority.as_bytes(),
            )?);
        }

        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.head.target.scheme,
            request.head.target.authority,
            request.head.target.path,
            request
                .head
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.head.method.as_str())
            .uri(uri)
            .body(CanonicalHttpBody::new(request.body)?.observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let mut response = self.request(outgoing, mode).await?;
        let source_version = protocol_version(response.version())?;
        let status = response.status().as_u16();
        let headers = block_from_headers(response.headers())?;
        let body = collect_incoming(
            response.body_mut(),
            max_response_bytes,
            body_idle_timeout,
            self.performance.as_ref(),
        )
        .await?;
        if let Some(performance) = &self.performance {
            performance.mark(Milestone::UpstreamResponseDone);
        }
        Ok(CanonicalResponse {
            head: ResponseHead {
                status,
                headers,
                source_version,
            },
            body,
        })
    }

    /// Starts one backpressured canonical exchange through a pooled Hyper
    /// connection and returns as soon as the final response head is available.
    ///
    /// Request and response bodies cross bounded frame channels. The response
    /// producer stops reading Hyper when the consumer is paused, preserving
    /// transport flow control instead of accumulating an exchange in memory.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid canonical data or a failure
    /// before the final response head is received. Later body failures are
    /// carried in-band by [`StreamingResponse::body`].
    pub async fn execute_streaming(
        &self,
        request: StreamingRequest,
        mode: HyperEgressMode,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
        body_idle_timeout: Duration,
    ) -> Result<StreamingResponse, HyperOriginError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        let mut headers = prepare_headers(
            &request.head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        if destination == HttpLegVersion::Http1 {
            headers.replace_all(HeaderField::try_new(
                HOST.as_str(),
                request.head.target.authority.as_bytes(),
            )?);
        }
        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.head.target.scheme,
            request.head.target.authority,
            request.head.target.path,
            request
                .head
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.head.method.as_str())
            .uri(uri)
            .body(CanonicalHttpBody::streaming(request.body).observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let response = self.request(outgoing, mode).await?;
        let source_version = protocol_version(response.version())?;
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version,
        };
        let (sender, body) = BodyStream::channel(body_channel_capacity);
        let observation = response.extensions().get::<ResponseObservation>().cloned();
        let event_stream = transmog_core::stream::is_event_stream(&head.headers);
        tokio::spawn(stream_incoming(
            response.into_body(),
            sender,
            if event_stream {
                usize::MAX
            } else {
                max_response_bytes
            },
            body_idle_timeout,
            self.performance.clone(),
            observation,
            event_stream,
        ));
        Ok(StreamingResponse { head, body })
    }

    /// Opens a WebSocket over H1 Upgrade or H2 extended CONNECT. The request
    /// supplies the H1 handshake representation, including its client nonce;
    /// the H2 adapter removes nonce and connection-specific fields. Auto mode
    /// falls back to H1 only before sending an opening request.
    ///
    /// A rejected upgrade remains an ordinary bounded streaming response.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid data, unavailable H2 capability,
    /// or a transport/deadline failure before the response head is available.
    #[allow(clippy::too_many_lines)]
    pub async fn execute_upgrade(
        &self,
        request: RequestHead,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
        body_idle_timeout: Duration,
        mode: HyperEgressMode,
        header_timeout: Duration,
    ) -> Result<HyperUpgradeResponse, HyperOriginError> {
        if mode == HyperEgressMode::Http2Only
            || (mode == HyperEgressMode::Auto && request.target.scheme == "https")
        {
            let result = timeout(
                header_timeout,
                self.websockets.execute(
                    self,
                    &request,
                    mode,
                    max_response_bytes,
                    body_channel_capacity,
                    body_idle_timeout,
                ),
            )
            .await
            .map_err(|_| HyperOriginError::UpgradeTimeout)?;
            match result {
                Err(HyperOriginError::ExtendedConnectUnsupported)
                    if mode == HyperEgressMode::Auto => {}
                result => return result,
            }
        }
        let mut headers = prepare_headers(
            &request.headers,
            TranslationOptions {
                destination: HttpLegVersion::Http1,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        headers.replace_all(HeaderField::try_new("connection", "Upgrade")?);
        headers.replace_all(HeaderField::try_new("upgrade", "websocket")?);
        headers.replace_all(HeaderField::try_new(
            HOST.as_str(),
            request.target.authority.as_bytes(),
        )?);
        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.target.scheme,
            request.target.authority,
            request.target.path,
            request
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.method.as_str())
            .uri(uri)
            .version(Version::HTTP_11)
            .body(CanonicalHttpBody::new(Vec::new())?.observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let sent_request = RequestHead {
            headers,
            source_version: HttpLegVersion::Http1,
            ..request
        };
        let mut response = timeout(
            header_timeout,
            self.request(outgoing, HyperEgressMode::Http1Only),
        )
        .await
        .map_err(|_| HyperOriginError::UpgradeTimeout)??;
        let source_version = protocol_version(response.version())?;
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version,
        };
        if response.status() == http::StatusCode::SWITCHING_PROTOCOLS {
            return Ok(HyperUpgradeResponse::Switched {
                head,
                request: sent_request,
                upgraded: {
                    let upgrade = hyper::upgrade::on(&mut response);
                    Box::pin(async move {
                        Ok(Box::new(hyper_util::rt::TokioIo::new(upgrade.await?))
                            as Box<dyn WebSocketIo>)
                    })
                },
            });
        }
        let (sender, body) = BodyStream::channel(body_channel_capacity);
        let observation = response.extensions().get::<ResponseObservation>().cloned();
        let event_stream = transmog_core::stream::is_event_stream(&head.headers);
        tokio::spawn(stream_incoming(
            response.into_body(),
            sender,
            if event_stream {
                usize::MAX
            } else {
                max_response_bytes
            },
            body_idle_timeout,
            self.performance.clone(),
            observation,
            event_stream,
        ));
        Ok(HyperUpgradeResponse::Rejected(
            StreamingResponse { head, body },
            sent_request,
        ))
    }
}

#[derive(Debug)]
struct CanonicalHttpBody {
    performance: Option<PerformanceRecorder>,
    inner: CanonicalHttpBodyInner,
}

#[derive(Debug)]
enum CanonicalHttpBodyInner {
    Buffered(VecDeque<Frame<Bytes>>),
    Streaming(BodyStream),
}

impl CanonicalHttpBody {
    fn observed(mut self, performance: Option<PerformanceRecorder>) -> Self {
        self.performance = performance;
        self
    }
    fn new(frames: Vec<BodyFrame>) -> Result<Self, HyperOriginError> {
        let mut output = VecDeque::with_capacity(frames.len());
        for frame in frames {
            output.push_back(match frame {
                BodyFrame::Data(data) => Frame::data(data),
                BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
            });
        }
        Ok(Self {
            performance: None,
            inner: CanonicalHttpBodyInner::Buffered(output),
        })
    }

    fn streaming(body: BodyStream) -> Self {
        Self {
            performance: None,
            inner: CanonicalHttpBodyInner::Streaming(body),
        }
    }
}

impl Body for CanonicalHttpBody {
    type Data = Bytes;
    type Error = BodyStreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let result = match &mut self.inner {
            CanonicalHttpBodyInner::Buffered(frames) => Poll::Ready(frames.pop_front().map(Ok)),
            CanonicalHttpBodyInner::Streaming(body) => match body.poll_recv(context) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(
                    canonical_to_http_frame(frame)
                        .map_err(|error| BodyStreamError::Failed(error.to_string())),
                )),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        };
        if (matches!(&result, Poll::Ready(None))
            || matches!(&result, Poll::Ready(Some(Ok(_)))) && self.is_end_stream())
            && let Some(performance) = &self.performance
        {
            performance.mark(Milestone::UpstreamRequestConsumed);
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        let empty = matches!(
            &self.inner,
            CanonicalHttpBodyInner::Buffered(frames) if frames.is_empty()
        );
        if empty && let Some(performance) = &self.performance {
            performance.mark(Milestone::UpstreamRequestConsumed);
        }
        empty
    }
}

fn canonical_to_http_frame(frame: BodyFrame) -> Result<Frame<Bytes>, HyperOriginError> {
    Ok(match frame {
        BodyFrame::Data(data) => Frame::data(data),
        BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
    })
}

async fn stream_incoming(
    mut body: Incoming,
    sender: transmog_core::BodyStreamSender,
    limit: usize,
    body_idle_timeout: Duration,
    performance: Option<PerformanceRecorder>,
    _observation: Option<ResponseObservation>,
    event_stream: bool,
) {
    let mut received = 0_usize;
    loop {
        let frame = match if event_stream {
            Ok(body.frame().await)
        } else {
            timeout(body_idle_timeout, body.frame()).await
        } {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                if let Some(performance) = &performance {
                    performance.mark(Milestone::UpstreamResponseDone);
                }
                return;
            }
            Err(_) => {
                let _ = sender.send(Err(BodyStreamError::IdleTimeout)).await;
                return;
            }
        };
        let canonical = match frame {
            Ok(frame) => match frame.into_data() {
                Ok(data) => {
                    let Some(attempted) = received.checked_add(data.len()) else {
                        let _ = sender
                            .send(Err(BodyStreamError::LimitExceeded {
                                limit,
                                attempted: usize::MAX,
                            }))
                            .await;
                        return;
                    };
                    if attempted > limit {
                        let _ = sender
                            .send(Err(BodyStreamError::LimitExceeded { limit, attempted }))
                            .await;
                        return;
                    }
                    received = attempted;
                    BodyFrame::Data(data)
                }
                Err(frame) => match frame.into_trailers() {
                    Ok(trailers) => match block_from_headers(&trailers) {
                        Ok(trailers) => BodyFrame::Trailers(trailers),
                        Err(error) => {
                            let _ = sender
                                .send(Err(BodyStreamError::Failed(error.to_string())))
                                .await;
                            return;
                        }
                    },
                    Err(_) => continue,
                },
            },
            Err(error) => {
                let _ = sender
                    .send(Err(BodyStreamError::Failed(error.to_string())))
                    .await;
                return;
            }
        };
        if matches!(&canonical,BodyFrame::Data(data) if !data.is_empty())
            && let Some(performance) = &performance
        {
            performance.mark(Milestone::UpstreamResponseFirstBody);
        }
        if sender.send(Ok(canonical)).await.is_err() {
            return;
        }
    }
}

async fn collect_incoming(
    body: &mut Incoming,
    limit: usize,
    body_idle_timeout: Duration,
    performance: Option<&PerformanceRecorder>,
) -> Result<Vec<BodyFrame>, HyperOriginError> {
    let mut frames = Vec::new();
    let mut received = 0_usize;
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame?,
            Ok(None) => return Ok(frames),
            Err(_) => return Err(HyperOriginError::BodyIdleTimeout),
        };
        let frame = match frame.into_data() {
            Ok(data) => {
                if !data.is_empty()
                    && let Some(performance) = performance
                {
                    performance.mark(Milestone::UpstreamResponseFirstBody);
                }
                received = received
                    .checked_add(data.len())
                    .ok_or(HyperOriginError::ResponseBodyTooLarge { limit })?;
                if received > limit {
                    return Err(HyperOriginError::ResponseBodyTooLarge { limit });
                }
                BodyFrame::Data(data)
            }
            Err(frame) => match frame.into_trailers() {
                Ok(trailers) => BodyFrame::Trailers(block_from_headers(&trailers)?),
                Err(_) => continue,
            },
        };
        frames.push(frame);
    }
}

fn append_headers(map: &mut HeaderMap, block: &HeaderBlock) -> Result<(), HyperOriginError> {
    for field in block.iter() {
        map.append(
            HeaderName::from_bytes(field.name())?,
            HeaderValue::from_bytes(field.value())?,
        );
    }
    Ok(())
}

fn map_from_block(block: &HeaderBlock) -> Result<HeaderMap, HyperOriginError> {
    let mut map = HeaderMap::new();
    append_headers(&mut map, block)?;
    Ok(map)
}

fn block_from_headers(headers: &HeaderMap) -> Result<HeaderBlock, HyperOriginError> {
    headers
        .iter()
        .map(|(name, value)| {
            HeaderField::try_new(name.as_str(), value.as_bytes()).map_err(Into::into)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(HeaderBlock::from_fields)
}

fn protocol_version(version: Version) -> Result<HttpLegVersion, HyperOriginError> {
    match version {
        Version::HTTP_09 | Version::HTTP_10 | Version::HTTP_11 => Ok(HttpLegVersion::Http1),
        Version::HTTP_2 => Ok(HttpLegVersion::Http2),
        _ => Err(HyperOriginError::UnsupportedVersion(version)),
    }
}

/// Hyper origin-adapter failure.
#[derive(Debug, Error)]
pub enum HyperOriginError {
    /// The peer did not advertise RFC 8441 support; no CONNECT was sent.
    #[error("origin does not support HTTP/2 extended CONNECT")]
    ExtendedConnectUnsupported,
    /// WebSocket connection or response headers did not arrive in time.
    #[error("WebSocket opening handshake timed out")]
    UpgradeTimeout,
    /// Opening the measured origin transport failed.
    #[error("origin transport failed: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),
    /// HTTP/2 stream operation failed.
    #[error(transparent)]
    H2(#[from] h2::Error),
    /// Shared TLS connector configuration failed.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// Canonical header validation failed.
    #[error(transparent)]
    Header(#[from] transmog_core::HeaderError),
    /// Canonical translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] transmog_core::TranslationError),
    /// A canonical URI could not be represented for Hyper.
    #[error(transparent)]
    Uri(#[from] InvalidUri),
    /// An HTTP request could not be built.
    #[error(transparent)]
    Http(#[from] http::Error),
    /// Header name was unexpectedly invalid after canonical validation.
    #[error(transparent)]
    HeaderName(#[from] http::header::InvalidHeaderName),
    /// Header value was unexpectedly invalid after canonical validation.
    #[error(transparent)]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    /// Hyper client or connection operation failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// Reading a Hyper response body failed.
    #[error(transparent)]
    Body(#[from] hyper::Error),
    /// Origin selected an unsupported HTTP version.
    #[error("origin selected unsupported HTTP version {0:?}")]
    UnsupportedVersion(Version),
    /// Response exceeded the explicit per-exchange byte limit.
    #[error("origin response body exceeded configured {limit}-byte limit")]
    ResponseBodyTooLarge {
        /// Configured maximum response bytes.
        limit: usize,
    },
    /// The origin left its response body idle past the configured deadline.
    #[error("origin response body exceeded its idle timeout")]
    BodyIdleTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Instant, SystemTime};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use transmog_tls::{SystemTrustSource, TrustSnapshot, UpstreamTlsPolicy};
    #[tokio::test]
    async fn failed_https_records_verification_path_and_cipher_policy() {
        use boring::ssl::{SslAcceptor, SslMethod, SslVersion};
        use transmog_tls::{EndpointIdentity, PemTrustSource, ProxyCa};
        for cipher_mismatch in [false, true] {
            let ca = ProxyCa::generate("TLS failure fixture", 2).unwrap();
            let leaf = ca
                .issue(EndpointIdentity::parse("127.0.0.1").unwrap(), 1)
                .unwrap();
            let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
            acceptor.set_certificate(&leaf.certificate).unwrap();
            acceptor.set_private_key(&leaf.private_key).unwrap();
            acceptor
                .set_max_proto_version(Some(SslVersion::TLS1_2))
                .unwrap();
            if cipher_mismatch {
                acceptor
                    .set_cipher_list("ECDHE-ECDSA-AES128-GCM-SHA256")
                    .unwrap();
            }
            let acceptor = acceptor.build();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                assert!(tokio_boring::accept(&acceptor, socket).await.is_err());
            });
            let unrelated = ProxyCa::generate("Unrelated TLS root", 2).unwrap();
            let root = if cipher_mismatch {
                ca.certificate()
            } else {
                unrelated.certificate()
            };
            let roots = PemTrustSource::from_pem(&root.to_pem().unwrap(), "Recorded failure trust")
                .unwrap();
            let factory = UpstreamTlsContextFactory::new(
                Arc::new(TrustSnapshot::load(&roots, 123).unwrap()),
                UpstreamTlsPolicy {
                    maximum_version: SslVersion::TLS1_2,
                    cipher_list: "ECDHE-ECDSA-AES256-GCM-SHA384".into(),
                    ..UpstreamTlsPolicy::default()
                },
            );
            let performance = PerformanceRecorder::new(SystemTime::now(), Instant::now());
            let client = HyperOriginClient::new(&factory)
                .unwrap()
                .with_performance(Some(performance.clone()));
            let request = CanonicalRequest {
                head: RequestHead {
                    method: "GET".into(),
                    target: transmog_core::Target {
                        scheme: "https".into(),
                        authority: address.to_string(),
                        host: "127.0.0.1".into(),
                        port: address.port(),
                        path: "/failed".into(),
                        query: None,
                    },
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http1,
                },
                body: vec![],
            };
            assert!(
                client
                    .execute(
                        request,
                        HyperEgressMode::Http1Only,
                        1024,
                        Duration::from_secs(2)
                    )
                    .await
                    .is_err()
            );
            let sample = performance.snapshot();
            let transport = &sample.transports[0];
            let tls = transport.tls.as_ref().unwrap();
            assert_eq!(
                transport.outcome,
                transmog_core::performance::TransportOutcome::TlsFailed
            );
            assert_eq!(tls.trust_generation, Some(123));
            assert_eq!(tls.server_name.as_deref(), Some("127.0.0.1"));
            assert!(!tls.errors.is_empty());
            assert!(
                tls.offered_ciphers
                    .iter()
                    .any(|cipher| cipher == "ECDHE-ECDSA-AES256-GCM-SHA384")
            );
            if !cipher_mismatch {
                assert!(!tls.server_chain.is_empty());
                assert!(!tls.server_chain[0].der.is_empty());
                assert!(matches!(
                    tls.verification,
                    Some(transmog_core::VerificationResult::Rejected { .. })
                ));
            }
            origin.await.unwrap();
        }
    }
    #[tokio::test]
    async fn separate_caller_pools_never_share_a_physical_origin_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let mut number = 0_u32;
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                number += 1;
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |_| async move {
                        Ok::<_, std::convert::Infallible>(http::Response::new(
                            http_body_util::Full::new(Bytes::from(number.to_string())),
                        ))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap());
        let first = HyperOriginClient::new(&UpstreamTlsContextFactory::new(
            trust,
            UpstreamTlsPolicy::default(),
        ))
        .unwrap();
        let second = first.separate_pool().unwrap();
        let request = || CanonicalRequest {
            head: RequestHead {
                method: "GET".into(),
                target: transmog_core::Target {
                    scheme: "http".into(),
                    authority: address.to_string(),
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    path: "/".into(),
                    query: None,
                },
                headers: HeaderBlock::new(),
                source_version: HttpLegVersion::Http1,
            },
            body: vec![],
        };
        let a = first
            .execute(
                request(),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        let b = second
            .execute(
                request(),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        let reused = first
            .execute(
                request(),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_ne!(a.body, b.body);
        assert_eq!(a.body, reused.body);
        origin.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn h2_policy_change_preserves_connected_sessions_and_rejects_reconnect() {
        use transmog_tls::{
            DownstreamTlsContextFactory, DownstreamTlsPolicy, EndpointIdentity, PemTrustSource,
            ProxyCa,
        };
        let ca = ProxyCa::generate("Transmog H2 policy fixture", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("127.0.0.1").unwrap(), 1)
            .unwrap();
        let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&leaf)
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let tls = tokio_boring::accept(&acceptor, socket).await.unwrap();
            assert_eq!(tls.ssl().selected_alpn_protocol(), Some(b"h2".as_slice()));
            let service = hyper::service::service_fn(|_| async {
                Ok::<_, std::convert::Infallible>(http::Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"h2"),
                )))
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                .await;
        });
        let roots =
            PemTrustSource::from_pem(&ca.certificate().to_pem().unwrap(), "H2 policy fixture")
                .unwrap();
        let factory = UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&roots, 2).unwrap()),
            UpstreamTlsPolicy::default(),
        );
        let policy = transmog_core::connection_policy::OutboundProtocolPolicy::default();
        let performance = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = HyperOriginClient::with_connection_policy(
            &factory,
            HappyEyeballsConfig::default(),
            policy.clone(),
        )
        .unwrap()
        .with_performance(Some(performance.clone()));
        let request = || CanonicalRequest {
            head: RequestHead {
                method: "GET".into(),
                target: transmog_core::Target {
                    scheme: "https".into(),
                    authority: address.to_string(),
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    path: "/".into(),
                    query: None,
                },
                headers: HeaderBlock::new(),
                source_version: HttpLegVersion::Http1,
            },
            body: vec![],
        };
        let first = client
            .execute(
                request(),
                HyperEgressMode::Auto,
                1024,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(first.head.source_version, HttpLegVersion::Http2);
        let sample = performance.snapshot();
        let tls = sample.transports[0].tls.as_ref().unwrap();
        assert_eq!(tls.server_chain.len(), 2);
        assert_eq!(
            tls.verification,
            Some(transmog_core::VerificationResult::Verified)
        );
        assert_eq!(tls.trust_generation, Some(2));
        assert!(
            tls.server_chain
                .iter()
                .all(|certificate| !certificate.der.is_empty())
        );
        policy.set(transmog_core::connection_policy::AllowedHttpProtocols {
            http1: true,
            http2: false,
            http3: false,
        });
        let second = client
            .execute(
                request(),
                HyperEgressMode::Auto,
                1024,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(second.head.source_version, HttpLegVersion::Http2);
        // A fresh pool cannot bypass the restriction with a new H2 connection.
        assert!(
            client
                .separate_pool()
                .unwrap()
                .execute(
                    request(),
                    HyperEgressMode::Http2Only,
                    1024,
                    Duration::from_secs(1)
                )
                .await
                .is_err()
        );
        origin.abort();
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One physical connection proves reuse, timing and admission changes.
    async fn pooled_requests_share_connection_counters_and_preserve_actual_http_minor_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut head = Vec::new();
                loop {
                    head.push(stream.read_u8().await.unwrap());
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 Custom reason\r\nContent-Length: 4\r\n\r\n")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
                stream.write_all(b"data").await.unwrap();
            }
        });
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap());
        let client = HyperOriginClient::new(&UpstreamTlsContextFactory::new(
            trust,
            UpstreamTlsPolicy::default(),
        ))
        .unwrap();
        let mut samples = Vec::new();
        for attempt in 0..2 {
            let performance = PerformanceRecorder::new(SystemTime::now(), Instant::now());
            if attempt == 1 {
                client
                    .protocols
                    .set(transmog_core::connection_policy::AllowedHttpProtocols {
                        http1: false,
                        http2: false,
                        http3: false,
                    });
            }
            let response = client
                .clone()
                .with_performance(Some(performance.clone()))
                .execute(
                    CanonicalRequest {
                        head: RequestHead {
                            method: "GET".into(),
                            target: transmog_core::Target {
                                scheme: "http".into(),
                                authority: address.to_string(),
                                host: "127.0.0.1".into(),
                                port: address.port(),
                                path: "/".into(),
                                query: None,
                            },
                            headers: HeaderBlock::new(),
                            source_version: HttpLegVersion::Http1,
                        },
                        body: vec![],
                    },
                    HyperEgressMode::Http1Only,
                    1024,
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert_eq!(
                response.body,
                vec![BodyFrame::Data(Bytes::from_static(b"data"))]
            );
            let sample = performance.snapshot();
            assert!(sample.valid());
            assert!(
                sample
                    .points
                    .iter()
                    .any(|point| point.milestone == Milestone::UpstreamResponseDone)
            );
            let protocol = sample
                .protocols
                .iter()
                .find(|item| item.boundary == "upstream-response")
                .unwrap();
            assert_eq!(protocol.version, "HTTP/1.1");
            assert_eq!(protocol.reason.as_deref(), Some("Custom reason"));
            samples.push(sample);
        }
        assert_eq!(
            samples[0].transports[0].connection_id,
            samples[1].transports[0].connection_id
        );
        assert!(!samples[0].transports[0].shared);
        assert!(samples[1].transports[0].shared);
        assert_eq!(samples[1].transports[0].tcp_micros, Some(0));
        assert!(
            samples[1].transports[0]
                .setup_timings
                .iter()
                .all(|timing| timing.ended_offset_micros < 0 && timing.request_wait_micros == 0)
        );
        assert!(samples[1].transports[0].bytes_read > samples[0].transports[0].bytes_read);
        let rejected_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rejected_address = rejected_listener.local_addr().unwrap();
        let denied = CanonicalRequest {
            head: RequestHead {
                method: "GET".into(),
                target: transmog_core::Target {
                    scheme: "http".into(),
                    authority: rejected_address.to_string(),
                    host: "127.0.0.1".into(),
                    port: rejected_address.port(),
                    path: "/".into(),
                    query: None,
                },
                headers: HeaderBlock::new(),
                source_version: HttpLegVersion::Http1,
            },
            body: vec![],
        };
        assert!(
            client
                .execute(
                    denied,
                    HyperEgressMode::Http1Only,
                    1024,
                    Duration::from_secs(1)
                )
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(30), rejected_listener.accept())
                .await
                .is_err()
        );
        origin.await.unwrap();
    }
}
