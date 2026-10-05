use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::sync::broadcast;
use transmog_core::{
    HeaderBlock, RequestHead, ResponseHead,
    intercept::{
        CompletedExchange, ExchangeFailure, ExchangeId, ExchangeMetadata, HookEffect,
        InitializationDiagnostic,
    },
    observe::{
        ExchangeBoundary, ObservedBodyChunk, ObservedBodyTrailers, ObservedRouteAttempt,
        ObserverEvent, ObserverEventKind,
    },
};
use transmog_runtime::WebSocketSessionEvidence;

/// Finite memory and fan-out policy for one live catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionLimits {
    /// Maximum simultaneously retained exchanges.
    pub max_sessions: NonZeroUsize,
    /// Maximum sampled body bytes retained across all boundaries of one exchange.
    /// Zero, the default, retains metadata only.
    pub body_bytes_per_session: usize,
    /// Maximum initialization diagnostics retained per exchange.
    pub initialization_diagnostics_per_session: usize,
    /// Maximum hook effects retained per exchange.
    pub hook_effects_per_session: usize,
    /// Maximum route attempts retained per exchange.
    pub route_attempts_per_session: usize,
    /// Maximum simultaneous delta subscribers.
    pub max_subscribers: NonZeroUsize,
    /// Capacity of the hint-only delta broadcast ring.
    pub subscriber_capacity: NonZeroUsize,
    /// Maximum snapshots returned by one query.
    pub max_page_size: NonZeroUsize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_sessions: NonZeroUsize::new(10_000).expect("constant is nonzero"),
            // Metadata-only is the safe default. Applications must make body
            // retention an explicit privacy and memory-policy decision.
            body_bytes_per_session: 0,
            initialization_diagnostics_per_session: 64,
            hook_effects_per_session: 256,
            route_attempts_per_session: 16,
            max_subscribers: NonZeroUsize::new(16).expect("constant is nonzero"),
            subscriber_capacity: NonZeroUsize::new(1_024).expect("constant is nonzero"),
            max_page_size: NonZeroUsize::new(200).expect("constant is nonzero"),
        }
    }
}

/// A request head retained at one explicit observation boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedRequestHead {
    /// Boundary at which the head was observed.
    pub boundary: ExchangeBoundary,
    /// Already-redacted canonical head.
    pub head: RequestHead,
}

/// A response head retained at one explicit observation boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedResponseHead {
    /// Boundary at which the head was observed.
    pub boundary: ExchangeBoundary,
    /// Already-redacted canonical head.
    pub head: ResponseHead,
}

/// Accumulated body evidence for one boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodySnapshot {
    /// Boundary represented by this counter and prefix.
    pub boundary: ExchangeBoundary,
    /// Total bytes observed, including bytes omitted from the prefix.
    pub observed_bytes: u64,
    /// Bounded retained prefix.
    pub retained_prefix: Bytes,
    /// Whether observed bytes were omitted.
    pub truncated: bool,
    /// Last terminal trailers at this boundary.
    pub trailers: Option<HeaderBlock>,
}

/// Terminal state retained for an exchange.
#[derive(Clone, Debug)]
pub enum SessionTerminal {
    /// The response completed successfully.
    Completed(CompletedExchange),
    /// The exchange failed with a redaction-safe error.
    Failed(ExchangeFailure),
}

/// Immutable point-in-time representation of one exchange.
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    /// Stable exchange identifier.
    pub exchange_id: ExchangeId,
    /// Original immutable exchange metadata.
    pub metadata: Arc<ExchangeMetadata>,
    /// Last accepted observer sequence.
    pub last_sequence: u64,
    /// Heads observed at request boundaries.
    pub request_heads: Vec<ObservedRequestHead>,
    /// Heads observed at response boundaries.
    pub response_heads: Vec<ObservedResponseHead>,
    /// At most four boundary body summaries.
    pub bodies: Vec<BodySnapshot>,
    /// Bounded optional-hook initialization diagnostics.
    pub initialization_diagnostics: Vec<InitializationDiagnostic>,
    /// Bounded centrally attributed hook effects.
    pub hook_effects: Vec<HookEffect>,
    /// Last selected route policy and reason.
    pub route_selection: Option<(Arc<str>, Arc<str>)>,
    /// Bounded upstream route attempts.
    pub route_attempts: Vec<ObservedRouteAttempt>,
    /// Terminal result, when known.
    pub terminal: Option<SessionTerminal>,
    /// Terminal relay evidence when the exchange upgraded to WebSocket.
    pub websocket: Option<WebSocketSessionEvidence>,
}

/// Monotonic catalog loss and pressure counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogCounters {
    /// New exchanges rejected because every retained session was active.
    pub admission_rejected: u64,
    /// Old terminal exchanges evicted to admit newer exchanges.
    pub evicted: u64,
    /// Missing per-exchange observer sequence numbers.
    pub sequence_gaps: u64,
    /// Duplicate or backward observer events ignored.
    pub stale_events: u64,
    /// Events for exchanges that were never admitted or were already evicted.
    pub unknown_exchange_events: u64,
    /// Events received after a retained exchange became terminal.
    pub post_terminal_events: u64,
    /// Bounded diagnostics/effects/attempts discarded at their configured limits.
    pub detail_records_dropped: u64,
    /// Delta messages skipped by slow subscribers.
    pub subscriber_lag: u64,
    /// WebSocket terminal reports skipped by a lagged runtime receiver.
    pub websocket_evidence_lag: u64,
}

/// Result of applying one observer event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogApply {
    /// The event changed authoritative state.
    Applied,
    /// A duplicate or backward event was ignored.
    Stale,
    /// A post-terminal event was ignored.
    PostTerminal,
    /// A new exchange could not be admitted.
    AdmissionRejected,
    /// The event referred to no retained exchange.
    UnknownExchange,
}

/// Small, lossy notification that authoritative state changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogDelta {
    /// Changed exchange.
    pub exchange_id: ExchangeId,
    /// Last applied observer sequence.
    pub sequence: u64,
    /// Whether the exchange is terminal after this change.
    pub terminal: bool,
}

/// Event yielded by a bounded subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionEvent {
    /// One catalog entry changed.
    Delta(CatalogDelta),
    /// The receiver fell behind and must query authoritative state.
    Lagged(u64),
}

/// Opaque stable continuation token for catalog paging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogCursor {
    admission: u64,
    exchange_id: ExchangeId,
}

/// Optional filters applied before paging.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionFilter {
    /// Restrict results to terminal (`true`) or active (`false`) sessions.
    pub terminal: Option<bool>,
    /// Restrict to an ASCII-case-insensitive request method.
    pub method: Option<String>,
    /// Restrict to an ASCII-case-insensitive original target host.
    pub host: Option<String>,
}

/// One bounded authoritative query.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogQuery {
    /// Optional exclusive continuation cursor.
    pub after: Option<CatalogCursor>,
    /// Requested page size, capped by [`SessionLimits::max_page_size`].
    pub limit: Option<NonZeroUsize>,
    /// Deterministic filters.
    pub filter: SessionFilter,
}

/// One page of immutable session snapshots.
#[derive(Clone, Debug)]
pub struct CatalogPage {
    /// Matching sessions in admission order.
    pub sessions: Vec<SessionSnapshot>,
    /// Cursor for the next page, if more matching sessions exist.
    pub next: Option<CatalogCursor>,
}

/// Failure to create or consume a bounded subscription.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CatalogError {
    /// Configured simultaneous subscriber limit was reached.
    #[error("catalog subscriber limit reached")]
    SubscriberLimit,
    /// The catalog was dropped and the subscription closed.
    #[error("catalog subscription closed")]
    SubscriptionClosed,
}

#[derive(Debug)]
struct MutableBody {
    boundary: ExchangeBoundary,
    observed_bytes: u64,
    retained_prefix: BytesMut,
    truncated: bool,
    trailers: Option<HeaderBlock>,
}

#[derive(Debug)]
struct MutableSession {
    metadata: Arc<ExchangeMetadata>,
    last_sequence: u64,
    request_heads: Vec<ObservedRequestHead>,
    response_heads: Vec<ObservedResponseHead>,
    bodies: Vec<MutableBody>,
    retained_body_bytes: usize,
    initialization_diagnostics: Vec<InitializationDiagnostic>,
    hook_effects: Vec<HookEffect>,
    route_selection: Option<(Arc<str>, Arc<str>)>,
    route_attempts: Vec<ObservedRouteAttempt>,
    terminal: Option<SessionTerminal>,
    websocket: Option<WebSocketSessionEvidence>,
}

impl MutableSession {
    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            exchange_id: self.metadata.exchange_id,
            metadata: Arc::clone(&self.metadata),
            last_sequence: self.last_sequence,
            request_heads: self.request_heads.clone(),
            response_heads: self.response_heads.clone(),
            bodies: self
                .bodies
                .iter()
                .map(|body| BodySnapshot {
                    boundary: body.boundary,
                    observed_bytes: body.observed_bytes,
                    retained_prefix: body.retained_prefix.clone().freeze(),
                    truncated: body.truncated,
                    trailers: body.trailers.clone(),
                })
                .collect(),
            initialization_diagnostics: self.initialization_diagnostics.clone(),
            hook_effects: self.hook_effects.clone(),
            route_selection: self.route_selection.clone(),
            route_attempts: self.route_attempts.clone(),
            terminal: self.terminal.clone(),
            websocket: self.websocket.clone(),
        }
    }
}

#[derive(Debug, Default)]
struct CatalogState {
    by_id: HashMap<ExchangeId, MutableSession>,
    order: BTreeMap<u64, ExchangeId>,
    next_admission: u64,
    subscribers: usize,
    counters: CatalogCounters,
}

#[derive(Debug)]
struct CatalogInner {
    limits: SessionLimits,
    state: Mutex<CatalogState>,
    deltas: broadcast::Sender<CatalogDelta>,
}

/// Thread-safe, finite, authoritative catalog of live proxy exchanges.
#[derive(Clone, Debug)]
pub struct SessionCatalog {
    inner: Arc<CatalogInner>,
}

impl SessionCatalog {
    /// Creates an empty catalog with explicit finite limits.
    pub fn new(limits: SessionLimits) -> Self {
        let (deltas, _) = broadcast::channel(limits.subscriber_capacity.get());
        Self {
            inner: Arc::new(CatalogInner {
                limits,
                state: Mutex::new(CatalogState::default()),
                deltas,
            }),
        }
    }

    /// Applies one already-redacted observer event.
    pub fn apply(&self, event: ObserverEvent) -> CatalogApply {
        let mut state = self.lock_state();
        let exchange_id = event.exchange_id;
        if !state.by_id.contains_key(&exchange_id) {
            if !matches!(event.kind, ObserverEventKind::ExchangeStarted { .. }) {
                state.counters.unknown_exchange_events =
                    state.counters.unknown_exchange_events.saturating_add(1);
                return CatalogApply::UnknownExchange;
            }
            if state.by_id.len() == self.inner.limits.max_sessions.get()
                && !evict_oldest_terminal(&mut state)
            {
                state.counters.admission_rejected =
                    state.counters.admission_rejected.saturating_add(1);
                return CatalogApply::AdmissionRejected;
            }
            let ObserverEventKind::ExchangeStarted { metadata } = &event.kind else {
                unreachable!();
            };
            state.next_admission = state.next_admission.saturating_add(1);
            let admission = state.next_admission;
            state.order.insert(admission, exchange_id);
            state.by_id.insert(
                exchange_id,
                MutableSession {
                    metadata: Arc::clone(metadata),
                    last_sequence: 0,
                    request_heads: Vec::with_capacity(2),
                    response_heads: Vec::with_capacity(2),
                    bodies: Vec::with_capacity(4),
                    retained_body_bytes: 0,
                    initialization_diagnostics: Vec::new(),
                    hook_effects: Vec::new(),
                    route_selection: None,
                    route_attempts: Vec::new(),
                    terminal: None,
                    websocket: None,
                },
            );
        }

        let Some(session) = state.by_id.get(&exchange_id) else {
            state.counters.unknown_exchange_events =
                state.counters.unknown_exchange_events.saturating_add(1);
            return CatalogApply::UnknownExchange;
        };
        let (last_sequence, terminal) = { (session.last_sequence, session.terminal.is_some()) };
        if event.sequence <= last_sequence {
            state.counters.stale_events = state.counters.stale_events.saturating_add(1);
            return CatalogApply::Stale;
        }
        if terminal {
            state.counters.post_terminal_events =
                state.counters.post_terminal_events.saturating_add(1);
            return CatalogApply::PostTerminal;
        }
        if event.sequence > last_sequence.saturating_add(1) {
            state.counters.sequence_gaps = state
                .counters
                .sequence_gaps
                .saturating_add(event.sequence - last_sequence - 1);
        }

        let mut detail_dropped = 0_u64;
        let limits = self.inner.limits;
        let Some(session) = state.by_id.get_mut(&exchange_id) else {
            state.counters.unknown_exchange_events =
                state.counters.unknown_exchange_events.saturating_add(1);
            return CatalogApply::UnknownExchange;
        };
        session.last_sequence = event.sequence;
        apply_kind(session, event.kind, limits, &mut detail_dropped);
        let delta = CatalogDelta {
            exchange_id,
            sequence: session.last_sequence,
            terminal: session.terminal.is_some(),
        };
        state.counters.detail_records_dropped = state
            .counters
            .detail_records_dropped
            .saturating_add(detail_dropped);
        drop(state);
        let _ = self.inner.deltas.send(delta);
        CatalogApply::Applied
    }

    /// Gets one immutable snapshot by exchange ID.
    pub fn get(&self, exchange_id: ExchangeId) -> Option<SessionSnapshot> {
        self.lock_state()
            .by_id
            .get(&exchange_id)
            .map(MutableSession::snapshot)
    }

    /// Pages authoritative state in stable admission order.
    pub fn query(&self, query: &CatalogQuery) -> CatalogPage {
        let state = self.lock_state();
        let start = query.after.map_or(0, |cursor| cursor.admission);
        let limit = query
            .limit
            .map_or(self.inner.limits.max_page_size.get(), NonZeroUsize::get)
            .min(self.inner.limits.max_page_size.get());
        let mut matching = state
            .order
            .range((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .filter_map(|(admission, exchange_id)| {
                let session = state.by_id.get(exchange_id)?;
                matches_filter(session, &query.filter).then_some((*admission, session))
            });
        let selected = matching.by_ref().take(limit).collect::<Vec<_>>();
        let has_more = matching.next().is_some();
        let sessions = selected
            .iter()
            .map(|(_, session)| session.snapshot())
            .collect();
        let next = if has_more {
            selected.last().map(|(admission, session)| CatalogCursor {
                admission: *admission,
                exchange_id: session.metadata.exchange_id,
            })
        } else {
            None
        };
        CatalogPage { sessions, next }
    }

    /// Returns monotonic pressure and loss counters.
    pub fn counters(&self) -> CatalogCounters {
        self.lock_state().counters
    }

    /// Attaches terminal WebSocket relay evidence to its originating exchange.
    pub fn apply_websocket(&self, evidence: WebSocketSessionEvidence) -> CatalogApply {
        let exchange_id = ExchangeId(evidence.session_id.0);
        let mut state = self.lock_state();
        let Some(session) = state.by_id.get_mut(&exchange_id) else {
            state.counters.unknown_exchange_events =
                state.counters.unknown_exchange_events.saturating_add(1);
            return CatalogApply::UnknownExchange;
        };
        session.websocket = Some(evidence);
        let delta = CatalogDelta {
            exchange_id,
            sequence: session.last_sequence,
            terminal: session.terminal.is_some(),
        };
        drop(state);
        let _ = self.inner.deltas.send(delta);
        CatalogApply::Applied
    }

    pub(crate) fn record_websocket_lag(&self, count: u64) {
        let mut state = self.lock_state();
        state.counters.websocket_evidence_lag =
            state.counters.websocket_evidence_lag.saturating_add(count);
    }

    /// Subscribes to bounded hint-only deltas.
    ///
    /// Consumers must query authoritative state after [`SubscriptionEvent::Lagged`].
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::SubscriberLimit`] when the configured number of
    /// concurrent subscriptions is already active.
    pub fn subscribe(&self) -> Result<CatalogSubscription, CatalogError> {
        let mut state = self.lock_state();
        if state.subscribers == self.inner.limits.max_subscribers.get() {
            return Err(CatalogError::SubscriberLimit);
        }
        state.subscribers += 1;
        drop(state);
        Ok(CatalogSubscription {
            inner: Arc::clone(&self.inner),
            receiver: self.inner.deltas.subscribe(),
        })
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CatalogState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One counted bounded delta subscription.
#[derive(Debug)]
pub struct CatalogSubscription {
    inner: Arc<CatalogInner>,
    receiver: broadcast::Receiver<CatalogDelta>,
}

impl CatalogSubscription {
    /// Waits for the next delta or a visible lag marker.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::SubscriptionClosed`] if the catalog is gone.
    pub async fn recv(&mut self) -> Result<SubscriptionEvent, CatalogError> {
        match self.receiver.recv().await {
            Ok(delta) => Ok(SubscriptionEvent::Delta(delta)),
            Err(broadcast::error::RecvError::Lagged(count)) => {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.counters.subscriber_lag = state.counters.subscriber_lag.saturating_add(count);
                Ok(SubscriptionEvent::Lagged(count))
            }
            Err(broadcast::error::RecvError::Closed) => Err(CatalogError::SubscriptionClosed),
        }
    }
}

impl Drop for CatalogSubscription {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.subscribers = state.subscribers.saturating_sub(1);
    }
}

fn evict_oldest_terminal(state: &mut CatalogState) -> bool {
    let candidate = state.order.iter().find_map(|(admission, exchange_id)| {
        state
            .by_id
            .get(exchange_id)
            .and_then(|session| session.terminal.as_ref())
            .map(|_| (*admission, *exchange_id))
    });
    let Some((admission, exchange_id)) = candidate else {
        return false;
    };
    state.order.remove(&admission);
    state.by_id.remove(&exchange_id);
    state.counters.evicted = state.counters.evicted.saturating_add(1);
    true
}

fn apply_kind(
    session: &mut MutableSession,
    kind: ObserverEventKind,
    limits: SessionLimits,
    detail_dropped: &mut u64,
) {
    match kind {
        ObserverEventKind::ExchangeStarted { .. } => {}
        ObserverEventKind::HookInitializationSkipped(diagnostic) => push_bounded(
            &mut session.initialization_diagnostics,
            diagnostic,
            limits.initialization_diagnostics_per_session,
            detail_dropped,
        ),
        ObserverEventKind::RequestHeadObserved { boundary, head } => {
            upsert_request_head(&mut session.request_heads, boundary, head);
        }
        ObserverEventKind::ResponseHeadObserved { boundary, head } => {
            upsert_response_head(&mut session.response_heads, boundary, head);
        }
        ObserverEventKind::HookEffect(effect) => push_bounded(
            &mut session.hook_effects,
            effect,
            limits.hook_effects_per_session,
            detail_dropped,
        ),
        ObserverEventKind::RequestHeadFinalized(head) => {
            upsert_request_head(
                &mut session.request_heads,
                ExchangeBoundary::UpstreamRequest,
                head,
            );
        }
        ObserverEventKind::BodyChunk(chunk) => apply_body_chunk(session, chunk, limits),
        ObserverEventKind::BodyTrailers(trailers) => apply_trailers(session, trailers),
        ObserverEventKind::RouteSelected { policy_id, reason } => {
            session.route_selection = Some((policy_id, reason));
        }
        ObserverEventKind::RouteAttempt(attempt) => push_bounded(
            &mut session.route_attempts,
            attempt,
            limits.route_attempts_per_session,
            detail_dropped,
        ),
        ObserverEventKind::ResponseHeadFinalized(head) => {
            upsert_response_head(
                &mut session.response_heads,
                ExchangeBoundary::ClientResponse,
                head,
            );
        }
        ObserverEventKind::Completed(completed) => {
            session.terminal = Some(SessionTerminal::Completed(completed));
        }
        ObserverEventKind::Failed(failure) => {
            session.terminal = Some(SessionTerminal::Failed(failure));
        }
    }
}

fn push_bounded<T>(items: &mut Vec<T>, item: T, limit: usize, dropped: &mut u64) {
    if items.len() < limit {
        items.push(item);
    } else {
        *dropped = dropped.saturating_add(1);
    }
}

fn upsert_request_head(
    heads: &mut Vec<ObservedRequestHead>,
    boundary: ExchangeBoundary,
    head: RequestHead,
) {
    if let Some(existing) = heads.iter_mut().find(|item| item.boundary == boundary) {
        existing.head = head;
    } else {
        heads.push(ObservedRequestHead { boundary, head });
    }
}

fn upsert_response_head(
    heads: &mut Vec<ObservedResponseHead>,
    boundary: ExchangeBoundary,
    head: ResponseHead,
) {
    if let Some(existing) = heads.iter_mut().find(|item| item.boundary == boundary) {
        existing.head = head;
    } else {
        heads.push(ObservedResponseHead { boundary, head });
    }
}

fn apply_body_chunk(session: &mut MutableSession, chunk: ObservedBodyChunk, limits: SessionLimits) {
    let index = if let Some(index) = session
        .bodies
        .iter()
        .position(|body| body.boundary == chunk.boundary)
    {
        index
    } else {
        session.bodies.push(MutableBody {
            boundary: chunk.boundary,
            observed_bytes: 0,
            retained_prefix: BytesMut::new(),
            truncated: false,
            trailers: None,
        });
        session.bodies.len() - 1
    };
    let remaining = limits
        .body_bytes_per_session
        .saturating_sub(session.retained_body_bytes);
    let body = &mut session.bodies[index];
    body.observed_bytes = body
        .observed_bytes
        .saturating_add(u64::try_from(chunk.byte_count).unwrap_or(u64::MAX));
    if let Some(sample) = chunk.sample {
        let take = remaining.min(sample.len());
        body.retained_prefix.extend_from_slice(&sample[..take]);
        session.retained_body_bytes = session.retained_body_bytes.saturating_add(take);
        body.truncated |= take < sample.len();
    }
    body.truncated |= chunk.truncated || remaining == 0 && chunk.byte_count > 0;
}

fn apply_trailers(session: &mut MutableSession, trailers: ObservedBodyTrailers) {
    if let Some(body) = session
        .bodies
        .iter_mut()
        .find(|body| body.boundary == trailers.boundary)
    {
        body.trailers = Some(trailers.trailers);
    } else {
        session.bodies.push(MutableBody {
            boundary: trailers.boundary,
            observed_bytes: 0,
            retained_prefix: BytesMut::new(),
            truncated: false,
            trailers: Some(trailers.trailers),
        });
    }
}

fn matches_filter(session: &MutableSession, filter: &SessionFilter) -> bool {
    if filter
        .terminal
        .is_some_and(|terminal| terminal != session.terminal.is_some())
    {
        return false;
    }
    if filter.host.as_ref().is_some_and(|host| {
        !session
            .metadata
            .original_target
            .as_target()
            .host
            .eq_ignore_ascii_case(host)
    }) {
        return false;
    }
    if let Some(method) = &filter.method {
        let observed = session
            .request_heads
            .iter()
            .find(|head| head.boundary == ExchangeBoundary::ClientRequest)
            .or_else(|| session.request_heads.first());
        if observed.is_none_or(|head| !head.head.method.eq_ignore_ascii_case(method)) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::SystemTime};

    use transmog_core::{
        ConnectionId, HeaderField, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeFailureKind, ExchangeStage},
        observe::{BodyObservation, ObservationInterest, Observer, ObserverConfig, ObserverHub},
    };

    use super::*;

    #[test]
    fn default_policy_retains_metadata_only() {
        assert_eq!(SessionLimits::default().body_bytes_per_session, 0);
    }

    fn limits(max_sessions: usize) -> SessionLimits {
        SessionLimits {
            max_sessions: NonZeroUsize::new(max_sessions).unwrap(),
            body_bytes_per_session: 4,
            initialization_diagnostics_per_session: 1,
            hook_effects_per_session: 1,
            route_attempts_per_session: 1,
            max_subscribers: NonZeroUsize::new(1).unwrap(),
            subscriber_capacity: NonZeroUsize::new(1).unwrap(),
            max_page_size: NonZeroUsize::new(1).unwrap(),
        }
    }

    fn metadata(id: u128, host: &str) -> Arc<ExchangeMetadata> {
        Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(id),
                downstream_connection_id: ConnectionId(id),
                stream_id: StreamId(id),
                client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            Target {
                scheme: "https".into(),
                authority: host.into(),
                host: host.into(),
                port: 443,
                path: "/".into(),
                query: None,
            },
            SystemTime::UNIX_EPOCH,
        ))
    }

    fn event(id: u128, sequence: u64, kind: ObserverEventKind) -> ObserverEvent {
        ObserverEvent {
            exchange_id: ExchangeId(id),
            sequence,
            kind,
        }
    }

    fn start(catalog: &SessionCatalog, id: u128, host: &str) {
        let metadata = metadata(id, host);
        assert_eq!(
            catalog.apply(event(
                id,
                1,
                ObserverEventKind::ExchangeStarted { metadata }
            )),
            CatalogApply::Applied
        );
    }

    fn fail(catalog: &SessionCatalog, id: u128, sequence: u64) {
        let metadata = metadata(id, "example.test");
        assert_eq!(
            catalog.apply(event(
                id,
                sequence,
                ObserverEventKind::Failed(ExchangeFailure {
                    metadata,
                    stage: ExchangeStage::Upstream,
                    kind: ExchangeFailureKind::Upstream,
                    request_committed: false,
                    response_committed: false,
                    message: "unavailable".into(),
                })
            )),
            CatalogApply::Applied
        );
    }

    #[test]
    fn active_sessions_are_never_evicted_and_oldest_terminal_is_deterministic() {
        let catalog = SessionCatalog::new(limits(2));
        start(&catalog, 1, "one.test");
        start(&catalog, 2, "two.test");
        assert_eq!(
            catalog.apply(event(
                3,
                1,
                ObserverEventKind::ExchangeStarted {
                    metadata: metadata(3, "three.test")
                }
            )),
            CatalogApply::AdmissionRejected
        );
        assert_eq!(catalog.counters().admission_rejected, 1);
        fail(&catalog, 1, 2);
        start(&catalog, 3, "three.test");
        assert!(catalog.get(ExchangeId(1)).is_none());
        assert!(catalog.get(ExchangeId(2)).is_some());
        assert!(catalog.get(ExchangeId(3)).is_some());
        assert_eq!(catalog.counters().evicted, 1);
    }

    #[test]
    fn stale_gapped_unknown_and_post_terminal_events_are_visible() {
        let catalog = SessionCatalog::new(limits(1));
        start(&catalog, 1, "one.test");
        assert_eq!(
            catalog.apply(event(
                1,
                1,
                ObserverEventKind::RequestHeadFinalized(head("GET"))
            )),
            CatalogApply::Stale
        );
        assert_eq!(
            catalog.apply(event(
                1,
                4,
                ObserverEventKind::RequestHeadFinalized(head("GET"))
            )),
            CatalogApply::Applied
        );
        fail(&catalog, 1, 5);
        assert_eq!(
            catalog.apply(event(
                1,
                6,
                ObserverEventKind::RequestHeadFinalized(head("GET"))
            )),
            CatalogApply::PostTerminal
        );
        assert_eq!(
            catalog.apply(event(
                9,
                1,
                ObserverEventKind::RequestHeadFinalized(head("GET"))
            )),
            CatalogApply::UnknownExchange
        );
        assert_eq!(
            catalog.counters(),
            CatalogCounters {
                sequence_gaps: 2,
                stale_events: 1,
                unknown_exchange_events: 1,
                post_terminal_events: 1,
                ..CatalogCounters::default()
            }
        );
    }

    fn head(method: &str) -> RequestHead {
        RequestHead {
            method: method.into(),
            target: metadata(1, "one.test").original_target.as_target().clone(),
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        }
    }

    #[test]
    fn body_retention_is_bounded_across_boundaries() {
        let catalog = SessionCatalog::new(limits(1));
        start(&catalog, 1, "one.test");
        for (sequence, boundary, sample) in [
            (2, ExchangeBoundary::ClientRequest, b"abc".as_slice()),
            (3, ExchangeBoundary::UpstreamResponse, b"def".as_slice()),
        ] {
            catalog.apply(event(
                1,
                sequence,
                ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary,
                    byte_count: sample.len(),
                    sample: Some(Bytes::copy_from_slice(sample)),
                    truncated: false,
                }),
            ));
        }
        let snapshot = catalog.get(ExchangeId(1)).unwrap();
        assert_eq!(
            snapshot
                .bodies
                .iter()
                .map(|body| body.retained_prefix.len())
                .sum::<usize>(),
            4
        );
        assert!(snapshot.bodies[1].truncated);
    }

    #[test]
    fn paging_and_filters_are_stable_and_capped() {
        let catalog = SessionCatalog::new(SessionLimits {
            max_sessions: NonZeroUsize::new(4).unwrap(),
            max_page_size: NonZeroUsize::new(1).unwrap(),
            ..limits(4)
        });
        start(&catalog, 1, "one.test");
        catalog.apply(event(
            1,
            2,
            ObserverEventKind::RequestHeadFinalized(head("GET")),
        ));
        start(&catalog, 2, "two.test");
        catalog.apply(event(
            2,
            2,
            ObserverEventKind::RequestHeadFinalized(head("POST")),
        ));
        let first = catalog.query(&CatalogQuery::default());
        assert_eq!(first.sessions.len(), 1);
        assert_eq!(first.sessions[0].exchange_id, ExchangeId(1));
        let second = catalog.query(&CatalogQuery {
            after: first.next,
            ..CatalogQuery::default()
        });
        assert_eq!(second.sessions[0].exchange_id, ExchangeId(2));
        assert!(second.next.is_none());
        let filtered = catalog.query(&CatalogQuery {
            filter: SessionFilter {
                host: Some("TWO.TEST".into()),
                ..SessionFilter::default()
            },
            ..CatalogQuery::default()
        });
        assert_eq!(filtered.sessions[0].exchange_id, ExchangeId(2));
    }

    #[tokio::test]
    async fn subscriptions_are_counted_and_report_lag() {
        let catalog = SessionCatalog::new(limits(3));
        let mut subscription = catalog.subscribe().unwrap();
        assert_eq!(
            catalog.subscribe().unwrap_err(),
            CatalogError::SubscriberLimit
        );
        start(&catalog, 1, "one.test");
        start(&catalog, 2, "two.test");
        assert_eq!(
            subscription.recv().await.unwrap(),
            SubscriptionEvent::Lagged(1)
        );
        assert_eq!(catalog.counters().subscriber_lag, 1);
        drop(subscription);
        assert!(catalog.subscribe().is_ok());
    }

    struct CatalogObserver(SessionCatalog);

    impl Observer for CatalogObserver {
        fn on_event(&self, event: ObserverEvent) -> transmog_core::observe::BoxObserverFuture<'_> {
            self.0.apply(event);
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn observer_boundary_removes_credentials_before_catalog_storage() {
        let catalog = SessionCatalog::new(limits(1));
        let hub = ObserverHub::new(vec![(
            Arc::new(CatalogObserver(catalog.clone())),
            ObserverConfig {
                interest: ObservationInterest {
                    lifecycle: true,
                    request_body: BodyObservation::Full,
                    response_body: BodyObservation::Full,
                },
                ..ObserverConfig::default()
            },
        )]);
        let metadata = metadata(1, "one.test");
        let observer = hub.start_exchange(Arc::clone(&metadata));
        observer
            .emit(ObserverEventKind::ExchangeStarted {
                metadata: Arc::clone(&metadata),
            })
            .await;
        let mut request = head("GET");
        request
            .headers
            .push(HeaderField::try_new("authorization", b"Bearer secret".to_vec()).unwrap());
        request
            .headers
            .push(HeaderField::try_new("x-safe", b"yes".to_vec()).unwrap());
        observer
            .emit(ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::ClientRequest,
                head: request,
            })
            .await;
        hub.shutdown().await;
        let snapshot = catalog.get(ExchangeId(1)).unwrap();
        let headers = &snapshot.request_heads[0].head.headers;
        assert!(headers.iter().all(|field| field.name() != b"authorization"));
        assert!(headers.iter().any(|field| field.name() == b"x-safe"));
    }

    #[test]
    fn cursor_exchange_identity_is_not_forgeable_through_query() {
        let catalog = SessionCatalog::new(limits(2));
        start(&catalog, 1, "one.test");
        start(&catalog, 2, "two.test");
        let page = catalog.query(&CatalogQuery::default());
        assert_eq!(page.next.unwrap().exchange_id, ExchangeId(1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_catalog_stress_preserves_bounds_and_terminal_state() {
        const EXCHANGES: usize = 512;
        let catalog = SessionCatalog::new(SessionLimits {
            max_sessions: NonZeroUsize::new(EXCHANGES).unwrap(),
            body_bytes_per_session: 8,
            max_page_size: NonZeroUsize::new(31).unwrap(),
            ..SessionLimits::default()
        });
        let mut tasks = Vec::with_capacity(EXCHANGES);
        for index in 0..EXCHANGES {
            let catalog = catalog.clone();
            tasks.push(tokio::spawn(async move {
                let id = index as u128 + 1;
                let metadata = metadata(id, "stress.test");
                assert_eq!(
                    catalog.apply(event(
                        id,
                        1,
                        ObserverEventKind::ExchangeStarted {
                            metadata: Arc::clone(&metadata)
                        }
                    )),
                    CatalogApply::Applied
                );
                assert_eq!(
                    catalog.apply(event(
                        id,
                        2,
                        ObserverEventKind::BodyChunk(ObservedBodyChunk {
                            boundary: ExchangeBoundary::ClientRequest,
                            byte_count: 32,
                            sample: Some(Bytes::from_static(b"0123456789abcdef")),
                            truncated: true,
                        })
                    )),
                    CatalogApply::Applied
                );
                assert_eq!(
                    catalog.apply(event(
                        id,
                        3,
                        ObserverEventKind::Failed(ExchangeFailure {
                            metadata,
                            stage: ExchangeStage::Upstream,
                            kind: ExchangeFailureKind::Upstream,
                            request_committed: false,
                            response_committed: false,
                            message: "unavailable".into(),
                        })
                    )),
                    CatalogApply::Applied
                );
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        let mut count = 0;
        let mut after = None;
        loop {
            let page = catalog.query(&CatalogQuery {
                after,
                ..CatalogQuery::default()
            });
            count += page.sessions.len();
            assert!(page.sessions.iter().all(|session| {
                session.terminal.is_some()
                    && session
                        .bodies
                        .iter()
                        .map(|body| body.retained_prefix.len())
                        .sum::<usize>()
                        <= 8
            }));
            let Some(next) = page.next else {
                break;
            };
            after = Some(next);
        }
        assert_eq!(count, EXCHANGES);
        assert_eq!(catalog.counters(), CatalogCounters::default());
    }
}
