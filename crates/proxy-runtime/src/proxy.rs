use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    str::FromStr,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use boring::ssl::NameType;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock, Semaphore, broadcast},
    task::JoinSet,
    time::timeout,
};
use tracing::{debug, warn};
use transmog_client_identity::{ClientIdentityResolver, SystemClientIdentityResolver};
use transmog_content::{ContentBodyPipeline, ContentPipelineError, ContentPolicy};
use transmog_core::{
    BodyFrame, BodySemantics, BodyStream, BodyStreamError, BodyStreamSender, BoundedBodyBuffer,
    CanonicalRequest, CanonicalResponse, ConnectionId, FallbackDecision, HeaderBlock, HeaderField,
    HttpLegVersion, MessageKind, Replayability, RequestHead, ResponseHead, RoutePolicy, SessionId,
    SessionMetadata, StreamId, StreamingRequest, StreamingResponse, Target, TranslationOptions,
    body_semantics,
    connection_policy::{
        AllowedHttpProtocols, CallerNetworkIsolation, NetworkIsolationKey, NetworkIsolationPolicy,
        OutboundProtocolPolicy,
    },
    intercept::{
        BodyHookError, BodyPipeline, BodyPipelineError, BodyPipelineLimits, ChainExecutionError,
        ChainInitError, CompletedExchange, ExchangeCancellation, ExchangeChain, ExchangeFailure,
        ExchangeFailureKind, ExchangeMetadata, ExchangeStage, HookAbort, HookExecutionError,
        InterceptorChainFactory, InterceptorFactory, InterceptorRegistration,
        InterceptorRequirement, RequestHeadOutcome, ResponseHeadOutcome,
    },
    observe::{
        ExchangeBoundary, ExchangeObserver, ObservedBodyChunk, ObservedBodyTrailers,
        ObservedRouteAttempt, Observer, ObserverConfig, ObserverEventKind, ObserverHub,
        ObserverStats,
    },
    performance::{
        ConnectionSetupTime, Milestone, PerformanceRecorder, ProtocolObservation,
        TransportObservation, TransportOutcome,
    },
    prepare_headers,
    route::{
        OriginalDestinationOnly, PolicyRouteSelector, RouteError, RouteInput,
        RouteSelectionService, RouteSelector, UpstreamPlan,
    },
    stream::{StreamProtocol, StreamState, StreamStatus},
    upstream::{UpstreamError, UpstreamExecutor, UpstreamService},
};
use transmog_h3::{
    AltSvcCache, H3OriginClient, H3OriginError, H3Telemetry, H3TransportLimits, Origin,
};
use transmog_http::{
    ConnectAuthority, HyperEgressMode, HyperOriginClient, HyperOriginError, HyperUpgradeResponse,
};
use transmog_network::{
    HappyEyeballsConfig,
    metrics::{ConnectionMetrics, MeteredIo},
};
use transmog_tls::{
    CachedMitmCertificateResolver, CertificateResolverError, DownstreamCertificateResolver,
    DownstreamTlsContextFactory, DownstreamTlsPolicy, EndpointIdentity, LeafCacheError, ProxyCa,
    TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy, normalize_connect_identity,
};
use transmog_websocket::{
    CompressionError as WebSocketCompressionError, Direction as WebSocketDirection,
    HandshakeError as WebSocketHandshakeError, PerMessageDeflateCodec, RelayError, RelayLimits,
    RelayReport, RequestHandshake, ResponseHandshake, SessionCancellation, WebSocketHookError,
    WebSocketHookFactory, WebSocketSessionMetadata, validate_request as validate_websocket_request,
    validate_response as validate_websocket_response,
};

use crate::{
    AtomicRuntimeIdGenerator, ListenerConfigError, RuntimeClock, RuntimeIdGenerator, RuntimeIdKind,
    RuntimeLimits, SystemRuntimeClock,
    activity::{ActivityKind, ActivityLease, ActivityTracker},
};

const FAILED_EXCHANGE_BODY: &[u8] = b"Transmog exchange failed\n";
mod connect_tunnel;
mod request_cancellation;
use request_cancellation::RequestCancellation;

/// Complete configuration for one explicit proxy instance.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Optional immutable desktop network recipe; embedding callers may retain live policy controls.
    pub network_profile: Option<transmog_core::network_profile::NetworkProfileSnapshot>,
    /// Decrypt CONNECT TLS; false forwards opaque destination tunnels without a CA.
    pub decrypt_https: bool,
    /// Protocols admitted for new outbound connections; existing connections are preserved.
    pub allowed_outbound_protocols: AllowedHttpProtocols,
    /// Isolate outbound sockets by caller process/source IP and downstream H2 session.
    pub isolate_outbound_connections: bool,
    /// Permit subsequent requests to share origin connections.
    pub reuse_server_connections: bool,
    /// Listener exposure policy.
    pub listener: crate::ListenerConfig,
    /// Origin protocol selection policy.
    pub route_policy: RoutePolicy,
    /// Memory, concurrency, certificate, and callback bounds.
    pub limits: RuntimeLimits,
    /// HTTP/3 transport flow-control and timeout bounds.
    pub h3: H3TransportLimits,
    /// Shared IPv6/IPv4 connection-racing policy for origin transports.
    pub happy_eyeballs: HappyEyeballsConfig,
    /// WebSocket frame, message, idle, write, and close-handshake bounds.
    pub websocket: RelayLimits,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            network_profile: None,
            listener: crate::ListenerConfig::default(),
            isolate_outbound_connections: true,
            reuse_server_connections: true,
            decrypt_https: true,
            allowed_outbound_protocols: AllowedHttpProtocols::default(),
            route_policy: RoutePolicy::Auto,
            limits: RuntimeLimits::default(),
            h3: H3TransportLimits::default(),
            happy_eyeballs: HappyEyeballsConfig::default(),
            websocket: RelayLimits::default(),
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
    authentication:
        Option<Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>>,
    network_isolation: Arc<dyn NetworkIsolationPolicy>,
    hooks: InterceptorChainFactory,
    content: ContentPolicy,
    observers: ObserverHub,
    route_selector: Option<Arc<dyn RouteSelector>>,
    application_upstream: Option<Arc<dyn UpstreamService>>,
    websocket_hooks: WebSocketHookFactory,
    certificates: Arc<dyn DownstreamCertificateResolver>,
    clock: Arc<dyn RuntimeClock>,
    ids: Arc<dyn RuntimeIdGenerator>,
    client_identity_resolver: Arc<dyn ClientIdentityResolver>,
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
            .field("websocket_hooks", &self.websocket_hooks)
            .field("client_identity_resolver", &"installed")
            .finish_non_exhaustive()
    }
}

impl ProxyComponents {
    /// Installs a controller for verified upstream certificate requests.
    /// Caller isolation remains mandatory for these connections even if general isolation is off.
    #[must_use]
    pub fn with_client_certificate_resolver(
        mut self,
        resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
    ) -> Self {
        self.authentication = Some(resolver);
        self
    }
    /// Creates component configuration with explicit hooks and certificates.
    #[must_use]
    pub fn new(
        hooks: InterceptorChainFactory,
        certificates: Arc<dyn DownstreamCertificateResolver>,
    ) -> Self {
        Self {
            hooks,
            authentication: None,
            network_isolation: Arc::new(CallerNetworkIsolation),
            content: ContentPolicy::default(),
            observers: ObserverHub::default(),
            route_selector: None,
            application_upstream: None,
            websocket_hooks: WebSocketHookFactory::empty(),
            certificates,
            clock: Arc::new(SystemRuntimeClock),
            ids: Arc::new(AtomicRuntimeIdGenerator::new()),
            client_identity_resolver: Arc::new(SystemClientIdentityResolver),
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

    /// Adds one bounded observer without replacing caller registrations.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn Observer>, config: ObserverConfig) -> Self {
        self.observers = self.observers.with_registration(observer, config);
        self
    }

    /// Adds one Hooks v2 registration after caller-supplied interceptors.
    #[must_use]
    pub fn with_interceptor(mut self, registration: InterceptorRegistration) -> Self {
        self.hooks = self.hooks.with_registration(registration);
        self
    }

    /// Installs an embedding policy for caller/session connection partitioning.
    #[must_use]
    pub fn with_network_isolation_policy(
        mut self,
        policy: Arc<dyn NetworkIsolationPolicy>,
    ) -> Self {
        self.network_isolation = policy;
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

    /// Installs optional WebSocket message/control hooks.
    ///
    /// With the default empty factory, accepted upgrades use a byte-transparent
    /// bidirectional copy and perform no frame parsing.
    #[must_use]
    pub fn with_websocket_hooks(mut self, hooks: WebSocketHookFactory) -> Self {
        self.websocket_hooks = hooks;
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

    /// Installs a caller-process resolver for newly accepted connections.
    ///
    /// Resolution is best-effort and never rejects traffic. Applications can
    /// replace the platform resolver for embedding, testing, or stricter host
    /// privacy policy.
    #[must_use]
    pub fn with_client_identity_resolver(
        mut self,
        resolver: Arc<dyn ClientIdentityResolver>,
    ) -> Self {
        self.client_identity_resolver = resolver;
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

/// Terminal outcome of one upgraded WebSocket session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketSessionOutcome {
    /// Relay ended after a complete close handshake or transparent EOF.
    Completed(RelayReport),
    /// Relay failed with a redaction-safe typed error string.
    Failed(String),
}

/// Auditable terminal evidence for one upgraded WebSocket session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketSessionEvidence {
    /// HTTP exchange/session identifier that established the upgrade.
    pub session_id: SessionId,
    /// Redaction-safe normalized `ws` or `wss` target without a query.
    pub target: String,
    /// Relay outcome, including attributed hook effects on success.
    pub outcome: WebSocketSessionOutcome,
}

/// Bound explicit proxy ready to accept connections.
pub struct ProxyServer {
    listener: TcpListener,
    state: Arc<ProxyState>,
    connections: Arc<Semaphore>,
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.state.final_shutdown.cancel();
        if let Ok(mut tunnels) = self.state.tunnels.try_lock() {
            tunnels.abort_all();
        }
    }
}

/// Cloneable control plane for changing generation-scoped proxy state.
#[derive(Clone)]
pub struct ProxyControl {
    state: Arc<ProxyState>,
}

impl ProxyControl {
    /// Sets decryption for new CONNECT tunnels; already admitted tunnels continue unchanged.
    /// Enabling requires a usable certificate resolver supplied when the server was created.
    pub fn set_https_decryption(&self, enabled: bool) {
        self.state.decrypt_https.store(enabled, Ordering::Release);
    }
    /// Changes protocol admission for new connections, preserving existing sockets and sessions.
    pub fn set_outbound_protocols(&self, allowed: AllowedHttpProtocols) {
        self.state.outbound_protocols.set(allowed);
    }
    /// Changes origin connection reuse for subsequent requests without aborting active work.
    pub fn set_server_connection_reuse(&self, enabled: bool) {
        self.state.outbound_protocols.set_reuse_connections(enabled);
    }
    /// Changes caller isolation for subsequent exchanges, without aborting admitted work.
    pub fn set_outbound_isolation(&self, enabled: bool) {
        self.state
            .isolate_outbound
            .store(enabled, Ordering::Release);
    }
    /// Current requests, negotiations, relays and connected clients.
    pub fn activity(&self) -> crate::ProxyActivity {
        self.state.activity.snapshot()
    }
    /// Pauses admission while existing exchanges and relays remain active.
    pub fn begin_drain(&self) -> u64 {
        self.state.activity.drain()
    }
    /// Cancels the named pending drain, preserving the existing listener.
    pub fn resume_drain(&self, generation: u64) -> bool {
        self.state.activity.resume(generation)
    }
    /// Waits without a deadline for active work, or returns false after resume.
    pub async fn wait_for_drain(&self, generation: u64) -> bool {
        self.state.activity.idle(generation).await
    }
    /// Atomically commits an idle, still-current drain to final shutdown.
    pub fn finish_drain(&self, generation: u64) -> bool {
        self.state.activity.seal(generation)
    }
    /// Expedites physical cleanup for restart after admitted work has finished.
    /// Returns false while requests, handshakes or upgraded relays are draining.
    pub fn finish_connection_cleanup(&self) -> bool {
        if !self.state.activity.finalized() {
            return false;
        }
        self.state.finish_connection_cleanup.cancel();
        true
    }
    /// Closes observer admission and drains accepted records after the runtime
    /// ends, including when its serving task was explicitly aborted.
    pub async fn finish_observation(&self) {
        self.state.observers.shutdown().await;
    }
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
        let mut active = self.state.upstream.write().await;
        let mut next = UpstreamGeneration::new(
            trust,
            self.state.config.h3,
            self.state.config.happy_eyeballs,
            self.state.outbound_protocols.clone(),
            self.state.authentication.clone(),
        )?;
        if let Some(profile) = active.partitions.tls.network_profile() {
            next = next.profiled(profile.clone(), Some(&active.h3))?;
        }
        let next = Arc::new(next);
        if next.trust_generation <= active.trust_generation {
            return Err(ProxyRuntimeError::TrustGenerationNotMonotonic {
                active: active.trust_generation,
                requested: next.trust_generation,
            });
        }
        let old = std::mem::replace(&mut *active, next);
        drop(active);
        tokio::spawn(async move {
            old.h3.retire_cached_connections().await;
        });
        Ok(())
    }

    /// Prepares fresh transports while holding the generation lock until commit or rollback.
    /// # Errors
    /// Returns a TLS construction error without changing the running proxy.
    pub async fn prepare_network_profile(
        &self,
        profile: transmog_core::network_profile::NetworkProfileSnapshot,
    ) -> Result<PreparedNetworkProfile<'_>, ProxyRuntimeError> {
        let active = self.state.upstream.write().await;
        let next = Arc::new(
            active
                .as_ref()
                .clone()
                .profiled(profile, Some(&active.h3))?,
        );
        Ok(PreparedNetworkProfile { active, next })
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

/// Prepared profile replacement. Dropping it preserves the previous generation.
pub struct PreparedNetworkProfile<'a> {
    active: tokio::sync::RwLockWriteGuard<'a, Arc<UpstreamGeneration>>,
    next: Arc<UpstreamGeneration>,
}
impl PreparedNetworkProfile<'_> {
    /// Installs already validated transports; persistence should succeed before this call.
    pub fn commit(mut self) {
        let old = self.active.h3.clone();
        *self.active = self.next;
        drop(self.active);
        tokio::spawn(async move {
            old.retire_cached_connections().await;
        });
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
        config.websocket.validate()?;
        if config.limits.max_connections == 0
            || config.limits.max_request_body_bytes == 0
            || config.limits.max_buffered_request_body_bytes == 0
            || config.limits.max_response_body_bytes == 0
            || config.limits.max_local_response_body_bytes == 0
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
        let outbound_protocols = OutboundProtocolPolicy::new(config.allowed_outbound_protocols);
        outbound_protocols.set_reuse_connections(config.reuse_server_connections);
        let mut upstream = UpstreamGeneration::new(
            trust,
            config.h3,
            config.happy_eyeballs,
            outbound_protocols.clone(),
            components.authentication.clone(),
        )?;
        if let Some(profile) = &config.network_profile {
            upstream = upstream.profiled(profile.clone(), None)?;
        }
        let upstream = Arc::new(upstream);
        let listener = TcpListener::bind(config.listener.listen_addr).await?;
        let (evidence, _) = broadcast::channel(1_024);
        let (websocket_evidence, _) = broadcast::channel(1_024);
        let state = Arc::new(ProxyState {
            authentication: components.authentication,
            activity: Arc::new(ActivityTracker::default()),
            final_shutdown: ExchangeCancellation::new(),
            finish_connection_cleanup: ExchangeCancellation::new(),
            request_cleanup: Arc::default(),
            tunnels: Mutex::new(JoinSet::new()),
            hooks: components.hooks,
            content: components.content,
            observers: components.observers,
            network_isolation: components.network_isolation,
            isolate_outbound: AtomicBool::new(config.isolate_outbound_connections),
            decrypt_https: AtomicBool::new(config.decrypt_https),
            outbound_protocols,
            route_selector: components.route_selector,
            application_upstream: components.application_upstream,
            websocket_hooks: components.websocket_hooks,
            downstream_tls: DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default()),
            certificates: components.certificates,
            clock: components.clock,
            ids: components.ids,
            client_identity_resolver: components.client_identity_resolver,
            upstream: RwLock::new(upstream),
            config: config.clone(),
            evidence,
            websocket_evidence,
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

    /// Subscribes to terminal WebSocket relay reports, including attributed
    /// hook decisions for inspected sessions.
    pub fn subscribe_websocket_evidence(&self) -> broadcast::Receiver<WebSocketSessionEvidence> {
        self.state.websocket_evidence.subscribe()
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
                    if !self.state.activity.snapshot().accepting { continue; }
                    let Ok(permit) = Arc::clone(&self.connections).try_acquire_owned() else {
                        warn!(%client_addr, "downstream connection limit reached");
                        continue;
                    };
                    let proxy_addr = stream.local_addr()?;
                    let accepted_at = self.state.clock.instant();
                    let metrics = Arc::new(ConnectionMetrics::default());
                    let state = Arc::clone(&self.state);
                    let identity_resolver = Arc::clone(&state.client_identity_resolver);
                    let connection_id = ConnectionId(
                        state.ids.next_id(RuntimeIdKind::Connection)
                    );
                    tasks.spawn(async move {
                        let _permit = permit;
                        let _client = state.activity.acquire(ActivityKind::Client);
                        let identity = tokio::task::spawn_blocking(move || {
                            identity_resolver.resolve(client_addr, proxy_addr)
                        })
                        .await
                        .unwrap_or_else(|_| {
                            if client_addr.ip().is_loopback() {
                                transmog_core::ClientIdentity::LocalUnknown
                            } else {
                                transmog_core::ClientIdentity::Remote
                            }
                        });
                        if let Err(error) = serve_explicit_connection(
                            state,
                            stream,
                            ConnectionContext {
                                accepted_at,
                                outbound_pool: Arc::new(std::sync::Mutex::new(None)),
                                identity_done: Instant::now(),
                                socket_metrics: metrics,
                                exchange_count: Arc::new(AtomicU64::new(0)),
                                tls: None,
                                client_addr,
                                proxy_addr,
                                client_identity: identity,
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
        self.state.activity.drain();
        self.state.final_shutdown.cancel();
        // Admitted work has already finished before a drain is sealed. Idle
        // HTTP/2 peers can still withhold the PING ACK required by graceful
        // GOAWAY, so physical connection cleanup must always have a deadline.
        let cleanup = timeout(self.state.config.limits.shutdown_timeout, async {
            while tasks.join_next().await.is_some() {}
            let mut tunnels = self.state.tunnels.lock().await;
            while tunnels.join_next().await.is_some() {}
        });
        tokio::select! {
            _ = cleanup => {},
            () = self.state.finish_connection_cleanup.cancelled() => {},
        }
        tasks.shutdown().await;
        {
            let mut tunnels = self.state.tunnels.lock().await;
            tunnels.shutdown().await;
        }
        // Dropped HTTP/2 handlers enqueue terminal cleanup before releasing their
        // activity leases. Drain it before shutting down the observer workers.
        let mut cleanup = std::mem::take(
            &mut *self
                .state
                .request_cleanup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        while cleanup.join_next().await.is_some() {}
        self.state.observers.shutdown().await;
        Ok(())
    }
}

struct ProxyState {
    authentication:
        Option<Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>>,
    decrypt_https: AtomicBool,
    outbound_protocols: OutboundProtocolPolicy,
    network_isolation: Arc<dyn NetworkIsolationPolicy>,
    isolate_outbound: AtomicBool,
    activity: Arc<ActivityTracker>,
    final_shutdown: ExchangeCancellation,
    finish_connection_cleanup: ExchangeCancellation,
    request_cleanup: Arc<std::sync::Mutex<JoinSet<()>>>,
    tunnels: Mutex<JoinSet<()>>,
    hooks: InterceptorChainFactory,
    content: ContentPolicy,
    observers: ObserverHub,
    route_selector: Option<Arc<dyn RouteSelector>>,
    application_upstream: Option<Arc<dyn UpstreamService>>,
    websocket_hooks: WebSocketHookFactory,
    downstream_tls: DownstreamTlsContextFactory,
    certificates: Arc<dyn DownstreamCertificateResolver>,
    clock: Arc<dyn RuntimeClock>,
    ids: Arc<dyn RuntimeIdGenerator>,
    client_identity_resolver: Arc<dyn ClientIdentityResolver>,
    upstream: RwLock<Arc<UpstreamGeneration>>,
    config: ProxyConfig,
    evidence: broadcast::Sender<ExchangeEvidence>,
    websocket_evidence: broadcast::Sender<WebSocketSessionEvidence>,
}

#[derive(Clone, Debug)]
struct RuntimeExchangeObserver(Arc<ExchangeObserver>);

struct RuntimeExtendedConnect;

#[derive(Clone)]
struct UpstreamGeneration {
    pool_generation: u64,
    hyper: HyperOriginClient,
    h3: H3OriginClient,
    trust_generation: u64,
    partitions: Arc<UpstreamPartitions>,
}

struct UpstreamPartitions {
    alt_svc: Mutex<AltSvcCache>,
    protocols: OutboundProtocolPolicy,
    tls: UpstreamTlsContextFactory,
    happy: HappyEyeballsConfig,
    clients: std::sync::Mutex<HashMap<NetworkIsolationKey, Weak<UpstreamGeneration>>>,
}

type ConnectionPoolLease =
    Arc<std::sync::Mutex<Option<(u64, NetworkIsolationKey, Arc<UpstreamGeneration>)>>>;

impl UpstreamGeneration {
    fn observed(&self, performance: Option<PerformanceRecorder>) -> Self {
        Self {
            pool_generation: self.pool_generation,
            hyper: self.hyper.clone().with_performance(performance.clone()),
            h3: self.h3.clone().with_performance(performance),
            trust_generation: self.trust_generation,
            partitions: Arc::clone(&self.partitions),
        }
    }
    fn new(
        trust: Arc<TrustSnapshot>,
        h3_limits: H3TransportLimits,
        happy_eyeballs: HappyEyeballsConfig,
        protocols: OutboundProtocolPolicy,
        authentication: Option<
            Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
        >,
    ) -> Result<Self, ProxyRuntimeError> {
        h3_limits.validate().map_err(H3OriginError::from)?;
        let mut tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        if let Some(resolver) = authentication {
            tls = tls.with_client_certificate_resolver(resolver, "proxy");
        }
        let hyper =
            HyperOriginClient::with_connection_policy(&tls, happy_eyeballs, protocols.clone())?;
        let h3 = H3OriginClient::with_happy_eyeballs(tls.clone(), h3_limits, happy_eyeballs)
            .with_outbound_protocols(protocols.clone());
        Ok(Self {
            pool_generation: next_pool_generation(),
            hyper,
            h3,
            trust_generation: tls.snapshot().generation(),
            partitions: Arc::new(UpstreamPartitions {
                alt_svc: Mutex::new(AltSvcCache::new(1_024)),
                protocols,
                tls,
                happy: happy_eyeballs,
                clients: std::sync::Mutex::new(HashMap::new()),
            }),
        })
    }

    fn profiled(
        mut self,
        profile: transmog_core::network_profile::NetworkProfileSnapshot,
        previous_h3: Option<&H3OriginClient>,
    ) -> Result<Self, ProxyRuntimeError> {
        let protocols = OutboundProtocolPolicy::new(profile.settings.protocols);
        protocols.set_reuse_connections(profile.settings.reuse_connections);
        let tls = self
            .partitions
            .tls
            .clone()
            .with_network_profile(profile)
            .map_err(HyperOriginError::from)?;
        self.hyper = HyperOriginClient::with_connection_policy(
            &tls,
            self.partitions.happy,
            protocols.clone(),
        )?;
        self.h3 = previous_h3
            .unwrap_or(&self.h3)
            .clone()
            .with_tls_factory(tls.clone())
            .with_outbound_protocols(protocols.clone());
        self.pool_generation = next_pool_generation();
        self.partitions = Arc::new(UpstreamPartitions {
            alt_svc: Mutex::new(AltSvcCache::new(1_024)),
            protocols,
            tls,
            happy: self.partitions.happy,
            clients: std::sync::Mutex::new(HashMap::new()),
        });
        Ok(self)
    }

    fn isolated(
        &self,
        key: &NetworkIsolationKey,
        lease: &ConnectionPoolLease,
    ) -> Result<Arc<Self>, ProxyRuntimeError> {
        let mut held = lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((generation, previous, client)) = held.as_ref()
            && *generation == self.pool_generation
            && previous == key
        {
            return Ok(Arc::clone(client));
        }
        let selected = if key == &NetworkIsolationKey::shared() {
            Arc::new(self.clone())
        } else {
            let mut clients = self
                .partitions
                .clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Downstream connections hold leases; dead caller/session pools do not
            // accumulate when browsers repeatedly open and close sessions.
            clients.retain(|_, client| client.strong_count() > 0);
            if let Some(client) = clients.get(key).and_then(Weak::upgrade) {
                client
            } else {
                let client = Arc::new(Self {
                    pool_generation: self.pool_generation,
                    hyper: HyperOriginClient::with_connection_policy(
                        &self
                            .partitions
                            .tls
                            .clone()
                            .for_client_certificate_caller(format!("{key:?}")),
                        self.partitions.happy,
                        self.partitions.protocols.clone(),
                    )?,
                    h3: self.h3.clone().with_network_isolation(key.clone()),
                    trust_generation: self.trust_generation,
                    partitions: Arc::clone(&self.partitions),
                });
                clients.insert(key.clone(), Arc::downgrade(&client));
                client
            }
        };
        *held = Some((self.pool_generation, key.clone(), Arc::clone(&selected)));
        Ok(selected)
    }
}

fn next_pool_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone)]
struct ConnectionContext {
    outbound_pool: ConnectionPoolLease,
    accepted_at: Instant,
    identity_done: Instant,
    socket_metrics: Arc<ConnectionMetrics>,
    exchange_count: Arc<AtomicU64>,
    tls: Option<Arc<ClientTlsObservation>>,
    client_addr: SocketAddr,
    proxy_addr: SocketAddr,
    client_identity: transmog_core::ClientIdentity,
    connection_id: ConnectionId,
    tunnel: Option<TunnelContext>,
}

struct ClientTlsObservation {
    evidence: transmog_core::TlsEvidence,
    began: Instant,
    done: Instant,
    certificate_began: Instant,
    certificate_done: Instant,
    version: String,
    resumed: bool,
    cipher: Option<String>,
    alpn: Option<String>,
}

#[derive(Clone)]
struct TunnelContext {
    authority: ConnectAuthority,
    protocol: TunnelProtocol,
}

#[derive(Clone, Copy)]
enum TunnelProtocol {
    InterceptedTls,
    PlainWebSocket,
}

impl TunnelProtocol {
    fn target_scheme(self) -> &'static str {
        match self {
            Self::InterceptedTls => "https",
            Self::PlainWebSocket => "http",
        }
    }

    fn requires_websocket(self) -> bool {
        matches!(self, Self::PlainWebSocket)
    }
}

struct PrefixedIo<T> {
    prefix: Option<u8>,
    inner: T,
}

impl<T> PrefixedIo<T> {
    fn new(prefix: u8, inner: T) -> Self {
        Self {
            prefix: Some(prefix),
            inner,
        }
    }
}

impl<T> AsyncRead for PrefixedIo<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(prefix) = this.prefix.take() {
            buffer.put_slice(&[prefix]);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buffer)
    }
}

impl<T> AsyncWrite for PrefixedIo<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

async fn serve_explicit_connection(
    state: Arc<ProxyState>,
    stream: TcpStream,
    context: ConnectionContext,
) -> Result<(), ProxyRuntimeError> {
    stream.set_nodelay(true)?;
    let stream = MeteredIo::new_tcp(stream, context.socket_metrics.clone());
    let shutdown = state.final_shutdown.clone();
    let header_read_timeout = state.config.limits.header_read_timeout;
    let max_header_count = state.config.limits.max_header_count;
    let max_header_bytes = state.config.limits.max_header_bytes;
    let service = service_fn(move |request| {
        let state = Arc::clone(&state);
        let context = context.clone();
        async move {
            let client_version = request.version();
            let Some(activity) = state.activity.acquire(ActivityKind::Request) else {
                return Ok::<_, Infallible>(draining_response());
            };
            let mut cancellation = RequestCancellation::new(
                activity,
                state.final_shutdown.clone(),
                state.request_cleanup.clone(),
            );
            let mut response =
                match Box::pin(state.handle_outer_request(request, context, &mut cancellation))
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        warn!(%error, "explicit proxy request failed");
                        failed_exchange_response(&error)
                    }
                };
            response.body_mut().activity = Some(cancellation.finish());
            *response.version_mut() = client_version;
            Ok::<_, Infallible>(response)
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout)
        .max_headers(max_header_count)
        .max_buf_size(max_header_bytes)
        .preserve_header_case(true);
    let connection = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        result = &mut connection => { result?; }
        () = shutdown.cancelled() => { connection.as_mut().graceful_shutdown(); connection.await?; }
    }
    Ok(())
}

impl ProxyState {
    async fn handle_outer_request(
        self: Arc<Self>,
        mut request: Request<Incoming>,
        context: ConnectionContext,
        cancellation: &mut RequestCancellation,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        if request.method() != Method::CONNECT {
            return Box::pin(self.handle_intercepted_request(request, context, cancellation)).await;
        }
        let authority_text = request
            .uri()
            .authority()
            .map_or_else(|| request.uri().path(), http::uri::Authority::as_str);
        let authority = ConnectAuthority::from_str(authority_text)?;
        if !self.decrypt_https.load(Ordering::Acquire) {
            return self
                .handle_opaque_connect(request, context, authority)
                .await;
        }
        let upgraded = hyper::upgrade::on(&mut request);
        let state = Arc::clone(&self);
        let tunnel_client = state.activity.acquire(ActivityKind::Client);
        let mut tunnels = self.tunnels.lock().await;
        while tunnels.try_join_next().is_some() {}
        tunnels.spawn(async move {
            let _client = tunnel_client;
            match upgraded.await {
                Ok(upgraded) => {
                    if let Err(error) = state
                        .serve_intercepted_tunnel(upgraded, context, authority)
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
        const TLS_HANDSHAKE_CONTENT_TYPE: u8 = 0x16;

        let mut transport = TokioIo::new(upgraded);
        let mut first_byte = [0_u8; 1];
        {
            let read = timeout(
                self.config.limits.header_read_timeout,
                transport.read_exact(&mut first_byte),
            );
            tokio::pin!(read);
            tokio::select! { result = &mut read => { result
            .map_err(|_| ProxyRuntimeError::TunnelPrefaceTimeout)??;
            }, () = self.final_shutdown.cancelled() => { return Ok(()); } }
        }
        let transport = PrefixedIo::new(first_byte[0], transport);

        if first_byte[0] == b'G' {
            return self
                .serve_intercepted_http(
                    transport,
                    ConnectionContext {
                        tunnel: Some(TunnelContext {
                            authority,
                            protocol: TunnelProtocol::PlainWebSocket,
                        }),
                        ..context
                    },
                    false,
                )
                .await;
        }
        if first_byte[0] != TLS_HANDSHAKE_CONTENT_TYPE {
            return Err(ProxyRuntimeError::UnsupportedTunnelPreface(first_byte[0]));
        }

        let Some(handshake) = self.activity.acquire(ActivityKind::Transport) else {
            return Ok(());
        };

        let certificate_began = self.clock.instant();
        let identity = EndpointIdentity::parse(authority.host())?;
        let leaves = self
            .certificates
            .resolve_credentials(&identity, self.clock.system_time())?;
        for leaf in &leaves {
            validate_resolved_leaf(&identity, leaf)?;
        }
        let public_tls = transmog_tls::TlsCapture::new(transmog_core::TlsEvidence {
            server_name: Some(authority.host().to_owned()),
            ..transmog_core::TlsEvidence::default()
        });
        let acceptor = self
            .downstream_tls
            .acceptor_credentials_captured(&leaves, Some(&public_tls))?;
        let certificate_done = self.clock.instant();
        let tls_began = self.clock.instant();
        let tls = timeout(
            self.config.limits.tls_handshake_timeout,
            tokio_boring::accept(&acceptor, transport),
        )
        .await
        .map_err(|_| ProxyRuntimeError::DownstreamTlsTimeout)?
        .map_err(|error| ProxyRuntimeError::DownstreamTlsHandshake(error.to_string()))?;
        normalize_connect_identity(authority.host(), tls.ssl().servername(NameType::HOST_NAME))?;
        let negotiated_h2 = tls.ssl().selected_alpn_protocol() == Some(b"h2");
        public_tls.completed(tls.ssl(), true);
        public_tls.server_credential_issuers(&leaves);
        let tls_observation = Arc::new(ClientTlsObservation {
            evidence: public_tls.snapshot(),
            began: tls_began,
            done: self.clock.instant(),
            certificate_began,
            certificate_done,
            version: tls.ssl().version_str().into(),
            resumed: tls.ssl().session_reused(),
            cipher: tls
                .ssl()
                .current_cipher()
                .map(|cipher| cipher.name().into()),
            alpn: tls
                .ssl()
                .selected_alpn_protocol()
                .map(|alpn| String::from_utf8_lossy(alpn).into_owned()),
        });
        drop(handshake);
        self.serve_intercepted_http(
            tls,
            ConnectionContext {
                tls: Some(tls_observation),
                tunnel: Some(TunnelContext {
                    authority,
                    protocol: TunnelProtocol::InterceptedTls,
                }),
                ..context
            },
            negotiated_h2,
        )
        .await
    }

    async fn serve_intercepted_http<S>(
        self: Arc<Self>,
        stream: S,
        context: ConnectionContext,
        negotiated_h2: bool,
    ) -> Result<(), ProxyRuntimeError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let shutdown = self.final_shutdown.clone();
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
                let client_version = request.version();
                let Some(activity) = state.activity.acquire(ActivityKind::Request) else {
                    return Ok::<_, Infallible>(draining_response());
                };
                let mut cancellation = RequestCancellation::new(
                    activity,
                    state.final_shutdown.clone(),
                    state.request_cleanup.clone(),
                );
                let mut response = match Box::pin(state.handle_intercepted_request(
                    request,
                    context,
                    &mut cancellation,
                ))
                .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        warn!(%error, "intercepted request failed");
                        failed_exchange_response(&error)
                    }
                };
                response.body_mut().activity = Some(cancellation.finish());
                *response.version_mut() = client_version;
                Ok::<_, Infallible>(response)
            }
        });
        if negotiated_h2 {
            let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
            builder
                .timer(TokioTimer::new())
                .enable_connect_protocol()
                .max_concurrent_streams(max_h2_streams)
                .max_header_list_size(max_header_list_size)
                .keep_alive_interval(Some(body_idle_timeout))
                .keep_alive_timeout(body_idle_timeout);
            let connection = builder.serve_connection(TokioIo::new(stream), service);
            tokio::pin!(connection);
            tokio::select! { result = &mut connection => {result?;}, () = shutdown.cancelled() => {connection.as_mut().graceful_shutdown();connection.await?;} }
        } else {
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout)
                .max_headers(max_header_count)
                .max_buf_size(max_header_bytes)
                .preserve_header_case(true);
            let connection = builder
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            tokio::pin!(connection);
            tokio::select! { result = &mut connection => {result?;}, () = shutdown.cancelled() => {connection.as_mut().graceful_shutdown();connection.await?;} }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_intercepted_request(
        &self,
        mut request: Request<Incoming>,
        context: ConnectionContext,
        cancellation: &mut RequestCancellation,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let extended_websocket = request.version() == Version::HTTP_2
            && request.method() == Method::CONNECT
            && request
                .extensions()
                .get::<hyper::ext::Protocol>()
                .is_some_and(|p| p.as_str() == "websocket");
        if request.method() == Method::CONNECT && !extended_websocket {
            return Err(ProxyRuntimeError::NestedConnectUnsupported);
        }
        let ingress_version = protocol_version(request.version())?;
        let performance = PerformanceRecorder::new(self.clock.system_time(), self.clock.instant());
        performance.mark_at(Milestone::RequestHeaders, self.clock.instant());
        performance.mark_at(Milestone::ClientConnected, context.accepted_at);
        performance.mark_at(Milestone::ClientIdentityDone, context.identity_done);
        if let Some(first) = context.socket_metrics.first_read_at() {
            performance.mark_at(Milestone::ClientFirstByte, first);
        }
        if let Some(tls) = &context.tls {
            performance.mark_at(Milestone::ClientTlsBegin, tls.began);
            performance.mark_at(Milestone::ClientTlsDone, tls.done);
        }
        performance.protocol(ProtocolObservation {
            boundary: "client-request".into(),
            version: format!("{:?}", request.version()),
            reason: None,
        });
        if request.body().is_end_stream() {
            performance.mark(Milestone::ClientRequestDone);
        }
        let shared_connection = context.exchange_count.fetch_add(1, Ordering::Relaxed) > 0;
        let mut request_head = canonical_head(&request, &context, ingress_version)?;
        if context
            .tunnel
            .as_ref()
            .is_some_and(|tunnel| tunnel.protocol.requires_websocket())
            && (request_head.method != "GET"
                || !is_websocket_upgrade_candidate(&request_head.headers))
        {
            return Err(ProxyRuntimeError::PlaintextConnectRequiresWebSocket);
        }
        let original_target = request_head.target.clone();
        let mut session = SessionMetadata {
            session_id: SessionId(self.ids.next_id(RuntimeIdKind::Exchange)),
            downstream_connection_id: context.connection_id,
            stream_id: StreamId(self.ids.next_id(RuntimeIdKind::Stream)),
            client_addr: context.client_addr,
            client_identity: context.client_identity.clone(),
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
        // Start admission is atomic: a cancelled capacity wait has not exposed
        // this request, while an admitted start must receive terminal evidence.
        cancellation.start(
            Arc::clone(&observer),
            Arc::new(metadata.clone()),
            performance.clone(),
        );
        let initialized = {
            let _timing = performance.work("initialization", "Hook initialization");
            self.hooks.create_exchange(metadata)
        };
        let chain = match initialized {
            Ok(chain) => chain,
            Err(error) => {
                performance.mark(Milestone::ExchangeDone);
                observer
                    .emit(ObserverEventKind::Performance(performance.snapshot()))
                    .await;
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
        cancellation.initializing_chain = Some(chain);
        let chain = cancellation
            .initializing_chain
            .as_mut()
            .expect("initialized exchange chain");
        chain
            .context()
            .extensions()
            .insert(RuntimeExchangeObserver(Arc::clone(&observer)));
        chain.context().extensions().insert(performance);
        if extended_websocket {
            chain.context().extensions().insert(RuntimeExtendedConnect);
        }
        chain
            .context()
            .extensions()
            .insert(ClientPerformanceSource {
                context: context.clone(),
                shared: shared_connection,
            });
        publish_performance(chain, None).await;
        watch_performance(chain);
        for diagnostic in chain.initialization_diagnostics() {
            observer
                .emit(ObserverEventKind::HookInitializationSkipped(
                    diagnostic.clone(),
                ))
                .await;
        }
        observe_request_head(chain, ExchangeBoundary::ClientRequest, &request_head).await;
        let request_outcome = chain.request_head(request_head).await;
        observe_hook_effects(chain).await;
        let request_outcome = match request_outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.fail_chain(
                    chain,
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
                let chain = cancellation.share_chain();
                return self
                    .finish_local_response(response, &session, &effective_request, chain)
                    .await;
            }
            RequestHeadOutcome::RespondStreaming {
                request_head: effective_request,
                response,
            } => {
                observer
                    .emit(ObserverEventKind::RequestHeadFinalized(
                        effective_request.clone(),
                    ))
                    .await;
                let chain = cancellation.share_chain();
                return self
                    .finish_local_streaming_response(response, &session, &effective_request, chain)
                    .await;
            }
            RequestHeadOutcome::Abort(reason) => {
                self.fail_chain(
                    chain,
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
        let chain = cancellation.share_chain();

        let isolation_key = if self.isolate_outbound.load(Ordering::Acquire) {
            let computed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.network_isolation.compute(chain.context().metadata())
            }));
            let Ok(key) = computed else {
                let error = ProxyRuntimeError::IsolationPolicyPanicked;
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, &error, false, false)
                    .await;
                return Err(error);
            };
            key
        } else {
            NetworkIsolationKey::shared()
        };
        let isolation_key = if self.authentication.is_some() {
            NetworkIsolationKey::new(format!(
                "{isolation_key:?}/certificate:{:?}/connection:{}",
                NetworkIsolationKey::for_caller(chain.context().metadata()),
                chain.context().metadata().downstream_connection_id.0,
            ))
        } else {
            isolation_key
        };
        let selected = self
            .upstream
            .read()
            .await
            .isolated(&isolation_key, &context.outbound_pool);
        let upstream = match selected {
            Ok(selected) => Arc::new(selected.observed(performance_recorder(&chain))),
            Err(error) => {
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, &error, false, false)
                    .await;
                return Err(error);
            }
        };
        let replayability = request_replayability(&request_head);
        publish_performance(&chain, Some(Milestone::RouteBegin)).await;
        let mut plan = match self
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
        plan.pool_key.network_isolation = isolation_key;
        let allowed = upstream.partitions.protocols.snapshot();
        if plan.pool_key.version_policy == RoutePolicy::Auto
            && allowed.http3
            && !allowed.http1
            && !allowed.http2
        {
            plan.pool_key.version_policy = RoutePolicy::Http3Only;
        }
        observer
            .emit(ObserverEventKind::RouteSelected {
                policy_id: Arc::clone(&plan.route_id),
                reason: Arc::clone(&plan.reason),
            })
            .await;
        publish_performance(&chain, Some(Milestone::RouteDone)).await;
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

        if extended_websocket || is_websocket_upgrade_candidate(&request_head.headers) {
            if self.application_upstream.is_some() {
                let error = ProxyRuntimeError::WebSocketApplicationUpstreamUnsupported;
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, &error, false, false)
                    .await;
                return Err(error);
            }
            if !matches!(
                plan.pool_key.version_policy,
                RoutePolicy::Http1Only | RoutePolicy::Http2Only | RoutePolicy::Auto
            ) {
                let error = ProxyRuntimeError::WebSocketRequiresHttp1Or2;
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, &error, false, false)
                    .await;
                return Err(error);
            }
            let result = self
                .handle_websocket_upgrade(
                    request,
                    session,
                    request_head,
                    upstream,
                    chain.clone(),
                    plan.pool_key.version_policy,
                )
                .await;
            if let Err(error) = &result {
                self.fail_runtime_exchange(&chain, ExchangeStage::Upstream, error, false, false)
                    .await;
            }
            return result;
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
            .streaming_hyper_mode(
                &upstream,
                &request_head,
                plan.pool_key.version_policy,
                request.body().size_hint().upper().is_none_or(|bytes| {
                    bytes > self.config.limits.max_buffered_request_body_bytes as u64
                }),
            )
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
            self.config
                .limits
                .max_request_body_bytes
                .min(self.config.limits.max_buffered_request_body_bytes),
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
        publish_performance(&chain, Some(Milestone::ClientRequestDone)).await;
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
        observe_request_head(&chain, ExchangeBoundary::UpstreamRequest, &request_head).await;
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
        observe_route_attempts(&chain, &routed.attempts).await;
        observe_response_head(&chain, ExchangeBoundary::UpstreamResponse, &response.head).await;
        observe_complete_body(&chain, ExchangeBoundary::UpstreamResponse, &response.body).await;
        let response_outcome = chain
            .response_head(&request_head, response.head, Some(response.body), false)
            .await;
        observe_hook_effects(&chain).await;
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

        observe_response_head(&chain, ExchangeBoundary::ClientResponse, &response.head).await;

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

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn handle_websocket_upgrade(
        &self,
        mut request: Request<Incoming>,
        mut session: SessionMetadata,
        request_head: RequestHead,
        upstream_generation: Arc<UpstreamGeneration>,
        chain: Arc<ExchangeChain>,
        route: RoutePolicy,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let extended = request.version() == Version::HTTP_2;
        if !matches!(
            request_head.source_version,
            HttpLegVersion::Http1 | HttpLegVersion::Http2
        ) {
            return Err(ProxyRuntimeError::WebSocketRequiresHttp1Or2);
        }
        if websocket_request_has_body(&request_head.headers)? {
            return Err(ProxyRuntimeError::InvalidWebSocketHandshake(
                "opening handshake must not declare an HTTP body".to_owned(),
            ));
        }
        let client_handshake = websocket_handshake_request(&request_head, extended)?;
        // The H1 representation supplies a fresh nonce for a possible H2 -> H1
        // bridge. The origin adapter removes it for extended CONNECT.
        let mut origin_request = request_head.clone();
        if extended {
            origin_request.method = "GET".into();
            origin_request.headers.replace_all(HeaderField::try_new(
                "sec-websocket-key",
                transmog_websocket::generate_key()?,
            )?);
            origin_request
                .headers
                .replace_all(HeaderField::try_new("connection", "Upgrade")?);
            origin_request
                .headers
                .replace_all(HeaderField::try_new("upgrade", "websocket")?);
        }
        origin_request.source_version = HttpLegVersion::Http1;
        let origin_handshake = websocket_handshake_request(&origin_request, false)?;
        let mode = match route {
            RoutePolicy::Http1Only => HyperEgressMode::Http1Only,
            RoutePolicy::Http2Only => HyperEgressMode::Http2Only,
            _ => HyperEgressMode::Auto,
        };
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let upstream = upstream_generation
            .hyper
            .execute_upgrade(
                origin_request,
                self.config.limits.max_response_body_bytes,
                capacity,
                self.config.limits.body_idle_timeout,
                mode,
                self.config.limits.header_read_timeout,
            )
            .await?;
        let sent_head = match &upstream {
            HyperUpgradeResponse::Switched { request, .. }
            | HyperUpgradeResponse::Rejected(_, request) => request,
        };
        let upstream_version = sent_head.source_version;
        session.egress_version = Some(upstream_version);
        observe_request_head(&chain, ExchangeBoundary::UpstreamRequest, sent_head).await;
        let HyperUpgradeResponse::Switched {
            head: upstream_head,
            upgraded: upstream_upgrade,
            ..
        } = upstream
        else {
            let HyperUpgradeResponse::Rejected(response, _) = upstream else {
                unreachable!("upgrade response variants are exhaustive")
            };
            return self
                .finish_rejected_websocket_upgrade(
                    response,
                    session,
                    request_head,
                    upstream_generation.trust_generation,
                    chain,
                )
                .await;
        };

        observe_response_head(&chain, ExchangeBoundary::UpstreamResponse, &upstream_head).await;
        let negotiated_origin = websocket_handshake_response(
            &origin_handshake,
            &upstream_head,
            upstream_version == HttpLegVersion::Http2,
        )?;
        let client_head =
            websocket_response_for_client(upstream_head, &client_handshake, extended)?;
        let response_outcome = chain
            .response_head(&request_head, client_head, None, false)
            .await;
        observe_hook_effects(&chain).await;
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
                    format!("hook aborted WebSocket handshake: {reason:?}"),
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
        if replacement_body.is_some()
            || local_response
            || !websocket_success(response_head.status, extended)
        {
            drop(upstream_upgrade);
            let mut response_head = response_head;
            if websocket_success(response_head.status, extended) {
                response_head.status = 502;
            }
            return self
                .finish_prepared_local_response(
                    CanonicalResponse {
                        head: response_head,
                        body: replacement_body.unwrap_or_default(),
                    },
                    &session,
                    &request_head,
                    chain,
                    true,
                )
                .await;
        }

        let negotiated = websocket_handshake_response(&client_handshake, &response_head, extended)?;
        if negotiated != negotiated_origin {
            return Err(ProxyRuntimeError::InvalidWebSocketHandshake(
                "response hooks changed the negotiated WebSocket protocol or compression".into(),
            ));
        }
        if !self.websocket_hooks.is_empty()
            && let Some(compression) = negotiated.permessage_deflate
        {
            PerMessageDeflateCodec::new(compression, WebSocketDirection::ClientToServer)?;
            PerMessageDeflateCodec::new(compression, WebSocketDirection::ServerToClient)?;
        }
        let downstream_upgrade = hyper::upgrade::on(&mut request);
        let cancellation = SessionCancellation::new();
        let websocket_target = format!(
            "{}://{}{}",
            if request_head.target.scheme == "https" {
                "wss"
            } else {
                "ws"
            },
            request_head.target.authority,
            request_head.target.path
        );
        let websocket_chain = if self.websocket_hooks.is_empty() {
            None
        } else {
            Some(Arc::new(self.websocket_hooks.create_session(
                WebSocketSessionMetadata {
                    session_id: session.session_id.0,
                    target: Arc::from(websocket_target.clone()),
                    subprotocol: negotiated.subprotocol.clone().map(Arc::from),
                },
                cancellation.clone(),
            )?))
        };
        let downstream_response = response_to_hyper(CanonicalResponse {
            head: response_head.clone(),
            body: Vec::new(),
        })?;
        observe_response_head(&chain, ExchangeBoundary::ClientResponse, &response_head).await;
        let stream_observer = (!self.observers.is_empty())
            .then(|| runtime_observer(&chain))
            .flatten();
        if let Some(observer) = &stream_observer {
            observer
                .emit(ObserverEventKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Connecting,
                    None,
                )))
                .await;
        }
        self.complete_chain(&chain, &request_head, &response_head)
            .await;
        let attempts = vec![RouteAttemptEvidence {
            protocol: upstream_version,
            outcome: "websocket-upgrade".to_owned(),
        }];
        observe_route_attempts(&chain, &attempts).await;
        let _ = self.evidence.send(ExchangeEvidence {
            session_id: session.session_id,
            downstream_connection_id: session.downstream_connection_id,
            stream_id: session.stream_id,
            target_host: request_head.target.host.clone(),
            target_scheme: request_head.target.scheme.clone(),
            target_path: request_head.target.path.clone(),
            ingress_version: session.ingress_version,
            egress_version: upstream_version,
            request_breakpoint_fired: true,
            response_breakpoint_fired: true,
            route_attempts: attempts,
            h3: None,
            trust_generation: upstream_generation.trust_generation,
        });

        let upgrade_timeout = self.config.limits.header_read_timeout;
        let relay_limits = self.config.websocket;
        let relay_activity = self.activity.continue_websocket();
        let websocket_evidence = self.websocket_evidence.clone();
        let websocket_session_id = session.session_id;
        let mut tunnels = self.tunnels.lock().await;
        while tunnels.try_join_next().is_some() {}
        let shutdown = self.final_shutdown.clone();
        tunnels.spawn(async move {
            let _relay_activity = relay_activity;
            let relay = async {
                let (downstream, upstream) = tokio::join!(
                    timeout(upgrade_timeout, downstream_upgrade),
                    timeout(upgrade_timeout, upstream_upgrade),
                );
                let downstream = downstream
                    .map_err(|_| "downstream upgrade timed out".to_owned())?
                    .map_err(|error| format!("downstream upgrade failed: {error}"))?;
                let upstream = upstream
                    .map_err(|_| "upstream upgrade timed out".to_owned())?
                    .map_err(|error| format!("upstream upgrade failed: {error}"))?;
                if let Some(observer) = &stream_observer {
                    observer
                        .emit(ObserverEventKind::StreamState(StreamState::now(
                            StreamProtocol::WebSocket,
                            StreamStatus::Open,
                            None,
                        )))
                        .await;
                }
                let result = if let Some(hooks) = websocket_chain {
                    transmog_websocket::relay_inspected_observed(
                        TokioIo::new(downstream),
                        upstream,
                        negotiated,
                        relay_limits,
                        hooks,
                        cancellation,
                        stream_observer.clone().map(|observer| {
                            Arc::new(crate::streams::WebSocketObserver(observer))
                                as Arc<dyn transmog_websocket::RelayObserver>
                        }),
                    )
                    .await
                } else if let Some(observer) = stream_observer.clone() {
                    transmog_websocket::relay_observed(
                        TokioIo::new(downstream),
                        upstream,
                        negotiated,
                        relay_limits,
                        cancellation,
                        Arc::new(crate::streams::WebSocketObserver(observer)),
                    )
                    .await
                } else {
                    transmog_websocket::relay_transparent(
                        TokioIo::new(downstream),
                        upstream,
                        relay_limits,
                        cancellation,
                    )
                    .await
                };
                result.map_err(|error| error.to_string())
            };
            let (result, interrupted) = tokio::select! {
                result = relay => (result, false),
                () = shutdown.cancelled() => (Err("Application shutdown".to_owned()), true),
            };
            if let Some(observer) = &stream_observer {
                let status = if interrupted {
                    StreamStatus::Interrupted
                } else if result.is_ok() {
                    StreamStatus::Closed
                } else {
                    StreamStatus::Failed
                };
                observer
                    .emit(ObserverEventKind::StreamState(StreamState::now(
                        StreamProtocol::WebSocket,
                        status,
                        result.as_ref().err().cloned(),
                    )))
                    .await;
            }
            let outcome = match result {
                Ok(report) => WebSocketSessionOutcome::Completed(report),
                Err(error) => WebSocketSessionOutcome::Failed(error),
            };
            let _ = websocket_evidence.send(WebSocketSessionEvidence {
                session_id: websocket_session_id,
                target: websocket_target,
                outcome,
            });
        });
        Ok(downstream_response)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn finish_rejected_websocket_upgrade(
        &self,
        mut upstream: StreamingResponse,
        session: SessionMetadata,
        request_head: RequestHead,
        trust_generation: u64,
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        observe_response_head(&chain, ExchangeBoundary::UpstreamResponse, &upstream.head).await;
        let upstream_version = upstream.head.source_version;
        // A 2xx decline from H1 would otherwise open an H2 CONNECT tunnel.
        if session.ingress_version == HttpLegVersion::Http2
            && (200..300).contains(&upstream.head.status)
        {
            upstream.head.status = 502;
        }
        let response_outcome = chain
            .response_head(&request_head, upstream.head, None, false)
            .await?;
        observe_hook_effects(&chain).await;
        let ResponseHeadOutcome::Continue {
            mut head,
            replacement_body,
            local_response,
        } = response_outcome
        else {
            let ResponseHeadOutcome::Abort(reason) = response_outcome else {
                unreachable!("response outcomes are exhaustive")
            };
            return Err(reason.into());
        };
        // Hooks may author a rejection, but cannot synthesize an accepted
        // tunnel after the origin has declined it.
        if websocket_success(
            head.status,
            session.ingress_version == HttpLegVersion::Http2,
        ) {
            head.status = 502;
        }
        upstream.head = head;
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
        if body_semantics(&request_head.method, upstream.head.status) == BodySemantics::Forbidden {
            drop(upstream.body);
            return self
                .finish_streaming_bodyless(
                    upstream.head,
                    &session,
                    &request_head,
                    vec![RouteAttemptEvidence {
                        protocol: upstream_version,
                        outcome: "websocket-rejected".to_owned(),
                    }],
                    None,
                    trust_generation,
                    chain,
                )
                .await;
        }
        let mut response_head = upstream.head;
        let response_pipeline = self
            .prepare_streaming_response_pipeline(
                &chain,
                &request_head,
                &mut response_head,
                local_response,
                session.ingress_version,
            )
            .await?;
        let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
            .ok_or(ProxyRuntimeError::InvalidConfiguration)?;
        let (downstream_sender, downstream_body) = BodyStream::channel(capacity);
        let attempts = vec![RouteAttemptEvidence {
            protocol: upstream_version,
            outcome: "websocket-rejected".to_owned(),
        }];
        tokio::spawn(stream_response_through_hooks(
            upstream.body,
            downstream_sender,
            response_pipeline,
            Arc::clone(&chain),
            session,
            request_head,
            response_head.clone(),
            self.config.limits.max_response_body_bytes,
            self.config.limits.body_idle_timeout,
            self.evidence.clone(),
            attempts,
            None,
            trust_generation,
        ));
        response_stream_to_hyper(&response_head, downstream_body)
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
                    original_target: transmog_core::intercept::OriginalTarget::new(original_target),
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
        upstream: &UpstreamGeneration,
        request: &RequestHead,
        route_policy: RoutePolicy,
        requires_streaming: bool,
    ) -> Option<HyperEgressMode> {
        match route_policy {
            RoutePolicy::Http1Only => Some(HyperEgressMode::Http1Only),
            RoutePolicy::Http2Only => Some(HyperEgressMode::Http2Only),
            RoutePolicy::Http3Only => None,
            RoutePolicy::Auto => {
                if !upstream.partitions.protocols.snapshot().http3 {
                    return Some(HyperEgressMode::Auto);
                }
                let origin = Origin {
                    host: request.target.host.clone(),
                    port: request.target.port,
                };
                let has_h3_alternative = upstream
                    .partitions
                    .alt_svc
                    .lock()
                    .await
                    .get(&origin, self.clock.instant())
                    .is_some();
                (!has_h3_alternative || requires_streaming).then_some(HyperEgressMode::Auto)
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

        observe_response_head(&chain, ExchangeBoundary::UpstreamResponse, &upstream.head).await;
        let response_outcome = chain
            .response_head(&request_head, upstream.head, None, false)
            .await;
        observe_hook_effects(&chain).await;
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
        self.learn_alt_svc_head(&upstream_generation, &origin, &upstream.head)
            .await;

        observe_response_head(&chain, ExchangeBoundary::UpstreamResponse, &upstream.head).await;
        let response_outcome = chain
            .response_head(&request_head, upstream.head, None, false)
            .await;
        observe_hook_effects(&chain).await;
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
        self.learn_alt_svc_head(&upstream_generation, &origin, &upstream.response.head)
            .await;

        observe_response_head(
            &chain,
            ExchangeBoundary::UpstreamResponse,
            &upstream.response.head,
        )
        .await;
        let response_outcome = chain
            .response_head(&request_head, upstream.response.head, None, false)
            .await;
        observe_hook_effects(&chain).await;
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
        observe_hook_effects(&chain).await;
        match outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            }) => {
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

    #[allow(clippy::too_many_lines)]
    async fn finish_local_streaming_response(
        &self,
        response: transmog_core::LocalStreamingResponse,
        session: &SessionMetadata,
        request: &RequestHead,
        chain: Arc<ExchangeChain>,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let transmog_core::LocalStreamingResponse {
            head,
            body,
            body_length: _,
        } = response;
        let outcome = chain.response_head(request, head, None, true).await;
        observe_hook_effects(&chain).await;
        match outcome {
            Ok(ResponseHeadOutcome::Continue {
                head,
                replacement_body: Some(replacement_body),
                local_response,
            }) => {
                drop(body);
                self.finish_prepared_local_response(
                    CanonicalResponse {
                        head,
                        body: replacement_body,
                    },
                    session,
                    request,
                    chain,
                    local_response,
                )
                .await
            }
            Ok(ResponseHeadOutcome::Continue {
                mut head,
                replacement_body: None,
                local_response,
            }) => {
                reject_unbacked_extended_connect(&chain, &mut head);
                if body_semantics(&request.method, head.status) == BodySemantics::Forbidden {
                    drop(body);
                    return self
                        .finish_prepared_local_response(
                            CanonicalResponse {
                                head,
                                body: Vec::new(),
                            },
                            session,
                            request,
                            chain,
                            local_response,
                        )
                        .await;
                }
                let pipeline = self
                    .prepare_streaming_response_pipeline(
                        &chain,
                        request,
                        &mut head,
                        local_response,
                        session.ingress_version,
                    )
                    .await?;
                let capacity = NonZeroUsize::new(self.config.limits.body_channel_capacity)
                    .expect("validated body channel capacity is nonzero");
                let (sender, downstream_body) = BodyStream::channel(capacity);
                let response_head = head.clone();
                let request_head = request.clone();
                let body_limit = self.config.limits.max_local_response_body_bytes;
                let body_idle_timeout = self.config.limits.body_idle_timeout;
                tokio::spawn(async move {
                    stream_local_response_through_hooks(
                        body,
                        sender,
                        pipeline,
                        chain,
                        request_head,
                        response_head,
                        body_limit,
                        body_idle_timeout,
                    )
                    .await;
                });
                response_stream_to_hyper(&head, downstream_body)
            }
            Ok(ResponseHeadOutcome::Abort(reason)) => {
                drop(body);
                self.fail_chain(
                    &chain,
                    ExchangeStage::ResponseHead,
                    ExchangeFailureKind::HookAborted(reason.clone()),
                    format!("hook aborted streaming local response: {reason:?}"),
                    false,
                    false,
                )
                .await;
                Err(reason.into())
            }
            Err(error) => {
                drop(body);
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
        reject_unbacked_extended_connect(&chain, &mut response.head);
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
            observe_response_head(&chain, ExchangeBoundary::ClientResponse, &response.head).await;
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
        observe_response_head(&chain, ExchangeBoundary::ClientResponse, &response.head).await;
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
        observe_response_head(&chain, ExchangeBoundary::ClientResponse, &response_head).await;
        observe_route_attempts(&chain, &route_attempts).await;
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
        observe_complete_body(chain, ExchangeBoundary::ClientResponse, &[]).await;
        observe_hook_effects(chain).await;
        let outcome = CompletedExchange {
            metadata: Arc::clone(chain.context().metadata()),
            request_head: request.clone(),
            response_head: response.clone(),
        };
        publish_performance(chain, Some(Milestone::ClientResponseQueued)).await;
        publish_performance(chain, Some(Milestone::ExchangeDone)).await;
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
        observe_hook_effects(chain).await;
        let failure = ExchangeFailure {
            metadata: Arc::clone(chain.context().metadata()),
            stage,
            kind,
            request_committed,
            response_committed,
            message,
        };
        publish_performance(chain, Some(Milestone::ExchangeDone)).await;
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
        // The buffered ingress body is complete even if creating its hook pipeline fails.
        observe_complete_body(chain, ExchangeBoundary::ClientRequest, &frames).await;
        let pipeline = chain
            .request_body_pipeline(request_head, self.body_pipeline_limits())
            .await?;
        observe_hook_effects(chain).await;
        self.process_body_pipeline(
            chain,
            ExchangeBoundary::UpstreamRequest,
            pipeline,
            &request_head.headers,
            frames,
            self.config
                .limits
                .max_request_body_bytes
                .min(self.config.limits.max_buffered_request_body_bytes),
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
        observe_hook_effects(chain).await;
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
        observe_request_head(chain, ExchangeBoundary::UpstreamRequest, request_head).await;
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
        observe_hook_effects(chain).await;
        Ok(ContentBodyPipeline::from_policy(
            pipeline,
            &response_head.headers,
            if transmog_core::stream::is_event_stream(&response_head.headers) {
                self.content.for_event_stream()
            } else {
                self.content
            },
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
        observe_response_head(chain, ExchangeBoundary::ClientResponse, response_head).await;
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
        observe_hook_effects(chain).await;
        self.process_body_pipeline(
            chain,
            ExchangeBoundary::ClientResponse,
            pipeline,
            &response_head.headers,
            frames,
            self.config.limits.max_response_body_bytes,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_body_pipeline(
        &self,
        chain: &ExchangeChain,
        output_boundary: ExchangeBoundary,
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
        observe_complete_body(chain, output_boundary, &output).await;
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
                    .max_buffered_request_body_bytes
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
        self.learn_alt_svc(upstream, &origin, &response).await;
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
        self.learn_alt_svc(upstream, &origin, &response.response)
            .await;
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
        if !upstream.partitions.protocols.snapshot().http3 {
            return self
                .route_hyper(request, HyperEgressMode::Auto, upstream)
                .await;
        }
        let origin = Origin {
            host: request.head.target.host.clone(),
            port: request.head.target.port,
        };
        let alternative = upstream
            .partitions
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
                if matches!(
                    h3_error,
                    ProxyRuntimeError::H3Origin(H3OriginError::ClientAuthenticationFailed)
                ) {
                    return Err(h3_error);
                }
                upstream
                    .partitions
                    .alt_svc
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

    async fn learn_alt_svc(
        &self,
        upstream: &UpstreamGeneration,
        origin: &Origin,
        response: &CanonicalResponse,
    ) {
        self.learn_alt_svc_head(upstream, origin, &response.head)
            .await;
    }

    async fn learn_alt_svc_head(
        &self,
        upstream: &UpstreamGeneration,
        origin: &Origin,
        response: &ResponseHead,
    ) {
        for value in response.headers.values("alt-svc") {
            let Ok(value) = std::str::from_utf8(value) else {
                continue;
            };
            if let Err(error) = upstream.partitions.alt_svc.lock().await.observe(
                origin.clone(),
                value,
                self.clock.instant(),
            ) {
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

fn performance_recorder(chain: &ExchangeChain) -> Option<PerformanceRecorder> {
    chain
        .context()
        .extensions()
        .get::<PerformanceRecorder>()
        .map(|recorder| (*recorder).clone())
}

struct ClientPerformanceSource {
    context: ConnectionContext,
    shared: bool,
}
impl ClientPerformanceSource {
    fn sample(&self, recorder: &PerformanceRecorder) {
        let read = self.context.socket_metrics.bytes_read();
        let written = self.context.socket_metrics.bytes_written();
        let tls = self.context.tls.as_deref();
        let mut observation = TransportObservation {
            leg: "client".into(),
            connection_id: format!("client-{:032x}", self.context.connection_id.0),
            tunnel_destination: self
                .context
                .tunnel
                .as_ref()
                .map(|tunnel| tunnel.authority.to_string()),
            outcome: TransportOutcome::Connected,
            peer: Some(self.context.client_addr.to_string()),
            local: Some(self.context.proxy_addr.to_string()),
            shared: self.shared,
            tls_micros: tls.map(|tls| {
                u64::try_from(tls.done.saturating_duration_since(tls.began).as_micros())
                    .unwrap_or(u64::MAX)
            }),
            tls_version: tls.map(|tls| tls.version.clone()),
            tls_resumed: tls.map(|tls| tls.resumed),
            tls: tls.map(|tls| tls.evidence.clone()),
            cipher: tls.and_then(|tls| tls.cipher.clone()),
            alpn: tls.and_then(|tls| tls.alpn.clone()),
            bytes_read: Some(read),
            bytes_written: Some(written),
            ..TransportObservation::default()
        };
        self.context
            .socket_metrics
            .observe(&mut observation, recorder);
        if let Some(tls) = tls {
            recorder.project_connection_setup(
                &mut observation,
                &[
                    ConnectionSetupTime {
                        phase: "certificate",
                        began: tls.certificate_began,
                        ended: tls.certificate_done,
                    },
                    ConnectionSetupTime {
                        phase: "tls",
                        began: tls.began,
                        ended: tls.done,
                    },
                ],
            );
        }
        recorder.transport(observation);
    }
}

async fn publish_performance(chain: &ExchangeChain, point: Option<Milestone>) {
    let Some(recorder) = performance_recorder(chain) else {
        return;
    };
    if let Some(point) = point {
        recorder.mark(point);
    }
    if let Some(source) = chain
        .context()
        .extensions()
        .get::<ClientPerformanceSource>()
    {
        source.sample(&recorder);
    }
    if let Some(observer) = runtime_observer(chain) {
        observer
            .emit(ObserverEventKind::Performance(recorder.snapshot()))
            .await;
    }
}

fn watch_performance(chain: &ExchangeChain) {
    let Some(recorder) = chain.context().extensions().get::<PerformanceRecorder>() else {
        return;
    };
    let Some(source) = chain
        .context()
        .extensions()
        .get::<ClientPerformanceSource>()
    else {
        return;
    };
    let Some(observer) = runtime_observer(chain) else {
        return;
    };
    let (recorder, source, observer) = (
        Arc::downgrade(&recorder),
        Arc::downgrade(&source),
        Arc::downgrade(&observer),
    );
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut previous = None;
        loop {
            tick.tick().await;
            let (Some(recorder), Some(source), Some(observer)) =
                (recorder.upgrade(), source.upgrade(), observer.upgrade())
            else {
                break;
            };
            if recorder.finished() {
                break;
            }
            source.sample(&recorder);
            let snapshot = recorder.snapshot();
            if previous.as_ref() != Some(&snapshot) {
                observer
                    .emit(ObserverEventKind::Performance(snapshot.clone()))
                    .await;
                previous = Some(snapshot);
            }
        }
    });
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
    leaf: &transmog_tls::IssuedLeaf,
) -> Result<(), CertificateResolverError> {
    if &leaf.identity == requested {
        Ok(())
    } else {
        Err(CertificateResolverError::IdentityMismatch)
    }
}

async fn observe_request_head(
    chain: &ExchangeChain,
    boundary: ExchangeBoundary,
    head: &RequestHead,
) {
    if boundary == ExchangeBoundary::UpstreamRequest {
        publish_performance(chain, Some(Milestone::UpstreamBegin)).await;
    }
    if let Some(observer) = runtime_observer(chain) {
        observer
            .emit(ObserverEventKind::RequestHeadObserved {
                boundary,
                head: head.clone(),
            })
            .await;
    }
}

async fn observe_response_head(
    chain: &ExchangeChain,
    boundary: ExchangeBoundary,
    head: &ResponseHead,
) {
    if let Some(recorder) = performance_recorder(chain) {
        let version = if boundary == ExchangeBoundary::ClientResponse {
            recorder
                .snapshot()
                .protocols
                .iter()
                .find(|protocol| protocol.boundary == "client-request")
                .map(|protocol| protocol.version.clone())
        } else {
            Some(
                match head.source_version {
                    HttpLegVersion::Http1 => "HTTP/1.1",
                    HttpLegVersion::Http2 => "HTTP/2",
                    HttpLegVersion::Http3 => "HTTP/3",
                }
                .into(),
            )
        };
        if let Some(version) = version {
            // Hyper's origin adapter has the exact upstream minor version and
            // reason phrase; keep it rather than replacing it with a fallback.
            let name = if boundary == ExchangeBoundary::ClientResponse {
                "client-response"
            } else {
                "upstream-response"
            };
            if !recorder
                .snapshot()
                .protocols
                .iter()
                .any(|protocol| protocol.boundary == name)
            {
                recorder.protocol(ProtocolObservation {
                    boundary: name.into(),
                    version,
                    reason: None,
                });
            }
        }
    }
    publish_performance(
        chain,
        Some(if boundary == ExchangeBoundary::UpstreamResponse {
            Milestone::ResponseHeaders
        } else {
            Milestone::ClientResponseBegin
        }),
    )
    .await;
    if let Some(observer) = runtime_observer(chain) {
        observer
            .emit(ObserverEventKind::ResponseHeadObserved {
                boundary,
                head: head.clone(),
            })
            .await;
        if boundary == ExchangeBoundary::ClientResponse {
            if transmog_content::sse::is_event_stream(&head.headers)
                && head.status != 204
                && head.status != 304
            {
                observer
                    .emit(ObserverEventKind::StreamState(StreamState::now(
                        StreamProtocol::Sse,
                        StreamStatus::Open,
                        None,
                    )))
                    .await;
                let decoder = match transmog_content::sse::SseDecoder::new(&head.headers) {
                    Ok(decoder) => Some(decoder),
                    Err(error) => {
                        observer
                            .emit(ObserverEventKind::StreamMessage(
                                transmog_core::stream::StreamMessage::new(
                                    "server",
                                    "unavailable",
                                    Bytes::from(error),
                                ),
                            ))
                            .await;
                        None
                    }
                };
                chain
                    .context()
                    .extensions()
                    .insert(crate::streams::SseObservation(Mutex::new(decoder)));
            }
            observer
                .emit(ObserverEventKind::ResponseHeadFinalized(head.clone()))
                .await;
        }
    }
}

async fn observe_hook_effects(chain: &ExchangeChain) {
    let Some(observer) = runtime_observer(chain) else {
        return;
    };
    for effect in chain.take_unpublished_hook_effects() {
        observer.emit(ObserverEventKind::HookEffect(effect)).await;
    }
}

async fn observe_route_attempts(chain: &ExchangeChain, attempts: &[RouteAttemptEvidence]) {
    let Some(observer) = runtime_observer(chain) else {
        return;
    };
    for attempt in attempts {
        observer
            .emit(ObserverEventKind::RouteAttempt(ObservedRouteAttempt {
                protocol: attempt.protocol,
                outcome: Arc::from(attempt.outcome.as_str()),
            }))
            .await;
    }
}

async fn observe_body_frames(
    chain: &ExchangeChain,
    boundary: ExchangeBoundary,
    frames: &[BodyFrame],
) {
    observe_body(chain, boundary, frames, false).await;
}

async fn observe_complete_body(
    chain: &ExchangeChain,
    boundary: ExchangeBoundary,
    frames: &[BodyFrame],
) {
    observe_body(chain, boundary, frames, true).await;
}

async fn observe_body(
    chain: &ExchangeChain,
    boundary: ExchangeBoundary,
    frames: &[BodyFrame],
    complete: bool,
) {
    let Some(observer) = runtime_observer(chain) else {
        return;
    };
    if boundary == ExchangeBoundary::ClientResponse
        && let Some(observation) = chain
            .context()
            .extensions()
            .get::<crate::streams::SseObservation>()
    {
        let mut decoder = observation.0.lock().await;
        if let Some(active) = decoder.as_mut() {
            let result = async {
                let mut messages = Vec::new();
                for frame in frames {
                    messages.extend(active.push(frame.clone()).await?);
                }
                if complete {
                    messages.extend(active.finish().await?);
                }
                Ok::<_, String>(messages)
            }
            .await;
            match result {
                Ok(messages) => {
                    for message in messages {
                        observer
                            .emit(ObserverEventKind::StreamMessage(message))
                            .await;
                    }
                }
                Err(error) => {
                    *decoder = None;
                    observer
                        .emit(ObserverEventKind::StreamMessage(
                            transmog_core::stream::StreamMessage::new(
                                "server",
                                "unavailable",
                                Bytes::from(error),
                            ),
                        ))
                        .await;
                }
            }
        }
        if complete {
            *decoder = None;
        }
    }
    if boundary == ExchangeBoundary::UpstreamResponse
        && frames
            .iter()
            .any(|frame| matches!(frame,BodyFrame::Data(bytes) if !bytes.is_empty()))
        && let Some(recorder) = performance_recorder(chain)
    {
        recorder.mark(Milestone::UpstreamResponseFirstBody);
    }
    for (index, frame) in frames.iter().enumerate() {
        let event = match frame {
            BodyFrame::Data(data) => ObserverEventKind::BodyChunk(ObservedBodyChunk {
                boundary,
                byte_count: data.len(),
                sample: Some(data.clone()),
                truncated: false,
            }),
            BodyFrame::Trailers(trailers) => {
                ObserverEventKind::BodyTrailers(ObservedBodyTrailers {
                    boundary,
                    trailers: trailers.clone(),
                })
            }
        };
        if complete && index + 1 == frames.len() {
            observer
                .emit_batch(vec![event, ObserverEventKind::BodyCompleted { boundary }])
                .await;
        } else {
            observer.emit(event).await;
        }
    }
    if complete && frames.is_empty() {
        observer
            .emit(ObserverEventKind::BodyCompleted { boundary })
            .await;
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
        return Err(transmog_core::BodyLimitError::LimitExceeded {
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

#[allow(clippy::too_many_lines)] // One ingress stream owns capture completion and forwarding failures.
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
    let mut client_body_completed = false;
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
        // A final ingress frame completes capture even if forwarding that frame fails.
        if body.is_end_stream()
            && let Some(recorder) = performance_recorder(&chain)
        {
            recorder.mark(Milestone::ClientRequestDone);
        }
        let complete = body.is_end_stream();
        observe_body(
            &chain,
            ExchangeBoundary::ClientRequest,
            std::slice::from_ref(&canonical),
            complete,
        )
        .await;
        client_body_completed |= complete;
        let frames = match pipeline.process(canonical).await {
            Ok(frames) => frames,
            Err(error) => {
                fail_hook_pipeline(&sender, &chain, ExchangeStage::RequestBody, error).await;
                return;
            }
        };
        observe_body_frames(&chain, ExchangeBoundary::UpstreamRequest, &frames).await;
        if send_streaming_frames(
            &sender,
            &mut output,
            frames,
            &chain,
            "Upstream request queue",
        )
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
    if !client_body_completed {
        observe_complete_body(&chain, ExchangeBoundary::ClientRequest, &[]).await;
    }
    publish_performance(&chain, Some(Milestone::ClientRequestDone)).await;
    match pipeline.finish().await {
        Ok(frames) => {
            observe_complete_body(&chain, ExchangeBoundary::UpstreamRequest, &frames).await;
            if send_streaming_frames(
                &sender,
                &mut output,
                frames,
                &chain,
                "Upstream request queue",
            )
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

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One response stream owns failure and completion handling.
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
    let event_stream = transmog_core::stream::is_event_stream(&response_head.headers);
    let limit = if event_stream { usize::MAX } else { limit };
    let mut input = StreamingBodyTracker::new(limit);
    let mut output = StreamingBodyTracker::new(limit);
    loop {
        let frame = match if event_stream {
            Ok(body.recv().await)
        } else {
            timeout(body_idle_timeout, body.recv()).await
        } {
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
        observe_body_frames(
            &chain,
            ExchangeBoundary::UpstreamResponse,
            std::slice::from_ref(&canonical),
        )
        .await;
        let frames = match pipeline.process(canonical).await {
            Ok(frames) => frames,
            Err(error) => {
                fail_hook_pipeline(&sender, &chain, ExchangeStage::ResponseBody, error).await;
                return;
            }
        };
        observe_body_frames(&chain, ExchangeBoundary::ClientResponse, &frames).await;
        if send_streaming_frames(
            &sender,
            &mut output,
            frames,
            &chain,
            "Client response queue",
        )
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
    observe_complete_body(&chain, ExchangeBoundary::UpstreamResponse, &[]).await;
    publish_performance(&chain, Some(Milestone::UpstreamResponseDone)).await;
    let frames = match pipeline.finish().await {
        Ok(frames) => frames,
        Err(error) => {
            fail_hook_pipeline(&sender, &chain, ExchangeStage::ResponseBody, error).await;
            return;
        }
    };
    observe_complete_body(&chain, ExchangeBoundary::ClientResponse, &frames).await;
    if send_streaming_frames(
        &sender,
        &mut output,
        frames,
        &chain,
        "Client response queue",
    )
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

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn stream_local_response_through_hooks(
    mut body: BodyStream,
    sender: BodyStreamSender,
    mut pipeline: ContentBodyPipeline,
    chain: Arc<ExchangeChain>,
    request_head: RequestHead,
    response_head: ResponseHead,
    limit: usize,
    body_idle_timeout: Duration,
) {
    let event_stream = transmog_core::stream::is_event_stream(&response_head.headers);
    let limit = if event_stream { usize::MAX } else { limit };
    let mut input = StreamingBodyTracker::new(limit);
    let mut output = StreamingBodyTracker::new(limit);
    loop {
        let frame = match if event_stream {
            Ok(body.recv().await)
        } else {
            timeout(body_idle_timeout, body.recv()).await
        } {
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
        observe_body_frames(&chain, ExchangeBoundary::ClientResponse, &frames).await;
        if send_streaming_frames(
            &sender,
            &mut output,
            frames,
            &chain,
            "Client response queue",
        )
        .await
        .is_err()
        {
            emit_hook_failure(
                &chain,
                ExchangeStage::ResponseBody,
                ExchangeFailureKind::Body,
                "downstream local response-body consumer closed".to_owned(),
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
    observe_complete_body(&chain, ExchangeBoundary::ClientResponse, &frames).await;
    if send_streaming_frames(
        &sender,
        &mut output,
        frames,
        &chain,
        "Client response queue",
    )
    .await
    .is_err()
    {
        emit_hook_failure(
            &chain,
            ExchangeStage::ResponseBody,
            ExchangeFailureKind::Body,
            "downstream local response-body consumer closed".to_owned(),
        )
        .await;
        return;
    }
    observe_hook_effects(&chain).await;
    let outcome = CompletedExchange {
        metadata: Arc::clone(chain.context().metadata()),
        request_head,
        response_head,
    };
    publish_performance(&chain, Some(Milestone::ClientResponseQueued)).await;
    publish_performance(&chain, Some(Milestone::ExchangeDone)).await;
    let report = chain.completed(outcome.clone()).await;
    for error in report.errors() {
        warn!(%error, "terminal hook cleanup failed");
    }
    if let Some(observer) = runtime_observer(&chain) {
        observer.completed(outcome).await;
    }
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
    observe_hook_effects(chain).await;
    observe_route_attempts(chain, &attempts).await;
    let outcome = CompletedExchange {
        metadata: Arc::clone(chain.context().metadata()),
        request_head: request_head.clone(),
        response_head: response_head.clone(),
    };
    publish_performance(chain, Some(Milestone::ClientResponseQueued)).await;
    publish_performance(chain, Some(Milestone::ExchangeDone)).await;
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
    observe_hook_effects(chain).await;
    let failure = ExchangeFailure {
        metadata: Arc::clone(chain.context().metadata()),
        stage,
        kind,
        request_committed: stage == ExchangeStage::ResponseBody,
        response_committed: stage == ExchangeStage::ResponseBody,
        message,
    };
    publish_performance(chain, Some(Milestone::ExchangeDone)).await;
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
    chain: &ExchangeChain,
    label: &'static str,
) -> Result<(), BodyStreamError> {
    for frame in frames {
        if let Err(error) = tracker.accept(&frame) {
            let _ = sender.send(Err(error.clone())).await;
            return Err(error);
        }
        let data = matches!(&frame,BodyFrame::Data(bytes) if !bytes.is_empty());
        let future = sender.send(Ok(frame));
        tokio::pin!(future);
        let mut waiting = None;
        let result =
            std::future::poll_fn(|cx| match std::future::Future::poll(future.as_mut(), cx) {
                std::task::Poll::Pending => {
                    if waiting.is_none() {
                        waiting = performance_recorder(chain)
                            .map(|recorder| recorder.work("queue", label));
                    }
                    std::task::Poll::Pending
                }
                std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
            })
            .await;
        if waiting.is_none()
            && let Some(recorder) = performance_recorder(chain)
        {
            let now = std::time::Instant::now();
            recorder.record_work("queue", label, now, now);
        }
        drop(waiting);
        result.map_err(|error| BodyStreamError::Failed(error.to_string()))?;
        if data && let Some(recorder) = performance_recorder(chain) {
            recorder.mark(if label == "Client response queue" {
                Milestone::ClientResponseFirstBodyQueued
            } else {
                Milestone::UpstreamRequestFirstBodyQueued
            });
        }
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
        validate_tunnel_target(request, &tunnel.authority)?;
        target_from_tunnel(request, &tunnel.authority, tunnel.protocol.target_scheme())
    } else {
        target_from_absolute_uri(request)?
    };
    Ok(RequestHead {
        method: request.method().as_str().to_owned(),
        target,
        headers: transmog_http::header_block_from_hyper(request.headers(), request.extensions())?,
        source_version,
    })
}

fn is_websocket_upgrade_candidate(headers: &HeaderBlock) -> bool {
    headers
        .values("upgrade")
        .any(|value| header_bytes_have_token(value, b"websocket"))
}

fn websocket_success(status: u16, extended: bool) -> bool {
    if extended {
        (200..300).contains(&status)
    } else {
        status == 101
    }
}

fn reject_unbacked_extended_connect(chain: &ExchangeChain, response: &mut ResponseHead) {
    if chain
        .context()
        .extensions()
        .get::<RuntimeExtendedConnect>()
        .is_some()
        && websocket_success(response.status, true)
    {
        // Only the relay path can establish a tunnel. Synthetic HTTP responses
        // must not acknowledge an upgrade with no owner for its stream.
        response.status = 502;
    }
}

fn websocket_handshake_request(
    head: &RequestHead,
    extended: bool,
) -> Result<transmog_websocket::ClientHandshake, ProxyRuntimeError> {
    let version = required_single_header(&head.headers, "sec-websocket-version")?;
    let extensions = combined_header(&head.headers, "sec-websocket-extensions")?;
    let subprotocols = combined_header(&head.headers, "sec-websocket-protocol")?;
    Ok(if extended {
        if head.method != "CONNECT"
            || ["connection", "upgrade"]
                .iter()
                .any(|name| head.headers.values(name).next().is_some())
        {
            return Err(ProxyRuntimeError::InvalidWebSocketHandshake(
                "HTTP/2 WebSockets require extended CONNECT without Connection or Upgrade headers"
                    .into(),
            ));
        }
        transmog_websocket::validate_extended_request(
            &version,
            extensions.as_deref(),
            subprotocols.as_deref(),
        )?
    } else {
        validate_websocket_request(RequestHandshake {
            method: &head.method,
            connection: &required_combined_header(&head.headers, "connection")?,
            upgrade: &required_combined_header(&head.headers, "upgrade")?,
            version: &version,
            key: &required_single_header(&head.headers, "sec-websocket-key")?,
            extensions: extensions.as_deref(),
            subprotocols: subprotocols.as_deref(),
        })?
    })
}

fn websocket_handshake_response(
    request: &transmog_websocket::ClientHandshake,
    head: &ResponseHead,
    extended: bool,
) -> Result<transmog_websocket::NegotiatedExtensions, ProxyRuntimeError> {
    let extensions = combined_header(&head.headers, "sec-websocket-extensions")?;
    let subprotocol = optional_single_header(&head.headers, "sec-websocket-protocol")?;
    Ok(if extended {
        transmog_websocket::validate_extended_response(
            request,
            head.status,
            extensions.as_deref(),
            subprotocol.as_deref(),
        )?
    } else {
        validate_websocket_response(
            request,
            ResponseHandshake {
                status: head.status,
                connection: &required_combined_header(&head.headers, "connection")?,
                upgrade: &required_combined_header(&head.headers, "upgrade")?,
                accept: &required_single_header(&head.headers, "sec-websocket-accept")?,
                extensions: extensions.as_deref(),
                subprotocol: subprotocol.as_deref(),
            },
        )?
    })
}

fn websocket_response_for_client(
    mut head: ResponseHead,
    client: &transmog_websocket::ClientHandshake,
    extended: bool,
) -> Result<ResponseHead, ProxyRuntimeError> {
    let destination = if extended {
        HttpLegVersion::Http2
    } else {
        HttpLegVersion::Http1
    };
    head.headers = prepare_headers(
        &head.headers,
        TranslationOptions {
            destination,
            kind: MessageKind::Response,
            body_modified: false,
            force_identity_encoding: false,
        },
    )?;
    head.headers.remove_all("content-length");
    head.headers.remove_all("sec-websocket-accept");
    if extended {
        head.status = 200;
    } else {
        head.status = 101;
        head.headers
            .replace_all(HeaderField::try_new("connection", "Upgrade")?);
        head.headers
            .replace_all(HeaderField::try_new("upgrade", "websocket")?);
        head.headers.replace_all(HeaderField::try_new(
            "sec-websocket-accept",
            transmog_websocket::expected_accept(client.key()),
        )?);
    }
    head.source_version = destination;
    Ok(head)
}

fn header_bytes_have_token(value: &[u8], expected: &[u8]) -> bool {
    value
        .split(|byte| *byte == b',')
        .any(|token| trim_ascii(token).eq_ignore_ascii_case(expected))
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn combined_header(headers: &HeaderBlock, name: &str) -> Result<Option<String>, ProxyRuntimeError> {
    let values = headers
        .values(name)
        .map(|value| {
            std::str::from_utf8(value)
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| {
                    ProxyRuntimeError::InvalidWebSocketHandshake(format!(
                        "{name} is not valid ASCII/UTF-8"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!values.is_empty()).then(|| values.join(", ")))
}

fn required_combined_header(
    headers: &HeaderBlock,
    name: &str,
) -> Result<String, ProxyRuntimeError> {
    combined_header(headers, name)?.ok_or_else(|| {
        ProxyRuntimeError::InvalidWebSocketHandshake(format!("missing {name} header"))
    })
}

fn optional_single_header(
    headers: &HeaderBlock,
    name: &str,
) -> Result<Option<String>, ProxyRuntimeError> {
    let values = headers.values(name).collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(ProxyRuntimeError::InvalidWebSocketHandshake(format!(
            "duplicate {name} header"
        )));
    }
    values
        .first()
        .map(|value| {
            std::str::from_utf8(value)
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| {
                    ProxyRuntimeError::InvalidWebSocketHandshake(format!(
                        "{name} is not valid ASCII/UTF-8"
                    ))
                })
        })
        .transpose()
}

fn required_single_header(headers: &HeaderBlock, name: &str) -> Result<String, ProxyRuntimeError> {
    optional_single_header(headers, name)?.ok_or_else(|| {
        ProxyRuntimeError::InvalidWebSocketHandshake(format!("missing {name} header"))
    })
}

fn websocket_request_has_body(headers: &HeaderBlock) -> Result<bool, ProxyRuntimeError> {
    if headers.values("transfer-encoding").next().is_some() {
        return Ok(true);
    }
    for value in headers.values("content-length") {
        let value = std::str::from_utf8(value)
            .map_err(|_| {
                ProxyRuntimeError::InvalidWebSocketHandshake(
                    "invalid content-length header".to_owned(),
                )
            })?
            .trim()
            .parse::<u64>()
            .map_err(|_| {
                ProxyRuntimeError::InvalidWebSocketHandshake(
                    "invalid content-length header".to_owned(),
                )
            })?;
        if value != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn target_from_tunnel(
    request: &Request<Incoming>,
    tunnel: &ConnectAuthority,
    scheme: &str,
) -> Target {
    let path_and_query = request.uri().path_and_query();
    Target {
        scheme: scheme.to_owned(),
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
    let source_scheme = request
        .uri()
        .scheme_str()
        .ok_or(ProxyRuntimeError::AbsoluteFormRequired)?;
    let scheme = match source_scheme {
        "http" | "ws" => "http",
        _ => {
            return Err(ProxyRuntimeError::ConnectRequiredForHttps);
        }
    };
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

fn draining_response() -> Response<DownstreamBody> {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(http::header::CONTENT_LENGTH, "0")
        .body(DownstreamBody::empty())
        .expect("fixed drain response")
}

fn failed_exchange_response(error: &ProxyRuntimeError) -> Response<DownstreamBody> {
    let status = match error {
        ProxyRuntimeError::HookAborted(HookAbort::Rejected) => StatusCode::FORBIDDEN,
        ProxyRuntimeError::BodyLimit(_) => StatusCode::PAYLOAD_TOO_LARGE,
        ProxyRuntimeError::AbsoluteFormRequired
        | ProxyRuntimeError::ConnectRequiredForHttps
        | ProxyRuntimeError::TunnelAuthorityMismatch
        | ProxyRuntimeError::NestedConnectUnsupported
        | ProxyRuntimeError::PlaintextConnectRequiresWebSocket
        | ProxyRuntimeError::WebSocketHandshake(_)
        | ProxyRuntimeError::InvalidWebSocketHandshake(_)
        | ProxyRuntimeError::WebSocketRequiresHttp1Or2
        | ProxyRuntimeError::WebSocketApplicationUpstreamUnsupported
        | ProxyRuntimeError::ConnectAuthority(_)
        | ProxyRuntimeError::Identity(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    let mut response = Response::new(DownstreamBody::from_bytes(Bytes::from_static(
        FAILED_EXCHANGE_BODY,
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
    activity: Option<ActivityLease>,
    inner: DownstreamBodyInner,
}

enum DownstreamBodyInner {
    Buffered(VecDeque<Frame<Bytes>>),
    Streaming(BodyStream),
}

impl DownstreamBody {
    fn empty() -> Self {
        Self {
            activity: None,
            inner: DownstreamBodyInner::Buffered(VecDeque::new()),
        }
    }

    fn from_bytes(bytes: Bytes) -> Self {
        Self {
            activity: None,
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
            activity: None,
            inner: DownstreamBodyInner::Buffered(output),
        })
    }

    fn streaming(body: BodyStream) -> Self {
        Self {
            activity: None,
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
        let result = match &mut self.inner {
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
        };
        if matches!(result, Poll::Ready(None | Some(Err(_)))) {
            self.activity.take();
        }
        result
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
    /// An authority-form CONNECT carried an unsupported HTTP request body.
    #[error("CONNECT requests must not carry an HTTP message body")]
    ConnectRequestBodyUnsupported,
    /// Application connection-isolation computation panicked and was contained.
    #[error("outbound connection isolation policy panicked")]
    IsolationPolicyPanicked,
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
    Header(#[from] transmog_core::HeaderError),
    /// Header-name conversion failed.
    #[error(transparent)]
    HeaderName(#[from] http::header::InvalidHeaderName),
    /// Header-value conversion failed.
    #[error(transparent)]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    /// Header translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] transmog_core::TranslationError),
    /// Body exceeded its explicit bound or had illegal trailer ordering.
    #[error(transparent)]
    BodyLimit(#[from] transmog_core::BodyLimitError),
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
    /// WebSocket opening handshake was invalid or unauthenticated.
    #[error(transparent)]
    WebSocketHandshake(#[from] WebSocketHandshakeError),
    /// WebSocket relay configuration or execution failed.
    #[error(transparent)]
    WebSocketRelay(#[from] RelayError),
    /// WebSocket hook initialization or execution failed.
    #[error(transparent)]
    WebSocketHook(#[from] WebSocketHookError),
    /// Negotiated WebSocket compression cannot be inspected safely.
    #[error(transparent)]
    WebSocketCompression(#[from] WebSocketCompressionError),
    /// Handshake fields could not be interpreted safely.
    #[error("invalid WebSocket handshake: {0}")]
    InvalidWebSocketHandshake(String),
    /// WebSocket routing requires HTTP/1.1 or HTTP/2.
    #[error("WebSocket connections require HTTP/1.1 or HTTP/2 origin routing")]
    WebSocketRequiresHttp1Or2,
    /// Application-owned HTTP services cannot provide an upgraded byte stream.
    #[error("application-owned upstream services do not support WebSocket byte streams")]
    WebSocketApplicationUpstreamUnsupported,
    /// HTTP/3 origin adapter failed.
    #[error(transparent)]
    H3Origin(#[from] H3OriginError),
    /// CONNECT authority was invalid.
    #[error(transparent)]
    ConnectAuthority(#[from] transmog_http::AuthorityError),
    /// CONNECT/SNI or canonical identity was invalid.
    #[error(transparent)]
    Identity(#[from] transmog_tls::IdentityError),
    /// Leaf cache configuration or issuance failed.
    #[error(transparent)]
    Leaf(#[from] LeafCacheError),
    /// Downstream certificate provider failed or returned mismatched material.
    #[error(transparent)]
    CertificateResolver(#[from] CertificateResolverError),
    /// Browser-facing TLS context construction failed.
    #[error(transparent)]
    DownstreamTls(#[from] transmog_tls::DownstreamTlsError),
    /// Browser did not finish TLS within the configured deadline.
    #[error("downstream TLS handshake timed out")]
    DownstreamTlsTimeout,
    /// Browser did not send a TLS or HTTP preface within the configured deadline.
    #[error("CONNECT tunnel preface timed out")]
    TunnelPrefaceTimeout,
    /// CONNECT carried neither intercepted TLS nor an HTTP/1 WebSocket request.
    #[error("unsupported CONNECT tunnel preface byte 0x{0:02x}")]
    UnsupportedTunnelPreface(u8),
    /// Plaintext CONNECT is supported only for browser-style WebSocket upgrades.
    #[error("plaintext CONNECT payload must be an HTTP/1 WebSocket upgrade")]
    PlaintextConnectRequiresWebSocket,
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
#[path = "request_cancellation_tests.rs"]
mod request_cancellation_tests;

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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UdpSocket},
        sync::{Notify, oneshot},
        task::JoinHandle,
    };
    use transmog_content::{ContentCoding, ContentDecoder, ContentEncoder, ContentLimits};
    use transmog_core::intercept::{
        BodyFilter, BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, BufferedBody,
        ExchangeInterceptor, HookInitError, RequestBodyAction, RequestBodyEvent, RequestHeadAction,
        RequestHeadEvent, ResponseBodyAction, ResponseBodyEvent, ResponseHeadAction,
        ResponseHeadEvent,
    };
    use transmog_h3::H3UpstreamService;
    use transmog_http::HyperUpstreamService;
    use transmog_tls::{
        DownstreamTlsContextFactory, DownstreamTlsPolicy, LoadedTrust, ProxyCa, SystemTrustSource,
        TrustError, TrustSnapshot, TrustSource, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };
    use transmog_websocket::{
        BoxWebSocketFuture, CloseFrame, DataKind, Direction, Frame as WebSocketFrame, FrameLimits,
        MessageAction, MessageDecoder, MessageEvent, MessageEventHook, WebSocketHookLimits,
        WebSocketHookRegistration, WebSocketInterceptor, WebSocketInterceptorFactory,
        encode_frame as encode_websocket_frame,
    };

    use super::*;
    include!("websocket_h2_tests.rs");
    include!("pqc_tests.rs");

    #[derive(Clone, Copy)]
    struct EditingHandler;

    #[derive(Clone, Copy)]
    struct LocalResponseHandler;

    #[derive(Clone, Copy)]
    struct StreamingLocalResponseHandler;

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
        performance: Arc<StdMutex<transmog_core::performance::PerformanceEvidence>>,
        phases: Arc<StdMutex<Vec<&'static str>>>,
        client_identity: Arc<StdMutex<Option<transmog_core::ClientIdentity>>>,
    }

    struct StaticTrust(Vec<Vec<u8>>);

    #[derive(Debug)]
    struct AppendWebSocketFactory;

    #[derive(Debug)]
    struct AppendWebSocketHook;

    impl WebSocketInterceptorFactory for AppendWebSocketFactory {
        fn create(
            &self,
            _metadata: &WebSocketSessionMetadata,
        ) -> Result<Arc<dyn WebSocketInterceptor>, String> {
            Ok(Arc::new(AppendWebSocketHook))
        }
    }

    impl WebSocketInterceptor for AppendWebSocketHook {
        fn on_message(&self, event: MessageEventHook) -> BoxWebSocketFuture<'_, MessageAction> {
            let mut payload = event.message.payload.to_vec();
            payload.extend_from_slice(b"-hook");
            Box::pin(async move { MessageAction::Replace(Bytes::from(payload)) })
        }
    }

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

    impl InterceptorFactory for StreamingLocalResponseHandler {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(*self))
        }
    }

    impl ExchangeInterceptor for StreamingLocalResponseHandler {
        fn on_request_head(
            &self,
            _event: RequestHeadEvent,
        ) -> BoxHookFuture<'_, RequestHeadAction> {
            let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
            tokio::spawn(async move {
                sender
                    .send(Ok(BodyFrame::Data(Bytes::from_static(b"stream-"))))
                    .await
                    .unwrap();
                sender
                    .send(Ok(BodyFrame::Data(Bytes::from_static(b"asset"))))
                    .await
                    .unwrap();
            });
            Box::pin(async move {
                RequestHeadAction::RespondStreaming(transmog_core::LocalStreamingResponse {
                    head: ResponseHead {
                        status: 203,
                        headers: HeaderBlock::from_fields(vec![
                            HeaderField::try_new("content-length", "12").unwrap(),
                            HeaderField::try_new("x-streaming-local", "yes").unwrap(),
                        ]),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                    body_length: Some(12),
                })
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

    impl transmog_core::route::DestinationAuthorizer for AllowAllDestinations {
        fn authorize(
            &self,
            _original: &transmog_core::intercept::OriginalTarget,
            _proposed: &transmog_core::route::UpstreamDestination,
        ) -> Result<(), RouteError> {
            Ok(())
        }
    }

    impl UpstreamService for EmbeddedUpstream {
        fn execute(
            &self,
            mut request: StreamingRequest,
            plan: UpstreamPlan,
            _cancellation: transmog_core::intercept::ExchangeCancellation,
        ) -> transmog_core::upstream::BoxUpstreamFuture<'_> {
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
                Ok(transmog_core::StreamingResponse {
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
            _cancellation: transmog_core::intercept::ExchangeCancellation,
        ) -> transmog_core::upstream::BoxUpstreamFuture<'_> {
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
                Ok(transmog_core::StreamingResponse {
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
            _cancellation: transmog_core::intercept::ExchangeCancellation,
        ) -> transmog_core::upstream::BoxUpstreamFuture<'_> {
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
                Ok(transmog_core::StreamingResponse {
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
            pool_key: transmog_core::route::UpstreamPoolKey {
                network_isolation: NetworkIsolationKey::shared(),
                destination: transmog_core::route::UpstreamDestination {
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
                &transmog_core::intercept::ExchangeCancellation::new(),
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

    impl transmog_core::observe::Observer for LifecycleObserver {
        fn on_event(
            &self,
            event: transmog_core::observe::ObserverEvent,
        ) -> transmog_core::observe::BoxObserverFuture<'_> {
            if let ObserverEventKind::ExchangeStarted { metadata } = &event.kind {
                *self.client_identity.lock().unwrap() = Some(metadata.client_identity.clone());
            }
            let phase = match event.kind {
                ObserverEventKind::Performance(evidence) => {
                    self.performance.lock().unwrap().merge(&evidence);
                    return Box::pin(async { Ok(()) });
                }
                ObserverEventKind::ExchangeStarted { .. } => "started",
                ObserverEventKind::RequestHeadObserved { .. } => "request-head-boundary",
                ObserverEventKind::RequestHeadFinalized(_) => "request-head",
                ObserverEventKind::RouteSelected { .. } => "route",
                ObserverEventKind::RouteAttempt(_) => "route-attempt",
                ObserverEventKind::ResponseHeadObserved { .. } => "response-head-boundary",
                ObserverEventKind::ResponseHeadFinalized(_) => "response-head",
                ObserverEventKind::BodyChunk(_) => "body",
                ObserverEventKind::BodyTrailers(_) => "trailers",
                ObserverEventKind::BodyCompleted { .. }
                | ObserverEventKind::StreamState(_)
                | ObserverEventKind::StreamMessage(_) => {
                    return Box::pin(async { Ok(()) });
                }
                ObserverEventKind::HookEffect(_) => "hook-effect",
                ObserverEventKind::Completed(_) => "completed",
                ObserverEventKind::Failed(_) => "failed",
                ObserverEventKind::HookInitializationSkipped(_) => "hook-skipped",
            };
            self.phases.lock().unwrap().push(phase);
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
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
                ProxyCa::generate("Transmog embedded test", 2).unwrap(),
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let performance = Arc::new(StdMutex::new(
            transmog_core::performance::PerformanceEvidence::default(),
        ));
        let phases = Arc::new(StdMutex::new(Vec::new()));
        let client_identity = Arc::new(StdMutex::new(None));
        let observers = ObserverHub::new(vec![(
            Arc::new(LifecycleObserver {
                performance: performance.clone(),
                phases: Arc::clone(&phases),
                client_identity: Arc::clone(&client_identity),
            }),
            transmog_core::observe::ObserverConfig::default(),
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
                "request-head-boundary",
                "hook-effect",
                "request-head",
                "route",
                "request-head-boundary",
                "response-head-boundary",
                "response-head-boundary",
                "response-head",
                "body",
                "body",
                "route-attempt",
                "completed"
            ]
        );
        assert!(matches!(
            client_identity.lock().unwrap().as_ref(),
            Some(transmog_core::ClientIdentity::LocalProcess { pid, .. })
                if *pid == std::process::id()
        ));

        let timing = performance.lock().unwrap().clone();
        assert!(timing.valid());
        assert!(timing.elapsed_micros().is_some());
        for milestone in [
            Milestone::ClientConnected,
            Milestone::ClientFirstByte,
            Milestone::ClientIdentityDone,
            Milestone::RequestHeaders,
            Milestone::ClientRequestDone,
            Milestone::RouteDone,
            Milestone::ClientResponseQueued,
            Milestone::ExchangeDone,
        ] {
            assert!(
                timing
                    .points
                    .iter()
                    .any(|point| point.milestone == milestone),
                "{milestone:?}"
            );
        }
        assert!(
            timing
                .points
                .iter()
                .find(|point| point.milestone == Milestone::ClientConnected)
                .unwrap()
                .offset_micros
                <= 0
        );
        assert_eq!(
            timing
                .protocols
                .iter()
                .find(|item| item.boundary == "client-request")
                .unwrap()
                .version,
            "HTTP/1.1"
        );
        assert!(
            timing
                .transports
                .iter()
                .any(|connection| connection.leg == "client"
                    && connection.bytes_read.is_some_and(|count| count > 0))
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
                ProxyCa::generate("Transmog content policy test", 2).unwrap(),
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
        let origin_ca = ProxyCa::generate("Transmog h3 contract origin", 2).unwrap();
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
            ProxyCa::generate("Transmog trust reload test", 2).unwrap(),
            initial,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
    async fn profiles_replace_downstream_leases_without_changing_trust_and_rollback_is_inert() {
        let proxy = ProxyServer::bind(
            ProxyConfig::default(),
            ProxyCa::generate("Profile pool test", 2).unwrap(),
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 52).unwrap()),
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        let control = proxy.control();
        let lease = Arc::new(StdMutex::new(None));
        let key = NetworkIsolationKey::new("process:10/h2:1");
        let old = proxy
            .state
            .upstream
            .read()
            .await
            .isolated(&key, &lease)
            .unwrap();
        let mut profile = transmog_core::network_profile::NetworkProfileSnapshot::default();
        profile.settings.protocols.http2 = false;
        profile.settings.reuse_connections = false;
        drop(
            control
                .prepare_network_profile(profile.clone())
                .await
                .unwrap(),
        );
        let unchanged = proxy
            .state
            .upstream
            .read()
            .await
            .isolated(&key, &lease)
            .unwrap();
        assert!(Arc::ptr_eq(&old, &unchanged));
        control
            .prepare_network_profile(profile.clone())
            .await
            .unwrap()
            .commit();
        let active = proxy
            .state
            .upstream
            .read()
            .await
            .isolated(&key, &lease)
            .unwrap();
        assert!(!Arc::ptr_eq(&old, &active));
        assert_eq!(active.trust_generation, old.trust_generation);
        assert!(old.partitions.protocols.snapshot().http2);
        assert!(!active.partitions.protocols.snapshot().http2);
        assert!(!active.partitions.protocols.reuse_connections());
        assert_eq!(active.partitions.tls.network_profile(), Some(&profile));
        control
            .reload_trust(Arc::new(
                TrustSnapshot::load(&SystemTrustSource, 53).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(
            proxy
                .state
                .upstream
                .read()
                .await
                .partitions
                .tls
                .network_profile(),
            Some(&profile)
        );
    }

    #[tokio::test]
    async fn disabled_h3_skips_cached_alternatives_for_non_replayable_requests() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = origin.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = origin.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(socket.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /posted HTTP/1.1"));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let mut profile = transmog_core::network_profile::NetworkProfileSnapshot::default();
        profile.settings.protocols.http3 = false;
        let proxy = ProxyServer::bind(
            ProxyConfig {
                network_profile: Some(profile),
                ..Default::default()
            },
            ProxyCa::generate("Disabled H3 routing test", 2).unwrap(),
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 54).unwrap()),
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        let upstream = proxy.state.upstream.read().await.clone();
        upstream
            .partitions
            .alt_svc
            .lock()
            .await
            .observe(
                Origin {
                    host: "127.0.0.1".into(),
                    port: address.port(),
                },
                &format!("h3=\":{}\"; ma=60", address.port()),
                proxy.state.clock.instant(),
            )
            .unwrap();
        let request = CanonicalRequest {
            head: RequestHead {
                method: "POST".into(),
                source_version: HttpLegVersion::Http1,
                headers: HeaderBlock::new(),
                target: Target {
                    scheme: "http".into(),
                    authority: address.to_string(),
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    path: "/posted".into(),
                    query: None,
                },
            },
            body: Vec::new(),
        };
        assert_eq!(
            proxy
                .state
                .streaming_hyper_mode(&upstream, &request.head, RoutePolicy::Auto, false)
                .await,
            Some(HyperEgressMode::Auto)
        );
        let response = proxy.state.route_auto(request, &upstream).await.unwrap();
        assert_eq!(response.response.head.status, 200);
        assert_eq!(response.attempts.len(), 1);
        assert_eq!(response.protocol, HttpLegVersion::Http1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn network_isolation_shares_within_callers_and_releases_closed_sessions() {
        let generation = UpstreamGeneration::new(
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 150).unwrap()),
            H3TransportLimits::default(),
            HappyEyeballsConfig::default(),
            OutboundProtocolPolicy::default(),
            None,
        )
        .unwrap();
        let first = Arc::new(StdMutex::new(None));
        let second = Arc::new(StdMutex::new(None));
        let key = NetworkIsolationKey::new("process:10/h2:1");
        let a = generation.isolated(&key, &first).unwrap();
        let b = generation.isolated(&key, &second).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let other = generation
            .isolated(&NetworkIsolationKey::new("process:10/h2:2"), &second)
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &other));
        let weak = Arc::downgrade(&a);
        drop(a);
        drop(b);
        drop(first);
        assert!(weak.upgrade().is_none());
        assert!(Arc::ptr_eq(
            &other,
            &generation
                .isolated(
                    &NetworkIsolationKey::new("process:10/h2:2"),
                    &Arc::new(StdMutex::new(None))
                )
                .unwrap()
        ));
        assert_eq!(generation.partitions.clients.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Verify one raw tunnel's bytes, certificate avoidance and retained context together.
    async fn opaque_connect_relays_exact_bytes_and_records_destination_without_certificates() {
        struct NoCertificates;
        impl DownstreamCertificateResolver for NoCertificates {
            fn resolve(
                &self,
                _identity: &EndpointIdentity,
                _now: std::time::SystemTime,
            ) -> Result<transmog_tls::IssuedLeaf, CertificateResolverError> {
                panic!("opaque CONNECT tried to resolve an interception certificate");
            }
        }
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = origin.local_addr().unwrap();
        let payload = Bytes::from([0x16, 0x03, 0x03, 0, 0xff, 0x00, 0x80].repeat(8192));
        let expected = payload.clone();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes.as_slice(), expected.as_ref());
            stream.write_all(&bytes).await.unwrap();
        });
        let performance = Arc::new(StdMutex::new(
            transmog_core::performance::PerformanceEvidence::default(),
        ));
        let phases = Arc::new(StdMutex::new(Vec::new()));
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(
                Vec::new(),
                transmog_core::intercept::HookLimits::default(),
            ),
            Arc::new(NoCertificates),
        )
        .with_observer(
            Arc::new(LifecycleObserver {
                performance: performance.clone(),
                phases: phases.clone(),
                client_identity: Arc::new(StdMutex::new(None)),
            }),
            ObserverConfig::default(),
        );
        let proxy = ProxyServer::bind_with_components(
            ProxyConfig {
                decrypt_https: false,
                ..ProxyConfig::default()
            },
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 151).unwrap()),
            components,
        )
        .await
        .unwrap();
        let address = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(client.read_u8().await.unwrap());
        }
        assert!(headers.starts_with(b"HTTP/1.1 200"));
        client.write_all(&payload).await.unwrap();
        client.shutdown().await.unwrap();
        let mut echoed = Vec::new();
        timeout(Duration::from_secs(2), client.read_to_end(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(echoed.as_slice(), payload.as_ref());
        timeout(Duration::from_secs(1), async {
            while !phases.lock().unwrap().contains(&"completed") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let evidence = performance.lock().unwrap().clone();
        let transport = evidence
            .transports
            .iter()
            .find(|transport| transport.tunnel_destination.is_some())
            .unwrap();
        assert_eq!(
            transport.tunnel_destination.as_deref(),
            Some(destination.to_string().as_str())
        );
        assert_eq!(
            transport.peer.as_deref(),
            Some(destination.to_string().as_str())
        );
        assert_eq!(transport.bytes_written, Some(payload.len() as u64));
        assert_eq!(transport.bytes_read, Some(payload.len() as u64));
        assert!(transport.tls_version.is_none());
        assert!(evidence.valid());
        assert!(
            evidence
                .protocols
                .iter()
                .all(|protocol| !protocol.boundary.starts_with("upstream"))
        );
        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn opaque_connect_failure_retains_destination_and_terminal_context() {
        let unavailable = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = unavailable.local_addr().unwrap();
        drop(unavailable);
        let performance = Arc::new(StdMutex::new(
            transmog_core::performance::PerformanceEvidence::default(),
        ));
        let phases = Arc::new(StdMutex::new(Vec::new()));
        let proxy = ProxyServer::bind_with_components(
            ProxyConfig {
                decrypt_https: false,
                ..ProxyConfig::default()
            },
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 152).unwrap()),
            ProxyComponents::new(
                InterceptorChainFactory::new(
                    Vec::new(),
                    transmog_core::intercept::HookLimits::default(),
                ),
                Arc::new(
                    CachedMitmCertificateResolver::new(
                        ProxyCa::generate("Unused tunnel CA", 2).unwrap(),
                        8,
                        1,
                    )
                    .unwrap(),
                ),
            )
            .with_observer(
                Arc::new(LifecycleObserver {
                    performance: performance.clone(),
                    phases: phases.clone(),
                    client_identity: Arc::new(StdMutex::new(None)),
                }),
                ObserverConfig::default(),
            ),
        )
        .await
        .unwrap();
        let address = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(10), async {
            while !phases.lock().unwrap().contains(&"failed") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let evidence = performance.lock().unwrap().clone();
        assert!(evidence.valid());
        assert_eq!(
            evidence.transports[0].tunnel_destination.as_deref(),
            Some(destination.to_string().as_str())
        );
        assert_eq!(evidence.transports[0].outcome, TransportOutcome::TcpFailed);
        assert!(evidence.transports[0].tcp_micros.is_some());
        assert!(!phases.lock().unwrap().contains(&"completed"));
        drop(client);
        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn finalized_drain_does_not_wait_for_an_idle_h2_peer_to_ack_shutdown() {
        let proxy_ca = ProxyCa::generate("Transmog idle H2 drain test", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                limits: RuntimeLimits {
                    shutdown_timeout: Duration::from_millis(50),
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        let endpoint = proxy.local_addr().unwrap();
        let control = proxy.control();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));
        let mut transport = TcpStream::connect(endpoint).await.unwrap();
        transport
            .write_all(b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\n\r\n")
            .await
            .unwrap();
        assert!(
            read_http_head(&mut transport)
                .await
                .starts_with(b"HTTP/1.1 200")
        );
        let connector = UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
            .hyper_connector_builder(b"\x02h2")
            .unwrap()
            .build();
        let mut browser =
            tokio_boring::connect(connector.configure().unwrap(), "localhost", transport)
                .await
                .unwrap();
        // Complete the H2 preface, then keep the socket open without responding
        // to the PING used by graceful GOAWAY. There is no admitted request.
        browser
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
            .await
            .unwrap();
        timeout(Duration::from_secs(2), async {
            loop {
                let mut header = [0_u8; 9];
                browser.read_exact(&mut header).await.unwrap();
                let length = (usize::from(header[0]) << 16)
                    | (usize::from(header[1]) << 8)
                    | usize::from(header[2]);
                browser.read_exact(&mut vec![0_u8; length]).await.unwrap();
                if header[3] == 4 && header[4] == 1 {
                    break;
                }
            }
        })
        .await
        .expect("H2 settings were not acknowledged");
        let generation = control.begin_drain();
        assert!(control.wait_for_drain(generation).await);
        assert!(control.finish_drain(generation));
        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), task)
            .await
            .expect("Idle H2 peer blocked final shutdown")
            .unwrap()
            .unwrap();
        // A new run must be able to reclaim the listener even while a status
        // reader retains the old control handle.
        let listener = TcpListener::bind(endpoint).await.unwrap();
        drop(listener);
        drop(browser);
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
            ProxyCa::generate("Transmog slow header test", 2).unwrap(),
            trust,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
        let ca = ProxyCa::generate("Transmog runtime test", 2).unwrap();
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
    #[allow(clippy::too_many_lines)]
    async fn sse_messages_are_observed_live_across_idle_periods_and_drain_resume() {
        struct Events(tokio::sync::mpsc::UnboundedSender<transmog_core::stream::StreamMessage>);
        impl transmog_core::observe::Observer for Events {
            fn on_event(
                &self,
                event: transmog_core::observe::ObserverEvent,
            ) -> transmog_core::observe::BoxObserverFuture<'_> {
                if let ObserverEventKind::StreamMessage(message) = event.kind {
                    let _ = self.0.send(message);
                }
                Box::pin(async { Ok(()) })
            }
        }
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let (continue_tx, continue_rx) = oneshot::channel();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let _ = read_http_head(&mut stream).await;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\nD\r\ndata: first\n\n\r\n").await.unwrap();
            let _ = continue_rx.await;
            stream
                .write_all(b"E\r\ndata: second\n\n\r\n0\r\n\r\n")
                .await
                .unwrap();
        });
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            limits: RuntimeLimits {
                max_response_body_bytes: 4,
                body_idle_timeout: Duration::from_millis(40),
                shutdown_timeout: Duration::from_secs(1),
                ..RuntimeLimits::default()
            },
            ..ProxyConfig::default()
        };
        let certificates = Arc::new(
            CachedMitmCertificateResolver::new(
                ProxyCa::generate("SSE test", 2).unwrap(),
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(Vec::new(), config.limits.hooks),
            certificates,
        )
        .with_observer(
            Arc::new(Events(tx)),
            ObserverConfig {
                interest: transmog_core::observe::ObservationInterest {
                    response_body: transmog_core::observe::BodyObservation::Full,
                    ..Default::default()
                },
                ..ObserverConfig::default()
            },
        );
        let proxy = ProxyServer::bind_with_components(
            config,
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 181).unwrap()),
            components,
        )
        .await
        .unwrap();
        let address = proxy.local_addr().unwrap();
        let control = proxy.control();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("GET http://{origin_addr}/events HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            String::from_utf8(read_http_head(&mut client).await)
                .unwrap()
                .starts_with("HTTP/1.1 200")
        );
        let first = timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.payload, "first");
        let generation = control.begin_drain();
        assert!(
            timeout(
                Duration::from_millis(150),
                control.wait_for_drain(generation)
            )
            .await
            .is_err(),
            "quiet SSE stopped being active"
        );
        assert!(control.resume_drain(generation));
        continue_tx.send(()).unwrap();
        let second = timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.payload, "second");
        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn rejected_websocket_handshake_publishes_terminal_failure() {
        let ca = ProxyCa::generate("Transmog failed WebSocket", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 1).unwrap(),
        );
        let phases = Arc::new(StdMutex::new(Vec::new()));
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(Vec::new(), RuntimeLimits::default().hooks),
            Arc::new(CachedMitmCertificateResolver::new(ca, 32, 1).unwrap()),
        )
        .with_observer(
            Arc::new(LifecycleObserver {
                performance: Arc::new(StdMutex::new(
                    transmog_core::performance::PerformanceEvidence::default(),
                )),
                phases: phases.clone(),
                client_identity: Arc::new(StdMutex::new(None)),
            }),
            ObserverConfig::default(),
        );
        let proxy = ProxyServer::bind_with_components(ProxyConfig::default(), trust, components)
            .await
            .unwrap();
        let addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async {
            let _ = shutdown_rx.await;
        }));
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET http://localhost/socket HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: invalid\r\n\r\n").await.unwrap();
        assert!(
            read_http_head(&mut client)
                .await
                .starts_with(b"HTTP/1.1 400")
        );
        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(
            phases
                .lock()
                .unwrap()
                .iter()
                .filter(|phase| **phase == "failed")
                .count(),
            1
        );
        assert!(!phases.lock().unwrap().contains(&"completed"));
    }

    #[tokio::test]
    async fn websocket_without_hooks_is_a_byte_transparent_upgrade() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let request = String::from_utf8(read_http_head(&mut stream).await).unwrap();
            assert!(request.to_ascii_lowercase().contains("upgrade: websocket"));
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await
                .unwrap();
            let mut bytes = [0_u8; 22];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"not websocket framing!");
            stream.write_all(&bytes).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let proxy_ca = ProxyCa::generate("Transmog transparent websocket test", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                61,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    max_response_body_bytes: 4,
                    max_local_response_body_bytes: 16,
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            proxy_ca,
            trust,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
                    "GET ws://{origin_addr}/socket HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let response = String::from_utf8(read_http_head(&mut client).await).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"));
        client.write_all(b"not websocket framing!").await.unwrap();
        let mut echoed = [0_u8; 22];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"not websocket framing!");

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_inside_plain_connect_tunnel_is_a_byte_transparent_upgrade() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let request = String::from_utf8(read_http_head(&mut stream).await).unwrap();
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("get /socket http/1.1\r\n"));
            assert!(request.contains("upgrade: websocket"));
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await
                .unwrap();
            let mut bytes = [0_u8; 22];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"not websocket framing!");
            stream.write_all(&bytes).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let proxy_ca = ProxyCa::generate("Transmog CONNECT websocket test", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                63,
            )
            .unwrap(),
        );
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            proxy_ca,
            trust,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut rejected = TcpStream::connect(proxy_addr).await.unwrap();
        rejected
            .write_all(
                format!("CONNECT {origin_addr} HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let connect_response = String::from_utf8(read_http_head(&mut rejected).await).unwrap();
        assert!(connect_response.starts_with("HTTP/1.1 200"));
        rejected
            .write_all(format!("GET /plain HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let rejected_response = String::from_utf8(read_http_head(&mut rejected).await).unwrap();
        assert!(rejected_response.starts_with("HTTP/1.1 400"));
        drop(rejected);

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!("CONNECT {origin_addr} HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let connect_response = String::from_utf8(read_http_head(&mut client).await).unwrap();
        assert!(connect_response.starts_with("HTTP/1.1 200"));

        client
            .write_all(
                format!(
                    "GET /socket HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let response = String::from_utf8(read_http_head(&mut client).await).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"));
        client.write_all(b"not websocket framing!").await.unwrap();
        let mut echoed = [0_u8; 22];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"not websocket framing!");

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn websocket_hooks_modify_both_live_directions() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let _request = read_http_head(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await
                .unwrap();
            let mut decoder =
                MessageDecoder::new(Direction::ClientToServer, FrameLimits::default(), None)
                    .unwrap();
            let mut buffer = [0_u8; 256];
            let request_message = loop {
                let read = timeout(Duration::from_secs(2), stream.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(read, 0);
                if let Some(message) =
                    decoder
                        .push(&buffer[..read])
                        .unwrap()
                        .into_iter()
                        .find_map(|event| match event {
                            MessageEvent::Message(message) => Some(message),
                            MessageEvent::Control(_) => None,
                        })
                {
                    break message;
                }
            };
            assert_eq!(request_message.payload, "client-hook");
            let response = encode_websocket_frame(
                &WebSocketFrame {
                    fin: true,
                    compressed: false,
                    opcode: 1,
                    payload: Bytes::from_static(b"server"),
                },
                Direction::ServerToClient,
                None,
            )
            .unwrap();
            let close = encode_websocket_frame(
                &WebSocketFrame {
                    fin: true,
                    compressed: false,
                    opcode: 8,
                    payload: CloseFrame {
                        code: Some(1000),
                        reason: String::new(),
                    }
                    .encode()
                    .unwrap(),
                },
                Direction::ServerToClient,
                None,
            )
            .unwrap();
            stream.write_all(&response).await.unwrap();
            stream.write_all(&close).await.unwrap();
            loop {
                let read = timeout(Duration::from_secs(2), stream.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(read, 0);
                if decoder
                    .push(&buffer[..read])
                    .unwrap()
                    .into_iter()
                    .any(|event| matches!(event, MessageEvent::Control(_)))
                {
                    break;
                }
            }
        });

        let config = ProxyConfig {
            route_policy: RoutePolicy::Http1Only,
            ..ProxyConfig::default()
        };
        let proxy_ca = ProxyCa::generate("Transmog inspected websocket test", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
                62,
            )
            .unwrap(),
        );
        let certificates = Arc::new(
            CachedMitmCertificateResolver::new(
                proxy_ca,
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let websocket_hooks = WebSocketHookFactory::new(
            vec![
                WebSocketHookRegistration::new(
                    "append",
                    "append marker",
                    Arc::new(AppendWebSocketFactory),
                )
                .unwrap(),
            ],
            WebSocketHookLimits::default(),
        )
        .unwrap();
        let http_hooks = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "application",
                Arc::new(transmog_core::intercept::NoopInterceptorFactory),
                InterceptorRequirement::Required,
            )],
            config.limits.hooks,
        );
        let proxy = ProxyServer::bind_with_components(
            config,
            trust,
            ProxyComponents::new(http_hooks, certificates).with_websocket_hooks(websocket_hooks),
        )
        .await
        .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let mut websocket_evidence = proxy.subscribe_websocket_evidence();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET ws://{origin_addr}/socket HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let response = String::from_utf8(read_http_head(&mut client).await).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"));
        let request = encode_websocket_frame(
            &WebSocketFrame {
                fin: true,
                compressed: false,
                opcode: 1,
                payload: Bytes::from_static(b"client"),
            },
            Direction::ClientToServer,
            Some([1, 2, 3, 4]),
        )
        .unwrap();
        client.write_all(&request).await.unwrap();

        let mut decoder =
            MessageDecoder::new(Direction::ServerToClient, FrameLimits::default(), None).unwrap();
        let mut buffer = [0_u8; 256];
        let mut saw_message = false;
        loop {
            let read = timeout(Duration::from_secs(2), client.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(read, 0);
            let events = decoder.push(&buffer[..read]).unwrap();
            for event in events {
                match event {
                    MessageEvent::Message(message) => {
                        assert_eq!(message.kind, DataKind::Text);
                        assert_eq!(message.payload, "server-hook");
                        saw_message = true;
                    }
                    MessageEvent::Control(transmog_websocket::ControlFrame::Close(close)) => {
                        assert!(saw_message);
                        let reply = encode_websocket_frame(
                            &WebSocketFrame {
                                fin: true,
                                compressed: false,
                                opcode: 8,
                                payload: close.encode().unwrap(),
                            },
                            Direction::ClientToServer,
                            Some([5, 6, 7, 8]),
                        )
                        .unwrap();
                        client.write_all(&reply).await.unwrap();
                        let evidence = timeout(Duration::from_secs(2), websocket_evidence.recv())
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(evidence.target, format!("ws://{origin_addr}/socket"));
                        let WebSocketSessionOutcome::Completed(report) = evidence.outcome else {
                            panic!("expected completed WebSocket evidence");
                        };
                        assert!(report.clean_close);
                        assert_eq!(report.messages, 2);
                        assert_eq!(report.effects.len(), 4);
                        assert!(
                            report
                                .effects
                                .iter()
                                .all(|effect| effect.hook.id.as_str() == "append")
                        );
                        assert_eq!(
                            report
                                .effects
                                .iter()
                                .filter(|effect| {
                                    effect.action == transmog_websocket::HookActionKind::Replace
                                })
                                .count(),
                            2
                        );
                        shutdown_tx.send(()).unwrap();
                        proxy_task.await.unwrap().unwrap();
                        origin_task.await.unwrap();
                        return;
                    }
                    MessageEvent::Control(_) => {}
                }
            }
        }
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
        let ca = ProxyCa::generate("Transmog response streaming test", 2).unwrap();
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
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
    async fn queue_measurements_distinguish_pending_wait_and_canceled_send() {
        let metadata = ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(1),
                stream_id: StreamId(1),
                client_addr: "127.0.0.1:1".parse().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2".parse().unwrap(),
                ingress_version: HttpLegVersion::Http1,
                egress_version: None,
            },
            Target {
                scheme: "http".into(),
                authority: "example.invalid".into(),
                host: "example.invalid".into(),
                port: 80,
                path: "/".into(),
                query: None,
            },
        );
        let chain = Arc::new(
            transmog_core::intercept::InterceptorChainFactory::new(
                Vec::new(),
                transmog_core::intercept::HookLimits::default(),
            )
            .create_exchange(metadata)
            .unwrap(),
        );
        let performance = PerformanceRecorder::new(std::time::SystemTime::now(), Instant::now());
        chain.context().extensions().insert(performance.clone());
        let (sender, mut receiver) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"occupied"))))
            .await
            .unwrap();
        for cancel in [false, true] {
            let sender = sender.clone();
            let chain = chain.clone();
            let mut task = tokio::spawn(async move {
                send_streaming_frames(
                    &sender,
                    &mut StreamingBodyTracker::new(100),
                    vec![BodyFrame::Data(Bytes::from_static(b"next"))],
                    &chain,
                    "Client response queue",
                )
                .await
            });
            assert!(timeout(Duration::from_millis(20), &mut task).await.is_err());
            if cancel {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                assert!(receiver.recv().await.is_some());
                task.await.unwrap().unwrap();
            }
        }
        let snapshot = performance.snapshot();
        assert!(snapshot.valid());
        let wait = snapshot
            .work
            .iter()
            .find(|row| row.kind == "queue")
            .unwrap();
        assert_eq!(wait.calls, 2);
        assert!(wait.busy_nanos >= 30_000_000);
        assert!(
            snapshot
                .points
                .iter()
                .any(|point| point.milestone == Milestone::ClientResponseFirstBodyQueued)
        );
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
        let ca = ProxyCa::generate("Transmog request streaming test", 2).unwrap();
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
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
    async fn streaming_request_can_exceed_capture_and_buffering_limits() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = origin.local_addr().unwrap();
        let bytes = 32 * 1024 * 1024;
        let origin_task = tokio::spawn(async move {
            let (stream, _) = origin.accept().await.unwrap();
            let service = service_fn(move |mut request: Request<Incoming>| async move {
                let mut received = 0;
                while let Some(frame) = request.body_mut().frame().await {
                    if let Ok(data) = frame.unwrap().into_data() {
                        assert!(data.iter().all(|byte| *byte == 42));
                        received += data.len();
                    }
                }
                assert_eq!(received, bytes);
                Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"ok"),
                )))
            });
            hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 54).unwrap());
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                limits: RuntimeLimits {
                    max_request_body_bytes: usize::MAX,
                    max_buffered_request_body_bytes: 1024,
                    body_channel_capacity: 1,
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            },
            ProxyCa::generate("Transmog large request streaming", 2).unwrap(),
            trust,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        let proxy_address = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = shutdown_rx.await;
        }));
        let mut client = TcpStream::connect(proxy_address).await.unwrap();
        client.write_all(format!("POST http://{address}/large HTTP/1.1\r\nHost: {address}\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let chunk = vec![42; 32 * 1024];
        for _ in 0..1024 {
            client.write_all(&chunk).await.unwrap();
        }
        let mut response = Vec::new();
        timeout(Duration::from_secs(10), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
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
            ProxyCa::generate("Transmog stalled body test", 2).unwrap(),
            trust,
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
        let ca = ProxyCa::generate("Transmog request edit test", 2).unwrap();
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
        let ca = ProxyCa::generate("Transmog lifecycle test", 2).unwrap();
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
        let ca = ProxyCa::generate("Transmog body limit test", 2).unwrap();
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
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
        let ca = ProxyCa::generate("Transmog synthetic response test", 2).unwrap();
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
    async fn handler_can_stream_a_local_response_without_contacting_an_origin() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 115).unwrap());
        let ca = ProxyCa::generate("Transmog streaming response test", 2).unwrap();
        let proxy = ProxyServer::bind(
            ProxyConfig {
                route_policy: RoutePolicy::Http1Only,
                ..ProxyConfig::default()
            },
            ca,
            trust,
            Arc::new(StreamingLocalResponseHandler),
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
                b"GET http://127.0.0.1:9/local-stream HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 203"));
        assert_eq!(http1_response_body(&response), b"stream-asset");

        shutdown_tx.send(()).unwrap();
        proxy_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn connect_tls_is_intercepted_without_certificate_bypass() {
        let origin_ca = ProxyCa::generate("Transmog origin test root", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog CONNECT test root", 2).unwrap();
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
            let origin_ca = ProxyCa::generate("Transmog invalid upstream origin", 2).unwrap();
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
            let proxy_ca = ProxyCa::generate("Transmog invalid-cert proxy", 2).unwrap();
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
                Arc::new(transmog_core::intercept::NoopInterceptorFactory),
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
                    .windows(FAILED_EXCHANGE_BODY.len())
                    .any(|window| window == FAILED_EXCHANGE_BODY)
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
        let origin_ca = ProxyCa::generate("Transmog h2 origin root", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog h2 proxy root", 2).unwrap();
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
        let origin_ca = ProxyCa::generate("Transmog concurrent h2 origin", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog concurrent h2 proxy", 2).unwrap();
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
        let origin_ca = ProxyCa::generate("Transmog h2-to-h1 origin", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog h2-to-h1 proxy", 2).unwrap();
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
        let origin_ca = ProxyCa::generate("Transmog auto fallback origin", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog auto fallback proxy", 2).unwrap();
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
            .upstream
            .read()
            .await
            .partitions
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
        let proxy_ca = ProxyCa::generate("Transmog no replay proxy", 2).unwrap();
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
            Arc::new(transmog_core::intercept::NoopInterceptorFactory),
        )
        .await
        .unwrap();
        proxy
            .state
            .upstream
            .read()
            .await
            .partitions
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
        let origin_ca = ProxyCa::generate("Transmog proxy h3 origin", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog h3 proxy root", 2).unwrap();
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
        let origin_ca = ProxyCa::generate("Transmog h2-to-h3 origin", 2).unwrap();
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
        let proxy_ca = ProxyCa::generate("Transmog h2-to-h3 proxy", 2).unwrap();
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
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
    ) -> (SocketAddr, TestH3Origin) {
        spawn_h3_origin_with_groups(leaf, bind_ip, "X25519:P-256:P-384", 1).await
    }

    async fn spawn_h3_origin_with_groups(
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
        groups: &str,
        responses: usize,
    ) -> (SocketAddr, TestH3Origin) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_curves_list(groups).unwrap();
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
        let task = tokio::spawn(run_h3_origin(Arc::clone(&socket), local, config, responses));
        (local, TestH3Origin { socket, task })
    }

    async fn run_h3_origin(
        socket: Arc<UdpSocket>,
        local: SocketAddr,
        mut config: quiche::Config,
        responses: usize,
    ) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut http3 = None;
        let mut received = vec![0_u8; 65_535];
        let mut outgoing = vec![0_u8; 1_350];
        let mut sent_responses = 0;
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
                sent_responses += 1;
                if sent_responses >= responses {
                    return;
                }
            }
        }
    }
}
