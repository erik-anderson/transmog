use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use bytes::Bytes;
use http::{Method, StatusCode, uri::PathAndQuery};
use thiserror::Error;
use transmog_control_model::{
    Boundary, BreakpointInput, BreakpointPhase, Capability, ControlEvent, ControlExchangeId,
    DecisionAction, EventKind, Handshake, HeaderField as ControlHeaderField,
    RequestHead as ControlRequestHead, ResponseHead as ControlResponseHead,
};
use transmog_control_transport::{
    ControlController, ControlProducer, ControllerMessage, HandshakeError, NegotiatedSession,
    PendingDecision, RequestCancellation, TransportConfig, connect,
};
use transmog_core::{
    HeaderBlock, HeaderField, RequestHead, ResponseHead,
    intercept::{
        BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, BufferedBody, BufferedBodyHandler,
        ExchangeFailureKind, ExchangeInterceptor, ExchangeMetadata, HookAbort, HookEffectAction,
        HookInitError, InterceptorFactory, RequestBodyAction, RequestBodyEvent, RequestHeadAction,
        RequestHeadEvent, ResponseBodyAction, ResponseBodyEvent, ResponseHeadAction,
        ResponseHeadEvent,
    },
    observe::{ExchangeBoundary, ObserverEvent, ObserverEventKind},
};

/// Stable Hooks v2 identity used for every interactive controller effect.
pub const INTERACTIVE_CONTROL_HOOK_ID: &str = "transmog.session.interactive-control";

/// One explicitly enabled interactive breakpoint phase.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlPhase {
    /// Request head before routing.
    RequestHead,
    /// Complete decoded request body.
    RequestBody,
    /// Response head before downstream commitment.
    ResponseHead,
    /// Complete decoded response body.
    ResponseBody,
}

/// Breakpoint phases installed for one attached controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPolicy {
    /// Exact phase set in which the controller may affect traffic.
    pub phases: BTreeSet<ControlPhase>,
    /// Complete-body buffer and edit bound for either direction.
    pub body_limit: NonZeroUsize,
}

impl Default for ControlPolicy {
    fn default() -> Self {
        Self {
            phases: BTreeSet::from([ControlPhase::RequestHead, ControlPhase::ResponseHead]),
            body_limit: NonZeroUsize::new(4 * 1024 * 1024).expect("constant is nonzero"),
        }
    }
}

impl ControlPolicy {
    fn capabilities(&self) -> BTreeSet<Capability> {
        let mut capabilities = BTreeSet::from([Capability::HookEffects]);
        for phase in &self.phases {
            capabilities.insert(match phase {
                ControlPhase::RequestHead => Capability::RequestHead,
                ControlPhase::RequestBody => Capability::RequestBody,
                ControlPhase::ResponseHead => Capability::ResponseHead,
                ControlPhase::ResponseBody => Capability::ResponseBody,
            });
        }
        capabilities
    }

    fn enables(&self, phase: ControlPhase) -> bool {
        self.phases.contains(&phase)
    }
}

/// Monotonic controller publication counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControlStats {
    /// Lifecycle events accepted by the current controller queue.
    pub events_published: u64,
    /// Lifecycle events dropped because no controller was attached or its queue failed.
    pub events_dropped: u64,
}

/// Failure attaching a same-build controller.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ControlConnectionError {
    /// Another controller lease remains attached.
    #[error("an interactive controller is already attached")]
    AlreadyAttached,
    /// Same-build transport negotiation failed.
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
    /// The peer did not negotiate one of the configured breakpoint capabilities.
    #[error("controller did not negotiate required capability {0:?}")]
    MissingCapability(Capability),
    /// Peers negotiated an invalid zero body-edit bound.
    #[error("controller negotiated an invalid zero body-edit bound")]
    InvalidBodyLimit,
}

#[derive(Clone, Debug)]
struct Attachment {
    generation: u64,
    producer: ControlProducer,
    policy: ControlPolicy,
    next_event: u64,
}

#[derive(Debug, Default)]
struct ConnectorState {
    attachment: Option<Attachment>,
    next_generation: u64,
}

#[derive(Debug)]
struct ConnectorInner {
    build_id: Arc<str>,
    state: Mutex<ConnectorState>,
    published: AtomicU64,
    dropped: AtomicU64,
}

/// Same-process control endpoint manager and Hooks v2 adapter source.
#[derive(Clone, Debug)]
pub struct ControlConnector {
    inner: Arc<ConnectorInner>,
}

impl ControlConnector {
    /// Creates a same-build connector with an opaque nonempty build identity.
    ///
    /// # Errors
    ///
    /// Returns [`ControlConnectionError::Handshake`] when `build_id` is empty.
    pub fn new(build_id: impl Into<Arc<str>>) -> Result<Self, ControlConnectionError> {
        let build_id = build_id.into();
        Handshake::v0(build_id.to_string(), 1)
            .validate()
            .map_err(|error| {
                ControlConnectionError::Handshake(HandshakeError::InvalidProducer(error))
            })?;
        Ok(Self {
            inner: Arc::new(ConnectorInner {
                build_id,
                state: Mutex::new(ConnectorState::default()),
                published: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
            }),
        })
    }

    /// Negotiates and exclusively attaches one controller.
    ///
    /// # Errors
    ///
    /// Returns a typed exclusivity, handshake, or capability error.
    pub fn connect(
        &self,
        peer: &Handshake,
        transport: TransportConfig,
        mut policy: ControlPolicy,
    ) -> Result<AttachedController, ControlConnectionError> {
        let mut state = self.lock_state();
        if state.attachment.is_some() {
            return Err(ControlConnectionError::AlreadyAttached);
        }
        let mut local = Handshake::v0(self.inner.build_id.to_string(), policy.body_limit.get());
        local.capabilities = policy.capabilities();
        let (producer, controller) = connect(&local, peer, transport)?;
        for capability in policy.capabilities() {
            if !producer.negotiated().capabilities.contains(&capability) {
                return Err(ControlConnectionError::MissingCapability(capability));
            }
        }
        policy.body_limit = NonZeroUsize::new(producer.negotiated().max_body_edit_bytes)
            .ok_or(ControlConnectionError::InvalidBodyLimit)?;
        state.next_generation = state.next_generation.saturating_add(1);
        let generation = state.next_generation;
        state.attachment = Some(Attachment {
            generation,
            producer,
            policy,
            next_event: 0,
        });
        Ok(AttachedController {
            controller,
            connector: self.clone(),
            generation,
        })
    }

    /// Returns current event publication counters.
    pub fn stats(&self) -> ControlStats {
        ControlStats {
            events_published: self.inner.published.load(Ordering::Relaxed),
            events_dropped: self.inner.dropped.load(Ordering::Relaxed),
        }
    }

    /// Creates a dynamic interceptor factory for registration under
    /// [`INTERACTIVE_CONTROL_HOOK_ID`].
    pub fn interceptor_factory(&self) -> Arc<dyn InterceptorFactory> {
        Arc::new(ControlInterceptorFactory {
            connector: self.clone(),
        })
    }

    pub(crate) fn publish(&self, event: &ObserverEvent) {
        let Some(kind) = event_kind(&event.kind) else {
            return;
        };
        let (producer, sequence) = {
            let mut state = self.lock_state();
            let Some(attachment) = state.attachment.as_mut() else {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            };
            attachment.next_event = attachment.next_event.saturating_add(1);
            (attachment.producer.clone(), attachment.next_event)
        };
        if producer
            .publish(ControlEvent {
                sequence,
                exchange_id: ControlExchangeId(event.exchange_id.0),
                kind,
            })
            .is_ok()
        {
            self.inner.published.fetch_add(1, Ordering::Relaxed);
        } else {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn current(&self) -> Option<(ControlProducer, ControlPolicy)> {
        self.lock_state()
            .attachment
            .as_ref()
            .map(|attachment| (attachment.producer.clone(), attachment.policy.clone()))
    }

    fn detach(&self, generation: u64) {
        let mut state = self.lock_state();
        if state
            .attachment
            .as_ref()
            .is_some_and(|attachment| attachment.generation == generation)
        {
            state.attachment = None;
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ConnectorState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Exclusive controller lease. Dropping it permits a later attachment.
#[derive(Debug)]
pub struct AttachedController {
    controller: ControlController,
    connector: ControlConnector,
    generation: u64,
}

impl AttachedController {
    /// Returns immutable negotiated properties.
    pub fn negotiated(&self) -> &NegotiatedSession {
        self.controller.negotiated()
    }

    /// Receives one bounded lifecycle event.
    pub async fn recv_event(&mut self) -> Option<ControlEvent> {
        self.controller.recv_event().await
    }

    /// Receives one pending phase-typed breakpoint decision.
    pub async fn recv_decision(&mut self) -> Option<PendingDecision> {
        self.controller.recv_decision().await
    }

    /// Receives the next lifecycle event or pending decision without starving either queue.
    pub async fn recv(&mut self) -> Option<ControllerMessage> {
        self.controller.recv().await
    }
}

impl Drop for AttachedController {
    fn drop(&mut self) {
        self.connector.detach(self.generation);
    }
}

#[derive(Clone, Debug)]
struct ControlInterceptorFactory {
    connector: ControlConnector,
}

impl InterceptorFactory for ControlInterceptorFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(ControlInterceptor {
            attachment: self.connector.current(),
        }))
    }
}

#[derive(Clone, Debug)]
struct ControlInterceptor {
    attachment: Option<(ControlProducer, ControlPolicy)>,
}

impl ExchangeInterceptor for ControlInterceptor {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        let Some((producer, policy)) = self.attachment.clone() else {
            return Box::pin(async { RequestHeadAction::Continue });
        };
        if !policy.enables(ControlPhase::RequestHead) {
            return Box::pin(async { RequestHeadAction::Continue });
        }
        Box::pin(async move {
            let input = BreakpointInput {
                exchange_id: ControlExchangeId(event.context.metadata().exchange_id.0),
                phase: BreakpointPhase::RequestHead,
                request_head: Some(request_to_control(&event.head)),
                response_head: None,
                body: None,
            };
            match request_decision(&producer, input, &event.context).await {
                Ok(DecisionAction::Continue) => RequestHeadAction::Continue,
                Ok(DecisionAction::ReplaceRequestHead { head }) => {
                    match request_from_control(head, &event.head) {
                        Ok(head) => RequestHeadAction::Replace(head),
                        Err(reason) => RequestHeadAction::Abort(HookAbort::Policy(reason)),
                    }
                }
                Ok(DecisionAction::Abort { reason }) => {
                    RequestHeadAction::Abort(HookAbort::Policy(reason))
                }
                Ok(_) => RequestHeadAction::Abort(invalid_action()),
                Err(reason) => RequestHeadAction::Abort(HookAbort::Policy(reason)),
            }
        })
    }

    fn on_request_body(&self, event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        let Some((producer, policy)) = self.attachment.clone() else {
            return Box::pin(async { RequestBodyAction::pass_through() });
        };
        if !policy.enables(ControlPhase::RequestBody) {
            return Box::pin(async { RequestBodyAction::pass_through() });
        }
        Box::pin(async move {
            RequestBodyAction::decoded(BodyPlan::Buffer {
                limit: policy.body_limit,
                handler: Box::new(ControlBodyHandler {
                    producer,
                    exchange_id: event.context.metadata().exchange_id.0,
                    phase: BreakpointPhase::RequestBody,
                    cancellation: event.context.cancellation().clone(),
                    limit: policy.body_limit,
                }),
            })
        })
    }

    fn on_response_head(&self, event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        let Some((producer, policy)) = self.attachment.clone() else {
            return Box::pin(async { ResponseHeadAction::Continue });
        };
        if !policy.enables(ControlPhase::ResponseHead) {
            return Box::pin(async { ResponseHeadAction::Continue });
        }
        Box::pin(async move {
            let input = BreakpointInput {
                exchange_id: ControlExchangeId(event.context.metadata().exchange_id.0),
                phase: BreakpointPhase::ResponseHead,
                request_head: None,
                response_head: Some(response_to_control(&event.head)),
                body: None,
            };
            match request_decision(&producer, input, &event.context).await {
                Ok(DecisionAction::Continue) => ResponseHeadAction::Continue,
                Ok(DecisionAction::ReplaceResponseHead { head }) => {
                    match response_from_control(head, &event.head) {
                        Ok(head) => ResponseHeadAction::Replace(head),
                        Err(reason) => ResponseHeadAction::Abort(HookAbort::Policy(reason)),
                    }
                }
                Ok(DecisionAction::Abort { reason }) => {
                    ResponseHeadAction::Abort(HookAbort::Policy(reason))
                }
                Ok(_) => ResponseHeadAction::Abort(invalid_action()),
                Err(reason) => ResponseHeadAction::Abort(HookAbort::Policy(reason)),
            }
        })
    }

    fn on_response_body(&self, event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        let Some((producer, policy)) = self.attachment.clone() else {
            return Box::pin(async { ResponseBodyAction::pass_through() });
        };
        if !policy.enables(ControlPhase::ResponseBody) {
            return Box::pin(async { ResponseBodyAction::pass_through() });
        }
        Box::pin(async move {
            ResponseBodyAction::decoded(BodyPlan::Buffer {
                limit: policy.body_limit,
                handler: Box::new(ControlBodyHandler {
                    producer,
                    exchange_id: event.context.metadata().exchange_id.0,
                    phase: BreakpointPhase::ResponseBody,
                    cancellation: event.context.cancellation().clone(),
                    limit: policy.body_limit,
                }),
            })
        })
    }
}

struct ControlBodyHandler {
    producer: ControlProducer,
    exchange_id: u128,
    phase: BreakpointPhase,
    cancellation: transmog_core::intercept::ExchangeCancellation,
    limit: NonZeroUsize,
}

impl BufferedBodyHandler for ControlBodyHandler {
    fn on_body(
        &mut self,
        body: BufferedBody,
    ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>> {
        Box::pin(async move {
            let cancellation = RequestCancellation::new();
            let input = BreakpointInput {
                exchange_id: ControlExchangeId(self.exchange_id),
                phase: self.phase,
                request_head: None,
                response_head: None,
                body: Some(body.data().to_vec()),
            };
            let result = tokio::select! {
                result = self.producer.request(input, &cancellation) => result,
                () = self.cancellation.cancelled() => {
                    cancellation.cancel();
                    return Err(BodyHookError::Abort(HookAbort::Policy(
                        "interactive controller decision cancelled".into(),
                    )));
                }
            };
            match result {
                Ok(DecisionAction::Continue) => Ok(body),
                Ok(DecisionAction::ReplaceBody { body: replacement }) => BufferedBody::try_new(
                    self.limit.get(),
                    Bytes::from(replacement),
                    body.trailers().cloned(),
                )
                .map_err(BodyHookError::Limit),
                Ok(DecisionAction::Abort { reason }) => {
                    Err(BodyHookError::Abort(HookAbort::Policy(reason)))
                }
                Ok(_) => Err(BodyHookError::Abort(invalid_action())),
                Err(_) => Err(BodyHookError::Abort(HookAbort::Policy(
                    "interactive controller unavailable".into(),
                ))),
            }
        })
    }
}

async fn request_decision(
    producer: &ControlProducer,
    input: BreakpointInput,
    context: &transmog_core::intercept::HookContext,
) -> Result<DecisionAction, String> {
    let cancellation = RequestCancellation::new();
    tokio::select! {
        result = producer.request(input, &cancellation) => result.map_err(|_| {
            "interactive controller unavailable".to_owned()
        }),
        () = context.cancellation().cancelled() => {
            cancellation.cancel();
            Err("interactive controller decision cancelled".to_owned())
        }
    }
}

fn request_to_control(head: &RequestHead) -> ControlRequestHead {
    ControlRequestHead {
        method: head.method.clone(),
        scheme: head.target.scheme.clone(),
        authority: head.target.authority.clone(),
        path_and_query: path_and_query(&head.target.path, head.target.query.as_deref()),
        headers: headers_to_control(&head.headers),
    }
}

fn response_to_control(head: &ResponseHead) -> ControlResponseHead {
    ControlResponseHead {
        status: head.status,
        headers: headers_to_control(&head.headers),
    }
}

fn headers_to_control(headers: &HeaderBlock) -> Vec<ControlHeaderField> {
    headers
        .iter()
        .map(|field| ControlHeaderField {
            name: String::from_utf8_lossy(field.name()).into_owned(),
            value: field.value().to_vec(),
        })
        .collect()
}

fn request_from_control(
    replacement: ControlRequestHead,
    current: &RequestHead,
) -> Result<RequestHead, String> {
    if !replacement
        .scheme
        .eq_ignore_ascii_case(&current.target.scheme)
        || !replacement
            .authority
            .eq_ignore_ascii_case(&current.target.authority)
    {
        return Err("request-head edits cannot change scheme or authority".into());
    }
    Method::from_bytes(replacement.method.as_bytes())
        .map_err(|_| "controller returned an invalid HTTP method".to_owned())?;
    let path = replacement
        .path_and_query
        .parse::<PathAndQuery>()
        .map_err(|_| "controller returned an invalid path and query".to_owned())?;
    let mut target = current.target.clone();
    path.path().clone_into(&mut target.path);
    target.query = path.query().map(str::to_owned);
    Ok(RequestHead {
        method: replacement.method,
        target,
        headers: headers_from_control(replacement.headers)?,
        source_version: current.source_version,
    })
}

fn response_from_control(
    replacement: ControlResponseHead,
    current: &ResponseHead,
) -> Result<ResponseHead, String> {
    StatusCode::from_u16(replacement.status)
        .map_err(|_| "controller returned an invalid HTTP status".to_owned())?;
    Ok(ResponseHead {
        status: replacement.status,
        headers: headers_from_control(replacement.headers)?,
        source_version: current.source_version,
    })
}

fn headers_from_control(headers: Vec<ControlHeaderField>) -> Result<HeaderBlock, String> {
    headers
        .into_iter()
        .map(|field| {
            HeaderField::try_new(field.name, field.value)
                .map_err(|_| "controller returned an invalid HTTP header".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(HeaderBlock::from_fields)
}

fn path_and_query(path: &str, query: Option<&str>) -> String {
    query.map_or_else(|| path.to_owned(), |query| format!("{path}?{query}"))
}

fn invalid_action() -> HookAbort {
    HookAbort::Policy("controller returned an action invalid for this phase".into())
}

fn event_kind(kind: &ObserverEventKind) -> Option<EventKind> {
    match kind {
        ObserverEventKind::ExchangeStarted { .. } => Some(EventKind::ExchangeStarted),
        ObserverEventKind::RequestHeadObserved { boundary, .. }
        | ObserverEventKind::ResponseHeadObserved { boundary, .. } => {
            Some(EventKind::HeadObserved {
                boundary: boundary_to_control(*boundary),
            })
        }
        ObserverEventKind::BodyChunk(chunk) => Some(EventKind::BodyObserved {
            boundary: boundary_to_control(chunk.boundary),
            byte_count: chunk.byte_count,
        }),
        ObserverEventKind::HookEffect(effect) => Some(EventKind::HookEffect {
            hook_id: effect.interceptor.id.as_str().to_owned(),
            action: hook_action_name(&effect.action).to_owned(),
        }),
        ObserverEventKind::Completed(_) => Some(EventKind::Completed),
        ObserverEventKind::Failed(failure) => Some(EventKind::Failed {
            category: failure_kind_name(&failure.kind).to_owned(),
        }),
        ObserverEventKind::HookInitializationSkipped(_)
        | ObserverEventKind::RequestHeadFinalized(_)
        | ObserverEventKind::BodyTrailers(_)
        | ObserverEventKind::RouteSelected { .. }
        | ObserverEventKind::RouteAttempt(_)
        | ObserverEventKind::ResponseHeadFinalized(_) => None,
    }
}

const fn hook_action_name(action: &HookEffectAction) -> &'static str {
    match action {
        HookEffectAction::ReplaceRequestHead(_) => "replace-request-head",
        HookEffectAction::Reroute { .. } => "reroute",
        HookEffectAction::ReplaceResponseHead(_) => "replace-response-head",
        HookEffectAction::Respond { .. } => "respond",
        HookEffectAction::Abort(_) => "abort",
        HookEffectAction::BodyPlan { .. } => "body-plan",
    }
}

const fn failure_kind_name(kind: &ExchangeFailureKind) -> &'static str {
    match kind {
        ExchangeFailureKind::HookInitialization => "hook-initialization",
        ExchangeFailureKind::HookTimedOut => "hook-timed-out",
        ExchangeFailureKind::HookPanicked => "hook-panicked",
        ExchangeFailureKind::HookAborted(_) => "hook-aborted",
        ExchangeFailureKind::Cancelled => "cancelled",
        ExchangeFailureKind::Body => "body",
        ExchangeFailureKind::RequestTranslation => "request-translation",
        ExchangeFailureKind::ResponseTranslation => "response-translation",
        ExchangeFailureKind::Route => "route",
        ExchangeFailureKind::Upstream => "upstream",
        ExchangeFailureKind::Shutdown => "shutdown",
    }
}

const fn boundary_to_control(boundary: ExchangeBoundary) -> Boundary {
    match boundary {
        ExchangeBoundary::ClientRequest => Boundary::ClientRequest,
        ExchangeBoundary::UpstreamRequest => Boundary::UpstreamRequest,
        ExchangeBoundary::UpstreamResponse => Boundary::UpstreamResponse,
        ExchangeBoundary::ClientResponse => Boundary::ClientResponse,
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use transmog_control_model::DecisionCommand;
    use transmog_core::{
        BodyFrame, ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{
            BodyPipelineLimits, HookContext, HookLimits, InterceptorChainFactory,
            InterceptorRegistration, InterceptorRequirement, RequestHeadOutcome,
        },
    };

    use super::*;

    fn handshake(policy: &ControlPolicy) -> Handshake {
        let mut handshake = Handshake::v0("build", policy.body_limit.get());
        handshake.capabilities = policy.capabilities();
        handshake
    }

    fn config() -> TransportConfig {
        TransportConfig {
            event_capacity: NonZeroUsize::new(4).unwrap(),
            decision_capacity: NonZeroUsize::new(4).unwrap(),
            decision_timeout: Duration::from_secs(1),
        }
    }

    fn head() -> RequestHead {
        RequestHead {
            method: "GET".into(),
            target: Target {
                scheme: "https".into(),
                authority: "example.test".into(),
                host: "example.test".into(),
                port: 443,
                path: "/before".into(),
                query: None,
            },
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        }
    }

    fn context() -> HookContext {
        HookContext::new(
            ExchangeMetadata::from_session(
                &SessionMetadata {
                    session_id: SessionId(7),
                    downstream_connection_id: ConnectionId(8),
                    stream_id: StreamId(9),
                    client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                    client_identity: transmog_core::ClientIdentity::default(),
                    proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
                    ingress_version: HttpLegVersion::Http2,
                    egress_version: None,
                },
                head().target,
            ),
            Duration::from_secs(2),
        )
    }

    fn command(pending: &PendingDecision, action: DecisionAction) -> DecisionCommand {
        DecisionCommand {
            decision_id: pending.request().decision_id,
            exchange_id: pending.request().exchange_id,
            action,
        }
    }

    #[tokio::test]
    async fn request_head_edit_preserves_authority_and_is_phase_typed() {
        let connector = ControlConnector::new("build").unwrap();
        let policy = ControlPolicy::default();
        let mut controller = connector
            .connect(&handshake(&policy), config(), policy)
            .unwrap();
        let interceptor = connector
            .interceptor_factory()
            .create(context().metadata())
            .unwrap();
        let controller_task = tokio::spawn(async move {
            let pending = controller.recv_decision().await.unwrap();
            let mut replacement = pending.request().request_head.clone().unwrap();
            replacement.path_and_query = "/after?q=1".into();
            pending
                .reply(command(
                    &pending,
                    DecisionAction::ReplaceRequestHead { head: replacement },
                ))
                .unwrap();
        });
        let action = interceptor
            .on_request_head(RequestHeadEvent {
                context: context(),
                head: head(),
            })
            .await;
        let RequestHeadAction::Replace(replacement) = action else {
            panic!("expected request replacement");
        };
        assert_eq!(replacement.target.authority, "example.test");
        assert_eq!(replacement.target.path, "/after");
        assert_eq!(replacement.target.query.as_deref(), Some("q=1"));
        controller_task.await.unwrap();
    }

    #[tokio::test]
    async fn authority_change_and_wrong_phase_action_fail_closed() {
        for wrong_phase in [false, true] {
            let connector = ControlConnector::new("build").unwrap();
            let policy = ControlPolicy::default();
            let mut controller = connector
                .connect(&handshake(&policy), config(), policy)
                .unwrap();
            let interceptor = connector
                .interceptor_factory()
                .create(context().metadata())
                .unwrap();
            let controller_task = tokio::spawn(async move {
                let pending = controller.recv_decision().await.unwrap();
                let action = if wrong_phase {
                    DecisionAction::ReplaceBody { body: vec![1] }
                } else {
                    let mut replacement = pending.request().request_head.clone().unwrap();
                    replacement.authority = "attacker.test".into();
                    DecisionAction::ReplaceRequestHead { head: replacement }
                };
                pending.reply(command(&pending, action)).unwrap();
            });
            assert!(matches!(
                interceptor
                    .on_request_head(RequestHeadEvent {
                        context: context(),
                        head: head(),
                    })
                    .await,
                RequestHeadAction::Abort(HookAbort::Policy(_))
            ));
            controller_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn body_replacement_is_bounded_decoded_and_audited() {
        let connector = ControlConnector::new("build").unwrap();
        let policy = ControlPolicy {
            phases: BTreeSet::from([ControlPhase::RequestBody]),
            body_limit: NonZeroUsize::new(16).unwrap(),
        };
        let mut controller = connector
            .connect(&handshake(&policy), config(), policy)
            .unwrap();
        let factory = InterceptorChainFactory::new(
            vec![InterceptorRegistration::named(
                INTERACTIVE_CONTROL_HOOK_ID,
                "Interactive controller",
                connector.interceptor_factory(),
                InterceptorRequirement::Required,
            )],
            HookLimits::default(),
        );
        let mut chain = factory
            .create_exchange(context().metadata().as_ref().clone())
            .unwrap();
        let request = match chain.request_head(head()).await.unwrap() {
            RequestHeadOutcome::Continue { head, .. } => head,
            other => panic!("unexpected request action: {other:?}"),
        };
        let mut pipeline = chain
            .request_body_pipeline(&request, BodyPipelineLimits::default())
            .await
            .unwrap();
        let controller_task = tokio::spawn(async move {
            let pending = controller.recv_decision().await.unwrap();
            assert_eq!(pending.request().phase, BreakpointPhase::RequestBody);
            assert_eq!(
                pending.request().body.as_deref(),
                Some(b"before".as_slice())
            );
            pending
                .reply(command(
                    &pending,
                    DecisionAction::ReplaceBody {
                        body: b"after".to_vec(),
                    },
                ))
                .unwrap();
        });
        assert!(
            pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"before")))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            pipeline.finish().await.unwrap(),
            vec![BodyFrame::Data(Bytes::from_static(b"after"))]
        );
        controller_task.await.unwrap();
        let effects = chain.hook_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0].interceptor.id.as_str(),
            INTERACTIVE_CONTROL_HOOK_ID
        );
    }

    #[test]
    fn attachment_is_exclusive_and_capabilities_are_required() {
        let connector = ControlConnector::new("build").unwrap();
        let policy = ControlPolicy::default();
        let controller = connector
            .connect(&handshake(&policy), config(), policy.clone())
            .unwrap();
        assert!(matches!(
            connector.connect(&handshake(&policy), config(), policy.clone()),
            Err(ControlConnectionError::AlreadyAttached)
        ));
        drop(controller);
        let peer = Handshake::v0("build", policy.body_limit.get());
        assert!(matches!(
            connector.connect(&peer, config(), policy),
            Err(ControlConnectionError::MissingCapability(_))
        ));
    }
}
