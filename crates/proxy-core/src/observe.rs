use std::{
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use thiserror::Error;
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

use crate::{
    ExchangeExtensions, HeaderBlock, RequestHead, ResponseHead,
    intercept::{
        CompletedExchange, ExchangeCancellation, ExchangeFailure, ExchangeId, ExchangeMetadata,
        HookEffect, InitializationDiagnostic,
    },
    task::AbortOnDrop,
};

/// Boxed asynchronous observer callback.
pub type BoxObserverFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), ObserverError>> + Send + 'a>>;

/// Immutable lifecycle-event consumer.
pub trait Observer: Send + Sync {
    /// Consumes one event filtered and redacted according to its declared
    /// observation interest.
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_>;
}

/// Redacted observer implementation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("observer failed: {message}")]
pub struct ObserverError {
    /// Operator-safe failure description.
    pub message: String,
}

impl ObserverError {
    /// Creates an operator-safe observer failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Whether and how body bytes are exposed to an observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyObservation {
    /// Report byte counts only.
    MetadataOnly,
    /// Include at most this many leading bytes from each emitted chunk.
    Prefix(NonZeroUsize),
    /// Include every byte in each frame while retaining finite in-flight queue bounds.
    Full,
}

/// Explicit event-interest declaration for one observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationInterest {
    /// Receive lifecycle and finalized-head events.
    pub lifecycle: bool,
    /// Receive credential-bearing request and response fields.
    ///
    /// This is disabled by default and should be used only by protected local
    /// stores that never expose the values through presentation read models.
    pub sensitive_headers: bool,
    /// Request-body observation policy.
    pub request_body: BodyObservation,
    /// Response-body observation policy.
    pub response_body: BodyObservation,
}

impl Default for ObservationInterest {
    fn default() -> Self {
        Self {
            lifecycle: true,
            sensitive_headers: false,
            request_body: BodyObservation::MetadataOnly,
            response_body: BodyObservation::MetadataOnly,
        }
    }
}

/// Behavior when one observer's finite queue is full.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverDeliveryPolicy {
    /// Wait for queue capacity for no longer than the configured duration.
    Backpressure {
        /// Maximum time to wait for queue capacity.
        timeout: Duration,
    },
    /// Drop the new event and increment the visible loss counter.
    DropNewest,
    /// Disconnect the observer on the first saturated delivery.
    Disconnect,
}

/// Bounded observer registration settings.
#[derive(Clone, Copy, Debug)]
pub struct ObserverConfig {
    /// Event selection and body-byte policy.
    pub interest: ObservationInterest,
    /// Finite event queue capacity.
    pub queue_capacity: NonZeroUsize,
    /// Queue saturation behavior.
    pub delivery: ObserverDeliveryPolicy,
    /// Maximum duration of one observer callback.
    pub callback_timeout: Duration,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            interest: ObservationInterest::default(),
            queue_capacity: NonZeroUsize::new(256).expect("256 is nonzero"),
            delivery: ObserverDeliveryPolicy::DropNewest,
            callback_timeout: Duration::from_secs(2),
        }
    }
}

/// Direction of an observed body chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyDirection {
    /// Client-to-upstream request bytes.
    Request,
    /// Upstream-to-client response bytes.
    Response,
}

/// Logical exchange boundary at which a head or body frame was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExchangeBoundary {
    /// Request as received from the client, before request hooks.
    ClientRequest,
    /// Effective request committed toward the selected upstream.
    UpstreamRequest,
    /// Response received from the upstream, before response hooks.
    UpstreamResponse,
    /// Effective response committed toward the client.
    ClientResponse,
}

impl ExchangeBoundary {
    /// Body direction associated with this boundary.
    pub const fn direction(self) -> BodyDirection {
        match self {
            Self::ClientRequest | Self::UpstreamRequest => BodyDirection::Request,
            Self::UpstreamResponse | Self::ClientResponse => BodyDirection::Response,
        }
    }
}

/// Bounded body observation payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedBodyChunk {
    /// Logical boundary where these bytes were visible.
    pub boundary: ExchangeBoundary,
    /// Original chunk length even when the sample is truncated.
    pub byte_count: usize,
    /// Explicitly requested bounded prefix; absent by default.
    pub sample: Option<Bytes>,
    /// Whether bytes were omitted from the sample.
    pub truncated: bool,
}

/// Redacted terminal body trailers observed at one exchange boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedBodyTrailers {
    /// Logical boundary where the trailers were visible.
    pub boundary: ExchangeBoundary,
    /// Ordered, duplicate-preserving trailer fields.
    pub trailers: HeaderBlock,
}

/// Redacted outcome of one concrete upstream transport attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedRouteAttempt {
    /// HTTP version attempted on the upstream leg.
    pub protocol: crate::HttpLegVersion,
    /// Stable, operator-safe outcome category.
    pub outcome: Arc<str>,
}

/// Immutable observer lifecycle payload.
#[derive(Clone, Debug)]
pub enum ObserverEventKind {
    /// Bounded measured timing, protocol and transport evidence.
    Performance(crate::performance::PerformanceEvidence),
    /// An exchange and its isolated extension store were created.
    ExchangeStarted {
        /// Immutable exchange metadata.
        metadata: Arc<ExchangeMetadata>,
    },
    /// An optional interceptor could not initialize and was skipped.
    HookInitializationSkipped(InitializationDiagnostic),
    /// Request head at an explicit client or upstream boundary.
    RequestHeadObserved {
        /// Boundary where the head was visible.
        boundary: ExchangeBoundary,
        /// Protocol-neutral request head.
        head: RequestHead,
    },
    /// Response head at an explicit upstream or client boundary.
    ResponseHeadObserved {
        /// Boundary where the head was visible.
        boundary: ExchangeBoundary,
        /// Protocol-neutral response head.
        head: ResponseHead,
    },
    /// One centrally attributed hook effect.
    HookEffect(HookEffect),
    /// The final request head after all request hooks.
    RequestHeadFinalized(RequestHead),
    /// One request or response body chunk.
    BodyChunk(ObservedBodyChunk),
    /// Terminal body trailers.
    BodyTrailers(ObservedBodyTrailers),
    /// Redacted, auditable route-selection evidence.
    RouteSelected {
        /// Stable route policy identity.
        policy_id: Arc<str>,
        /// Operator-safe selection reason.
        reason: Arc<str>,
    },
    /// One concrete upstream connection or protocol attempt.
    RouteAttempt(ObservedRouteAttempt),
    /// The final response head before downstream commitment.
    ResponseHeadFinalized(ResponseHead),
    /// Successful terminal outcome.
    Completed(CompletedExchange),
    /// Failed terminal outcome.
    Failed(ExchangeFailure),
}

impl ObserverEventKind {
    fn is_lifecycle(&self) -> bool {
        !matches!(self, Self::BodyChunk(_) | Self::BodyTrailers(_))
    }
}

/// Sequenced event delivered to one observer.
#[derive(Clone, Debug)]
pub struct ObserverEvent {
    /// Exchange identifier.
    pub exchange_id: ExchangeId,
    /// Monotonic per-exchange sequence number, beginning at one.
    pub sequence: u64,
    /// Typed immutable payload.
    pub kind: ObserverEventKind,
}

impl ObserverEvent {
    /// Applies credential redaction before an application stores an event.
    #[must_use]
    pub fn redacted(mut self) -> Self {
        self.kind = redact(self.kind);
        self
    }
}

/// Outcome of one attempted observer delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverDelivery {
    /// Event was accepted by the bounded queue.
    Delivered,
    /// Event was intentionally dropped by `DropNewest`.
    Dropped,
    /// Backpressure exceeded its declared deadline.
    BackpressureTimedOut,
    /// Observer was disconnected by this saturated delivery.
    Disconnected,
    /// Observer had already disconnected or failed.
    AlreadyDisconnected,
}

/// Per-observer results for one emitted event.
#[derive(Clone, Debug, Default)]
pub struct ObserverDeliveryReport {
    deliveries: Vec<ObserverDelivery>,
}

impl ObserverDeliveryReport {
    /// Delivery result in registration order.
    pub fn deliveries(&self) -> &[ObserverDelivery] {
        &self.deliveries
    }

    /// Whether a deliberately backpressuring observer exceeded its deadline.
    pub fn backpressure_timed_out(&self) -> bool {
        self.deliveries
            .contains(&ObserverDelivery::BackpressureTimedOut)
    }
}

/// Live counters for one bounded observer adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObserverStats {
    /// Events accepted by the queue.
    pub delivered: u64,
    /// Events dropped by queue policy.
    pub dropped: u64,
    /// Whether the worker is disconnected.
    pub disconnected: bool,
}

struct ObserverDispatcher {
    sender: mpsc::Sender<ObserverEvent>,
    interest: ObservationInterest,
    delivery: ObserverDeliveryPolicy,
    delivered: AtomicU64,
    dropped: AtomicU64,
    disconnected: Arc<AtomicBool>,
    force_stop: ExchangeCancellation,
    drain: ExchangeCancellation,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for ObserverDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObserverDispatcher")
            .field("interest", &self.interest)
            .field("delivery", &self.delivery)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl ObserverDispatcher {
    fn new(observer: Arc<dyn Observer>, config: ObserverConfig) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel(config.queue_capacity.get());
        let disconnected = Arc::new(AtomicBool::new(false));
        let worker_disconnected = Arc::clone(&disconnected);
        let force_stop = ExchangeCancellation::new();
        let worker_stop = force_stop.clone();
        let drain = ExchangeCancellation::new();
        let worker_drain = drain.clone();
        let worker = tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    event = receiver.recv() => event,
                    () = worker_stop.cancelled() => break,
                    () = worker_drain.cancelled() => {
                        receiver.close();
                        while let Some(event) = receiver.recv().await {
                            if !deliver_observer_event(
                                Arc::clone(&observer),
                                event,
                                config.callback_timeout,
                                &worker_stop,
                            ).await {
                                break;
                            }
                        }
                        break;
                    },
                };
                let Some(event) = event else {
                    break;
                };
                if !deliver_observer_event(
                    Arc::clone(&observer),
                    event,
                    config.callback_timeout,
                    &worker_stop,
                )
                .await
                {
                    break;
                }
            }
            worker_disconnected.store(true, Ordering::Release);
        });
        Arc::new(Self {
            sender,
            interest: config.interest,
            delivery: config.delivery,
            delivered: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            disconnected,
            force_stop,
            drain,
            worker: Mutex::new(Some(worker)),
        })
    }

    async fn emit(&self, event: ObserverEvent) -> ObserverDelivery {
        if self.disconnected.load(Ordering::Acquire) {
            return ObserverDelivery::AlreadyDisconnected;
        }
        let Some(event) = self.prepare(event) else {
            return ObserverDelivery::Delivered;
        };
        let delivery = match self.delivery {
            ObserverDeliveryPolicy::Backpressure { timeout: deadline } => {
                match timeout(deadline, self.sender.send(event)).await {
                    Ok(Ok(())) => ObserverDelivery::Delivered,
                    Ok(Err(_)) => ObserverDelivery::AlreadyDisconnected,
                    Err(_) => ObserverDelivery::BackpressureTimedOut,
                }
            }
            ObserverDeliveryPolicy::DropNewest => match self.sender.try_send(event) {
                Ok(()) => ObserverDelivery::Delivered,
                Err(mpsc::error::TrySendError::Full(_)) => ObserverDelivery::Dropped,
                Err(mpsc::error::TrySendError::Closed(_)) => ObserverDelivery::AlreadyDisconnected,
            },
            ObserverDeliveryPolicy::Disconnect => match self.sender.try_send(event) {
                Ok(()) => ObserverDelivery::Delivered,
                Err(mpsc::error::TrySendError::Full(_) | mpsc::error::TrySendError::Closed(_)) => {
                    let first = !self.disconnected.swap(true, Ordering::AcqRel);
                    self.force_stop.cancel();
                    if first {
                        ObserverDelivery::Disconnected
                    } else {
                        ObserverDelivery::AlreadyDisconnected
                    }
                }
            },
        };
        match delivery {
            ObserverDelivery::Delivered => {
                self.delivered.fetch_add(1, Ordering::Relaxed);
            }
            ObserverDelivery::Dropped | ObserverDelivery::BackpressureTimedOut => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            ObserverDelivery::Disconnected | ObserverDelivery::AlreadyDisconnected => {}
        }
        delivery
    }

    fn prepare(&self, mut event: ObserverEvent) -> Option<ObserverEvent> {
        if !self.interest.sensitive_headers {
            event.kind = redact(event.kind);
        }
        if event.kind.is_lifecycle() {
            return self.interest.lifecycle.then_some(event);
        }
        let boundary = match &event.kind {
            ObserverEventKind::BodyChunk(chunk) => chunk.boundary,
            ObserverEventKind::BodyTrailers(trailers) => trailers.boundary,
            _ => return Some(event),
        };
        let policy = match boundary.direction() {
            BodyDirection::Request => self.interest.request_body,
            BodyDirection::Response => self.interest.response_body,
        };
        let ObserverEventKind::BodyChunk(chunk) = &mut event.kind else {
            return Some(event);
        };
        match policy {
            BodyObservation::MetadataOnly => {
                chunk.sample = None;
                chunk.truncated = chunk.byte_count > 0;
            }
            BodyObservation::Prefix(limit) => {
                if let Some(sample) = &chunk.sample {
                    let length = sample.len().min(limit.get());
                    chunk.sample = Some(sample.slice(..length));
                    chunk.truncated = chunk.byte_count > length;
                }
            }
            BodyObservation::Full => {}
        }
        Some(event)
    }

    fn stats(&self) -> ObserverStats {
        ObserverStats {
            delivered: self.delivered.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            disconnected: self.disconnected.load(Ordering::Acquire),
        }
    }

    async fn shutdown(&self) {
        self.drain.cancel();
        let worker = self.worker.lock().unwrap().take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
    }
}

impl Drop for ObserverDispatcher {
    fn drop(&mut self) {
        self.force_stop.cancel();
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            worker.abort();
        }
    }
}

async fn deliver_observer_event(
    observer: Arc<dyn Observer>,
    event: ObserverEvent,
    callback_timeout: Duration,
    force_stop: &ExchangeCancellation,
) -> bool {
    let mut task = AbortOnDrop::new(tokio::spawn(async move { observer.on_event(event).await }));
    let outcome = tokio::select! {
        outcome = timeout(callback_timeout, task.handle()) => Some(outcome),
        () = force_stop.cancelled() => None,
    };
    if matches!(outcome, Some(Ok(Ok(Ok(()))))) {
        return true;
    }
    if !task.is_finished() {
        task.abort_and_wait().await;
    }
    false
}

/// Immutable set of bounded observer registrations.
#[derive(Clone, Debug, Default)]
pub struct ObserverHub {
    dispatchers: Arc<[Arc<ObserverDispatcher>]>,
}

impl ObserverHub {
    /// Starts one bounded worker per registration.
    ///
    /// This constructor must be called from within an active Tokio runtime.
    pub fn new(registrations: Vec<(Arc<dyn Observer>, ObserverConfig)>) -> Self {
        Self {
            dispatchers: registrations
                .into_iter()
                .map(|(observer, config)| ObserverDispatcher::new(observer, config))
                .collect::<Vec<_>>()
                .into(),
        }
    }

    /// Adds one bounded registration while preserving existing observers.
    ///
    /// This is useful for higher-level composition layers that must observe a
    /// proxy without taking ownership of application-supplied registrations.
    /// Like [`Self::new`], this method must be called from an active Tokio
    /// runtime because it starts the new observer worker immediately.
    #[must_use]
    pub fn with_registration(self, observer: Arc<dyn Observer>, config: ObserverConfig) -> Self {
        let mut dispatchers = self.dispatchers.iter().cloned().collect::<Vec<_>>();
        dispatchers.push(ObserverDispatcher::new(observer, config));
        Self {
            dispatchers: dispatchers.into(),
        }
    }

    /// Creates sequenced observation state for one exchange.
    pub fn start_exchange(&self, metadata: Arc<ExchangeMetadata>) -> ExchangeObserver {
        ExchangeObserver {
            metadata,
            dispatchers: Arc::clone(&self.dispatchers),
            state: tokio::sync::Mutex::new(ExchangeObserverState {
                sequence: 0,
                terminal: false,
            }),
            extensions: ExchangeExtensions::new(),
        }
    }

    /// Current counters in registration order.
    pub fn stats(&self) -> Vec<ObserverStats> {
        self.dispatchers
            .iter()
            .map(|dispatcher| dispatcher.stats())
            .collect()
    }

    /// Stops every worker and waits for task release.
    pub async fn shutdown(&self) {
        for dispatcher in self.dispatchers.iter() {
            dispatcher.shutdown().await;
        }
    }
}

#[derive(Debug)]
struct ExchangeObserverState {
    sequence: u64,
    terminal: bool,
}

/// Per-exchange sequencer and terminal guard for immutable observation.
#[derive(Debug)]
pub struct ExchangeObserver {
    metadata: Arc<ExchangeMetadata>,
    dispatchers: Arc<[Arc<ObserverDispatcher>]>,
    state: tokio::sync::Mutex<ExchangeObserverState>,
    extensions: ExchangeExtensions,
}

impl ExchangeObserver {
    /// Observer-local extensions for embedding adapters.
    pub fn extensions(&self) -> &ExchangeExtensions {
        &self.extensions
    }

    /// Emits one non-terminal event in serialized sequence order.
    pub async fn emit(&self, kind: ObserverEventKind) -> ObserverDeliveryReport {
        let mut state = self.state.lock().await;
        if state.terminal {
            return ObserverDeliveryReport::default();
        }
        self.emit_locked(&mut state, kind).await
    }

    /// Emits successful terminal evidence exactly once.
    pub async fn completed(&self, outcome: CompletedExchange) -> ObserverDeliveryReport {
        let mut state = self.state.lock().await;
        if std::mem::replace(&mut state.terminal, true) {
            return ObserverDeliveryReport::default();
        }
        self.emit_locked(&mut state, ObserverEventKind::Completed(outcome))
            .await
    }

    /// Emits failed terminal evidence exactly once.
    pub async fn failed(&self, failure: ExchangeFailure) -> ObserverDeliveryReport {
        let mut state = self.state.lock().await;
        if std::mem::replace(&mut state.terminal, true) {
            return ObserverDeliveryReport::default();
        }
        self.emit_locked(&mut state, ObserverEventKind::Failed(failure))
            .await
    }

    async fn emit_locked(
        &self,
        state: &mut ExchangeObserverState,
        kind: ObserverEventKind,
    ) -> ObserverDeliveryReport {
        state.sequence = state.sequence.saturating_add(1);
        let event = ObserverEvent {
            exchange_id: self.metadata.exchange_id,
            sequence: state.sequence,
            kind,
        };
        let mut report = ObserverDeliveryReport::default();
        for dispatcher in self.dispatchers.iter() {
            report.deliveries.push(dispatcher.emit(event.clone()).await);
        }
        report
    }
}

fn redact(kind: ObserverEventKind) -> ObserverEventKind {
    match kind {
        ObserverEventKind::RequestHeadFinalized(mut head) => {
            redact_headers(&mut head.headers, true);
            ObserverEventKind::RequestHeadFinalized(head)
        }
        ObserverEventKind::ResponseHeadFinalized(mut head) => {
            redact_headers(&mut head.headers, false);
            ObserverEventKind::ResponseHeadFinalized(head)
        }
        ObserverEventKind::RequestHeadObserved { boundary, mut head } => {
            redact_headers(&mut head.headers, true);
            ObserverEventKind::RequestHeadObserved { boundary, head }
        }
        ObserverEventKind::ResponseHeadObserved { boundary, mut head } => {
            redact_headers(&mut head.headers, false);
            ObserverEventKind::ResponseHeadObserved { boundary, head }
        }
        ObserverEventKind::BodyTrailers(mut trailers) => {
            redact_headers(
                &mut trailers.trailers,
                matches!(
                    trailers.boundary,
                    ExchangeBoundary::ClientRequest | ExchangeBoundary::UpstreamRequest
                ),
            );
            ObserverEventKind::BodyTrailers(trailers)
        }
        ObserverEventKind::Completed(mut completed) => {
            redact_headers(&mut completed.request_head.headers, true);
            redact_headers(&mut completed.response_head.headers, false);
            ObserverEventKind::Completed(completed)
        }
        other => other,
    }
}

fn redact_headers(headers: &mut HeaderBlock, _request: bool) {
    headers.redact_sensitive();
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::Mutex as StdMutex};

    use crate::{
        ConnectionId, HeaderField, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
    };

    use super::*;

    struct RecordingObserver {
        events: Arc<StdMutex<Vec<ObserverEvent>>>,
    }

    impl Observer for RecordingObserver {
        fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
            self.events.lock().unwrap().push(event);
            Box::pin(async { Ok(()) })
        }
    }

    struct WaitingObserver;

    impl Observer for WaitingObserver {
        fn on_event(&self, _event: ObserverEvent) -> BoxObserverFuture<'_> {
            Box::pin(pending())
        }
    }

    fn metadata() -> Arc<ExchangeMetadata> {
        Arc::new(ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                client_identity: crate::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/".to_owned(),
                query: None,
            },
        ))
    }

    fn started(metadata: &Arc<ExchangeMetadata>) -> ObserverEventKind {
        ObserverEventKind::ExchangeStarted {
            metadata: Arc::clone(metadata),
        }
    }

    #[tokio::test]
    async fn events_are_sequenced_and_credentials_are_redacted() {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let hub = ObserverHub::new(vec![(
            Arc::new(RecordingObserver {
                events: Arc::clone(&events),
            }),
            ObserverConfig::default(),
        )]);
        let metadata = metadata();
        let observer = hub.start_exchange(Arc::clone(&metadata));
        observer.emit(started(&metadata)).await;
        let mut headers = HeaderBlock::new();
        headers.push(HeaderField::try_new("authorization", b"secret".to_vec()).unwrap());
        headers.push(HeaderField::try_new("x-safe", b"visible".to_vec()).unwrap());
        observer
            .emit(ObserverEventKind::RequestHeadFinalized(RequestHead {
                method: "GET".to_owned(),
                target: metadata.original_target.as_target().clone(),
                headers,
                source_version: HttpLegVersion::Http2,
            }))
            .await;
        while events.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
        {
            let recorded = events.lock().unwrap();
            assert_eq!(recorded[0].sequence, 1);
            assert_eq!(recorded[1].sequence, 2);
            let ObserverEventKind::RequestHeadFinalized(head) = &recorded[1].kind else {
                panic!("expected request head");
            };
            assert!(head.headers.values("authorization").next().is_none());
            assert_eq!(head.headers.values("x-safe").next(), Some(&b"visible"[..]));
        }
        hub.shutdown().await;
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn protected_observer_must_explicitly_opt_in_to_sensitive_headers() {
        let redacted_events = Arc::new(StdMutex::new(Vec::new()));
        let protected_events = Arc::new(StdMutex::new(Vec::new()));
        let hub = ObserverHub::new(vec![
            (
                Arc::new(RecordingObserver {
                    events: Arc::clone(&redacted_events),
                }),
                ObserverConfig::default(),
            ),
            (
                Arc::new(RecordingObserver {
                    events: Arc::clone(&protected_events),
                }),
                ObserverConfig {
                    interest: ObservationInterest {
                        sensitive_headers: true,
                        ..ObservationInterest::default()
                    },
                    ..ObserverConfig::default()
                },
            ),
        ]);
        let metadata = metadata();
        let observer = hub.start_exchange(Arc::clone(&metadata));
        observer
            .emit(ObserverEventKind::ResponseHeadFinalized(ResponseHead {
                status: 200,
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("set-cookie", "session=secret").unwrap(),
                    HeaderField::try_new("x-safe", "visible").unwrap(),
                ]),
                source_version: HttpLegVersion::Http2,
            }))
            .await;
        observer
            .emit(ObserverEventKind::Completed(CompletedExchange {
                metadata,
                request_head: RequestHead {
                    method: "GET".to_owned(),
                    target: Target {
                        scheme: "https".to_owned(),
                        authority: "example.test".to_owned(),
                        host: "example.test".to_owned(),
                        port: 443,
                        path: "/".to_owned(),
                        query: None,
                    },
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("authorization", "secret").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http2,
                },
                response_head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("set-cookie", "session=secret").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http2,
                },
            }))
            .await;
        while redacted_events.lock().unwrap().len() < 2
            || protected_events.lock().unwrap().len() < 2
        {
            tokio::task::yield_now().await;
        }
        let response_headers = |events: &Arc<StdMutex<Vec<ObserverEvent>>>| {
            let events = events.lock().unwrap();
            let ObserverEventKind::ResponseHeadFinalized(head) = &events[0].kind else {
                panic!("expected response head");
            };
            head.headers.clone()
        };
        assert!(
            response_headers(&redacted_events)
                .values("set-cookie")
                .next()
                .is_none()
        );
        assert_eq!(
            response_headers(&protected_events)
                .values("set-cookie")
                .next(),
            Some(&b"session=secret"[..])
        );
        {
            let redacted = redacted_events.lock().unwrap();
            let ObserverEventKind::Completed(completed) = &redacted[1].kind else {
                panic!("expected completed event");
            };
            assert!(
                completed
                    .request_head
                    .headers
                    .iter()
                    .all(|field| field.is_redacted() && field.value().is_empty())
            );
            assert!(
                completed
                    .response_head
                    .headers
                    .iter()
                    .all(|field| field.is_redacted() && field.value().is_empty())
            );
        }
        {
            let protected = protected_events.lock().unwrap();
            let ObserverEventKind::Completed(completed) = &protected[1].kind else {
                panic!("expected completed event");
            };
            assert_eq!(
                completed
                    .request_head
                    .headers
                    .values("authorization")
                    .next(),
                Some(&b"secret"[..])
            );
            assert_eq!(
                completed.response_head.headers.values("set-cookie").next(),
                Some(&b"session=secret"[..])
            );
        }
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn body_bytes_are_absent_by_default_and_bounded_when_requested() {
        let default_events = Arc::new(StdMutex::new(Vec::new()));
        let prefix_events = Arc::new(StdMutex::new(Vec::new()));
        let full_events = Arc::new(StdMutex::new(Vec::new()));
        let hub = ObserverHub::new(vec![
            (
                Arc::new(RecordingObserver {
                    events: Arc::clone(&default_events),
                }),
                ObserverConfig::default(),
            ),
            (
                Arc::new(RecordingObserver {
                    events: Arc::clone(&prefix_events),
                }),
                ObserverConfig {
                    interest: ObservationInterest {
                        request_body: BodyObservation::Prefix(NonZeroUsize::new(2).unwrap()),
                        ..ObservationInterest::default()
                    },
                    ..ObserverConfig::default()
                },
            ),
            (
                Arc::new(RecordingObserver {
                    events: Arc::clone(&full_events),
                }),
                ObserverConfig {
                    interest: ObservationInterest {
                        request_body: BodyObservation::Full,
                        ..ObservationInterest::default()
                    },
                    ..ObserverConfig::default()
                },
            ),
        ]);
        let observer = hub.start_exchange(metadata());
        observer
            .emit(ObserverEventKind::BodyChunk(ObservedBodyChunk {
                boundary: ExchangeBoundary::ClientRequest,
                byte_count: 4,
                sample: Some(Bytes::from_static(b"data")),
                truncated: false,
            }))
            .await;
        while default_events.lock().unwrap().is_empty()
            || prefix_events.lock().unwrap().is_empty()
            || full_events.lock().unwrap().is_empty()
        {
            tokio::task::yield_now().await;
        }
        {
            let default = default_events.lock().unwrap();
            let ObserverEventKind::BodyChunk(chunk) = &default[0].kind else {
                panic!("expected body chunk");
            };
            assert!(chunk.sample.is_none());
        }
        {
            let prefix = prefix_events.lock().unwrap();
            let ObserverEventKind::BodyChunk(chunk) = &prefix[0].kind else {
                panic!("expected body chunk");
            };
            assert_eq!(chunk.sample.as_deref(), Some(&b"da"[..]));
            assert!(chunk.truncated);
        }
        {
            let full = full_events.lock().unwrap();
            let ObserverEventKind::BodyChunk(chunk) = &full[0].kind else {
                panic!("expected body chunk");
            };
            assert_eq!(chunk.sample.as_deref(), Some(&b"data"[..]));
            assert!(!chunk.truncated);
        }
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn trailers_are_opt_in_by_direction_and_redacted() {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let hub = ObserverHub::new(vec![(
            Arc::new(RecordingObserver {
                events: Arc::clone(&events),
            }),
            ObserverConfig {
                interest: ObservationInterest {
                    request_body: BodyObservation::Full,
                    ..ObservationInterest::default()
                },
                ..ObserverConfig::default()
            },
        )]);
        let observer = hub.start_exchange(metadata());
        let mut trailers = HeaderBlock::new();
        trailers.push(HeaderField::try_new("authorization", b"secret".to_vec()).unwrap());
        trailers.push(HeaderField::try_new("x-checksum", b"visible".to_vec()).unwrap());
        observer
            .emit(ObserverEventKind::BodyTrailers(ObservedBodyTrailers {
                boundary: ExchangeBoundary::ClientRequest,
                trailers,
            }))
            .await;
        while events.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        {
            let recorded = events.lock().unwrap();
            let ObserverEventKind::BodyTrailers(event) = &recorded[0].kind else {
                panic!("expected trailers");
            };
            assert!(event.trailers.values("authorization").next().is_none());
            assert_eq!(
                event.trailers.values("x-checksum").next(),
                Some(&b"visible"[..])
            );
        }
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn queue_policies_are_bounded_and_loss_is_visible() {
        for (policy, expected) in [
            (
                ObserverDeliveryPolicy::DropNewest,
                ObserverDelivery::Dropped,
            ),
            (
                ObserverDeliveryPolicy::Disconnect,
                ObserverDelivery::Disconnected,
            ),
            (
                ObserverDeliveryPolicy::Backpressure {
                    timeout: Duration::from_millis(5),
                },
                ObserverDelivery::BackpressureTimedOut,
            ),
        ] {
            let hub = ObserverHub::new(vec![(
                Arc::new(WaitingObserver),
                ObserverConfig {
                    queue_capacity: NonZeroUsize::new(1).unwrap(),
                    delivery: policy,
                    callback_timeout: Duration::from_millis(50),
                    ..ObserverConfig::default()
                },
            )]);
            let metadata = metadata();
            let observer = hub.start_exchange(Arc::clone(&metadata));
            observer.emit(started(&metadata)).await;
            tokio::task::yield_now().await;
            observer.emit(started(&metadata)).await;
            let report = observer.emit(started(&metadata)).await;
            assert_eq!(report.deliveries(), &[expected]);
            let stats = hub.stats()[0];
            if matches!(
                expected,
                ObserverDelivery::Dropped | ObserverDelivery::BackpressureTimedOut
            ) {
                assert_eq!(stats.dropped, 1);
            }
            hub.shutdown().await;
        }
    }

    #[tokio::test]
    async fn graceful_shutdown_drains_an_accepted_terminal_event() {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let hub = ObserverHub::new(vec![(
            Arc::new(RecordingObserver {
                events: Arc::clone(&events),
            }),
            ObserverConfig::default(),
        )]);
        let metadata = metadata();
        let observer = hub.start_exchange(Arc::clone(&metadata));
        observer
            .completed(CompletedExchange {
                metadata: Arc::clone(&metadata),
                request_head: RequestHead {
                    method: "GET".to_owned(),
                    target: metadata.original_target.as_target().clone(),
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http1,
                },
                response_head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http1,
                },
            })
            .await;
        hub.shutdown().await;
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [ObserverEvent {
                kind: ObserverEventKind::Completed(_),
                ..
            }]
        ));
    }
}
