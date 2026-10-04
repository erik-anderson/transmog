use std::{
    collections::VecDeque,
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use boring::ssl::NameType;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustymiddle_content::{ContentBodyPipeline, ContentPipelineError, ContentPolicy};
use rustymiddle_core::{
    BodyFrame, BodySemantics, BodyStream, BodyStreamError, BodyStreamSender, BoundedBodyBuffer,
    CanonicalRequest, CanonicalResponse, ConnectionId, FallbackDecision, HeaderBlock, HeaderField,
    HttpLegVersion, MessageKind, Replayability, RequestHead, ResponseHead, RoutePolicy, SessionId,
    SessionMetadata, StreamId, StreamingRequest, Target, TranslationOptions, body_semantics,
    intercept::{
        BodyHookError, BodyPipeline, BodyPipelineError, BodyPipelineLimits, ChainExecutionError,
        ChainInitError, CompletedExchange, ExchangeChain, ExchangeFailure, ExchangeFailureKind,
        ExchangeMetadata, ExchangeStage, HookAbort, HookExecutionError, InterceptorChainFactory,
        InterceptorFactory, InterceptorRegistration, InterceptorRequirement, RequestHeadOutcome,
        ResponseHeadOutcome,
    },
    observe::{
        BodyDirection, ExchangeObserver, ObservedBodyChunk, ObserverEventKind, ObserverHub,
        ObserverStats,
    },
    prepare_headers,
    route::{
        OriginalDestinationOnly, PolicyRouteSelector, RouteError, RouteInput,
        RouteSelectionService, RouteSelector, UpstreamPlan,
    },
    upstream::{UpstreamError, UpstreamExecutor, UpstreamService},
};
use rustymiddle_h3::{
    AltSvcCache, H3OriginClient, H3OriginError, H3Telemetry, H3TransportLimits, Origin,
};
use rustymiddle_http::{ConnectAuthority, HyperEgressMode, HyperOriginClient, HyperOriginError};
use rustymiddle_tls::{
    CachedMitmCertificateResolver, CertificateResolverError, DownstreamCertificateResolver,
    DownstreamTlsContextFactory, DownstreamTlsPolicy, EndpointIdentity, LeafCacheError, ProxyCa,
    TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy, normalize_connect_identity,
};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock, Semaphore, broadcast},
    task::JoinSet,
    time::timeout,
};
use tracing::{debug, warn};

use crate::{
    AtomicRuntimeIdGenerator, ListenerConfigError, RuntimeClock, RuntimeIdGenerator, RuntimeIdKind,
    RuntimeLimits, SystemRuntimeClock,
};

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

/// Immutable application-supplied components for one proxy listener.
///
/// The default network upstream remains active unless an application-owned
/// [`UpstreamService`] is installed. Builder methods consume and return the
/// value so configuration cannot change after the listener starts.
#[derive(Clone)]
pub struct ProxyComponents {
    hooks: InterceptorChainFactory,
    content: ContentPolicy,
    observers: ObserverHub,
    route_selector: Option<Arc<dyn RouteSelector>>,
    application_upstream: Option<Arc<dyn UpstreamService>>,
    certificates: Arc<dyn DownstreamCertificateResolver>,
    clock: Arc<dyn RuntimeClock>,
    ids: Arc<dyn RuntimeIdGenerator>,
}

impl std::fmt::Debug for ProxyComponents {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyComponents")
            .field("hooks", &self.hooks)
            .field("content", &self.content)
            .field("observers", &self.observers)
            .field("custom_route_selector", &self.route_selector.is_some())
            .field("application_upstream", &self.application_upstream.is_some())
            .finish_non_exhaustive()
    }
}

impl ProxyComponents {
    /// Creates component configuration with explicit hooks and certificates.
    #[must_use]
    pub fn new(
        hooks: InterceptorChainFactory,
        certificates: Arc<dyn DownstreamCertificateResolver>,
    ) -> Self {
        Self {
            hooks,
            content: ContentPolicy::default(),
            observers: ObserverHub::default(),
            route_selector: None,
            application_upstream: None,
            certificates,
            clock: Arc::new(SystemRuntimeClock),
            ids: Arc::new(AtomicRuntimeIdGenerator::new()),
        }
    }

    /// Installs immutable content decoding, encoding, and resource policy.
    #[must_use]
    pub fn with_content_policy(mut self, policy: ContentPolicy) -> Self {
        self.content = policy;
        self
    }

    /// Installs an immutable bounded observer hub.
    #[must_use]
    pub fn with_observers(mut self, observers: ObserverHub) -> Self {
        self.observers = observers;
        self
    }

    /// Installs a custom route selector.
    #[must_use]
    pub fn with_route_selector(mut self, selector: Arc<dyn RouteSelector>) -> Self {
        self.route_selector = Some(selector);
        self
    }

    /// Routes canonical exchanges to an application-owned upstream service.
    #[must_use]
    pub fn with_upstream_service(mut self, service: Arc<dyn UpstreamService>) -> Self {
        self.application_upstream = Some(service);
        self
    }

    /// Installs deterministic or application-owned clock and ID providers.
    #[must_use]
    pub fn with_infrastructure(
        mut self,
        clock: Arc<dyn RuntimeClock>,
        ids: Arc<dyn RuntimeIdGenerator>,
    ) -> Self {
        self.clock = clock;
        self.ids = ids;
        self
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

    /// Returns bounded observer delivery counters in registration order.
    pub fn observer_stats(&self) -> Vec<ObserverStats> {
        self.state.observers.stats()
    }
}

impl ProxyServer {
    /// Binds a proxy with one required per-exchange interceptor factory.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError`] when configuration or listener setup fails.
    pub async fn bind(
        config: ProxyConfig,
        ca: ProxyCa,
        trust: Arc<TrustSnapshot>,
        interceptor: Arc<dyn InterceptorFactory>,
    ) -> Result<Self, ProxyRuntimeError> {
        let hooks = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "application",
                interceptor,
                InterceptorRequirement::Required,
            )],
            config.limits.hooks,
        );
        Self::bind_with_chain(config, ca, trust, hooks).await
    }

    /// Binds a loopback-by-default explicit proxy.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError`] for unsafe configuration, listener bind
    /// failure, invalid leaf-cache policy, or upstream connector construction
    /// failure.
    pub async fn bind_with_chain(
        config: ProxyConfig,
        ca: ProxyCa,
        trust: Arc<TrustSnapshot>,
        hooks: InterceptorChainFactory,
    ) -> Result<Self, ProxyRuntimeError> {
        let certificates = Arc::new(CachedMitmCertificateResolver::new(
            ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )?);
        Self::bind_with_components(config, trust, ProxyComponents::new(hooks, certificates)).await
    }

    /// Binds with application-supplied immutable runtime components.
    ///
    /// This low-level entry point does not require a proxy CA. A reverse-mode
    /// or application-owned listener can supply certificate material through
    /// [`DownstreamCertificateResolver`] without changing exchange hooks.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyRuntimeError`] for invalid bounds, listener setup, or
    /// upstream connector construction failure.
    pub async fn bind_with_components(
        config: ProxyConfig,
        trust: Arc<TrustSnapshot>,
        components: ProxyComponents,
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
            || config.limits.route_selection_timeout.is_zero()
            || config.limits.application_upstream_timeout.is_zero()
        {
            return Err(ProxyRuntimeError::InvalidConfiguration);
        }
        let upstream = Arc::new(UpstreamGeneration::new(trust, config.h3)?);
        let listener = TcpListener::bind(config.listener.listen_addr).await?;
        let (evidence, _) = broadcast::channel(1_024);
        let state = Arc::new(ProxyState {
            hooks: components.hooks,
            content: components.content,
            observers: components.observers,
            route_selector: components.route_selector,
            application_upstream: components.application_upstream,
            downstream_tls: DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default()),
            certificates: components.certificates,
            clock: components.clock,
            ids: components.ids,
            upstream: RwLock::new(upstream),
            alt_svc: Mutex::new(AltSvcCache::new(1_024)),
            config: config.clone(),
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
                    let connection_id = ConnectionId(
                        state.ids.next_id(RuntimeIdKind::Connection)
                    );
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
        self.state.observers.shutdown().await;
        Ok(())
    }
}

struct ProxyState {
    hooks: InterceptorChainFactory,
    content: ContentPolicy,
    observers: ObserverHub,
    route_selector: Option<Arc<dyn RouteSelector>>,
    application_upstream: Option<Arc<dyn UpstreamService>>,
    downstream_tls: DownstreamTlsContextFactory,
    certificates: Arc<dyn DownstreamCertificateResolver>,
    clock: Arc<dyn RuntimeClock>,
    ids: Arc<dyn RuntimeIdGenerator>,
    upstream: RwLock<Arc<UpstreamGeneration>>,
    alt_svc: Mutex<AltSvcCache>,
    config: ProxyConfig,
    evidence: broadcast::Sender<ExchangeEvidence>,
}

#[derive(Clone, Debug)]
struct RuntimeExchangeObserver(Arc<ExchangeObserver>);

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
            .certificates
            .resolve(&identity, self.clock.system_time())?;
        validate_resolved_leaf(&identity, &leaf)?;
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
        let original_target = request_head.target.clone();
        let mut session = SessionMetadata {
            session_id: SessionId(self.ids.next_id(RuntimeIdKind::Exchange)),
            downstream_connection_id: context.connection_id,
            stream_id: StreamId(self.ids.next_id(RuntimeIdKind::Stream)),
            client_addr: context.client_addr,
            proxy_addr: context.proxy_addr,
            ingress_version,
            egress_version: None,
        };

        let metadata = ExchangeMetadata::from_session_at(
            &session,
            original_target.clone(),
            self.clock.system_time(),
        );
        let observer = Arc::new(self.observers.start_exchange(Arc::new(metadata.clone())));
        observer
            .emit(ObserverEventKind::ExchangeStarted {
                metadata: Arc::new(metadata.clone()),
            })
            .await;
        let mut chain = match self.hooks.create_exchange(metadata) {
            Ok(chain) => chain,
            Err(error) => {
                observer
                    .failed(ExchangeFailure {
                        metadata: Arc::new(ExchangeMetadata::from_session_at(
                            &session,
                            original_target,
                            self.clock.system_time(),
                        )),
                        stage: ExchangeStage::Initialize,
                        kind: ExchangeFailureKind::HookInitialization,
                        request_committed: false,
                        response_committed: false,
                        message: error.to_string(),
                    })
                    .await;
                return Err(error.into());
            }
        };
        chain
            .context()
            .extensions()
            .insert(RuntimeExchangeObserver(Arc::clone(&observer)));
        for diagnostic in chain.initialization_diagnostics() {
            observer
                .emit(ObserverEventKind::HookInitializationSkipped(
                    diagnostic.clone(),
                ))
                .await;
        }
        let request_outcome = chain.request_head(request_head).await;
        let request_outcome = match request_outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::RequestHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        let reroute = match request_outcome {
            RequestHeadOutcome::Continue { head, reroute } => {
                request_head = head;
                reroute
            }
            RequestHeadOutcome::Respond {
                request_head: effective_request,
                response,
            } => {
                observer
                    .emit(ObserverEventKind::RequestHeadFinalized(
                        effective_request.clone(),
                    ))
                    .await;
                let chain = Arc::new(chain);
                return self
                    .finish_local_response(response, &session, &effective_request, chain)
                    .await;
            }
            RequestHeadOutcome::Abort(reason) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::RequestHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted exchange: {reason:?}"),
                    false,
                    false,
                )
                .await;
                return Err(reason.into());
            }
        };
        observer
            .emit(ObserverEventKind::RequestHeadFinalized(
                request_head.clone(),
            ))
            .await;
        let chain = Arc::new(chain);

        let upstream = self.upstream.read().await.clone();
        let replayability = request_replayability(&request_head);
        let plan = match self
            .select_route(
                &chain,
                &request_head,
                original_target,
                reroute,
                replayability,
                upstream.trust_generation,
            )
            .await
        {
            Ok(plan) => plan,
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::Upstream,
                    ExchangeFailureKind::Route,
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        observer
            .emit(ObserverEventKind::RouteSelected {
                policy_id: Arc::clone(&plan.route_id),
                reason: Arc::clone(&plan.reason),
            })
            .await;
        apply_route_destination(&mut request_head.target, &plan);

        let expected_tls_policy = format!("system-trust-v{}", upstream.trust_generation);
        if self.application_upstream.is_none()
            && (plan.pool_key.trust_generation != upstream.trust_generation
                || plan.pool_key.tls_policy_id.as_ref() != expected_tls_policy
                || &*plan.pool_key.connector_policy_id != "direct")
        {
            let error = ProxyRuntimeError::UnsupportedNetworkPlan(
                "route plan selected an inactive trust, TLS, or connector policy".to_owned(),
            );
            self.fail_chain(
                &chain,
                ExchangeStage::Upstream,
                ExchangeFailureKind::Route,
                error.to_string(),
                false,
                false,
            )
            .await;
            return Err(error);
        }

        if let Err(error) = validate_declared_body_limit(
            &request_head.headers,
            self.config.limits.max_request_body_bytes,
        ) {
            self.fail_exchange(
                &chain,
                ExchangeStage::RequestBody,
                error.to_string(),
                false,
                false,
            )
            .await;
            return Err(error);
        }

        if let Some(service) = &self.application_upstream {
            return self
                .handle_application_upstream(
                    request,
                    session,
                    request_head,
                    ingress_version,
                    plan,
                    Arc::clone(service),
                    chain,
                )
                .await;
        }

        if plan.pool_key.version_policy == RoutePolicy::Http3Only {
            return self
                .handle_streaming_h3(
                    request,
                    session,
                    request_head,
                    ingress_version,
                    upstream,
                    chain,
                )
                .await;
        }

        if let Some(mode) = self
            .streaming_hyper_mode(&request_head, plan.pool_key.version_policy)
            .await
        {
            return self
                .handle_streaming_hyper(
                    request,
                    session,
                    request_head,
                    ingress_version,
                    mode,
                    upstream,
                    chain,
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
                self.fail_runtime_exchange(
                    &chain,
                    ExchangeStage::RequestBody,
                    &error,
                    false,
                    false,
                )
                .await;
                return Err(error);
            }
        };
        let request_body_outcome = self
            .process_request_body(&chain, &request_head, raw_body)
            .await;
        let request_body_outcome = match request_body_outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.fail_runtime_exchange(
                    &chain,
                    ExchangeStage::RequestBody,
                    &error,
                    false,
                    false,
                )
                .await;
                return Err(error);
            }
        };
        let ProcessedBody {
            frames: request_body,
            headers: request_headers,
            modified: request_body_modified,
        } = request_body_outcome;
        request_head.headers = request_headers;

        let route = plan.pool_key.version_policy;
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
                self.fail_exchange(
                    &chain,
                    ExchangeStage::RequestHead,
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        let request = CanonicalRequest {
            head: request_head.clone(),
            body: request_body,
        };
        let routed = match self.route(request, &upstream, route).await {
            Ok(routed) => routed,
            Err(error) => {
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, &error, true, false)
                    .await;
                return Err(error);
            }
        };
        session.egress_version = Some(routed.protocol);

        let response = routed.response;
        let response_outcome = chain
            .response_head(&request_head, response.head, Some(response.body), false)
            .await;
        let (mut response, local_response) = match response_outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => (
                CanonicalResponse {
                    head,
                    body: replacement_body.unwrap_or_default(),
                },
                local_response,
            ),
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted exchange: {reason:?}"),
                    true,
                    false,
                )
                .await;
                return Err(reason.into());
            }
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        observe_response_head(&chain, &response.head).await;
        let outcome = if body_semantics(&request_head.method, response.head.status)
            == BodySemantics::Forbidden
        {
            ProcessedBody {
                frames: Vec::new(),
                headers: response.head.headers.clone(),
                modified: true,
            }
        } else {
            match self
                .process_response_body(
                    &chain,
                    &request_head,
                    &response.head,
                    local_response,
                    response.body,
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.fail_runtime_exchange(
                        &chain,
                        ExchangeStage::ResponseBody,
                        &error,
                        true,
                        false,
                    )
                    .await;
                    return Err(error);
                }
            }
        };
        let ProcessedBody {
            frames: body,
            headers,
            modified: body_modified,
        } = outcome;
        response.body = body;
        response.head.headers = headers;
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
                self.fail_exchange(
                    &chain,
                    ExchangeStage::ResponseHead,
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };

        self.complete_chain(&chain, &request_head, &response.head)
            .await;
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

    async fn select_route(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
        original_target: Target,
        explicit_reroute: Option<Target>,
        replayability: Replayability,
        trust_generation: u64,
    ) -> Result<UpstreamPlan, RouteError> {
        let selector = self.route_selector.clone().unwrap_or_else(|| {
            Arc::new(PolicyRouteSelector::new(
                Arc::new(OriginalDestinationOnly),
                self.config.route_policy,
                trust_generation,
                format!("system-trust-v{trust_generation}"),
                "direct",
                "default-network",
            ))
        });
        RouteSelectionService::new(selector, self.config.limits.route_selection_timeout)
            .select(
                RouteInput {
                    metadata: Arc::clone(chain.context().metadata()),
                    original_target: rustymiddle_core::intercept::OriginalTarget::new(
                        original_target,
                    ),
                    effective_request: request_head.clone(),
                    explicit_reroute,
                    replayability,
                },
                chain.context().cancellation(),
            )
            .await
    }

    async fn streaming_hyper_mode(
        &self,
        request: &RequestHead,
        route_policy: RoutePolicy,
    ) -> Option<HyperEgressMode> {
        match route_policy {
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
                    .get(&origin, self.clock.instant())
                    .is_some();
                (!has_h3_alternative).then_some(HyperEgressMode::Auto)
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn handle_application_upstream(
        &self,
        request: Request<Incoming>,
        mut session: SessionMetadata,
        mut request_head: RequestHead,
        ingress_version: HttpLegVersion,
        plan: UpstreamPlan,
        service: Arc<dyn UpstreamService>,
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let destination = request_head.source_version;
        let request_pipeline = self
            .prepare_streaming_request_pipeline(&chain, &mut request_head, destination)
            .await?;

        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request_chain = Arc::clone(&chain);
        let request_limit = self.config.limits.max_request_body_bytes;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        tokio::spawn(async move {
            stream_incoming_through_hooks(
                request.into_body(),
                request_sender,
                request_pipeline,
                request_chain,
                request_limit,
                body_idle_timeout,
            )
            .await;
        });

        let trust_generation = plan.pool_key.trust_generation;
        let upstream =
            UpstreamExecutor::new(service, self.config.limits.application_upstream_timeout)
                .execute(
                    StreamingRequest {
                        head: request_head.clone(),
                        body: request_body,
                    },
                    plan,
                    chain.context().cancellation(),
                )
                .await;
        let mut upstream = match upstream {
            Ok(response) => response,
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::Upstream,
                    ExchangeFailureKind::Upstream,
                    error.message.clone(),
                    error.request_committed,
                    error.response_started,
                )
                .await;
                return Err(error.into());
            }
        };
        session.egress_version = Some(upstream.head.source_version);

        let response_outcome = chain
            .response_head(&request_head, upstream.head, None, false)
            .await;
        let (response_head, replacement_body, local_response) = match response_outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => (head, replacement_body, local_response),
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted exchange: {reason:?}"),
                    true,
                    false,
                )
                .await;
                return Err(reason.into());
            }
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        upstream.head = response_head;
        observe_response_head(&chain, &upstream.head).await;
        if let Some(body) = replacement_body {
            drop(upstream.body);
            return self
                .finish_prepared_local_response(
                    CanonicalResponse {
                        head: upstream.head,
                        body,
                    },
                    &session,
                    &request_head,
                    chain,
                    local_response,
                )
                .await;
        }
        let mut response_head = upstream.head;
        let attempts = vec![RouteAttemptEvidence {
            protocol: session.egress_version.expect("egress was assigned"),
            outcome: "application-success".to_owned(),
        }];
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
                    chain,
                )
                .await;
        }
        let response_pipeline = self
            .prepare_streaming_response_pipeline(
                &chain,
                &request_head,
                &mut response_head,
                local_response,
                ingress_version,
            )
            .await?;
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let evidence = self.evidence.clone();
        let response_head_for_body = response_head.clone();
        let response_limit = self.config.limits.max_response_body_bytes;
        let response_chain = Arc::clone(&chain);
        tokio::spawn(async move {
            stream_response_through_hooks(
                upstream.body,
                downstream_sender,
                response_pipeline,
                response_chain,
                session,
                request_head,
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

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn handle_streaming_hyper(
        &self,
        request: Request<Incoming>,
        mut session: SessionMetadata,
        mut request_head: RequestHead,
        ingress_version: HttpLegVersion,
        mode: HyperEgressMode,
        upstream_generation: Arc<UpstreamGeneration>,
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        let request_pipeline = self
            .prepare_streaming_request_pipeline(&chain, &mut request_head, destination)
            .await?;
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request_chain = Arc::clone(&chain);
        let request_limit = self.config.limits.max_request_body_bytes;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        tokio::spawn(async move {
            stream_incoming_through_hooks(
                request.into_body(),
                request_sender,
                request_pipeline,
                request_chain,
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
                self.fail_exchange(
                    &chain,
                    ExchangeStage::Upstream,
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        session.egress_version = Some(upstream.head.source_version);
        self.learn_alt_svc_head(&origin, &upstream.head).await;

        let response_outcome = chain
            .response_head(&request_head, upstream.head, None, false)
            .await;
        let (response_head, replacement_body, local_response) = match response_outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => (head, replacement_body, local_response),
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted exchange: {reason:?}"),
                    true,
                    false,
                )
                .await;
                return Err(reason.into());
            }
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        upstream.head = response_head;
        observe_response_head(&chain, &upstream.head).await;
        if let Some(body) = replacement_body {
            drop(upstream.body);
            return self
                .finish_prepared_local_response(
                    CanonicalResponse {
                        head: upstream.head,
                        body,
                    },
                    &session,
                    &request_head,
                    chain,
                    local_response,
                )
                .await;
        }
        let mut response_head = upstream.head;
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
                    chain,
                )
                .await;
        }
        let response_pipeline = self
            .prepare_streaming_response_pipeline(
                &chain,
                &request_head,
                &mut response_head,
                local_response,
                ingress_version,
            )
            .await?;
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let evidence = self.evidence.clone();
        let response_session = session.clone();
        let response_request_head = request_head.clone();
        let response_head_for_body = response_head.clone();
        let response_limit = self.config.limits.max_response_body_bytes;
        let response_chain = Arc::clone(&chain);
        tokio::spawn(async move {
            stream_response_through_hooks(
                upstream.body,
                downstream_sender,
                response_pipeline,
                response_chain,
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
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if request_head.target.scheme != "https" {
            self.fail_exchange(
                &chain,
                ExchangeStage::Upstream,
                "HTTP/3 requires HTTPS".to_owned(),
                false,
                false,
            )
            .await;
            return Err(ProxyRuntimeError::Http3RequiresHttps);
        }
        let request_pipeline = self
            .prepare_streaming_request_pipeline(&chain, &mut request_head, HttpLegVersion::Http3)
            .await?;
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request_chain = Arc::clone(&chain);
        let request_limit = self.config.limits.max_request_body_bytes;
        let body_idle_timeout = self.config.limits.body_idle_timeout;
        tokio::spawn(async move {
            stream_incoming_through_hooks(
                request.into_body(),
                request_sender,
                request_pipeline,
                request_chain,
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
                self.fail_exchange(
                    &chain,
                    ExchangeStage::Upstream,
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        session.egress_version = Some(HttpLegVersion::Http3);
        self.learn_alt_svc_head(&origin, &upstream.response.head)
            .await;

        let response_outcome = chain
            .response_head(&request_head, upstream.response.head, None, false)
            .await;
        let (response_head, replacement_body, local_response) = match response_outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => (head, replacement_body, local_response),
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted exchange: {reason:?}"),
                    true,
                    false,
                )
                .await;
                return Err(reason.into());
            }
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        upstream.response.head = response_head;
        observe_response_head(&chain, &upstream.response.head).await;
        if let Some(body) = replacement_body {
            drop(upstream.response.body);
            return self
                .finish_prepared_local_response(
                    CanonicalResponse {
                        head: upstream.response.head,
                        body,
                    },
                    &session,
                    &request_head,
                    chain,
                    local_response,
                )
                .await;
        }
        let mut response_head = upstream.response.head;
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
                    chain,
                )
                .await;
        }
        let response_pipeline = self
            .prepare_streaming_response_pipeline(
                &chain,
                &request_head,
                &mut response_head,
                local_response,
                ingress_version,
            )
            .await?;
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let evidence = self.evidence.clone();
        let response_session = session.clone();
        let response_request_head = request_head;
        let response_head_for_body = response_head.clone();
        let response_limit = self.config.limits.max_response_body_bytes;
        let response_chain = Arc::clone(&chain);
        tokio::spawn(async move {
            stream_response_through_hooks(
                upstream.response.body,
                downstream_sender,
                response_pipeline,
                response_chain,
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
        response: CanonicalResponse,
        session: &SessionMetadata,
        request: &RequestHead,
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let outcome = chain
            .response_head(request, response.head, Some(response.body), true)
            .await;
        match outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => {
                observe_response_head(&chain, &head).await;
                self.finish_prepared_local_response(
                    CanonicalResponse {
                        head,
                        body: replacement_body.unwrap_or_default(),
                    },
                    session,
                    request,
                    chain,
                    local_response,
                )
                .await
            }
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted local response: {reason:?}"),
                    false,
                    false,
                )
                .await;
                Err(reason.into())
            }
            Err(error) => {
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    hook_failure_kind(&error),
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                Err(error.into())
            }
        }
    }

    async fn finish_prepared_local_response(
        &self,
        mut response: CanonicalResponse,
        session: &SessionMetadata,
        request: &RequestHead,
        chain: Arc<ExchangeChain>,
        local_response: bool,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if body_semantics(&request.method, response.head.status) == BodySemantics::Forbidden {
            response.body.clear();
        } else {
            let processed = self
                .process_response_body(
                    &chain,
                    request,
                    &response.head,
                    local_response,
                    response.body,
                )
                .await;
            let ProcessedBody {
                frames: body,
                headers,
                modified: body_modified,
            } = match processed {
                Ok(body) => body,
                Err(error) => {
                    self.fail_runtime_exchange(
                        &chain,
                        ExchangeStage::ResponseBody,
                        &error,
                        false,
                        false,
                    )
                    .await;
                    return Err(error);
                }
            };
            response.body = body;
            response.head.headers = headers;
            let prepared_headers = prepare_headers(
                &response.head.headers,
                TranslationOptions {
                    destination: session.ingress_version,
                    kind: MessageKind::Response,
                    body_modified,
                    force_identity_encoding: false,
                },
            );
            response.head.headers = match prepared_headers {
                Ok(headers) => headers,
                Err(error) => {
                    self.fail_exchange(
                        &chain,
                        ExchangeStage::ResponseHead,
                        error.to_string(),
                        false,
                        false,
                    )
                    .await;
                    return Err(error.into());
                }
            };
            self.complete_chain(&chain, request, &response.head).await;
            return response_to_hyper(response);
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
                self.fail_exchange(
                    &chain,
                    ExchangeStage::ResponseHead,
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        self.complete_chain(&chain, request, &response.head).await;
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
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        self.complete_chain(&chain, request_head, &response_head)
            .await;
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

    async fn complete_chain(
        &self,
        chain: &ExchangeChain,
        request: &RequestHead,
        response: &ResponseHead,
    ) {
        let outcome = CompletedExchange {
            metadata: Arc::clone(chain.context().metadata()),
            request_head: request.clone(),
            response_head: response.clone(),
        };
        let report = chain.completed(outcome.clone()).await;
        for error in report.errors() {
            warn!(%error, "terminal hook cleanup failed");
        }
        if let Some(observer) = runtime_observer(chain) {
            observer.completed(outcome).await;
        }
    }

    async fn fail_exchange(
        &self,
        chain: &ExchangeChain,
        stage: ExchangeStage,
        message: String,
        request_committed: bool,
        response_committed: bool,
    ) {
        let kind = match stage {
            ExchangeStage::RequestBody | ExchangeStage::ResponseBody => ExchangeFailureKind::Body,
            ExchangeStage::Upstream => ExchangeFailureKind::Upstream,
            ExchangeStage::RequestHead => ExchangeFailureKind::RequestTranslation,
            ExchangeStage::ResponseHead => ExchangeFailureKind::ResponseTranslation,
            ExchangeStage::Initialize => ExchangeFailureKind::HookInitialization,
            ExchangeStage::Terminal => ExchangeFailureKind::Shutdown,
        };
        self.fail_chain(
            chain,
            stage,
            kind,
            message,
            request_committed,
            response_committed,
        )
        .await;
    }

    async fn fail_runtime_exchange(
        &self,
        chain: &ExchangeChain,
        stage: ExchangeStage,
        error: &ProxyRuntimeError,
        request_committed: bool,
        response_committed: bool,
    ) {
        self.fail_chain(
            chain,
            stage,
            runtime_failure_kind(error, stage),
            error.to_string(),
            request_committed,
            response_committed,
        )
        .await;
    }

    async fn fail_chain(
        &self,
        chain: &ExchangeChain,
        stage: ExchangeStage,
        kind: ExchangeFailureKind,
        message: String,
        request_committed: bool,
        response_committed: bool,
    ) {
        let failure = ExchangeFailure {
            metadata: Arc::clone(chain.context().metadata()),
            stage,
            kind,
            request_committed,
            response_committed,
            message,
        };
        let report = chain.failed(failure.clone()).await;
        for error in report.errors() {
            warn!(%error, "terminal hook cleanup failed");
        }
        if let Some(observer) = runtime_observer(chain) {
            observer.failed(failure).await;
        }
    }

    async fn process_request_body(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
        frames: Vec<BodyFrame>,
    ) -> Result<ProcessedBody, ProxyRuntimeError> {
        let pipeline = chain
            .request_body_pipeline(request_head, self.body_pipeline_limits())
            .await?;
        self.process_body_pipeline(
            chain,
            BodyDirection::Request,
            pipeline,
            &request_head.headers,
            frames,
            self.config.limits.max_request_body_bytes,
        )
        .await
    }

    async fn request_content_pipeline(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
    ) -> Result<ContentBodyPipeline, ProxyRuntimeError> {
        let pipeline = chain
            .request_body_pipeline(request_head, self.body_pipeline_limits())
            .await?;
        Ok(ContentBodyPipeline::from_policy(
            pipeline,
            &request_head.headers,
            self.content,
        )?)
    }

    async fn prepare_streaming_request_pipeline(
        &self,
        chain: &ExchangeChain,
        request_head: &mut RequestHead,
        destination: HttpLegVersion,
    ) -> Result<ContentBodyPipeline, ProxyRuntimeError> {
        let pipeline = match self.request_content_pipeline(chain, request_head).await {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.fail_runtime_exchange(chain, ExchangeStage::RequestBody, &error, false, false)
                    .await;
                return Err(error);
            }
        };
        request_head.headers = pipeline.output_headers().clone();
        request_head.headers = match prepare_headers(
            &request_head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                body_modified: pipeline.modifies_body(),
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.fail_exchange(
                    chain,
                    ExchangeStage::RequestHead,
                    error.to_string(),
                    false,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        Ok(pipeline)
    }

    async fn response_content_pipeline(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
        response_head: &ResponseHead,
        local_response: bool,
    ) -> Result<ContentBodyPipeline, ProxyRuntimeError> {
        let pipeline = chain
            .response_body_pipeline(
                request_head,
                response_head,
                local_response,
                self.body_pipeline_limits(),
            )
            .await?;
        Ok(ContentBodyPipeline::from_policy(
            pipeline,
            &response_head.headers,
            self.content,
        )?)
    }

    async fn prepare_streaming_response_pipeline(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
        response_head: &mut ResponseHead,
        local_response: bool,
        destination: HttpLegVersion,
    ) -> Result<ContentBodyPipeline, ProxyRuntimeError> {
        let pipeline = match self
            .response_content_pipeline(chain, request_head, response_head, local_response)
            .await
        {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.fail_runtime_exchange(chain, ExchangeStage::ResponseBody, &error, true, false)
                    .await;
                return Err(error);
            }
        };
        response_head.headers = pipeline.output_headers().clone();
        response_head.headers = match prepare_headers(
            &response_head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Response,
                body_modified: pipeline.modifies_body(),
                force_identity_encoding: false,
            },
        ) {
            Ok(headers) => headers,
            Err(error) => {
                self.fail_exchange(
                    chain,
                    ExchangeStage::ResponseHead,
                    error.to_string(),
                    true,
                    false,
                )
                .await;
                return Err(error.into());
            }
        };
        Ok(pipeline)
    }

    async fn process_response_body(
        &self,
        chain: &ExchangeChain,
        request_head: &RequestHead,
        response_head: &ResponseHead,
        local_response: bool,
        frames: Vec<BodyFrame>,
    ) -> Result<ProcessedBody, ProxyRuntimeError> {
        let pipeline = chain
            .response_body_pipeline(
                request_head,
                response_head,
                local_response,
                self.body_pipeline_limits(),
            )
            .await?;
        self.process_body_pipeline(
            chain,
            BodyDirection::Response,
            pipeline,
            &response_head.headers,
            frames,
            self.config.limits.max_response_body_bytes,
        )
        .await
    }

    async fn process_body_pipeline(
        &self,
        chain: &ExchangeChain,
        direction: BodyDirection,
        pipeline: BodyPipeline,
        source_headers: &HeaderBlock,
        frames: Vec<BodyFrame>,
        limit: usize,
    ) -> Result<ProcessedBody, ProxyRuntimeError> {
        let mut pipeline =
            ContentBodyPipeline::from_policy(pipeline, source_headers, self.content)?;
        let modified = pipeline.modifies_body();
        let headers = pipeline.output_headers().clone();
        let mut output = Vec::new();
        for frame in frames {
            output.extend(pipeline.process(frame).await?);
        }
        output.extend(pipeline.finish().await?);
        validate_frames(&output, limit)?;
        observe_body_frames(chain, direction, &output).await;
        Ok(ProcessedBody {
            frames: output,
            headers,
            modified,
        })
    }

    fn body_pipeline_limits(&self) -> BodyPipelineLimits {
        BodyPipelineLimits {
            max_output_frames_per_call: NonZeroUsize::new(1_024).expect("1024 is nonzero"),
            max_output_bytes_per_call: NonZeroUsize::new(
                self.config
                    .limits
                    .max_request_body_bytes
                    .max(self.config.limits.max_response_body_bytes),
            )
            .expect("runtime body limits are validated as nonzero"),
        }
    }

    async fn route(
        &self,
        request: CanonicalRequest,
        upstream: &UpstreamGeneration,
        route_policy: RoutePolicy,
    ) -> Result<RoutedResponse, ProxyRuntimeError> {
        match route_policy {
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
            .get(&origin, self.clock.instant())
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
                    .mark_broken(&origin, self.clock.instant() + Duration::from_secs(30));
                let replayability = if request.head.method.eq_ignore_ascii_case("GET")
                    || request.head.method.eq_ignore_ascii_case("HEAD")
                {
                    Replayability::SafeMethod
                } else {
                    Replayability::NotReplayable
                };
                if RoutePolicy::Auto.fallback_after(HttpLegVersion::Http3, replayability, false)
                    != FallbackDecision::Retry(HttpLegVersion::Http2)
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
                    .observe(origin.clone(), value, self.clock.instant())
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

struct ProcessedBody {
    frames: Vec<BodyFrame>,
    headers: HeaderBlock,
    modified: bool,
}

fn runtime_observer(chain: &ExchangeChain) -> Option<Arc<ExchangeObserver>> {
    chain
        .context()
        .extensions()
        .get::<RuntimeExchangeObserver>()
        .map(|observer| Arc::clone(&observer.0))
}

fn hook_failure_kind(error: &ChainExecutionError) -> ExchangeFailureKind {
    match error.source {
        HookExecutionError::TimedOut => ExchangeFailureKind::HookTimedOut,
        HookExecutionError::Panicked => ExchangeFailureKind::HookPanicked,
        HookExecutionError::Cancelled => ExchangeFailureKind::Cancelled,
        HookExecutionError::ShuttingDown => ExchangeFailureKind::Shutdown,
    }
}

fn runtime_failure_kind(error: &ProxyRuntimeError, stage: ExchangeStage) -> ExchangeFailureKind {
    match error {
        ProxyRuntimeError::HookExecution(error) => hook_failure_kind(error),
        ProxyRuntimeError::BodyPipeline(BodyPipelineError::Planning(source)) => {
            hook_failure_kind(source)
        }
        ProxyRuntimeError::BodyPipeline(BodyPipelineError::Execution(source)) => match source {
            HookExecutionError::TimedOut => ExchangeFailureKind::HookTimedOut,
            HookExecutionError::Panicked => ExchangeFailureKind::HookPanicked,
            HookExecutionError::Cancelled => ExchangeFailureKind::Cancelled,
            HookExecutionError::ShuttingDown => ExchangeFailureKind::Shutdown,
        },
        ProxyRuntimeError::BodyPipeline(BodyPipelineError::Hook(BodyHookError::Abort(reason))) => {
            ExchangeFailureKind::HookAborted(reason.clone())
        }
        ProxyRuntimeError::ContentPipeline(error) => content_pipeline_failure_kind(error),
        ProxyRuntimeError::Route(_) | ProxyRuntimeError::UnsupportedNetworkPlan(_) => {
            ExchangeFailureKind::Route
        }
        ProxyRuntimeError::Translation(_) => match stage {
            ExchangeStage::ResponseHead => ExchangeFailureKind::ResponseTranslation,
            _ => ExchangeFailureKind::RequestTranslation,
        },
        ProxyRuntimeError::HookAborted(reason) => ExchangeFailureKind::HookAborted(reason.clone()),
        ProxyRuntimeError::ApplicationUpstream(_)
        | ProxyRuntimeError::HyperOrigin(_)
        | ProxyRuntimeError::H3Origin(_)
        | ProxyRuntimeError::Io(_)
        | ProxyRuntimeError::DownstreamTls(_)
        | ProxyRuntimeError::DownstreamTlsTimeout
        | ProxyRuntimeError::DownstreamTlsHandshake(_)
        | ProxyRuntimeError::CertificateResolver(_)
        | ProxyRuntimeError::Leaf(_)
        | ProxyRuntimeError::Identity(_) => ExchangeFailureKind::Upstream,
        ProxyRuntimeError::BodyPipeline(_)
        | ProxyRuntimeError::BodyLimit(_)
        | ProxyRuntimeError::BodyIdleTimeout => ExchangeFailureKind::Body,
        ProxyRuntimeError::HookInitialization(_) => ExchangeFailureKind::HookInitialization,
        _ => match stage {
            ExchangeStage::RequestHead => ExchangeFailureKind::RequestTranslation,
            ExchangeStage::ResponseHead => ExchangeFailureKind::ResponseTranslation,
            ExchangeStage::RequestBody | ExchangeStage::ResponseBody => ExchangeFailureKind::Body,
            ExchangeStage::Upstream => ExchangeFailureKind::Upstream,
            ExchangeStage::Initialize => ExchangeFailureKind::HookInitialization,
            ExchangeStage::Terminal => ExchangeFailureKind::Shutdown,
        },
    }
}

fn body_pipeline_failure_kind(error: &BodyPipelineError) -> ExchangeFailureKind {
    match error {
        BodyPipelineError::Planning(source) => hook_failure_kind(source),
        BodyPipelineError::Execution(source) => match source {
            HookExecutionError::TimedOut => ExchangeFailureKind::HookTimedOut,
            HookExecutionError::Panicked => ExchangeFailureKind::HookPanicked,
            HookExecutionError::Cancelled => ExchangeFailureKind::Cancelled,
            HookExecutionError::ShuttingDown => ExchangeFailureKind::Shutdown,
        },
        BodyPipelineError::Hook(BodyHookError::Abort(reason)) => {
            ExchangeFailureKind::HookAborted(reason.clone())
        }
        BodyPipelineError::Hook(_)
        | BodyPipelineError::Representation(_)
        | BodyPipelineError::InvalidSequence(_)
        | BodyPipelineError::OutputFrameLimit { .. }
        | BodyPipelineError::OutputByteLimit { .. }
        | BodyPipelineError::Input(_)
        | BodyPipelineError::OutputClosed(_) => ExchangeFailureKind::Body,
    }
}

fn content_pipeline_failure_kind(error: &ContentPipelineError) -> ExchangeFailureKind {
    match error {
        ContentPipelineError::Body(error) => body_pipeline_failure_kind(error),
        ContentPipelineError::DecodingDisabled
        | ContentPipelineError::Representation(_)
        | ContentPipelineError::Coding(_)
        | ContentPipelineError::Codec(_)
        | ContentPipelineError::Limit(_)
        | ContentPipelineError::AlreadyFinished => ExchangeFailureKind::Body,
    }
}

fn validate_resolved_leaf(
    requested: &EndpointIdentity,
    leaf: &rustymiddle_tls::IssuedLeaf,
) -> Result<(), CertificateResolverError> {
    if &leaf.identity == requested {
        Ok(())
    } else {
        Err(CertificateResolverError::IdentityMismatch)
    }
}

async fn observe_response_head(chain: &ExchangeChain, head: &ResponseHead) {
    if let Some(observer) = runtime_observer(chain) {
        observer
            .emit(ObserverEventKind::ResponseHeadFinalized(head.clone()))
            .await;
    }
}

async fn observe_body_frames(
    chain: &ExchangeChain,
    direction: BodyDirection,
    frames: &[BodyFrame],
) {
    let Some(observer) = runtime_observer(chain) else {
        return;
    };
    for frame in frames {
        if let BodyFrame::Data(data) = frame {
            observer
                .emit(ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    direction,
                    byte_count: data.len(),
                    sample: Some(data.clone()),
                    truncated: false,
                }))
                .await;
        }
    }
}

fn validate_frames(frames: &[BodyFrame], limit: usize) -> Result<(), ProxyRuntimeError> {
    let mut buffer = BoundedBodyBuffer::new(limit);
    for frame in frames {
        buffer.push(frame.clone())?;
    }
    Ok(())
}

fn apply_route_destination(effective: &mut Target, plan: &UpstreamPlan) {
    let destination = &plan.pool_key.destination;
    effective.scheme.clone_from(&destination.scheme);
    effective.host.clone_from(&destination.host);
    effective.port = destination.port;
    effective.authority =
        normalized_authority(&destination.scheme, &destination.host, destination.port);
}

fn normalized_authority(scheme: &str, host: &str, port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    if matches!((scheme, port), ("http", 80) | ("https", 443)) {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn request_replayability(request: &RequestHead) -> Replayability {
    if request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD") {
        Replayability::SafeMethod
    } else {
        Replayability::NotReplayable
    }
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

async fn stream_incoming_through_hooks(
    mut body: Incoming,
    sender: BodyStreamSender,
    mut pipeline: ContentBodyPipeline,
    chain: Arc<ExchangeChain>,
    limit: usize,
    body_idle_timeout: Duration,
) {
    let mut input = StreamingBodyTracker::new(limit);
    let mut output = StreamingBodyTracker::new(limit);
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                fail_hook_stream(
                    &sender,
                    &chain,
                    ExchangeStage::RequestBody,
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
                fail_hook_stream(&sender, &chain, ExchangeStage::RequestBody, error).await;
                return;
            }
        };
        if let Err(error) = input.accept(&canonical) {
            fail_hook_stream(&sender, &chain, ExchangeStage::RequestBody, error).await;
            return;
        }
        let frames = match pipeline.process(canonical).await {
            Ok(frames) => frames,
            Err(error) => {
                fail_hook_pipeline(&sender, &chain, ExchangeStage::RequestBody, error).await;
                return;
            }
        };
        observe_body_frames(&chain, BodyDirection::Request, &frames).await;
        if send_streaming_frames(&sender, &mut output, frames)
            .await
            .is_err()
        {
            emit_hook_failure(
                &chain,
                ExchangeStage::RequestBody,
                ExchangeFailureKind::Body,
                "upstream request-body consumer closed".to_owned(),
            )
            .await;
            return;
        }
    }
    match pipeline.finish().await {
        Ok(frames) => {
            observe_body_frames(&chain, BodyDirection::Request, &frames).await;
            if send_streaming_frames(&sender, &mut output, frames)
                .await
                .is_err()
            {
                emit_hook_failure(
                    &chain,
                    ExchangeStage::RequestBody,
                    ExchangeFailureKind::Body,
                    "upstream request-body consumer closed".to_owned(),
                )
                .await;
            }
        }
        Err(error) => {
            fail_hook_pipeline(&sender, &chain, ExchangeStage::RequestBody, error).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_response_through_hooks(
    mut body: BodyStream,
    sender: BodyStreamSender,
    mut pipeline: ContentBodyPipeline,
    chain: Arc<ExchangeChain>,
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
    loop {
        let frame = match timeout(body_idle_timeout, body.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                fail_hook_stream(
                    &sender,
                    &chain,
                    ExchangeStage::ResponseBody,
                    BodyStreamError::IdleTimeout,
                )
                .await;
                return;
            }
        };
        let canonical = match frame {
            Ok(frame) => frame,
            Err(error) => {
                fail_hook_stream(&sender, &chain, ExchangeStage::ResponseBody, error).await;
                return;
            }
        };
        if let Err(error) = input.accept(&canonical) {
            fail_hook_stream(&sender, &chain, ExchangeStage::ResponseBody, error).await;
            return;
        }
        let frames = match pipeline.process(canonical).await {
            Ok(frames) => frames,
            Err(error) => {
                fail_hook_pipeline(&sender, &chain, ExchangeStage::ResponseBody, error).await;
                return;
            }
        };
        observe_body_frames(&chain, BodyDirection::Response, &frames).await;
        if send_streaming_frames(&sender, &mut output, frames)
            .await
            .is_err()
        {
            emit_hook_failure(
                &chain,
                ExchangeStage::ResponseBody,
                ExchangeFailureKind::Body,
                "downstream response-body consumer closed".to_owned(),
            )
            .await;
            return;
        }
    }
    let frames = match pipeline.finish().await {
        Ok(frames) => frames,
        Err(error) => {
            fail_hook_pipeline(&sender, &chain, ExchangeStage::ResponseBody, error).await;
            return;
        }
    };
    observe_body_frames(&chain, BodyDirection::Response, &frames).await;
    if send_streaming_frames(&sender, &mut output, frames)
        .await
        .is_err()
    {
        emit_hook_failure(
            &chain,
            ExchangeStage::ResponseBody,
            ExchangeFailureKind::Body,
            "downstream response-body consumer closed".to_owned(),
        )
        .await;
        return;
    }
    complete_hook_streaming_exchange(
        &chain,
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
async fn complete_hook_streaming_exchange(
    chain: &ExchangeChain,
    session: &SessionMetadata,
    request_head: &RequestHead,
    response_head: &ResponseHead,
    evidence: broadcast::Sender<ExchangeEvidence>,
    attempts: Vec<RouteAttemptEvidence>,
    h3: Option<H3Telemetry>,
    trust_generation: u64,
) {
    let outcome = CompletedExchange {
        metadata: Arc::clone(chain.context().metadata()),
        request_head: request_head.clone(),
        response_head: response_head.clone(),
    };
    let report = chain.completed(outcome.clone()).await;
    for error in report.errors() {
        warn!(%error, "terminal hook cleanup failed");
    }
    if let Some(observer) = runtime_observer(chain) {
        observer.completed(outcome).await;
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

async fn fail_hook_stream(
    sender: &BodyStreamSender,
    chain: &ExchangeChain,
    stage: ExchangeStage,
    error: BodyStreamError,
) {
    let message = error.to_string();
    let _ = sender.send(Err(error)).await;
    emit_hook_failure(chain, stage, ExchangeFailureKind::Body, message).await;
}

async fn fail_hook_pipeline(
    sender: &BodyStreamSender,
    chain: &ExchangeChain,
    stage: ExchangeStage,
    error: ContentPipelineError,
) {
    let kind = content_pipeline_failure_kind(&error);
    let message = error.to_string();
    let _ = sender
        .send(Err(BodyStreamError::Failed(message.clone())))
        .await;
    emit_hook_failure(chain, stage, kind, message).await;
}

async fn emit_hook_failure(
    chain: &ExchangeChain,
    stage: ExchangeStage,
    kind: ExchangeFailureKind,
    message: String,
) {
    let failure = ExchangeFailure {
        metadata: Arc::clone(chain.context().metadata()),
        stage,
        kind,
        request_committed: stage == ExchangeStage::ResponseBody,
        response_committed: stage == ExchangeStage::ResponseBody,
        message,
    };
    let report = chain.failed(failure.clone()).await;
    for error in report.errors() {
        warn!(%error, "terminal hook cleanup failed");
    }
    if let Some(observer) = runtime_observer(chain) {
        observer.failed(failure).await;
    }
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
        ProxyRuntimeError::HookAborted(HookAbort::Rejected) => StatusCode::FORBIDDEN,
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

/// Listener, interception-hook, routing, or origin-adapter failure.
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
    /// A required per-exchange interceptor could not be created.
    #[error(transparent)]
    HookInitialization(#[from] ChainInitError),
    /// A hook timed out, was cancelled, panicked, or shut down.
    #[error(transparent)]
    HookExecution(#[from] ChainExecutionError),
    /// A composed body hook or plan failed.
    #[error(transparent)]
    BodyPipeline(#[from] BodyPipelineError),
    /// Content decoding, encoding, policy, or whole-pipeline accounting failed.
    #[error(transparent)]
    ContentPipeline(#[from] ContentPipelineError),
    /// Route selection or destination authorization failed.
    #[error(transparent)]
    Route(#[from] RouteError),
    /// An application-owned upstream service failed.
    #[error(transparent)]
    ApplicationUpstream(#[from] UpstreamError),
    /// A custom route plan cannot be executed by the default network adapter.
    #[error("unsupported network upstream plan: {0}")]
    UnsupportedNetworkPlan(String),
    /// Hooks v2 interceptor intentionally aborted the exchange.
    #[error("hook aborted exchange: {0:?}")]
    HookAborted(HookAbort),
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
    /// Downstream certificate provider failed or returned mismatched material.
    #[error(transparent)]
    CertificateResolver(#[from] CertificateResolverError),
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

impl From<HookAbort> for ProxyRuntimeError {
    fn from(reason: HookAbort) -> Self {
        Self::HookAborted(reason)
    }
}

#[cfg(test)]
#[path = "content_matrix_tests.rs"]
mod content_matrix_tests;

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex as StdMutex},
        time::Instant,
    };

    use boring::{
        rand::rand_bytes,
        ssl::{SslContext, SslMethod},
    };
    use quiche::h3::NameValue;
    use rustymiddle_content::{ContentCoding, ContentDecoder, ContentEncoder, ContentLimits};
    use rustymiddle_core::intercept::{
        BodyFilter, BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, BufferedBody,
        ExchangeInterceptor, HookInitError, RequestBodyAction, RequestBodyEvent, RequestHeadAction,
        RequestHeadEvent, ResponseBodyAction, ResponseBodyEvent, ResponseHeadAction,
        ResponseHeadEvent,
    };
    use rustymiddle_h3::H3UpstreamService;
    use rustymiddle_http::HyperUpstreamService;
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

    #[derive(Clone, Copy)]
    struct EditingHandler;

    #[derive(Clone, Copy)]
    struct LocalResponseHandler;

    #[derive(Clone, Default)]
    struct RejectingLifecycleHandler {
        phases: Arc<StdMutex<Vec<&'static str>>>,
    }

    #[derive(Clone, Default)]
    struct PausingHandler {
        slow_entered: Arc<Notify>,
        fast_entered: Arc<Notify>,
        release_slow: Arc<Notify>,
    }

    #[derive(Clone, Copy)]
    struct ReroutingHandler;

    struct AllowAllDestinations;

    struct EmbeddedUpstream {
        observed: Arc<StdMutex<Vec<(String, String)>>>,
    }

    struct CompressedUpstream {
        observed: Arc<StdMutex<Option<ObservedCompressedRequest>>>,
        response: Bytes,
    }

    struct ObservedCompressedRequest {
        headers: HeaderBlock,
        body: Vec<u8>,
    }

    struct ContractApplicationUpstream;

    #[derive(Clone, Copy)]
    struct DecodedPrefixHandler;

    #[derive(Default)]
    struct PrefixFirstData {
        prefixed: bool,
    }

    struct LifecycleObserver {
        phases: Arc<StdMutex<Vec<&'static str>>>,
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

    impl InterceptorFactory for EditingHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(*self))
        }
    }

    impl ExchangeInterceptor for EditingHandler {
        fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
            Box::pin(async move {
                let mut head = event.head;
                head.headers
                    .push(HeaderField::try_new("x-from-breakpoint", "yes").unwrap());
                RequestHeadAction::Replace(head)
            })
        }

        fn on_response_head(
            &self,
            event: ResponseHeadEvent,
        ) -> BoxHookFuture<'_, ResponseHeadAction> {
            Box::pin(async move {
                let mut head = event.head;
                head.headers
                    .push(HeaderField::try_new("x-intercepted", "yes").unwrap());
                ResponseHeadAction::Replace(head)
            })
        }

        fn on_request_body(
            &self,
            _event: RequestBodyEvent,
        ) -> BoxHookFuture<'_, RequestBodyAction> {
            Box::pin(async {
                RequestBodyAction::decoded(BodyPlan::Replace(
                    BufferedBody::try_new(64, Bytes::from_static(b"request-edited"), None).unwrap(),
                ))
            })
        }

        fn on_response_body(
            &self,
            _event: ResponseBodyEvent,
        ) -> BoxHookFuture<'_, ResponseBodyAction> {
            Box::pin(async {
                ResponseBodyAction::decoded(BodyPlan::Replace(
                    BufferedBody::try_new(64, Bytes::from_static(b"edited"), None).unwrap(),
                ))
            })
        }
    }

    impl InterceptorFactory for DecodedPrefixHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(*self))
        }
    }

    impl ExchangeInterceptor for DecodedPrefixHandler {
        fn on_request_body(
            &self,
            _event: RequestBodyEvent,
        ) -> BoxHookFuture<'_, RequestBodyAction> {
            Box::pin(async {
                RequestBodyAction::decoded(BodyPlan::Transform(
                    Box::new(PrefixFirstData::default()),
                ))
            })
        }

        fn on_response_body(
            &self,
            _event: ResponseBodyEvent,
        ) -> BoxHookFuture<'_, ResponseBodyAction> {
            Box::pin(async {
                ResponseBodyAction::decoded(BodyPlan::Transform(Box::new(
                    PrefixFirstData::default(),
                )))
            })
        }
    }

    impl BodyFilter for PrefixFirstData {
        fn on_frame(
            &mut self,
            frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            Box::pin(async move {
                match frame {
                    BodyFrame::Data(data) if !data.is_empty() && !self.prefixed => {
                        self.prefixed = true;
                        let mut output = Vec::with_capacity(data.len().saturating_add(1));
                        output.push(b'x');
                        output.extend_from_slice(&data);
                        Ok(vec![BodyFrame::Data(Bytes::from(output))])
                    }
                    frame => Ok(vec![frame]),
                }
            })
        }
    }

    impl InterceptorFactory for RejectingLifecycleHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(self.clone()))
        }
    }

    impl ExchangeInterceptor for RejectingLifecycleHandler {
        fn on_request_head(
            &self,
            _event: RequestHeadEvent,
        ) -> BoxHookFuture<'_, RequestHeadAction> {
            self.phases.lock().unwrap().push("request");
            Box::pin(async { RequestHeadAction::Abort(HookAbort::Rejected) })
        }

        fn on_failed(&self, _failure: ExchangeFailure) -> BoxHookFuture<'_, ()> {
            self.phases.lock().unwrap().push("failed");
            Box::pin(async {})
        }
    }

    impl InterceptorFactory for LocalResponseHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(*self))
        }
    }

    impl ExchangeInterceptor for LocalResponseHandler {
        fn on_request_head(
            &self,
            _event: RequestHeadEvent,
        ) -> BoxHookFuture<'_, RequestHeadAction> {
            Box::pin(async {
                RequestHeadAction::Respond(CanonicalResponse::local(
                    202,
                    HeaderBlock::from_fields(vec![
                        HeaderField::try_new("x-local-response", "yes")
                            .expect("static test header is valid"),
                    ]),
                    Bytes::from_static(b"synthetic"),
                ))
            })
        }
    }

    impl InterceptorFactory for PausingHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(self.clone()))
        }
    }

    impl ExchangeInterceptor for PausingHandler {
        fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
            Box::pin(async move {
                match event.head.target.path.as_str() {
                    "/slow" => {
                        self.slow_entered.notify_one();
                        self.release_slow.notified().await;
                    }
                    "/fast" => self.fast_entered.notify_one(),
                    _ => {}
                }
                RequestHeadAction::Continue
            })
        }
    }

    impl InterceptorFactory for ReroutingHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(*self))
        }
    }

    impl ExchangeInterceptor for ReroutingHandler {
        fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
            Box::pin(async move {
                let mut target = event.head.target.clone();
                target.scheme = "http".to_owned();
                target.authority = "application.internal:8080".to_owned();
                target.host = "application.internal".to_owned();
                target.port = 8080;
                RequestHeadAction::Reroute {
                    head: event.head,
                    target,
                }
            })
        }
    }

    impl rustymiddle_core::route::DestinationAuthorizer for AllowAllDestinations {
        fn authorize(
            &self,
            _original: &rustymiddle_core::intercept::OriginalTarget,
            _proposed: &rustymiddle_core::route::UpstreamDestination,
        ) -> Result<(), RouteError> {
            Ok(())
        }
    }

    impl UpstreamService for EmbeddedUpstream {
        fn execute(
            &self,
            mut request: StreamingRequest,
            plan: UpstreamPlan,
            _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
        ) -> rustymiddle_core::upstream::BoxUpstreamFuture<'_> {
            let observed = Arc::clone(&self.observed);
            Box::pin(async move {
                while let Some(frame) = request.body.recv().await {
                    frame.map_err(|error| UpstreamError::application(error.to_string()))?;
                }
                observed.lock().unwrap().push((
                    request.head.target.host.clone(),
                    request.head.target.path.clone(),
                ));
                assert_eq!(plan.pool_key.destination.host, "application.internal");
                let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
                tokio::spawn(async move {
                    sender
                        .send(Ok(BodyFrame::Data(Bytes::from_static(b"embedded"))))
                        .await
                        .unwrap();
                });
                Ok(rustymiddle_core::StreamingResponse {
                    head: ResponseHead {
                        status: 200,
                        headers: HeaderBlock::new(),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                })
            })
        }
    }

    impl UpstreamService for CompressedUpstream {
        fn execute(
            &self,
            mut request: StreamingRequest,
            _plan: UpstreamPlan,
            _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
        ) -> rustymiddle_core::upstream::BoxUpstreamFuture<'_> {
            let observed = Arc::clone(&self.observed);
            let response = self.response.clone();
            Box::pin(async move {
                let mut request_data = Vec::new();
                while let Some(frame) = request.body.recv().await {
                    match frame.map_err(|error| UpstreamError::application(error.to_string()))? {
                        BodyFrame::Data(data) => request_data.extend_from_slice(&data),
                        BodyFrame::Trailers(_) => {}
                    }
                }
                *observed.lock().unwrap() = Some(ObservedCompressedRequest {
                    headers: request.head.headers,
                    body: request_data,
                });

                let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
                tokio::spawn(async move {
                    sender.send(Ok(BodyFrame::Data(response))).await.unwrap();
                });
                Ok(rustymiddle_core::StreamingResponse {
                    head: ResponseHead {
                        status: 200,
                        headers: HeaderBlock::from_fields(vec![
                            HeaderField::try_new("content-encoding", "br").unwrap(),
                            HeaderField::try_new("content-length", "999").unwrap(),
                            HeaderField::try_new("etag", "\"stale\"").unwrap(),
                        ]),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                })
            })
        }
    }

    impl UpstreamService for ContractApplicationUpstream {
        fn execute(
            &self,
            mut request: StreamingRequest,
            plan: UpstreamPlan,
            _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
        ) -> rustymiddle_core::upstream::BoxUpstreamFuture<'_> {
            Box::pin(async move {
                assert_eq!(request.head.target.scheme, plan.pool_key.destination.scheme);
                assert_eq!(request.head.target.host, plan.pool_key.destination.host);
                assert_eq!(request.head.target.port, plan.pool_key.destination.port);
                let mut request_data = Vec::new();
                while let Some(frame) = request.body.recv().await {
                    match frame.map_err(|error| UpstreamError::application(error.to_string()))? {
                        BodyFrame::Data(data) => request_data.extend_from_slice(&data),
                        BodyFrame::Trailers(_) => {}
                    }
                }
                assert_eq!(request_data, b"contract-request");
                let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
                tokio::spawn(async move {
                    sender
                        .send(Ok(BodyFrame::Data(Bytes::from_static(b"contract-origin"))))
                        .await
                        .unwrap();
                });
                Ok(rustymiddle_core::StreamingResponse {
                    head: ResponseHead {
                        status: 200,
                        headers: HeaderBlock::new(),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                })
            })
        }
    }

    async fn assert_upstream_service_contract(
        service: Arc<dyn UpstreamService>,
        target: Target,
        policy: RoutePolicy,
        trust_generation: u64,
        expected_version: HttpLegVersion,
        expected_body: &'static [u8],
    ) {
        let plan = UpstreamPlan {
            pool_key: rustymiddle_core::route::UpstreamPoolKey {
                destination: rustymiddle_core::route::UpstreamDestination {
                    scheme: target.scheme.clone(),
                    host: target.host.clone(),
                    port: target.port,
                },
                version_policy: policy,
                trust_generation,
                tls_policy_id: "contract".into(),
                connector_policy_id: "direct".into(),
            },
            route_id: "shared-upstream-contract".into(),
            reason: "shared upstream contract test".into(),
            replayability: Replayability::NotReplayable,
        };
        let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let producer = tokio::spawn(async move {
            sender
                .send(Ok(BodyFrame::Data(Bytes::from_static(b"contract-request"))))
                .await
                .unwrap();
        });
        let executor = UpstreamExecutor::new(service, Duration::from_secs(2));
        let mut response = executor
            .execute(
                StreamingRequest {
                    head: RequestHead {
                        method: "POST".to_owned(),
                        target,
                        headers: HeaderBlock::from_fields(vec![
                            HeaderField::try_new("x-from-breakpoint", "yes").unwrap(),
                        ]),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                },
                plan,
                &rustymiddle_core::intercept::ExchangeCancellation::new(),
            )
            .await
            .unwrap();
        producer.await.unwrap();
        assert_eq!(response.head.status, 200);
        assert_eq!(response.head.source_version, expected_version);
        let mut response_data = Vec::new();
        while let Some(frame) = response.body.recv().await {
            match frame.unwrap() {
                BodyFrame::Data(data) => response_data.extend_from_slice(&data),
                BodyFrame::Trailers(_) => {}
            }
        }
        assert_eq!(response_data, expected_body);
    }

    impl rustymiddle_core::observe::Observer for LifecycleObserver {
        fn on_event(
            &self,
            event: rustymiddle_core::observe::ObserverEvent,
        ) -> rustymiddle_core::observe::BoxObserverFuture<'_> {
            let phase = match event.kind {
                ObserverEventKind::ExchangeStarted { .. } => "started",
                ObserverEventKind::RequestHeadFinalized(_) => "request-head",
                ObserverEventKind::RouteSelected { .. } => "route",
                ObserverEventKind::ResponseHeadFinalized(_) => "response-head",
                ObserverEventKind::BodyChunk(_) => "body",
                ObserverEventKind::Completed(_) => "completed",
                ObserverEventKind::Failed(_) => "failed",
                ObserverEventKind::HookInitializationSkipped(_) => "hook-skipped",
            };
            self.phases.lock().unwrap().push(phase);
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn custom_components_route_to_streaming_application_upstream_and_observers() {
        let trust_generation = 53;
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, trust_generation).unwrap());
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            ..ProxyConfig::default()
        };
        let hooks = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "reroute",
                Arc::new(ReroutingHandler),
                InterceptorRequirement::Required,
            )],
            config.limits.hooks,
        );
        let certificates = Arc::new(
            CachedMitmCertificateResolver::new(
                ProxyCa::generate("rustymiddle embedded test", 2).unwrap(),
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let phases = Arc::new(StdMutex::new(Vec::new()));
        let observers = ObserverHub::new(vec![(
            Arc::new(LifecycleObserver {
                phases: Arc::clone(&phases),
            }),
            rustymiddle_core::observe::ObserverConfig::default(),
        )]);
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let selector = PolicyRouteSelector::new(
            Arc::new(AllowAllDestinations),
            RoutePolicy::Http1Only,
            999,
            "application",
            "in-process",
            "embedded-test",
        );
        let components = ProxyComponents::new(hooks, certificates)
            .with_observers(observers)
            .with_route_selector(Arc::new(selector))
            .with_upstream_service(Arc::new(EmbeddedUpstream {
                observed: Arc::clone(&observed),
            }));
        let proxy = ProxyServer::bind_with_components(config, trust, components)
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
                b"GET http://original.invalid/sample?q=1 HTTP/1.1\r\nHost: original.invalid\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.windows(8).any(|window| window == b"embedded"));
        let proof = evidence.recv().await.unwrap();
        assert_eq!(proof.target_host, "application.internal");
        assert_eq!(proof.target_path, "/sample");
        assert_eq!(proof.trust_generation, 999);
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            &[("application.internal".to_owned(), "/sample".to_owned())]
        );

        timeout(Duration::from_secs(1), async {
            loop {
                if phases.lock().unwrap().contains(&"completed") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            phases.lock().unwrap().as_slice(),
            &[
                "started",
                "request-head",
                "route",
                "response-head",
                "body",
                "completed"
            ]
        );

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    async fn encode_test_content(coding: ContentCoding, input: &[u8]) -> Bytes {
        let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).unwrap();
        let mut frames = encoder
            .on_frame(BodyFrame::Data(Bytes::copy_from_slice(input)))
            .await
            .unwrap();
        frames.extend(encoder.finish().await.unwrap());
        Bytes::from(body_frame_data(frames))
    }

    async fn decode_test_content(coding: ContentCoding, input: &[u8]) -> Vec<u8> {
        let mut decoder = ContentDecoder::new(coding, ContentLimits::default()).unwrap();
        let mut frames = decoder
            .on_frame(BodyFrame::Data(Bytes::copy_from_slice(input)))
            .await
            .unwrap();
        frames.extend(decoder.finish().await.unwrap());
        body_frame_data(frames)
    }

    fn body_frame_data(frames: Vec<BodyFrame>) -> Vec<u8> {
        frames
            .into_iter()
            .filter_map(|frame| match frame {
                BodyFrame::Data(data) => Some(data),
                BodyFrame::Trailers(_) => None,
            })
            .flatten()
            .collect()
    }

    fn http1_response_body(response: &[u8]) -> Vec<u8> {
        let head_end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response contains a complete HTTP/1 head");
        let body = &response[head_end + 4..];
        let head = String::from_utf8_lossy(&response[..head_end]).to_ascii_lowercase();
        if !head.contains("transfer-encoding: chunked") {
            return body.to_vec();
        }

        let mut decoded = Vec::new();
        let mut cursor = 0;
        loop {
            let size_end = body[cursor..]
                .windows(2)
                .position(|window| window == b"\r\n")
                .map(|offset| cursor + offset)
                .expect("chunk contains a size line");
            let size_text = std::str::from_utf8(&body[cursor..size_end]).unwrap();
            let size = usize::from_str_radix(size_text.split(';').next().unwrap(), 16).unwrap();
            cursor = size_end + 2;
            if size == 0 {
                break;
            }
            decoded.extend_from_slice(&body[cursor..cursor + size]);
            cursor += size;
            assert_eq!(&body[cursor..cursor + 2], b"\r\n");
            cursor += 2;
        }
        decoded
    }

    #[tokio::test]
    async fn content_policy_decodes_hooks_and_restores_request_and_response_codings() {
        let gzip_request = encode_test_content(ContentCoding::Gzip, b"request").await;
        let brotli_response = encode_test_content(ContentCoding::Brotli, b"response").await;
        let observed = Arc::new(StdMutex::new(None));

        let trust_generation = 64;
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, trust_generation).unwrap());
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            ..ProxyConfig::default()
        };
        let hooks = InterceptorChainFactory::new(
            vec![
                InterceptorRegistration::new(
                    "reroute",
                    Arc::new(ReroutingHandler),
                    InterceptorRequirement::Required,
                ),
                InterceptorRegistration::new(
                    "decoded-prefix",
                    Arc::new(DecodedPrefixHandler),
                    InterceptorRequirement::Required,
                ),
            ],
            config.limits.hooks,
        );
        let certificates = Arc::new(
            CachedMitmCertificateResolver::new(
                ProxyCa::generate("rustymiddle content policy test", 2).unwrap(),
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let selector = PolicyRouteSelector::new(
            Arc::new(AllowAllDestinations),
            RoutePolicy::Http1Only,
            trust_generation,
            "application",
            "in-process",
            "content-policy-test",
        );
        let components = ProxyComponents::new(hooks, certificates)
            .with_content_policy(ContentPolicy::preserve_original_output(
                ContentLimits::default(),
            ))
            .with_route_selector(Arc::new(selector))
            .with_upstream_service(Arc::new(CompressedUpstream {
                observed: Arc::clone(&observed),
                response: brotli_response,
            }));
        let proxy = ProxyServer::bind_with_components(config, trust, components)
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut request = format!(
            "POST http://original.invalid/content HTTP/1.1\r\nHost: original.invalid\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nETag: \"stale\"\r\nConnection: close\r\n\r\n",
            gzip_request.len()
        )
        .into_bytes();
        request.extend_from_slice(&gzip_request);
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();

        let observed_request = observed.lock().unwrap().take().unwrap();
        let upstream_headers = &observed_request.headers;
        assert_eq!(
            upstream_headers
                .values("content-encoding")
                .collect::<Vec<_>>(),
            [b"gzip".as_slice()]
        );
        assert!(upstream_headers.values("content-length").next().is_none());
        assert!(upstream_headers.values("etag").next().is_none());
        assert_eq!(
            decode_test_content(ContentCoding::Gzip, &observed_request.body).await,
            b"xrequest"
        );

        let response_head_end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let response_head =
            String::from_utf8_lossy(&response[..response_head_end]).to_ascii_lowercase();
        assert!(response_head.contains("content-encoding: br"));
        assert!(!response_head.contains("etag:"));
        let response_body = http1_response_body(&response);
        assert_eq!(
            decode_test_content(ContentCoding::Brotli, &response_body).await,
            b"xresponse"
        );

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    async fn assert_application_upstream_contract() {
        let application_target = Target {
            scheme: "http".to_owned(),
            authority: "application.internal:8080".to_owned(),
            host: "application.internal".to_owned(),
            port: 8080,
            path: "/contract".to_owned(),
            query: None,
        };
        assert_upstream_service_contract(
            Arc::new(ContractApplicationUpstream),
            application_target,
            RoutePolicy::Http1Only,
            1,
            HttpLegVersion::Http1,
            b"contract-origin",
        )
        .await;
    }

    async fn assert_hyper_upstream_contract() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0, "contract request ended before its body");
                request.extend_from_slice(&chunk[..read]);
                if request.windows(5).any(|window| window == b"0\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(text.contains("x-from-breakpoint: yes"));
            assert!(
                request
                    .windows(b"contract-request".len())
                    .any(|window| window == b"contract-request")
            );
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\ncontract-origin",
                )
                .await
                .unwrap();
        });
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 61).unwrap());
        let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let hyper_service = HyperUpstreamService::new(
            HyperOriginClient::new(&tls).unwrap(),
            1024,
            NonZeroUsize::new(1).unwrap(),
            Duration::from_secs(1),
        );
        assert_upstream_service_contract(
            Arc::new(hyper_service),
            Target {
                scheme: "http".to_owned(),
                authority: origin_addr.to_string(),
                host: origin_addr.ip().to_string(),
                port: origin_addr.port(),
                path: "/contract".to_owned(),
                query: None,
            },
            RoutePolicy::Http1Only,
            61,
            HttpLegVersion::Http1,
            b"contract-origin",
        )
        .await;
        origin_task.await.unwrap();
    }

    async fn assert_h3_upstream_contract() {
        let origin_ca = ProxyCa::generate("rustymiddle h3 contract origin", 2).unwrap();
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
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                62,
            )
            .unwrap(),
        );
        let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let h3_service = H3UpstreamService::new(
            H3OriginClient::new(tls, H3TransportLimits::default()),
            1024,
            NonZeroUsize::new(1).unwrap(),
        );
        assert_upstream_service_contract(
            Arc::new(h3_service),
            Target {
                scheme: "https".to_owned(),
                authority: format!("localhost:{}", origin_addr.port()),
                host: "localhost".to_owned(),
                port: origin_addr.port(),
                path: "/contract".to_owned(),
                query: None,
            },
            RoutePolicy::Http3Only,
            62,
            HttpLegVersion::Http3,
            b"h3-origin",
        )
        .await;
        origin_task.finish().await;
    }

    #[tokio::test]
    async fn network_and_application_upstreams_pass_the_shared_contract() {
        assert_application_upstream_contract().await;
        assert_hyper_upstream_contract().await;
        assert_h3_upstream_contract().await;
    }

    #[tokio::test]
    async fn trust_reload_swaps_complete_pool_generation_and_rejects_reuse() {
        let initial = Arc::new(TrustSnapshot::load(&SystemTrustSource, 50).unwrap());
        let proxy = ProxyServer::bind(
            ProxyConfig::default(),
            ProxyCa::generate("rustymiddle trust reload test", 2).unwrap(),
            initial,
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
    async fn ordinary_http_flows_through_hooks_and_h1_origin() {
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
            vec!["request", "failed"]
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
                Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
            .expect("slow H2 stream never reached its hook");
        let fast = sender.send_request(
            Request::builder()
                .uri(format!("https://{authority}/fast"))
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap(),
        );
        timeout(Duration::from_millis(300), handler.fast_entered.notified())
            .await
            .expect("fast H2 stream never reached its independent hook");
        let fast_evidence = timeout(Duration::from_secs(1), evidence.recv()).await;
        let fast = timeout(Duration::from_secs(1), fast).await;
        handler.release_slow.notify_one();
        let fast_evidence = fast_evidence
            .expect("fast H2 stream stalled after its hook")
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
            Arc::new(rustymiddle_core::intercept::NoopInterceptorFactory),
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
        origin_task.finish().await;
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
        origin_task.finish().await;
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

    struct TestH3Origin {
        // Keep the UDP port bound after the fixture task sends its final
        // packets; otherwise Linux can surface an early ICMP connection error.
        socket: Arc<UdpSocket>,
        task: JoinHandle<()>,
    }

    impl TestH3Origin {
        async fn finish(self) {
            let Self { socket, task } = self;
            let result = task.await;
            drop(socket);
            result.unwrap();
        }
    }

    async fn spawn_h3_origin(
        leaf: rustymiddle_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
    ) -> (SocketAddr, TestH3Origin) {
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
        let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
        let local = socket.local_addr().unwrap();
        let task = tokio::spawn(run_h3_origin(Arc::clone(&socket), local, config));
        (local, TestH3Origin { socket, task })
    }

    async fn run_h3_origin(socket: Arc<UdpSocket>, local: SocketAddr, mut config: quiche::Config) {
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
