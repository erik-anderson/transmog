use std::{
    collections::VecDeque,
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime},
};

use boring::ssl::NameType;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustymiddle_core::{
    AbortReason, BodyFrame, BodySemantics, BodyStream, BodyStreamError, BodyStreamSender,
    BodyTransform, BoundedBodyBuffer, BreakpointDecision, BreakpointEvent, BreakpointHandler,
    BreakpointPhase, BreakpointRunner, CanonicalRequest, CanonicalResponse, ConnectionId,
    FallbackDecision, HeaderBlock, HeaderField, HttpLegVersion, MessageKind, Replayability,
    RequestHead, ResponseHead, RoutePolicy, SessionId, SessionMetadata, StreamId, StreamingRequest,
    Target, TranslationOptions, body_semantics, prepare_headers,
};
use rustymiddle_h3::{
    AltSvcCache, H3OriginClient, H3OriginError, H3Telemetry, H3TransportLimits, Origin,
};
use rustymiddle_http::{ConnectAuthority, HyperEgressMode, HyperOriginClient, HyperOriginError};
use rustymiddle_tls::{
    DownstreamTlsContextFactory, DownstreamTlsPolicy, EndpointIdentity, LeafCache, LeafCacheError,
    ProxyCa, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    normalize_connect_identity,
};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock, Semaphore, broadcast},
    task::JoinSet,
    time::timeout,
};
use tracing::{debug, warn};

use crate::{ListenerConfigError, RuntimeLimits};

/// Complete configuration for one explicit proxy instance.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Listener exposure policy.
    pub listener: crate::ListenerConfig,
    /// Origin protocol selection policy.
    pub route_policy: RoutePolicy,
    /// Memory, concurrency, certificate, and callback bounds.
    pub limits: RuntimeLimits,
    /// HTTP/3 transport flow-control and timeout bounds.
    pub h3: H3TransportLimits,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listener: crate::ListenerConfig::default(),
            route_policy: RoutePolicy::Auto,
            limits: RuntimeLimits::default(),
            h3: H3TransportLimits::default(),
        }
    }
}

/// One attempted origin route and its outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteAttemptEvidence {
    /// Protocol adapter that was attempted.
    pub protocol: HttpLegVersion,
    /// Success or redacted failure classification.
    pub outcome: String,
}

/// Metadata-only proof emitted for a completed intercepted exchange.
#[derive(Clone, Debug)]
pub struct ExchangeEvidence {
    /// Protocol-neutral exchange identifier.
    pub session_id: SessionId,
    /// Browser-facing transport connection identifier.
    pub downstream_connection_id: ConnectionId,
    /// Browser-facing protocol stream identifier.
    pub stream_id: StreamId,
    /// Normalized origin host.
    pub target_host: String,
    /// Normalized origin scheme.
    pub target_scheme: String,
    /// Origin-form request path.
    pub target_path: String,
    /// Browser-facing HTTP protocol.
    pub ingress_version: HttpLegVersion,
    /// Origin-facing HTTP protocol.
    pub egress_version: HttpLegVersion,
    /// Whether the before-request callback completed.
    pub request_breakpoint_fired: bool,
    /// Whether the before-response callback completed.
    pub response_breakpoint_fired: bool,
    /// Ordered route attempts, including safe fallback.
    pub route_attempts: Vec<RouteAttemptEvidence>,
    /// QUIC evidence when HTTP/3 was selected.
    pub h3: Option<H3Telemetry>,
    /// Trust generation used by all upstream contexts.
    pub trust_generation: u64,
}

/// Bound explicit proxy ready to accept connections.
pub struct ProxyServer {
    listener: TcpListener,
    state: Arc<ProxyState>,
    connections: Arc<Semaphore>,
}

/// Cloneable control plane for changing generation-scoped proxy state.
#[derive(Clone)]
pub struct ProxyControl {
    state: Arc<ProxyState>,
}

impl ProxyControl {
    /// Replaces the complete upstream TLS context and connection-pool generation.
    ///
    /// Exchanges that already captured the previous generation may finish on
    /// its pools. Exchanges routed after this method returns use only the new
    /// Hyper and quiche clients.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError::TrustGenerationNotMonotonic`] unless the
    /// supplied snapshot generation is newer than the active generation, or
    /// an adapter-construction error if the new policy cannot be installed.
    pub async fn reload_trust(&self, trust: Arc<TrustSnapshot>) -> Result<(), ProxyRuntimeError> {
        let next = Arc::new(UpstreamGeneration::new(trust, self.state.config.h3)?);
        let mut active = self.state.upstream.write().await;
        if next.trust_generation <= active.trust_generation {
            return Err(ProxyRuntimeError::TrustGenerationNotMonotonic {
                active: active.trust_generation,
                requested: next.trust_generation,
            });
        }
        *active = next;
        Ok(())
    }

    /// Returns the generation used by newly routed upstream exchanges.
    pub async fn trust_generation(&self) -> u64 {
        self.state.upstream.read().await.trust_generation
    }
}

impl ProxyServer {
    /// Binds a loopback-by-default explicit proxy.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError`] for unsafe configuration, listener bind
    /// failure, invalid leaf-cache policy, or upstream connector construction
    /// failure.
    pub async fn bind(
        config: ProxyConfig,
        ca: ProxyCa,
        trust: Arc<TrustSnapshot>,
        handler: Arc<dyn BreakpointHandler>,
    ) -> Result<Self, ProxyRuntimeError> {
        config.listener.validate()?;
        if config.limits.max_connections == 0
            || config.limits.max_request_body_bytes == 0
            || config.limits.max_response_body_bytes == 0
            || config.limits.body_channel_capacity == 0
            || config.limits.max_h2_streams == 0
            || config.limits.max_header_count == 0
            || config.limits.max_header_bytes < 8 * 1024
            || u32::try_from(config.limits.max_header_bytes).is_err()
            || config.limits.header_read_timeout.is_zero()
            || config.limits.body_idle_timeout.is_zero()
            || config.limits.tls_handshake_timeout.is_zero()
            || config.limits.shutdown_timeout.is_zero()
        {
            return Err(ProxyRuntimeError::InvalidConfiguration);
        }
        let upstream = Arc::new(UpstreamGeneration::new(trust, config.h3)?);
        let leaves = LeafCache::new(
            ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )?;
        let listener = TcpListener::bind(config.listener.listen_addr).await?;
        let (evidence, _) = broadcast::channel(1_024);
        let state = Arc::new(ProxyState {
            runner: BreakpointRunner::new(handler, config.limits.breakpoints),
            downstream_tls: DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default()),
            leaves: Mutex::new(leaves),
            upstream: RwLock::new(upstream),
            alt_svc: Mutex::new(AltSvcCache::new(1_024)),
            config: config.clone(),
            next_connection: AtomicU64::new(1),
            next_session: AtomicU64::new(1),
            next_stream: AtomicU64::new(1),
            evidence,
        });
        Ok(Self {
            listener,
            state,
            connections: Arc::new(Semaphore::new(config.limits.max_connections)),
        })
    }

    /// Actual socket address, including an assigned ephemeral port.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener address cannot be queried.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    /// Subscribes to metadata-only completed-exchange evidence.
    pub fn subscribe_evidence(&self) -> broadcast::Receiver<ExchangeEvidence> {
        self.state.evidence.subscribe()
    }

    /// Creates a handle that remains usable after [`ProxyServer::serve`] takes ownership.
    pub fn control(&self) -> ProxyControl {
        ProxyControl {
            state: Arc::clone(&self.state),
        }
    }

    /// Accepts connections until `shutdown` resolves, then drains active tasks.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError`] if accepting a connection fails.
    pub async fn serve<F>(self, shutdown: F) -> Result<(), ProxyRuntimeError>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = self.listener.accept() => {
                    let (stream, client_addr) = accepted?;
                    let Ok(permit) = Arc::clone(&self.connections).try_acquire_owned() else {
                        warn!(%client_addr, "downstream connection limit reached");
                        continue;
                    };
                    let proxy_addr = stream.local_addr()?;
                    let state = Arc::clone(&self.state);
                    let connection_id = ConnectionId(u128::from(
                        state.next_connection.fetch_add(1, Ordering::Relaxed)
                    ));
                    tasks.spawn(async move {
                        let _permit = permit;
                        if let Err(error) = serve_explicit_connection(
                            state,
                            stream,
                            ConnectionContext {
                                client_addr,
                                proxy_addr,
                                connection_id,
                                tunnel: None,
                            },
                        ).await {
                            debug!(%client_addr, %error, "downstream connection ended");
                        }
                    });
                }
            }
        }
        while timeout(self.state.config.limits.shutdown_timeout, tasks.join_next())
            .await
            .ok()
            .flatten()
            .is_some()
        {}
        Ok(())
    }
}

struct ProxyState {
    runner: BreakpointRunner<dyn BreakpointHandler>,
    downstream_tls: DownstreamTlsContextFactory,
    leaves: Mutex<LeafCache>,
    upstream: RwLock<Arc<UpstreamGeneration>>,
    alt_svc: Mutex<AltSvcCache>,
    config: ProxyConfig,
    next_connection: AtomicU64,
    next_session: AtomicU64,
    next_stream: AtomicU64,
    evidence: broadcast::Sender<ExchangeEvidence>,
}

struct UpstreamGeneration {
    hyper: HyperOriginClient,
    h3: H3OriginClient,
    trust_generation: u64,
}

impl UpstreamGeneration {
    fn new(
        trust: Arc<TrustSnapshot>,
        h3_limits: H3TransportLimits,
    ) -> Result<Self, ProxyRuntimeError> {
        h3_limits.validate().map_err(H3OriginError::from)?;
        let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let hyper = HyperOriginClient::new(&tls)?;
        let h3 = H3OriginClient::new(tls.clone(), h3_limits);
        Ok(Self {
            hyper,
            h3,
            trust_generation: tls.snapshot().generation(),
        })
    }
}

#[derive(Clone)]
struct ConnectionContext {
    client_addr: SocketAddr,
    proxy_addr: SocketAddr,
    connection_id: ConnectionId,
    tunnel: Option<ConnectAuthority>,
}

async fn serve_explicit_connection(
    state: Arc<ProxyState>,
    stream: TcpStream,
    context: ConnectionContext,
) -> Result<(), ProxyRuntimeError> {
    let header_read_timeout = state.config.limits.header_read_timeout;
    let max_header_count = state.config.limits.max_header_count;
    let max_header_bytes = state.config.limits.max_header_bytes;
    let service = service_fn(move |request| {
        let state = Arc::clone(&state);
        let context = context.clone();
        async move {
            match state.handle_outer_request(request, context).await {
                Ok(response) => Ok::<_, Infallible>(response),
                Err(error) => {
                    warn!(%error, "explicit proxy request failed");
                    Ok(failed_exchange_response(&error))
                }
            }
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout)
        .max_headers(max_header_count)
        .max_buf_size(max_header_bytes);
    builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await?;
    Ok(())
}

impl ProxyState {
    async fn handle_outer_request(
        self: Arc<Self>,
        mut request: Request<Incoming>,
        context: ConnectionContext,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if request.method() != Method::CONNECT {
            return self.handle_intercepted_request(request, context).await;
        }
        let authority_text = request
            .uri()
            .authority()
            .map_or_else(|| request.uri().path(), http::uri::Authority::as_str);
        let authority = ConnectAuthority::from_str(authority_text)?;
        let upgraded = hyper::upgrade::on(&mut request);
        let state = Arc::clone(&self);
        let tunnel_context = ConnectionContext {
            tunnel: Some(authority.clone()),
            ..context
        };
        tokio::spawn(async move {
            match upgraded.await {
                Ok(upgraded) => {
                    if let Err(error) = state
                        .serve_intercepted_tunnel(upgraded, tunnel_context, authority)
                        .await
                    {
                        debug!(%error, "intercepted CONNECT tunnel ended");
                    }
                }
                Err(error) => debug!(%error, "CONNECT upgrade failed"),
            }
        });
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(DownstreamBody::empty())?)
    }

    async fn serve_intercepted_tunnel(
        self: Arc<Self>,
        upgraded: hyper::upgrade::Upgraded,
        context: ConnectionContext,
        authority: ConnectAuthority,
    ) -> Result<(), ProxyRuntimeError> {
        let identity = EndpointIdentity::parse(authority.host())?;
        let leaf = self
            .leaves
            .lock()
            .await
            .get_or_issue(identity, SystemTime::now())?;
        let acceptor = self.downstream_tls.acceptor(&leaf)?;
        let tls = timeout(
            self.config.limits.tls_handshake_timeout,
            tokio_boring::accept(&acceptor, TokioIo::new(upgraded)),
        )
        .await
        .map_err(|_| ProxyRuntimeError::DownstreamTlsTimeout)?
        .map_err(|error| ProxyRuntimeError::DownstreamTlsHandshake(error.to_string()))?;
        normalize_connect_identity(authority.host(), tls.ssl().servername(NameType::HOST_NAME))?;
        let negotiated_h2 = tls.ssl().selected_alpn_protocol() == Some(b"h2");
        let max_h2_streams = self.config.limits.max_h2_streams;
        let max_header_bytes = self.config.limits.max_header_bytes;
        let max_header_list_size =
            u32::try_from(max_header_bytes).map_err(|_| ProxyRuntimeError::InvalidConfiguration)?;
        let max_header_count = self.config.limits.max_header_count;
        let header_read_timeout = self.config.limits.header_read_timeout;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        let service = service_fn(move |request| {
            let state = Arc::clone(&self);
            let context = context.clone();
            async move {
                match state.handle_intercepted_request(request, context).await {
                    Ok(response) => Ok::<_, Infallible>(response),
                    Err(error) => {
                        warn!(%error, "intercepted request failed");
                        Ok(failed_exchange_response(&error))
                    }
                }
            }
        });
        if negotiated_h2 {
            let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
            builder
                .timer(TokioTimer::new())
                .max_concurrent_streams(max_h2_streams)
                .max_header_list_size(max_header_list_size)
                .keep_alive_interval(Some(body_idle_timeout))
                .keep_alive_timeout(body_idle_timeout);
            builder.serve_connection(TokioIo::new(tls), service).await?;
        } else {
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout)
                .max_headers(max_header_count)
                .max_buf_size(max_header_bytes);
            builder
                .serve_connection(TokioIo::new(tls), service)
                .with_upgrades()
                .await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_intercepted_request(
        &self,
        mut request: Request<Incoming>,
        context: ConnectionContext,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if request.method() == Method::CONNECT {
            return Err(ProxyRuntimeError::NestedConnectUnsupported);
        }
        let ingress_version = protocol_version(request.version())?;
        let mut request_head = canonical_head(&request, &context, ingress_version)?;
        let mut session = SessionMetadata {
            session_id: SessionId(u128::from(
                self.next_session.fetch_add(1, Ordering::Relaxed),
            )),
            downstream_connection_id: context.connection_id,
            stream_id: StreamId(u128::from(self.next_stream.fetch_add(1, Ordering::Relaxed))),
            client_addr: context.client_addr,
            proxy_addr: context.proxy_addr,
            ingress_version,
            egress_version: None,
        };

        let request_head_decision = self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::BeforeRequestHeaders,
                request_head: Some(request_head.clone()),
                response_head: None,
                body_frame: None,
            })
            .await;
        let request_head_decision = match request_head_decision {
            Ok(decision) => decision,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        match request_head_decision {
            BreakpointDecision::Continue => {}
            BreakpointDecision::ReplaceRequestHead(head) => request_head = head,
            BreakpointDecision::RespondLocally(response) => {
                return self
                    .finish_local_response(response, &session, &request_head)
                    .await;
            }
            BreakpointDecision::Abort(reason) => {
                self.failed(&session, &request_head, None).await;
                return Err(reason.into());
            }
            _ => {
                self.failed(&session, &request_head, None).await;
                return Err(ProxyRuntimeError::InvalidBreakpointDecision);
            }
        }

        if let Err(error) = validate_declared_body_limit(
            &request_head.headers,
            self.config.limits.max_request_body_bytes,
        ) {
            self.failed(&session, &request_head, None).await;
            return Err(error);
        }

        if self.config.route_policy == RoutePolicy::Http3Only {
            let upstream = self.upstream.read().await.clone();
            return self
                .handle_streaming_h3(request, session, request_head, ingress_version, upstream)
                .await;
        }

        if let Some(mode) = self.streaming_hyper_mode(&request_head).await {
            let upstream = self.upstream.read().await.clone();
            return self
                .handle_streaming_hyper(
                    request,
                    session,
                    request_head,
                    ingress_version,
                    mode,
                    upstream,
                )
                .await;
        }

        let raw_body = collect_incoming(
            request.body_mut(),
            self.config.limits.max_request_body_bytes,
            self.config.limits.body_idle_timeout,
        )
        .await;
        let raw_body = match raw_body {
            Ok(body) => body,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error);
            }
        };
        let request_body_outcome = self
            .process_body(
                &session,
                BreakpointPhase::RequestBody,
                Some(&request_head),
                None,
                raw_body,
                self.config.limits.max_request_body_bytes,
            )
            .await;
        let request_body_outcome = match request_body_outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error);
            }
        };
        let (request_body, request_body_modified) = match request_body_outcome {
            BodyOutcome::Body { frames, modified } => (frames, modified),
            BodyOutcome::Local(response) => {
                return self
                    .finish_local_response(response, &session, &request_head)
                    .await;
            }
        };

        let route = self.config.route_policy;
        let intended = match route {
            RoutePolicy::Http1Only => HttpLegVersion::Http1,
            RoutePolicy::Http2Only | RoutePolicy::Auto => HttpLegVersion::Http2,
            RoutePolicy::Http3Only => HttpLegVersion::Http3,
        };
        let prepared_request_headers = prepare_headers(
            &request_head.headers,
            TranslationOptions {
                destination: intended,
                kind: MessageKind::Request,
                body_modified: request_body_modified,
                force_identity_encoding: false,
            },
        );
        request_head.headers = match prepared_request_headers {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        let request = CanonicalRequest {
            head: request_head.clone(),
            body: request_body,
        };
        let upstream = self.upstream.read().await.clone();
        let routed = match self.route(request, &upstream).await {
            Ok(routed) => routed,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error);
            }
        };
        session.egress_version = Some(routed.protocol);

        let mut response = routed.response;
        let response_head_decision = self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::BeforeResponseHeaders,
                request_head: Some(request_head.clone()),
                response_head: Some(response.head.clone()),
                body_frame: None,
            })
            .await;
        let response_head_decision = match response_head_decision {
            Ok(decision) => decision,
            Err(error) => {
                self.failed(&session, &request_head, Some(&response.head))
                    .await;
                return Err(error.into());
            }
        };
        match response_head_decision {
            BreakpointDecision::Continue => {}
            BreakpointDecision::ReplaceResponseHead(head) => response.head = head,
            BreakpointDecision::RespondLocally(local) => response = local,
            BreakpointDecision::Abort(reason) => {
                self.failed(&session, &request_head, Some(&response.head))
                    .await;
                return Err(reason.into());
            }
            _ => {
                self.failed(&session, &request_head, Some(&response.head))
                    .await;
                return Err(ProxyRuntimeError::InvalidBreakpointDecision);
            }
        }
        let outcome = if body_semantics(&request_head.method, response.head.status)
            == BodySemantics::Forbidden
        {
            BodyOutcome::Body {
                modified: true,
                frames: Vec::new(),
            }
        } else {
            match self
                .process_body(
                    &session,
                    BreakpointPhase::ResponseBody,
                    Some(&request_head),
                    Some(&response.head),
                    response.body,
                    self.config.limits.max_response_body_bytes,
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.failed(&session, &request_head, Some(&response.head))
                        .await;
                    return Err(error);
                }
            }
        };
        let (body, body_modified) = match outcome {
            BodyOutcome::Body { frames, modified } => (frames, modified),
            BodyOutcome::Local(local) => {
                response = local;
                (response.body.clone(), true)
            }
        };
        response.body = body;
        let prepared_response_headers = prepare_headers(
            &response.head.headers,
            TranslationOptions {
                destination: ingress_version,
                kind: MessageKind::Response,
                body_modified,
                force_identity_encoding: false,
            },
        );
        response.head.headers = match prepared_response_headers {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, Some(&response.head))
                    .await;
                return Err(error.into());
            }
        };

        if let Err(error) = self
            .complete(&session, &request_head, Some(&response.head))
            .await
        {
            self.failed(&session, &request_head, Some(&response.head))
                .await;
            return Err(error);
        }
        let _ = self.evidence.send(ExchangeEvidence {
            session_id: session.session_id,
            downstream_connection_id: session.downstream_connection_id,
            stream_id: session.stream_id,
            target_host: request_head.target.host.clone(),
            target_scheme: request_head.target.scheme.clone(),
            target_path: request_head.target.path.clone(),
            ingress_version,
            egress_version: routed.protocol,
            request_breakpoint_fired: true,
            response_breakpoint_fired: true,
            route_attempts: routed.attempts,
            h3: routed.h3,
            trust_generation: upstream.trust_generation,
        });
        response_to_hyper(response)
    }

    async fn streaming_hyper_mode(&self, request: &RequestHead) -> Option<HyperEgressMode> {
        match self.config.route_policy {
            RoutePolicy::Http1Only => Some(HyperEgressMode::Http1Only),
            RoutePolicy::Http2Only => Some(HyperEgressMode::Http2Only),
            RoutePolicy::Http3Only => None,
            RoutePolicy::Auto => {
                let origin = Origin {
                    host: request.target.host.clone(),
                    port: request.target.port,
                };
                let has_h3_alternative = self
                    .alt_svc
                    .lock()
                    .await
                    .get(&origin, Instant::now())
                    .is_some();
                (!has_h3_alternative).then_some(HyperEgressMode::Auto)
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_streaming_hyper(
        &self,
        request: Request<Incoming>,
        mut session: SessionMetadata,
        mut request_head: RequestHead,
        ingress_version: HttpLegVersion,
        mode: HyperEgressMode,
        upstream_generation: Arc<UpstreamGeneration>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        request_head.headers = match prepare_headers(
            &request_head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                // A body callback can change length after the upstream head is
                // sent, so streaming requests deliberately use synthesized
                // destination framing.
                body_modified: true,
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request_runner = self.runner.clone();
        let request_session = session.clone();
        let request_head_for_body = request_head.clone();
        let request_limit = self.config.limits.max_request_body_bytes;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        tokio::spawn(async move {
            stream_incoming_through_breakpoint(
                request.into_body(),
                request_sender,
                request_runner,
                request_session,
                request_head_for_body,
                request_limit,
                body_idle_timeout,
            )
            .await;
        });

        let origin = Origin {
            host: request_head.target.host.clone(),
            port: request_head.target.port,
        };
        let upstream = upstream_generation
            .hyper
            .execute_streaming(
                StreamingRequest {
                    head: request_head.clone(),
                    body: request_body,
                },
                mode,
                self.config.limits.max_response_body_bytes,
                capacity,
                self.config.limits.body_idle_timeout,
            )
            .await;
        let mut upstream = match upstream {
            Ok(response) => response,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        session.egress_version = Some(upstream.head.source_version);
        self.learn_alt_svc_head(&origin, &upstream.head).await;

        let response_head_decision = self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::BeforeResponseHeaders,
                request_head: Some(request_head.clone()),
                response_head: Some(upstream.head.clone()),
                body_frame: None,
            })
            .await;
        let response_head_decision = match response_head_decision {
            Ok(decision) => decision,
            Err(error) => {
                self.failed(&session, &request_head, Some(&upstream.head))
                    .await;
                return Err(error.into());
            }
        };
        match response_head_decision {
            BreakpointDecision::Continue => {}
            BreakpointDecision::ReplaceResponseHead(head) => upstream.head = head,
            BreakpointDecision::RespondLocally(response) => {
                return self
                    .finish_local_response(response, &session, &request_head)
                    .await;
            }
            BreakpointDecision::Abort(reason) => {
                self.failed(&session, &request_head, Some(&upstream.head))
                    .await;
                return Err(reason.into());
            }
            _ => {
                self.failed(&session, &request_head, Some(&upstream.head))
                    .await;
                return Err(ProxyRuntimeError::InvalidBreakpointDecision);
            }
        }
        upstream.head.headers = match prepare_headers(
            &upstream.head.headers,
            TranslationOptions {
                destination: ingress_version,
                kind: MessageKind::Response,
                // The streaming callback may alter bytes after the response
                // head is released, so stale length and digest fields cannot
                // cross the boundary.
                body_modified: true,
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, Some(&upstream.head))
                    .await;
                return Err(error.into());
            }
        };

        let response_head = upstream.head;
        let attempts = vec![RouteAttemptEvidence {
            protocol: session.egress_version.expect("egress was assigned"),
            outcome: "success".to_owned(),
        }];
        let trust_generation = upstream_generation.trust_generation;
        if body_semantics(&request_head.method, response_head.status) == BodySemantics::Forbidden {
            drop(upstream.body);
            return self
                .finish_streaming_bodyless(
                    response_head,
                    &session,
                    &request_head,
                    attempts,
                    None,
                    trust_generation,
                )
                .await;
        }
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let response_runner = self.runner.clone();
        let evidence = self.evidence.clone();
        let response_session = session.clone();
        let response_request_head = request_head.clone();
        let response_head_for_body = response_head.clone();
        let response_limit = self.config.limits.max_response_body_bytes;
        tokio::spawn(async move {
            stream_response_through_breakpoint(
                upstream.body,
                downstream_sender,
                response_runner,
                response_session,
                response_request_head,
                response_head_for_body,
                response_limit,
                body_idle_timeout,
                evidence,
                attempts,
                None,
                trust_generation,
            )
            .await;
        });

        response_stream_to_hyper(&response_head, downstream_body)
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_streaming_h3(
        &self,
        request: Request<Incoming>,
        mut session: SessionMetadata,
        mut request_head: RequestHead,
        ingress_version: HttpLegVersion,
        upstream_generation: Arc<UpstreamGeneration>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if request_head.target.scheme != "https" {
            self.failed(&session, &request_head, None).await;
            return Err(ProxyRuntimeError::Http3RequiresHttps);
        }
        request_head.headers = match prepare_headers(
            &request_head.headers,
            TranslationOptions {
                destination: HttpLegVersion::Http3,
                kind: MessageKind::Request,
                body_modified: true,
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request_runner = self.runner.clone();
        let request_session = session.clone();
        let request_head_for_body = request_head.clone();
        let request_limit = self.config.limits.max_request_body_bytes;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        tokio::spawn(async move {
            stream_incoming_through_breakpoint(
                request.into_body(),
                request_sender,
                request_runner,
                request_session,
                request_head_for_body,
                request_limit,
                body_idle_timeout,
            )
            .await;
        });

        let origin = Origin {
            host: request_head.target.host.clone(),
            port: request_head.target.port,
        };
        let upstream = upstream_generation
            .h3
            .execute_duplex_streaming(
                StreamingRequest {
                    head: request_head.clone(),
                    body: request_body,
                },
                request_head.target.port,
                self.config.limits.max_response_body_bytes,
                capacity,
            )
            .await;
        let mut upstream = match upstream {
            Ok(response) => response,
            Err(error) => {
                self.failed(&session, &request_head, None).await;
                return Err(error.into());
            }
        };
        session.egress_version = Some(HttpLegVersion::Http3);
        self.learn_alt_svc_head(&origin, &upstream.response.head)
            .await;

        let response_head_decision = self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::BeforeResponseHeaders,
                request_head: Some(request_head.clone()),
                response_head: Some(upstream.response.head.clone()),
                body_frame: None,
            })
            .await;
        let response_head_decision = match response_head_decision {
            Ok(decision) => decision,
            Err(error) => {
                self.failed(&session, &request_head, Some(&upstream.response.head))
                    .await;
                return Err(error.into());
            }
        };
        match response_head_decision {
            BreakpointDecision::Continue => {}
            BreakpointDecision::ReplaceResponseHead(head) => upstream.response.head = head,
            BreakpointDecision::RespondLocally(response) => {
                return self
                    .finish_local_response(response, &session, &request_head)
                    .await;
            }
            BreakpointDecision::Abort(reason) => {
                self.failed(&session, &request_head, Some(&upstream.response.head))
                    .await;
                return Err(reason.into());
            }
            _ => {
                self.failed(&session, &request_head, Some(&upstream.response.head))
                    .await;
                return Err(ProxyRuntimeError::InvalidBreakpointDecision);
            }
        }
        upstream.response.head.headers = match prepare_headers(
            &upstream.response.head.headers,
            TranslationOptions {
                destination: ingress_version,
                kind: MessageKind::Response,
                body_modified: true,
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(&session, &request_head, Some(&upstream.response.head))
                    .await;
                return Err(error.into());
            }
        };

        let response_head = upstream.response.head;
        let h3 = upstream.telemetry;
        let attempts = vec![RouteAttemptEvidence {
            protocol: HttpLegVersion::Http3,
            outcome: "success".to_owned(),
        }];
        let trust_generation = upstream_generation.trust_generation;
        if body_semantics(&request_head.method, response_head.status) == BodySemantics::Forbidden {
            drop(upstream.response.body);
            return self
                .finish_streaming_bodyless(
                    response_head,
                    &session,
                    &request_head,
                    attempts,
                    Some(h3),
                    trust_generation,
                )
                .await;
        }
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let response_runner = self.runner.clone();
        let evidence = self.evidence.clone();
        let response_session = session.clone();
        let response_request_head = request_head;
        let response_head_for_body = response_head.clone();
        let response_limit = self.config.limits.max_response_body_bytes;
        tokio::spawn(async move {
            stream_response_through_breakpoint(
                upstream.response.body,
                downstream_sender,
                response_runner,
                response_session,
                response_request_head,
                response_head_for_body,
                response_limit,
                body_idle_timeout,
                evidence,
                attempts,
                Some(h3),
                trust_generation,
            )
            .await;
        });

        response_stream_to_hyper(&response_head, downstream_body)
    }

    async fn finish_local_response(
        &self,
        mut response: CanonicalResponse,
        session: &SessionMetadata,
        request: &RequestHead,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if body_semantics(&request.method, response.head.status) == BodySemantics::Forbidden {
            response.body.clear();
        }
        let prepared_headers = prepare_headers(
            &response.head.headers,
            TranslationOptions {
                destination: session.ingress_version,
                kind: MessageKind::Response,
                body_modified: true,
                force_identity_encoding: false,
            },
        );
        response.head.headers = match prepared_headers {
            Ok(headers) => headers,
            Err(error) => {
                self.failed(session, request, Some(&response.head)).await;
                return Err(error.into());
            }
        };
        if let Err(error) = self.complete(session, request, Some(&response.head)).await {
            self.failed(session, request, Some(&response.head)).await;
            return Err(error);
        }
        response_to_hyper(response)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_streaming_bodyless(
        &self,
        response_head: ResponseHead,
        session: &SessionMetadata,
        request_head: &RequestHead,
        route_attempts: Vec<RouteAttemptEvidence>,
        h3: Option<H3Telemetry>,
        trust_generation: u64,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if let Err(error) = self
            .complete(session, request_head, Some(&response_head))
            .await
        {
            self.failed(session, request_head, Some(&response_head))
                .await;
            return Err(error);
        }
        let _ = self.evidence.send(ExchangeEvidence {
            session_id: session.session_id,
            downstream_connection_id: session.downstream_connection_id,
            stream_id: session.stream_id,
            target_host: request_head.target.host.clone(),
            target_scheme: request_head.target.scheme.clone(),
            target_path: request_head.target.path.clone(),
            ingress_version: session.ingress_version,
            egress_version: session.egress_version.expect("egress was assigned"),
            request_breakpoint_fired: true,
            response_breakpoint_fired: true,
            route_attempts,
            h3,
            trust_generation,
        });
        response_to_hyper(CanonicalResponse {
            head: response_head,
            body: Vec::new(),
        })
    }

    async fn complete(
        &self,
        session: &SessionMetadata,
        request: &RequestHead,
        response: Option<&ResponseHead>,
    ) -> Result<(), ProxyRuntimeError> {
        match self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::Completed,
                request_head: Some(request.clone()),
                response_head: response.cloned(),
                body_frame: None,
            })
            .await?
        {
            BreakpointDecision::Continue => Ok(()),
            _ => Err(ProxyRuntimeError::InvalidBreakpointDecision),
        }
    }

    async fn failed(
        &self,
        session: &SessionMetadata,
        request: &RequestHead,
        response: Option<&ResponseHead>,
    ) {
        let _ = self
            .runner
            .run(BreakpointEvent {
                session: session.clone(),
                phase: BreakpointPhase::Failed,
                request_head: Some(request.clone()),
                response_head: response.cloned(),
                body_frame: None,
            })
            .await;
    }

    async fn process_body(
        &self,
        session: &SessionMetadata,
        phase: BreakpointPhase,
        request_head: Option<&RequestHead>,
        response_head: Option<&ResponseHead>,
        frames: Vec<BodyFrame>,
        limit: usize,
    ) -> Result<BodyOutcome, ProxyRuntimeError> {
        let mut output = Vec::new();
        let mut input = frames.into_iter();
        while let Some(frame) = input.next() {
            let decision = self
                .runner
                .run(BreakpointEvent {
                    session: session.clone(),
                    phase,
                    request_head: request_head.cloned(),
                    response_head: response_head.cloned(),
                    body_frame: Some(frame.clone()),
                })
                .await?;
            match decision {
                BreakpointDecision::Continue => output.push(frame),
                BreakpointDecision::ReplaceBody { data, trailers } => {
                    let mut replacement = vec![BodyFrame::Data(data)];
                    if let Some(trailers) = trailers {
                        replacement.push(BodyFrame::Trailers(trailers));
                    }
                    validate_frames(&replacement, limit)?;
                    return Ok(BodyOutcome::Body {
                        frames: replacement,
                        modified: true,
                    });
                }
                BreakpointDecision::TransformBodyStream(mut transform) => {
                    output.extend(transform.transform(frame)?);
                    for frame in input {
                        output.extend(transform.transform(frame)?);
                    }
                    output.extend(transform.finish()?);
                    validate_frames(&output, limit)?;
                    return Ok(BodyOutcome::Body {
                        frames: output,
                        modified: true,
                    });
                }
                BreakpointDecision::RespondLocally(response) => {
                    return Ok(BodyOutcome::Local(response));
                }
                BreakpointDecision::Abort(reason) => return Err(reason.into()),
                _ => return Err(ProxyRuntimeError::InvalidBreakpointDecision),
            }
        }
        validate_frames(&output, limit)?;
        Ok(BodyOutcome::Body {
            frames: output,
            modified: false,
        })
    }

    async fn route(
        &self,
        request: CanonicalRequest,
        upstream: &UpstreamGeneration,
    ) -> Result<RoutedResponse, ProxyRuntimeError> {
        match self.config.route_policy {
            RoutePolicy::Http1Only => {
                self.route_hyper(request, HyperEgressMode::Http1Only, upstream)
                    .await
            }
            RoutePolicy::Http2Only => {
                self.route_hyper(request, HyperEgressMode::Http2Only, upstream)
                    .await
            }
            RoutePolicy::Http3Only => self.route_h3(request, None, upstream).await,
            RoutePolicy::Auto => self.route_auto(request, upstream).await,
        }
    }

    async fn route_hyper(
        &self,
        request: CanonicalRequest,
        mode: HyperEgressMode,
        upstream: &UpstreamGeneration,
    ) -> Result<RoutedResponse, ProxyRuntimeError> {
        let origin = Origin {
            host: request.head.target.host.clone(),
            port: request.head.target.port,
        };
        let response = upstream
            .hyper
            .execute(
                request,
                mode,
                self.config.limits.max_response_body_bytes,
                self.config.limits.body_idle_timeout,
            )
            .await?;
        let protocol = response.head.source_version;
        self.learn_alt_svc(&origin, &response).await;
        Ok(RoutedResponse {
            response,
            protocol,
            attempts: vec![RouteAttemptEvidence {
                protocol,
                outcome: "success".to_owned(),
            }],
            h3: None,
        })
    }

    async fn route_h3(
        &self,
        request: CanonicalRequest,
        port: Option<u16>,
        upstream: &UpstreamGeneration,
    ) -> Result<RoutedResponse, ProxyRuntimeError> {
        if request.head.target.scheme != "https" {
            return Err(ProxyRuntimeError::Http3RequiresHttps);
        }
        let peer_port = port.unwrap_or(request.head.target.port);
        let origin = Origin {
            host: request.head.target.host.clone(),
            port: request.head.target.port,
        };
        let response = upstream
            .h3
            .execute(
                request,
                peer_port,
                self.config.limits.max_response_body_bytes,
            )
            .await?;
        self.learn_alt_svc(&origin, &response.response).await;
        Ok(RoutedResponse {
            response: response.response,
            protocol: HttpLegVersion::Http3,
            attempts: vec![RouteAttemptEvidence {
                protocol: HttpLegVersion::Http3,
                outcome: "success".to_owned(),
            }],
            h3: Some(response.telemetry),
        })
    }

    async fn route_auto(
        &self,
        request: CanonicalRequest,
        upstream: &UpstreamGeneration,
    ) -> Result<RoutedResponse, ProxyRuntimeError> {
        let origin = Origin {
            host: request.head.target.host.clone(),
            port: request.head.target.port,
        };
        let alternative = self
            .alt_svc
            .lock()
            .await
            .get(&origin, Instant::now())
            .cloned();
        let Some(alternative) = alternative else {
            return self
                .route_hyper(request, HyperEgressMode::Auto, upstream)
                .await;
        };

        match self
            .route_h3(request.clone(), Some(alternative.port), upstream)
            .await
        {
            Ok(response) => Ok(response),
            Err(h3_error) => {
                self.alt_svc
                    .lock()
                    .await
                    .mark_broken(&origin, Instant::now() + Duration::from_secs(30));
                let replayability = if request.head.method.eq_ignore_ascii_case("GET")
                    || request.head.method.eq_ignore_ascii_case("HEAD")
                {
                    Replayability::SafeMethod
                } else {
                    Replayability::NotReplayable
                };
                if self.config.route_policy.fallback_after(
                    HttpLegVersion::Http3,
                    replayability,
                    false,
                ) != FallbackDecision::Retry(HttpLegVersion::Http2)
                {
                    return Err(h3_error);
                }
                let mut response = self
                    .route_hyper(request, HyperEgressMode::Auto, upstream)
                    .await?;
                response.attempts.insert(
                    0,
                    RouteAttemptEvidence {
                        protocol: HttpLegVersion::Http3,
                        outcome: "failed-before-response".to_owned(),
                    },
                );
                Ok(response)
            }
        }
    }

    async fn learn_alt_svc(&self, origin: &Origin, response: &CanonicalResponse) {
        self.learn_alt_svc_head(origin, &response.head).await;
    }

    async fn learn_alt_svc_head(&self, origin: &Origin, response: &ResponseHead) {
        for value in response.headers.values("alt-svc") {
            let Ok(value) = std::str::from_utf8(value) else {
                continue;
            };
            if let Err(error) =
                self.alt_svc
                    .lock()
                    .await
                    .observe(origin.clone(), value, Instant::now())
            {
                debug!(%error, "ignored unsafe or unsupported Alt-Svc advertisement");
            }
        }
    }
}

struct RoutedResponse {
    response: CanonicalResponse,
    protocol: HttpLegVersion,
    attempts: Vec<RouteAttemptEvidence>,
    h3: Option<H3Telemetry>,
}

enum BodyOutcome {
    Body {
        frames: Vec<BodyFrame>,
        modified: bool,
    },
    Local(CanonicalResponse),
}

fn validate_frames(frames: &[BodyFrame], limit: usize) -> Result<(), ProxyRuntimeError> {
    let mut buffer = BoundedBodyBuffer::new(limit);
    for frame in frames {
        buffer.push(frame.clone())?;
    }
    Ok(())
}

fn validate_declared_body_limit(
    headers: &HeaderBlock,
    limit: usize,
) -> Result<(), ProxyRuntimeError> {
    let Some(value) = headers.values("content-length").next() else {
        return Ok(());
    };
    let Ok(value) = std::str::from_utf8(value) else {
        return Ok(());
    };
    let Ok(declared) = value.trim().parse::<u64>() else {
        return Ok(());
    };
    if declared > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(rustymiddle_core::BodyLimitError::LimitExceeded {
            limit,
            attempted: usize::try_from(declared).unwrap_or(usize::MAX),
        }
        .into());
    }
    Ok(())
}

async fn collect_incoming(
    body: &mut Incoming,
    limit: usize,
    body_idle_timeout: Duration,
) -> Result<Vec<BodyFrame>, ProxyRuntimeError> {
    let mut frames = Vec::new();
    let mut buffer = BoundedBodyBuffer::new(limit);
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame?,
            Ok(None) => return Ok(frames),
            Err(_) => return Err(ProxyRuntimeError::BodyIdleTimeout),
        };
        let canonical = match frame.into_data() {
            Ok(data) => BodyFrame::Data(data),
            Err(frame) => match frame.into_trailers() {
                Ok(trailers) => BodyFrame::Trailers(block_from_headers(&trailers)?),
                Err(_) => continue,
            },
        };
        buffer.push(canonical.clone())?;
        frames.push(canonical);
    }
}

#[allow(clippy::too_many_lines)]
async fn stream_incoming_through_breakpoint(
    mut body: Incoming,
    sender: BodyStreamSender,
    runner: BreakpointRunner<dyn BreakpointHandler>,
    session: SessionMetadata,
    request_head: RequestHead,
    limit: usize,
    body_idle_timeout: Duration,
) {
    let mut input = StreamingBodyTracker::new(limit);
    let mut output = StreamingBodyTracker::new(limit);
    let mut transform: Option<Box<dyn BodyTransform>> = None;
    let mut discard_remaining = false;

    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                fail_streaming_body(
                    &sender,
                    &runner,
                    &session,
                    &request_head,
                    None,
                    BodyStreamError::IdleTimeout,
                )
                .await;
                return;
            }
        };
        let canonical = match incoming_frame(frame) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                fail_streaming_body(&sender, &runner, &session, &request_head, None, error).await;
                return;
            }
        };
        if let Err(error) = input.accept(&canonical) {
            fail_streaming_body(&sender, &runner, &session, &request_head, None, error).await;
            return;
        }
        if discard_remaining {
            continue;
        }

        let frames = if let Some(active) = transform.as_mut() {
            match active.transform(canonical) {
                Ok(frames) => frames,
                Err(error) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        None,
                        BodyStreamError::Failed(format!("body transform aborted: {error:?}")),
                    )
                    .await;
                    return;
                }
            }
        } else {
            let decision = runner
                .run(BreakpointEvent {
                    session: session.clone(),
                    phase: BreakpointPhase::RequestBody,
                    request_head: Some(request_head.clone()),
                    response_head: None,
                    body_frame: Some(canonical.clone()),
                })
                .await;
            match decision {
                Ok(BreakpointDecision::Continue) => vec![canonical],
                Ok(BreakpointDecision::ReplaceBody { data, trailers }) => {
                    discard_remaining = true;
                    replacement_frames(data, trailers)
                }
                Ok(BreakpointDecision::TransformBodyStream(mut body_transform)) => {
                    let frames = match body_transform.transform(canonical) {
                        Ok(frames) => frames,
                        Err(error) => {
                            fail_streaming_body(
                                &sender,
                                &runner,
                                &session,
                                &request_head,
                                None,
                                BodyStreamError::Failed(format!(
                                    "body transform aborted: {error:?}"
                                )),
                            )
                            .await;
                            return;
                        }
                    };
                    transform = Some(body_transform);
                    frames
                }
                Ok(BreakpointDecision::Abort(reason)) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        None,
                        BodyStreamError::Failed(format!("breakpoint aborted: {reason:?}")),
                    )
                    .await;
                    return;
                }
                Ok(_) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        None,
                        BodyStreamError::Failed(
                            "invalid request-body breakpoint decision".to_owned(),
                        ),
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        None,
                        BodyStreamError::Failed(error.to_string()),
                    )
                    .await;
                    return;
                }
            }
        };
        if send_streaming_frames(&sender, &mut output, frames)
            .await
            .is_err()
        {
            emit_failed(&runner, &session, &request_head, None).await;
            return;
        }
    }

    if let Some(mut transform) = transform {
        match transform.finish() {
            Ok(frames) => {
                if send_streaming_frames(&sender, &mut output, frames)
                    .await
                    .is_err()
                {
                    emit_failed(&runner, &session, &request_head, None).await;
                }
            }
            Err(error) => {
                fail_streaming_body(
                    &sender,
                    &runner,
                    &session,
                    &request_head,
                    None,
                    BodyStreamError::Failed(format!("body transform aborted: {error:?}")),
                )
                .await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn stream_response_through_breakpoint(
    mut body: BodyStream,
    sender: BodyStreamSender,
    runner: BreakpointRunner<dyn BreakpointHandler>,
    session: SessionMetadata,
    request_head: RequestHead,
    response_head: ResponseHead,
    limit: usize,
    body_idle_timeout: Duration,
    evidence: broadcast::Sender<ExchangeEvidence>,
    attempts: Vec<RouteAttemptEvidence>,
    h3: Option<H3Telemetry>,
    trust_generation: u64,
) {
    let mut input = StreamingBodyTracker::new(limit);
    let mut output = StreamingBodyTracker::new(limit);
    let mut transform: Option<Box<dyn BodyTransform>> = None;

    loop {
        let frame = match timeout(body_idle_timeout, body.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                fail_streaming_body(
                    &sender,
                    &runner,
                    &session,
                    &request_head,
                    Some(&response_head),
                    BodyStreamError::IdleTimeout,
                )
                .await;
                return;
            }
        };
        let canonical = match frame {
            Ok(frame) => frame,
            Err(error) => {
                fail_streaming_body(
                    &sender,
                    &runner,
                    &session,
                    &request_head,
                    Some(&response_head),
                    error,
                )
                .await;
                return;
            }
        };
        if let Err(error) = input.accept(&canonical) {
            fail_streaming_body(
                &sender,
                &runner,
                &session,
                &request_head,
                Some(&response_head),
                error,
            )
            .await;
            return;
        }

        let frames = if let Some(active) = transform.as_mut() {
            match active.transform(canonical) {
                Ok(frames) => frames,
                Err(error) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        Some(&response_head),
                        BodyStreamError::Failed(format!("body transform aborted: {error:?}")),
                    )
                    .await;
                    return;
                }
            }
        } else {
            let decision = runner
                .run(BreakpointEvent {
                    session: session.clone(),
                    phase: BreakpointPhase::ResponseBody,
                    request_head: Some(request_head.clone()),
                    response_head: Some(response_head.clone()),
                    body_frame: Some(canonical.clone()),
                })
                .await;
            match decision {
                Ok(BreakpointDecision::Continue) => vec![canonical],
                Ok(BreakpointDecision::ReplaceBody { data, trailers }) => {
                    let frames = replacement_frames(data, trailers);
                    if send_streaming_frames(&sender, &mut output, frames)
                        .await
                        .is_err()
                    {
                        emit_failed(&runner, &session, &request_head, Some(&response_head)).await;
                        return;
                    }
                    return complete_streaming_exchange(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        &response_head,
                        evidence,
                        attempts,
                        h3,
                        trust_generation,
                    )
                    .await;
                }
                Ok(BreakpointDecision::TransformBodyStream(mut body_transform)) => {
                    let frames = match body_transform.transform(canonical) {
                        Ok(frames) => frames,
                        Err(error) => {
                            fail_streaming_body(
                                &sender,
                                &runner,
                                &session,
                                &request_head,
                                Some(&response_head),
                                BodyStreamError::Failed(format!(
                                    "body transform aborted: {error:?}"
                                )),
                            )
                            .await;
                            return;
                        }
                    };
                    transform = Some(body_transform);
                    frames
                }
                Ok(BreakpointDecision::Abort(reason)) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        Some(&response_head),
                        BodyStreamError::Failed(format!("breakpoint aborted: {reason:?}")),
                    )
                    .await;
                    return;
                }
                Ok(_) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        Some(&response_head),
                        BodyStreamError::Failed(
                            "invalid response-body breakpoint decision".to_owned(),
                        ),
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    fail_streaming_body(
                        &sender,
                        &runner,
                        &session,
                        &request_head,
                        Some(&response_head),
                        BodyStreamError::Failed(error.to_string()),
                    )
                    .await;
                    return;
                }
            }
        };
        if send_streaming_frames(&sender, &mut output, frames)
            .await
            .is_err()
        {
            emit_failed(&runner, &session, &request_head, Some(&response_head)).await;
            return;
        }
    }

    if let Some(mut transform) = transform {
        match transform.finish() {
            Ok(frames) => {
                if send_streaming_frames(&sender, &mut output, frames)
                    .await
                    .is_err()
                {
                    emit_failed(&runner, &session, &request_head, Some(&response_head)).await;
                    return;
                }
            }
            Err(error) => {
                fail_streaming_body(
                    &sender,
                    &runner,
                    &session,
                    &request_head,
                    Some(&response_head),
                    BodyStreamError::Failed(format!("body transform aborted: {error:?}")),
                )
                .await;
                return;
            }
        }
    }
    complete_streaming_exchange(
        &sender,
        &runner,
        &session,
        &request_head,
        &response_head,
        evidence,
        attempts,
        h3,
        trust_generation,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn complete_streaming_exchange(
    sender: &BodyStreamSender,
    runner: &BreakpointRunner<dyn BreakpointHandler>,
    session: &SessionMetadata,
    request_head: &RequestHead,
    response_head: &ResponseHead,
    evidence: broadcast::Sender<ExchangeEvidence>,
    attempts: Vec<RouteAttemptEvidence>,
    h3: Option<H3Telemetry>,
    trust_generation: u64,
) {
    let completed = runner
        .run(BreakpointEvent {
            session: session.clone(),
            phase: BreakpointPhase::Completed,
            request_head: Some(request_head.clone()),
            response_head: Some(response_head.clone()),
            body_frame: None,
        })
        .await;
    match completed {
        Ok(BreakpointDecision::Continue) => {}
        Ok(_) => {
            let _ = sender
                .send(Err(BodyStreamError::Failed(
                    "invalid completed breakpoint decision".to_owned(),
                )))
                .await;
            emit_failed(runner, session, request_head, Some(response_head)).await;
            return;
        }
        Err(error) => {
            let _ = sender
                .send(Err(BodyStreamError::Failed(error.to_string())))
                .await;
            emit_failed(runner, session, request_head, Some(response_head)).await;
            return;
        }
    }
    let _ = evidence.send(ExchangeEvidence {
        session_id: session.session_id,
        downstream_connection_id: session.downstream_connection_id,
        stream_id: session.stream_id,
        target_host: request_head.target.host.clone(),
        target_scheme: request_head.target.scheme.clone(),
        target_path: request_head.target.path.clone(),
        ingress_version: session.ingress_version,
        egress_version: session.egress_version.expect("egress was assigned"),
        request_breakpoint_fired: true,
        response_breakpoint_fired: true,
        route_attempts: attempts,
        h3,
        trust_generation,
    });
}

async fn fail_streaming_body(
    sender: &BodyStreamSender,
    runner: &BreakpointRunner<dyn BreakpointHandler>,
    session: &SessionMetadata,
    request_head: &RequestHead,
    response_head: Option<&ResponseHead>,
    error: BodyStreamError,
) {
    let _ = sender.send(Err(error)).await;
    emit_failed(runner, session, request_head, response_head).await;
}

async fn emit_failed(
    runner: &BreakpointRunner<dyn BreakpointHandler>,
    session: &SessionMetadata,
    request_head: &RequestHead,
    response_head: Option<&ResponseHead>,
) {
    let _ = runner
        .run(BreakpointEvent {
            session: session.clone(),
            phase: BreakpointPhase::Failed,
            request_head: Some(request_head.clone()),
            response_head: response_head.cloned(),
            body_frame: None,
        })
        .await;
}

async fn send_streaming_frames(
    sender: &BodyStreamSender,
    tracker: &mut StreamingBodyTracker,
    frames: Vec<BodyFrame>,
) -> Result<(), BodyStreamError> {
    for frame in frames {
        if let Err(error) = tracker.accept(&frame) {
            let _ = sender.send(Err(error.clone())).await;
            return Err(error);
        }
        sender
            .send(Ok(frame))
            .await
            .map_err(|error| BodyStreamError::Failed(error.to_string()))?;
    }
    Ok(())
}

fn replacement_frames(data: Bytes, trailers: Option<HeaderBlock>) -> Vec<BodyFrame> {
    let mut frames = vec![BodyFrame::Data(data)];
    if let Some(trailers) = trailers {
        frames.push(BodyFrame::Trailers(trailers));
    }
    frames
}

fn incoming_frame(
    frame: Result<Frame<Bytes>, hyper::Error>,
) -> Result<Option<BodyFrame>, BodyStreamError> {
    let frame = frame.map_err(|error| BodyStreamError::Failed(error.to_string()))?;
    Ok(match frame.into_data() {
        Ok(data) => Some(BodyFrame::Data(data)),
        Err(frame) => match frame.into_trailers() {
            Ok(trailers) => Some(BodyFrame::Trailers(
                block_from_headers(&trailers)
                    .map_err(|error| BodyStreamError::Failed(error.to_string()))?,
            )),
            Err(_) => None,
        },
    })
}

struct StreamingBodyTracker {
    limit: usize,
    bytes: usize,
    trailers_seen: bool,
}

impl StreamingBodyTracker {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: 0,
            trailers_seen: false,
        }
    }

    fn accept(&mut self, frame: &BodyFrame) -> Result<(), BodyStreamError> {
        match frame {
            BodyFrame::Data(data) => {
                if self.trailers_seen {
                    return Err(BodyStreamError::DataAfterTrailers);
                }
                let attempted =
                    self.bytes
                        .checked_add(data.len())
                        .ok_or(BodyStreamError::LimitExceeded {
                            limit: self.limit,
                            attempted: usize::MAX,
                        })?;
                if attempted > self.limit {
                    return Err(BodyStreamError::LimitExceeded {
                        limit: self.limit,
                        attempted,
                    });
                }
                self.bytes = attempted;
            }
            BodyFrame::Trailers(_) => {
                if self.trailers_seen {
                    return Err(BodyStreamError::DuplicateTrailers);
                }
                self.trailers_seen = true;
            }
        }
        Ok(())
    }
}

fn canonical_head(
    request: &Request<Incoming>,
    context: &ConnectionContext,
    source_version: HttpLegVersion,
) -> Result<RequestHead, ProxyRuntimeError> {
    let target = if let Some(tunnel) = &context.tunnel {
        validate_tunnel_target(request, tunnel)?;
        target_from_tunnel(request, tunnel)
    } else {
        target_from_absolute_uri(request)?
    };
    Ok(RequestHead {
        method: request.method().as_str().to_owned(),
        target,
        headers: block_from_headers(request.headers())?,
        source_version,
    })
}

fn target_from_tunnel(request: &Request<Incoming>, tunnel: &ConnectAuthority) -> Target {
    let path_and_query = request.uri().path_and_query();
    Target {
        scheme: "https".to_owned(),
        authority: tunnel.to_string(),
        host: tunnel.host().to_owned(),
        port: tunnel.port(),
        path: path_and_query
            .map_or("/", http::uri::PathAndQuery::path)
            .to_owned(),
        query: path_and_query
            .and_then(http::uri::PathAndQuery::query)
            .map(str::to_owned),
    }
}

fn target_from_absolute_uri(request: &Request<Incoming>) -> Result<Target, ProxyRuntimeError> {
    let scheme = request
        .uri()
        .scheme_str()
        .ok_or(ProxyRuntimeError::AbsoluteFormRequired)?;
    if scheme != "http" {
        return Err(ProxyRuntimeError::ConnectRequiredForHttps);
    }
    let authority = request
        .uri()
        .authority()
        .ok_or(ProxyRuntimeError::AbsoluteFormRequired)?;
    let identity = EndpointIdentity::parse(authority.host())?;
    let port = authority.port_u16().unwrap_or(80);
    let normalized_authority = format_authority(&identity.as_text(), port, 80);
    let path_and_query = request.uri().path_and_query();
    Ok(Target {
        scheme: scheme.to_owned(),
        authority: normalized_authority,
        host: identity.as_text(),
        port,
        path: path_and_query
            .map_or("/", http::uri::PathAndQuery::path)
            .to_owned(),
        query: path_and_query
            .and_then(http::uri::PathAndQuery::query)
            .map(str::to_owned),
    })
}

fn validate_tunnel_target(
    request: &Request<Incoming>,
    tunnel: &ConnectAuthority,
) -> Result<(), ProxyRuntimeError> {
    let selected = if let Some(authority) = request.uri().authority() {
        Some((
            authority.host().to_owned(),
            authority.port_u16().unwrap_or(443),
        ))
    } else if let Some(host) = request.headers().get(http::header::HOST) {
        let host = host
            .to_str()
            .map_err(|_| ProxyRuntimeError::TunnelAuthorityMismatch)?;
        let authority = http::uri::Authority::from_str(host)
            .map_err(|_| ProxyRuntimeError::TunnelAuthorityMismatch)?;
        Some((
            authority.host().to_owned(),
            authority.port_u16().unwrap_or(443),
        ))
    } else {
        None
    };
    let Some((host, port)) = selected else {
        return Err(ProxyRuntimeError::TunnelAuthorityMismatch);
    };
    let selected = EndpointIdentity::parse(&host)?;
    let connected = EndpointIdentity::parse(tunnel.host())?;
    if selected != connected || port != tunnel.port() {
        return Err(ProxyRuntimeError::TunnelAuthorityMismatch);
    }
    Ok(())
}

fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let bracketed = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    if port == default_port {
        bracketed
    } else {
        format!("{bracketed}:{port}")
    }
}

fn response_to_hyper(
    response: CanonicalResponse,
) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
    let mut outgoing = Response::builder()
        .status(response.head.status)
        .body(DownstreamBody::new(response.body)?)?;
    append_headers(outgoing.headers_mut(), &response.head.headers)?;
    Ok(outgoing)
}

fn response_stream_to_hyper(
    head: &ResponseHead,
    body: BodyStream,
) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
    let mut outgoing = Response::builder()
        .status(head.status)
        .body(DownstreamBody::streaming(body))?;
    append_headers(outgoing.headers_mut(), &head.headers)?;
    Ok(outgoing)
}

fn failed_exchange_response(error: &ProxyRuntimeError) -> Response<DownstreamBody> {
    let status = match error {
        ProxyRuntimeError::Aborted(AbortReason::Rejected) => StatusCode::FORBIDDEN,
        ProxyRuntimeError::BodyLimit(_) => StatusCode::PAYLOAD_TOO_LARGE,
        ProxyRuntimeError::AbsoluteFormRequired
        | ProxyRuntimeError::ConnectRequiredForHttps
        | ProxyRuntimeError::TunnelAuthorityMismatch
        | ProxyRuntimeError::NestedConnectUnsupported
        | ProxyRuntimeError::ConnectAuthority(_)
        | ProxyRuntimeError::Identity(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    let mut response = Response::new(DownstreamBody::from_bytes(Bytes::from_static(
        b"rustymiddle exchange failed\n",
    )));
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

fn append_headers(map: &mut HeaderMap, block: &HeaderBlock) -> Result<(), ProxyRuntimeError> {
    for field in block.iter() {
        map.append(
            HeaderName::from_bytes(field.name())?,
            HeaderValue::from_bytes(field.value())?,
        );
    }
    Ok(())
}

fn block_from_headers(headers: &HeaderMap) -> Result<HeaderBlock, ProxyRuntimeError> {
    headers
        .iter()
        .map(|(name, value)| {
            HeaderField::try_new(name.as_str(), value.as_bytes()).map_err(Into::into)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(HeaderBlock::from_fields)
}

fn protocol_version(version: Version) -> Result<HttpLegVersion, ProxyRuntimeError> {
    match version {
        Version::HTTP_09 | Version::HTTP_10 | Version::HTTP_11 => Ok(HttpLegVersion::Http1),
        Version::HTTP_2 => Ok(HttpLegVersion::Http2),
        _ => Err(ProxyRuntimeError::UnsupportedIngressVersion(version)),
    }
}

struct DownstreamBody {
    inner: DownstreamBodyInner,
}

enum DownstreamBodyInner {
    Buffered(VecDeque<Frame<Bytes>>),
    Streaming(BodyStream),
}

impl DownstreamBody {
    fn empty() -> Self {
        Self {
            inner: DownstreamBodyInner::Buffered(VecDeque::new()),
        }
    }

    fn from_bytes(bytes: Bytes) -> Self {
        Self {
            inner: DownstreamBodyInner::Buffered(VecDeque::from([Frame::data(bytes)])),
        }
    }

    fn new(frames: Vec<BodyFrame>) -> Result<Self, ProxyRuntimeError> {
        let mut output = VecDeque::with_capacity(frames.len());
        for frame in frames {
            output.push_back(match frame {
                BodyFrame::Data(data) => Frame::data(data),
                BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
            });
        }
        Ok(Self {
            inner: DownstreamBodyInner::Buffered(output),
        })
    }

    fn streaming(body: BodyStream) -> Self {
        Self {
            inner: DownstreamBodyInner::Streaming(body),
        }
    }
}

impl Body for DownstreamBody {
    type Data = Bytes;
    type Error = BodyStreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut self.inner {
            DownstreamBodyInner::Buffered(frames) => Poll::Ready(frames.pop_front().map(Ok)),
            DownstreamBodyInner::Streaming(body) => match body.poll_recv(context) {
                Poll::Ready(Some(Ok(BodyFrame::Data(data)))) => {
                    Poll::Ready(Some(Ok(Frame::data(data))))
                }
                Poll::Ready(Some(Ok(BodyFrame::Trailers(trailers)))) => Poll::Ready(Some(
                    map_from_block(&trailers)
                        .map(Frame::trailers)
                        .map_err(|error| BodyStreamError::Failed(error.to_string())),
                )),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(
            &self.inner,
            DownstreamBodyInner::Buffered(frames) if frames.is_empty()
        )
    }
}

fn map_from_block(block: &HeaderBlock) -> Result<HeaderMap, ProxyRuntimeError> {
    let mut map = HeaderMap::new();
    append_headers(&mut map, block)?;
    Ok(map)
}

/// Listener, interception, breakpoint, or origin-adapter failure.
#[derive(Debug, Error)]
pub enum ProxyRuntimeError {
    /// Listener exposure configuration was unsafe.
    #[error(transparent)]
    Listener(#[from] ListenerConfigError),
    /// Runtime bounds were zero or otherwise invalid.
    #[error("invalid proxy runtime configuration")]
    InvalidConfiguration,
    /// A trust reload did not advance the immutable generation number.
    #[error("trust snapshot generation must increase (active {active}, requested {requested})")]
    TrustGenerationNotMonotonic {
        /// Generation currently serving new upstream exchanges.
        active: u64,
        /// Generation supplied by the rejected reload.
        requested: u64,
    },
    /// TCP, UDP, or socket operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Hyper server connection failed.
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    /// Building an HTTP message failed.
    #[error(transparent)]
    Http(#[from] http::Error),
    /// Canonical header validation failed.
    #[error(transparent)]
    Header(#[from] rustymiddle_core::HeaderError),
    /// Header-name conversion failed.
    #[error(transparent)]
    HeaderName(#[from] http::header::InvalidHeaderName),
    /// Header-value conversion failed.
    #[error(transparent)]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    /// Header translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] rustymiddle_core::TranslationError),
    /// Body exceeded its explicit bound or had illegal trailer ordering.
    #[error(transparent)]
    BodyLimit(#[from] rustymiddle_core::BodyLimitError),
    /// Breakpoint callback timed out or the runner shut down.
    #[error(transparent)]
    Breakpoint(#[from] rustymiddle_core::BreakpointRunnerError),
    /// Breakpoint handler selected an action that is invalid for the phase.
    #[error("breakpoint decision is invalid for the current phase")]
    InvalidBreakpointDecision,
    /// Breakpoint handler intentionally aborted the exchange.
    #[error("breakpoint aborted exchange: {0:?}")]
    Aborted(AbortReason),
    /// HTTP/1.1 or HTTP/2 origin adapter failed.
    #[error(transparent)]
    HyperOrigin(#[from] HyperOriginError),
    /// HTTP/3 origin adapter failed.
    #[error(transparent)]
    H3Origin(#[from] H3OriginError),
    /// CONNECT authority was invalid.
    #[error(transparent)]
    ConnectAuthority(#[from] rustymiddle_http::AuthorityError),
    /// CONNECT/SNI or canonical identity was invalid.
    #[error(transparent)]
    Identity(#[from] rustymiddle_tls::IdentityError),
    /// Leaf cache configuration or issuance failed.
    #[error(transparent)]
    Leaf(#[from] LeafCacheError),
    /// Browser-facing TLS context construction failed.
    #[error(transparent)]
    DownstreamTls(#[from] rustymiddle_tls::DownstreamTlsError),
    /// Browser did not finish TLS within the configured deadline.
    #[error("downstream TLS handshake timed out")]
    DownstreamTlsTimeout,
    /// Browser-facing TLS handshake failed.
    #[error("downstream TLS handshake failed: {0}")]
    DownstreamTlsHandshake(String),
    /// A request or response body made no progress before its idle deadline.
    #[error("HTTP body exceeded its configured idle timeout")]
    BodyIdleTimeout,
    /// Plain explicit-proxy requests must use absolute-form URIs.
    #[error("ordinary explicit-proxy requests require an absolute-form URI")]
    AbsoluteFormRequired,
    /// HTTPS must arrive through CONNECT interception.
    #[error("HTTPS absolute-form requests require CONNECT interception")]
    ConnectRequiredForHttps,
    /// Inner request authority did not match CONNECT.
    #[error("inner request authority does not match CONNECT target")]
    TunnelAuthorityMismatch,
    /// Nested CONNECT/general tunnel forwarding is outside initial scope.
    #[error("nested CONNECT is not supported")]
    NestedConnectUnsupported,
    /// HTTP/3 is only supported for HTTPS origins.
    #[error("HTTP/3 origin routing requires HTTPS")]
    Http3RequiresHttps,
    /// Downstream selected an unsupported HTTP version.
    #[error("unsupported ingress HTTP version {0:?}")]
    UnsupportedIngressVersion(Version),
}

impl From<AbortReason> for ProxyRuntimeError {
    fn from(reason: AbortReason) -> Self {
        Self::Aborted(reason)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use boring::{
        rand::rand_bytes,
        ssl::{SslContext, SslMethod},
    };
    use quiche::h3::NameValue;
    use rustymiddle_core::{BoxBreakpointFuture, BreakpointHandler};
    use rustymiddle_tls::{
        DownstreamTlsContextFactory, DownstreamTlsPolicy, LoadedTrust, ProxyCa, SystemTrustSource,
        TrustError, TrustSnapshot, TrustSource, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UdpSocket},
        sync::{Notify, oneshot},
        task::JoinHandle,
    };

    use super::*;

    struct EditingHandler;

    struct LocalResponseHandler;

    #[derive(Default)]
    struct RejectingLifecycleHandler {
        phases: StdMutex<Vec<BreakpointPhase>>,
    }

    #[derive(Default)]
    struct PausingHandler {
        slow_entered: Notify,
        fast_entered: Notify,
        release_slow: Notify,
    }

    struct StaticTrust(Vec<Vec<u8>>);

    impl TrustSource for StaticTrust {
        fn load(&self) -> Result<LoadedTrust, TrustError> {
            Ok(LoadedTrust {
                certificates_der: self.0.clone(),
                source_description: "local integration root".to_owned(),
                source_version: Some("test".to_owned()),
                diagnostics: Vec::new(),
            })
        }
    }

    impl BreakpointHandler for EditingHandler {
        fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(async move {
                match event.phase {
                    BreakpointPhase::BeforeRequestHeaders => {
                        let mut head = event.request_head.unwrap();
                        head.headers
                            .push(HeaderField::try_new("x-from-breakpoint", "yes").unwrap());
                        BreakpointDecision::ReplaceRequestHead(head)
                    }
                    BreakpointPhase::BeforeResponseHeaders => {
                        let mut head = event.response_head.unwrap();
                        head.headers
                            .push(HeaderField::try_new("x-intercepted", "yes").unwrap());
                        BreakpointDecision::ReplaceResponseHead(head)
                    }
                    BreakpointPhase::RequestBody => BreakpointDecision::ReplaceBody {
                        data: Bytes::from_static(b"request-edited"),
                        trailers: None,
                    },
                    BreakpointPhase::ResponseBody => BreakpointDecision::ReplaceBody {
                        data: Bytes::from_static(b"edited"),
                        trailers: None,
                    },
                    _ => BreakpointDecision::Continue,
                }
            })
        }
    }

    impl BreakpointHandler for RejectingLifecycleHandler {
        fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(async move {
                self.phases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(event.phase);
                if event.phase == BreakpointPhase::BeforeRequestHeaders {
                    BreakpointDecision::Abort(AbortReason::Rejected)
                } else {
                    BreakpointDecision::Continue
                }
            })
        }
    }

    impl BreakpointHandler for LocalResponseHandler {
        fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(async move {
                if event.phase == BreakpointPhase::BeforeRequestHeaders {
                    BreakpointDecision::RespondLocally(CanonicalResponse::local(
                        202,
                        HeaderBlock::from_fields(vec![
                            HeaderField::try_new("x-local-response", "yes")
                                .expect("static test header is valid"),
                        ]),
                        Bytes::from_static(b"synthetic"),
                    ))
                } else {
                    BreakpointDecision::Continue
                }
            })
        }
    }

    impl BreakpointHandler for PausingHandler {
        fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(async move {
                if event.phase == BreakpointPhase::BeforeRequestHeaders {
                    match event
                        .request_head
                        .as_ref()
                        .map(|head| head.target.path.as_str())
                    {
                        Some("/slow") => {
                            self.slow_entered.notify_one();
                            self.release_slow.notified().await;
                        }
                        Some("/fast") => self.fast_entered.notify_one(),
                        _ => {}
                    }
                }
                BreakpointDecision::Continue
            })
        }
    }

    #[tokio::test]
    async fn trust_reload_swaps_complete_pool_generation_and_rejects_reuse() {
        let initial = Arc::new(TrustSnapshot::load(&SystemTrustSource, 50).unwrap());
        let proxy = ProxyServer::bind(
            ProxyConfig::default(),
            ProxyCa::generate("rustymiddle trust reload test", 2).unwrap(),
            initial,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let retained = proxy.state.upstream.read().await.clone();
        let control = proxy.control();
        assert_eq!(control.trust_generation().await, 50);

        let reused = Arc::new(TrustSnapshot::load(&SystemTrustSource, 50).unwrap());
        assert!(matches!(
            control.reload_trust(reused).await,
            Err(ProxyRuntimeError::TrustGenerationNotMonotonic {
                active: 50,
                requested: 50
            })
        ));

        let replacement = Arc::new(TrustSnapshot::load(&SystemTrustSource, 51).unwrap());
        control.reload_trust(replacement).await.unwrap();
        let active = proxy.state.upstream.read().await.clone();
        assert_eq!(active.trust_generation, 51);
        assert_eq!(retained.trust_generation, 50);
        assert!(!Arc::ptr_eq(&retained, &active));
    }

    #[tokio::test]
    async fn partial_http1_headers_are_closed_after_the_configured_deadline() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 52).unwrap());
        let proxy = ProxyServer::bind(
            ProxyConfig {
                limits: RuntimeLimits {
                    header_read_timeout: Duration::from_millis(50),
                    shutdown_timeout: Duration::from_secs(1),
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ProxyCa::generate("rustymiddle slow header test", 2).unwrap(),
            trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(b"GET http://127.0.0.1:9/ HTTP/1.1\r\nHost:")
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("partial header connection survived its deadline")
            .unwrap();

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn ordinary_http_flows_through_breakpoints_and_h1_origin() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(text.contains("x-from-breakpoint: yes"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });

        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 11).unwrap());
        let ca = ProxyCa::generate("rustymiddle runtime test", 2).unwrap();
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            ..ProxyConfig::default()
        };
        let proxy = ProxyServer::bind(config, ca, trust, Arc::new(EditingHandler))
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://{origin_addr}/proof HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response).to_ascii_lowercase();
        assert!(response.contains("x-intercepted: yes"));
        assert!(response.contains("\r\nedited\r\n"));

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http1);
        assert_eq!(proof.egress_version, HttpLegVersion::Http1);
        assert!(proof.request_breakpoint_fired);
        assert!(proof.response_breakpoint_fired);

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn response_body_streams_before_the_origin_finishes() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let first_chunk_sent = Arc::new(Notify::new());
        let release_final_chunk = Arc::new(Notify::new());
        let origin_first_chunk_sent = first_chunk_sent.clone();
        let origin_release_final_chunk = release_final_chunk.clone();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let _ = read_http_head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n")
                .await
                .unwrap();
            origin_first_chunk_sent.notify_one();
            origin_release_final_chunk.notified().await;
            stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n").await.unwrap();
        });

        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 16).unwrap());
        let ca = ProxyCa::generate("rustymiddle response streaming test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    body_channel_capacity: 1,
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://{origin_addr}/stream HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(1), first_chunk_sent.notified())
            .await
            .expect("origin never produced its first response chunk");
        let mut response = Vec::new();
        let mut chunk = [0_u8; 128];
        timeout(Duration::from_millis(500), async {
            while !response.windows(5).any(|window| window == b"first") {
                let read = client.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0, "proxy closed before streaming the first chunk");
                response.extend_from_slice(&chunk[..read]);
            }
        })
        .await
        .expect("proxy buffered the response until origin completion");
        assert!(matches!(
            evidence.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        release_final_chunk.notify_one();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.windows(6).any(|window| window == b"second"));
        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.egress_version, HttpLegVersion::Http1);

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn request_body_streams_before_the_client_finishes() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let first_chunk_seen = Arc::new(Notify::new());
        let origin_first_chunk_seen = first_chunk_seen.clone();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let service = service_fn(move |mut request: Request<Incoming>| {
                let first_chunk_seen = origin_first_chunk_seen.clone();
                async move {
                    let first = request
                        .body_mut()
                        .frame()
                        .await
                        .unwrap()
                        .unwrap()
                        .into_data()
                        .unwrap();
                    assert_eq!(first, Bytes::from_static(b"first"));
                    first_chunk_seen.notify_one();
                    let rest = request.into_body().collect().await.unwrap().to_bytes();
                    assert_eq!(rest, Bytes::from_static(b"second"));
                    Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                        Bytes::from_static(b"ok"),
                    )))
                }
            });
            hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 17).unwrap());
        let ca = ProxyCa::generate("rustymiddle request streaming test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    body_channel_capacity: 1,
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "POST http://{origin_addr}/stream HTTP/1.1\r\nHost: {origin_addr}\r\nContent-Length: 11\r\nConnection: close\r\n\r\nfirst"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        timeout(Duration::from_millis(500), first_chunk_seen.notified())
            .await
            .expect("proxy buffered the request until client completion");
        client.write_all(b"second").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn stalled_request_body_is_terminated_by_the_idle_deadline() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut request = Vec::new();
            timeout(Duration::from_secs(1), stream.read_to_end(&mut request))
                .await
                .expect("proxy did not cancel the stalled upstream request")
                .unwrap();
            assert!(request.windows(5).any(|window| window == b"first"));
        });

        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 53).unwrap());
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    body_idle_timeout: Duration::from_millis(50),
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ProxyCa::generate("rustymiddle stalled body test", 2).unwrap(),
            trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "POST http://{origin_addr}/stall HTTP/1.1\r\nHost: {origin_addr}\r\nContent-Length: 10\r\nConnection: close\r\n\r\nfirst"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("stalled request did not fail within its body deadline")
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 502"));

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn ordinary_http_repairs_framing_after_request_body_replacement() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let service = service_fn(|request: Request<Incoming>| async move {
                assert_eq!(request.headers()["x-from-breakpoint"], "yes");
                assert_ne!(
                    request.headers().get(http::header::CONTENT_LENGTH),
                    Some(&HeaderValue::from_static("8"))
                );
                let body = request.into_body().collect().await.unwrap().to_bytes();
                assert_eq!(body, Bytes::from_static(b"request-edited"));
                Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"ok"),
                )))
            });
            hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 13).unwrap());
        let ca = ProxyCa::generate("rustymiddle request edit test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(EditingHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "POST http://{origin_addr}/edit HTTP/1.1\r\nHost: {origin_addr}\r\nContent-Length: 8\r\nConnection: close\r\n\r\noriginal"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert!(response.windows(6).any(|window| window == b"edited"));

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn rejected_exchange_emits_failed_and_returns_a_bounded_error() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 12).unwrap());
        let ca = ProxyCa::generate("rustymiddle lifecycle test", 2).unwrap();
        let handler = Arc::new(RejectingLifecycleHandler::default());
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            ca,
            trust,
            handler.clone(),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                b"GET http://127.0.0.1:9/rejected HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 403"));
        assert!(response.len() < 1_024);
        assert_eq!(
            *handler
                .phases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                BreakpointPhase::BeforeRequestHeaders,
                BreakpointPhase::Failed
            ]
        );

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_limit_returns_413_before_contacting_an_origin() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 14).unwrap());
        let ca = ProxyCa::generate("rustymiddle body limit test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    max_request_body_bytes: 4,
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                b"POST http://127.0.0.1:9/too-large HTTP/1.1\r\nHost: 127.0.0.1:9\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 413"));

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn handler_can_synthesize_a_response_and_head_suppresses_its_body() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 15).unwrap());
        let ca = ProxyCa::generate("rustymiddle synthetic response test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(LocalResponseHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                b"GET http://127.0.0.1:9/local HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 202"));
        assert!(
            response
                .windows(b"x-local-response: yes".len())
                .any(|window| window.eq_ignore_ascii_case(b"x-local-response: yes"))
        );
        assert!(
            response
                .windows(b"synthetic".len())
                .any(|window| window == b"synthetic")
        );

        let mut head_client = TcpStream::connect(proxy_addr).await.unwrap();
        head_client
            .write_all(
                b"HEAD http://127.0.0.1:9/local HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut head_response = Vec::new();
        head_client.read_to_end(&mut head_response).await.unwrap();
        assert!(head_response.starts_with(b"HTTP/1.1 202"));
        let body_offset = head_response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert!(head_response[body_offset..].is_empty());

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn connect_tls_is_intercepted_without_certificate_bypass() {
        let origin_ca = ProxyCa::generate("rustymiddle origin test root", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&origin_leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let mut stream = tokio_boring::accept(&origin_acceptor, stream)
                .await
                .unwrap();
            let request = read_http_head(&mut stream).await;
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(text.contains("x-from-breakpoint: yes"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });

        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                23,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle CONNECT test root", 2).unwrap();
        let proxy_client_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            ..ProxyConfig::default()
        };
        let proxy = ProxyServer::bind(config, proxy_ca, origin_trust, Arc::new(EditingHandler))
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let connect_response = read_http_head(&mut transport).await;
        assert!(connect_response.starts_with(b"HTTP/1.1 200"));

        let browser_tls =
            UpstreamTlsContextFactory::new(proxy_client_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap()
                .build();
        let mut browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        browser
            .write_all(
                format!(
                    "GET /proof HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        browser.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response).to_ascii_lowercase();
        assert!(response.contains("x-intercepted: yes"));
        assert!(response.contains("\r\nedited\r\n"));

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http1);
        assert_eq!(proof.egress_version, HttpLegVersion::Http1);
        assert_eq!(proof.trust_generation, 23);

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_rejects_invalid_certificates_for_h1_and_h2_egress() {
        for (route_policy, wrong_hostname) in [
            (RoutePolicy::Http1Only, false),
            (RoutePolicy::Http1Only, true),
            (RoutePolicy::Http2Only, false),
            (RoutePolicy::Http2Only, true),
        ] {
            let origin_ca = ProxyCa::generate("rustymiddle invalid upstream origin", 2).unwrap();
            let leaf_identity = if wrong_hostname {
                "wrong.example"
            } else {
                "localhost"
            };
            let origin_leaf = origin_ca
                .issue(EndpointIdentity::parse(leaf_identity).unwrap(), 1)
                .unwrap();
            let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
                .acceptor(&origin_leaf)
                .unwrap();
            let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin_addr = origin.local_addr().unwrap();
            let origin_task = tokio::spawn(async move {
                let (stream, _) = origin.accept().await.unwrap();
                if let Ok(mut stream) = tokio_boring::accept(&origin_acceptor, stream).await {
                    let mut byte = [0_u8; 1];
                    let _ = timeout(Duration::from_secs(1), stream.read(&mut byte)).await;
                }
            });

            let trusted_root = if wrong_hostname {
                origin_ca.certificate().to_der().unwrap()
            } else {
                ProxyCa::generate("unrelated upstream root", 2)
                    .unwrap()
                    .certificate()
                    .to_der()
                    .unwrap()
            };
            let upstream_trust =
                Arc::new(TrustSnapshot::load(&StaticTrust(vec![trusted_root]), 37).unwrap());
            let proxy_ca = ProxyCa::generate("rustymiddle invalid-cert proxy", 2).unwrap();
            let browser_trust = Arc::new(
                TrustSnapshot::load(
                    &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                    1,
                )
                .unwrap(),
            );
            let proxy = ProxyServer::bind(
                ProxyConfig {
                    route_policy,
                    ..ProxyConfig::default()
                },
                proxy_ca,
                upstream_trust,
                Arc::new(rustymiddle_core::ContinueHandler),
            )
            .await
            .unwrap();
            let proxy_addr = proxy.local_addr().unwrap();
            let mut evidence = proxy.subscribe_evidence();
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let proxy_task = tokio::spawn(proxy.serve(async move {
                let _ = shutdown_rx.await;
            }));

            let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
            transport
                .write_all(
                    format!(
                        "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                        origin_addr.port(),
                        origin_addr.port()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            assert!(
                read_http_head(&mut transport)
                    .await
                    .starts_with(b"HTTP/1.1 200")
            );
            let browser_tls =
                UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                    .hyper_connector_builder(b"\x08http/1.1")
                    .unwrap()
                    .build();
            let mut browser =
                tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                    .await
                    .unwrap();
            browser
            .write_all(
                format!(
                    "GET /must-fail HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
            let mut response = Vec::new();
            let read_result =
                timeout(Duration::from_secs(2), browser.read_to_end(&mut response)).await;
            assert!(
                read_result.is_ok(),
                "proxy did not terminate failed exchange"
            );
            assert!(response.starts_with(b"HTTP/1.1 502"));
            assert!(
                response
                    .windows(28)
                    .any(|window| window == b"rustymiddle exchange failed\n")
            );
            assert!(matches!(
                evidence.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));

            shutdown_tx.send(()).unwrap();
            proxy_task.await.unwrap().unwrap();
            origin_task.await.unwrap();
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_h1_ingress_can_force_verified_h2_egress() {
        let origin_ca = ProxyCa::generate("rustymiddle h2 origin root", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&origin_leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let stream = tokio_boring::accept(&origin_acceptor, stream)
                .await
                .unwrap();
            assert_eq!(
                stream.ssl().selected_alpn_protocol(),
                Some(b"h2".as_slice())
            );
            let service = service_fn(|request: Request<Incoming>| async move {
                assert_eq!(request.version(), Version::HTTP_2);
                assert_eq!(request.headers()["x-from-breakpoint"], "yes");
                Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"ok"),
                )))
            });
            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                29,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle h2 proxy root", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http2Only,
            ..ProxyConfig::default()
        };
        let proxy = ProxyServer::bind(config, proxy_ca, origin_trust, Arc::new(EditingHandler))
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );

        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap()
                .build();
        let mut browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        browser
            .write_all(
                format!(
                    "GET /proof HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        browser.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response).to_ascii_lowercase();
        assert!(response.contains("x-intercepted: yes"));
        assert!(response.contains("\r\nedited\r\n"));

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http1);
        assert_eq!(proof.egress_version, HttpLegVersion::Http2);
        assert_eq!(proof.trust_generation, 29);

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn paused_h2_stream_does_not_block_an_unrelated_stream() {
        let origin_ca = ProxyCa::generate("rustymiddle concurrent h2 origin", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&origin_leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let stream = tokio_boring::accept(&origin_acceptor, stream)
                .await
                .unwrap();
            let service = service_fn(|request: Request<Incoming>| async move {
                let body = if request.uri().path() == "/slow" {
                    Bytes::from_static(b"slow")
                } else {
                    Bytes::from_static(b"fast")
                };
                Ok::<_, Infallible>(Response::new(http_body_util::Full::new(body)))
            });
            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                31,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle concurrent h2 proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let handler = Arc::new(PausingHandler::default());
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http2Only,
                ..ProxyConfig::default()
            },
            proxy_ca,
            origin_trust,
            handler.clone(),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x02h2")
                .unwrap()
                .build();
        let browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let authority = format!("localhost:{}", origin_addr.port());
        let slow = sender.send_request(
            Request::builder()
                .uri(format!("https://{authority}/slow"))
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap(),
        );
        let slow_task = tokio::spawn(slow);
        timeout(Duration::from_secs(1), handler.slow_entered.notified())
            .await
            .expect("slow H2 stream never reached its breakpoint");
        let fast = sender.send_request(
            Request::builder()
                .uri(format!("https://{authority}/fast"))
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap(),
        );
        timeout(Duration::from_millis(300), handler.fast_entered.notified())
            .await
            .expect("fast H2 stream never reached its independent breakpoint");
        let fast_evidence = timeout(Duration::from_secs(1), evidence.recv()).await;
        let fast = timeout(Duration::from_secs(1), fast).await;
        handler.release_slow.notify_one();
        let fast_evidence = fast_evidence
            .expect("fast H2 stream stalled after its breakpoint")
            .unwrap();
        let fast = fast
            .expect("fast H2 stream was blocked by paused stream")
            .unwrap();
        assert_eq!(
            fast.into_body().collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"fast")
        );
        let slow = slow_task.await.unwrap().unwrap();
        assert_eq!(
            slow.into_body().collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"slow")
        );

        let slow_evidence = evidence.recv().await.unwrap();
        assert_eq!(fast_evidence.ingress_version, HttpLegVersion::Http2);
        assert_eq!(slow_evidence.ingress_version, HttpLegVersion::Http2);
        assert_ne!(fast_evidence.session_id, slow_evidence.session_id);

        drop(sender);
        connection_task.abort();
        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_h2_ingress_can_force_verified_h1_egress() {
        let origin_ca = ProxyCa::generate("rustymiddle h2-to-h1 origin", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&origin_leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let mut stream = tokio_boring::accept(&origin_acceptor, stream)
                .await
                .unwrap();
            assert_eq!(
                stream.ssl().selected_alpn_protocol(),
                Some(b"http/1.1".as_slice())
            );
            let request = read_http_head(&mut stream).await;
            assert!(
                String::from_utf8_lossy(&request)
                    .to_ascii_lowercase()
                    .contains("x-from-breakpoint: yes")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });

        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                45,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle h2-to-h1 proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            proxy_ca,
            origin_trust,
            Arc::new(EditingHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x02h2")
                .unwrap()
                .build();
        let browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let response = sender
            .send_request(
                Request::builder()
                    .uri(format!("https://localhost:{}/h2-to-h1", origin_addr.port()))
                    .body(http_body_util::Empty::<Bytes>::new())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-intercepted"], "yes");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"edited")
        );
        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http2);
        assert_eq!(proof.egress_version, HttpLegVersion::Http1);
        assert_eq!(proof.trust_generation, 45);

        drop(sender);
        connection_task.abort();
        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn auto_safely_falls_back_from_unreachable_h3_before_response_start() {
        let origin_ca = ProxyCa::generate("rustymiddle auto fallback origin", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let origin_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&origin_leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let stream = tokio_boring::accept(&origin_acceptor, stream)
                .await
                .unwrap();
            assert_eq!(
                stream.ssl().selected_alpn_protocol(),
                Some(b"h2".as_slice())
            );
            let service = service_fn(|request: Request<Incoming>| async move {
                assert_eq!(request.version(), Version::HTTP_2);
                assert_eq!(request.headers()["x-from-breakpoint"], "yes");
                Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"ok"),
                )))
            });
            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                54,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle auto fallback proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Auto,
                h3: H3TransportLimits {
                    idle_timeout: Duration::from_millis(100),
                    ..H3TransportLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            origin_trust,
            Arc::new(EditingHandler),
        )
        .await
        .unwrap();
        proxy
            .state
            .alt_svc
            .lock()
            .await
            .observe(
                Origin {
                    host: "localhost".to_owned(),
                    port: origin_addr.port(),
                },
                &format!("h3=\":{}\"; ma=60", origin_addr.port()),
                Instant::now(),
            )
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap()
                .build();
        let mut browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        browser
            .write_all(
                format!(
                    "GET /fallback HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), browser.read_to_end(&mut response))
            .await
            .expect("Auto fallback did not complete")
            .unwrap();
        assert!(response.windows(6).any(|window| window == b"edited"));

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.egress_version, HttpLegVersion::Http2);
        assert_eq!(proof.trust_generation, 54);
        assert_eq!(
            proof.route_attempts,
            vec![
                RouteAttemptEvidence {
                    protocol: HttpLegVersion::Http3,
                    outcome: "failed-before-response".to_owned(),
                },
                RouteAttemptEvidence {
                    protocol: HttpLegVersion::Http2,
                    outcome: "success".to_owned(),
                },
            ]
        );

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn auto_refuses_to_replay_a_non_idempotent_request_after_h3_failure() {
        let tcp_origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = tcp_origin.local_addr().unwrap();
        let upstream_trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 55).unwrap());
        let proxy_ca = ProxyCa::generate("rustymiddle no replay proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Auto,
                h3: H3TransportLimits {
                    idle_timeout: Duration::from_millis(100),
                    ..H3TransportLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            upstream_trust,
            Arc::new(rustymiddle_core::ContinueHandler),
        )
        .await
        .unwrap();
        proxy
            .state
            .alt_svc
            .lock()
            .await
            .observe(
                Origin {
                    host: "localhost".to_owned(),
                    port: origin_addr.port(),
                },
                &format!("h3=\":{}\"; ma=60", origin_addr.port()),
                Instant::now(),
            )
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap()
                .build();
        let mut browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        browser
            .write_all(
                format!(
                    "POST /must-not-replay HTTP/1.1\r\nHost: localhost:{}\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), browser.read_to_end(&mut response))
            .await
            .expect("non-replayable H3 failure did not terminate")
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 502"));
        assert!(
            timeout(Duration::from_millis(300), tcp_origin.accept())
                .await
                .is_err(),
            "Auto replayed a POST over the TCP fallback"
        );

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_h1_ingress_can_force_verified_h3_egress() {
        let origin_ca = ProxyCa::generate("rustymiddle proxy h3 origin", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin_addr, origin_task) = spawn_h3_origin(origin_leaf, bind_ip).await;
        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                43,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle h3 proxy root", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http3Only,
                h3: H3TransportLimits {
                    idle_timeout: Duration::from_secs(2),
                    ..H3TransportLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            origin_trust,
            Arc::new(EditingHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap()
                .build();
        let mut browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        browser
            .write_all(
                format!(
                    "GET /proof HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        browser.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response).to_ascii_lowercase();
        assert!(response.contains("x-intercepted: yes"));
        assert!(response.contains("\r\nedited\r\n"));

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http1);
        assert_eq!(proof.egress_version, HttpLegVersion::Http3);
        assert_eq!(proof.trust_generation, 43);
        assert_eq!(proof.h3.as_ref().unwrap().alpn, "h3");
        assert_eq!(proof.route_attempts.len(), 1);
        assert_eq!(proof.route_attempts[0].outcome, "success");

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_h2_ingress_can_force_verified_h3_egress() {
        let origin_ca = ProxyCa::generate("rustymiddle h2-to-h3 origin", 2).unwrap();
        let origin_leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin_addr, origin_task) = spawn_h3_origin(origin_leaf, bind_ip).await;
        let origin_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                46,
            )
            .unwrap(),
        );
        let proxy_ca = ProxyCa::generate("rustymiddle h2-to-h3 proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http3Only,
                h3: H3TransportLimits {
                    idle_timeout: Duration::from_secs(2),
                    ..H3TransportLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            origin_trust,
            Arc::new(EditingHandler),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
        transport
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x02h2")
                .unwrap()
                .build();
        let browser =
            tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let response = sender
            .send_request(
                Request::builder()
                    .uri(format!("https://localhost:{}/h2-to-h3", origin_addr.port()))
                    .body(http_body_util::Empty::<Bytes>::new())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-intercepted"], "yes");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"edited")
        );

        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.ingress_version, HttpLegVersion::Http2);
        assert_eq!(proof.egress_version, HttpLegVersion::Http3);
        assert_eq!(proof.trust_generation, 46);
        assert_eq!(proof.h3.as_ref().unwrap().alpn, "h3");
        assert_eq!(proof.route_attempts.len(), 1);
        assert_eq!(proof.route_attempts[0].outcome, "success");

        drop(sender);
        connection_task.abort();
        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    async fn read_http_head<S>(stream: &mut S) -> Vec<u8>
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let mut result = Vec::new();
        let mut byte = [0_u8; 1];
        while !result.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut byte).await.unwrap();
            assert_ne!(read, 0, "connection ended before HTTP head completed");
            result.push(byte[0]);
        }
        result
    }

    async fn spawn_h3_origin(
        leaf: rustymiddle_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
    ) -> (SocketAddr, JoinHandle<()>) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_certificate(&leaf.certificate).unwrap();
        tls.set_private_key(&leaf.private_key).unwrap();
        tls.check_private_key().unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();
        config.set_max_idle_timeout(2_000);
        config.set_max_recv_udp_payload_size(1_350);
        config.set_max_send_udp_payload_size(1_350);
        config.set_initial_max_data(1024 * 1024);
        config.set_initial_max_stream_data_bidi_local(256 * 1024);
        config.set_initial_max_stream_data_bidi_remote(256 * 1024);
        config.set_initial_max_stream_data_uni(256 * 1024);
        config.set_initial_max_streams_bidi(16);
        config.set_initial_max_streams_uni(16);
        config.set_disable_active_migration(true);
        let socket = UdpSocket::bind((bind_ip, 0)).await.unwrap();
        let local = socket.local_addr().unwrap();
        let task = tokio::spawn(run_h3_origin(socket, local, config));
        (local, task)
    }

    async fn run_h3_origin(socket: UdpSocket, local: SocketAddr, mut config: quiche::Config) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut http3 = None;
        let mut received = vec![0_u8; 65_535];
        let mut outgoing = vec![0_u8; 1_350];
        loop {
            let (read, from) = timeout(Duration::from_secs(3), socket.recv_from(&mut received))
                .await
                .expect("proxy H3 client stalled")
                .unwrap();
            if connection.is_none() {
                let header =
                    quiche::Header::from_slice(&mut received[..read], quiche::MAX_CONN_ID_LEN)
                        .unwrap();
                assert_eq!(header.ty, quiche::Type::Initial);
                let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
                rand_bytes(&mut source_id).unwrap();
                let source_id = quiche::ConnectionId::from_ref(&source_id);
                connection =
                    Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
            }
            let connection = connection.as_mut().unwrap();
            match connection.recv(&mut received[..read], quiche::RecvInfo { from, to: local }) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(error) => panic!("proxy H3 fixture recv failed: {error}"),
            }
            if connection.is_established() && http3.is_none() {
                http3 =
                    Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
            }
            let mut response_sent = false;
            if let Some(http3) = http3.as_mut() {
                loop {
                    match http3.poll(connection) {
                        Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                            assert!(list.iter().any(|header| {
                                header.name() == b"x-from-breakpoint" && header.value() == b"yes"
                            }));
                            let headers = [
                                quiche::h3::Header::new(b":status", b"200"),
                                quiche::h3::Header::new(b"content-type", b"text/plain"),
                            ];
                            http3
                                .send_response(connection, stream_id, &headers, false)
                                .unwrap();
                            http3
                                .send_body(connection, stream_id, b"h3-origin", true)
                                .unwrap();
                            response_sent = true;
                        }
                        Ok((stream_id, quiche::h3::Event::Data)) => loop {
                            match http3.recv_body(connection, stream_id, &mut received) {
                                Ok(_) => {}
                                Err(quiche::h3::Error::Done) => break,
                                Err(error) => panic!("proxy H3 fixture body failed: {error}"),
                            }
                        },
                        Ok((_, _)) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => panic!("proxy H3 fixture poll failed: {error}"),
                    }
                }
            }
            loop {
                match connection.send(&mut outgoing) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&outgoing[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("proxy H3 fixture send failed: {error}"),
                }
            }
            if response_sent {
                return;
            }
        }
    }
}
