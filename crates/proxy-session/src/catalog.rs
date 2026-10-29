use std::{
    collections::{BTreeMap, HashMap, HashSet},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::sync::broadcast;
use transmog_core::{
    HeaderBlock, RequestHead, ResponseHead,
    intercept::{
        CompletedExchange, ExchangeFailure, ExchangeFailureKind, ExchangeId, ExchangeMetadata,
        ExchangeStage, HookEffect, InitializationDiagnostic,
    },
    observe::{
        ExchangeBoundary, ObservedBodyChunk, ObservedBodyTrailers, ObservedRouteAttempt,
        ObserverEvent, ObserverEventKind,
    },
};
use transmog_runtime::WebSocketSessionEvidence;
#[path = "catalog_views.rs"]
mod views;

/// Finite memory and fan-out policy for one live catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionLimits {
    /// Optional live-entry limit; active exchanges may temporarily exceed it.
    pub max_sessions: Option<NonZeroUsize>,
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
            max_sessions: None,
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
    /// Immutable workspace admission number, assigned on catalog publication.
    pub request_number: u64,
    /// Measured local timing and physical connection observations.
    pub performance: transmog_core::performance::PerformanceEvidence,
    /// Whether this is immutable saved evidence, independent of live retention.
    pub imported: bool,
    /// Stable exchange identifier.
    pub exchange_id: ExchangeId,
    /// Original immutable exchange metadata.
    pub metadata: Arc<ExchangeMetadata>,
    /// Last accepted observer sequence.
    pub last_sequence: u64,
    /// Count of observer sequence values known to be missing for this exchange.
    pub sequence_loss: u64,
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
    /// Wall-clock time at which a terminal event reached the catalog.
    pub terminal_at: Option<SystemTime>,
    /// Terminal relay evidence when the exchange upgraded to `WebSocket`.
    pub websocket: Option<WebSocketSessionEvidence>,
    /// Latest application stream state, independent of HTTP completion.
    pub stream: Option<transmog_core::stream::StreamState>,
    /// Bounded stream messages and lifetime counters.
    pub stream_history: transmog_core::stream::StreamHistory,
}

/// Borrowed retained heads for bounded native metadata admission without cloning bodies or history.
pub struct SessionHeads<'a> {
    /// Immutable exchange identity and caller/target evidence.
    pub metadata: &'a ExchangeMetadata,
    /// Stable workspace admission order.
    pub request_number: u64,
    /// Whether the entry came from saved evidence.
    pub imported: bool,
    /// Request heads at their exact observation boundaries.
    pub request_heads: &'a [ObservedRequestHead],
    /// Response heads at their exact observation boundaries.
    pub response_heads: &'a [ObservedResponseHead],
}
impl<'a> From<&'a SessionSnapshot> for SessionHeads<'a> {
    fn from(snapshot: &'a SessionSnapshot) -> Self {
        Self {
            metadata: &snapshot.metadata,
            request_number: snapshot.request_number,
            imported: snapshot.imported,
            request_heads: &snapshot.request_heads,
            response_heads: &snapshot.response_heads,
        }
    }
}

/// Monotonic catalog loss and pressure counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogCounters {
    /// New exchanges rejected by admission policy, including exhausted numbering.
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
    /// `WebSocket` terminal reports skipped by a lagged runtime receiver.
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
    /// Native interest classification, carrying no header or body values.
    pub change: CatalogChange,
}

/// Native message interests let optional consumers skip unrelated metadata and body growth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogChange {
    /// Request/response headers changed at an exact observation boundary.
    Head(ExchangeBoundary),
    /// Body bytes or trailers progressed at this boundary.
    Body(ExchangeBoundary),
    /// A body reached its terminal retained state, independently of HTTP completion.
    BodyCompleted(ExchangeBoundary),
    /// HTTP completion or failure; inspect both message sides that are present.
    Terminal,
    /// Retention, visibility, import, stream or performance metadata changed.
    Metadata,
}
impl CatalogChange {
    fn from_kind(kind: &ObserverEventKind) -> Self {
        use ObserverEventKind::{
            BodyChunk, BodyCompleted, BodyTrailers, Completed, Failed, RequestHeadFinalized,
            RequestHeadObserved, ResponseHeadFinalized, ResponseHeadObserved,
        };
        match kind {
            RequestHeadObserved { boundary, .. } | ResponseHeadObserved { boundary, .. } => {
                Self::Head(*boundary)
            }
            RequestHeadFinalized(_) => Self::Head(ExchangeBoundary::UpstreamRequest),
            ResponseHeadFinalized(_) => Self::Head(ExchangeBoundary::ClientResponse),
            BodyChunk(chunk) => Self::Body(chunk.boundary),
            BodyTrailers(trailers) => Self::Body(trailers.boundary),
            BodyCompleted { boundary } => Self::BodyCompleted(*boundary),
            Completed(_) | Failed(_) => Self::Terminal,
            _ => Self::Metadata,
        }
    }
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
    /// No further unique request numbers are available in this workspace.
    #[error("workspace request number exhausted")]
    RequestNumberExhausted,
    /// Saved evidence reached its independent finite entry bound or collided.
    #[error("imported session limit or identifier collision")]
    ImportLimit,
    /// The caller canceled a saved batch before its atomic publication.
    #[error("import canceled before publication")]
    ImportCanceled,
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
    run_ended: bool,
    request_number: u64,
    performance: transmog_core::performance::PerformanceEvidence,
    imported: bool,
    metadata: Arc<ExchangeMetadata>,
    last_sequence: u64,
    sequence_loss: u64,
    request_heads: Vec<ObservedRequestHead>,
    response_heads: Vec<ObservedResponseHead>,
    bodies: Vec<MutableBody>,
    retained_body_bytes: usize,
    initialization_diagnostics: Vec<InitializationDiagnostic>,
    hook_effects: Vec<HookEffect>,
    route_selection: Option<(Arc<str>, Arc<str>)>,
    route_attempts: Vec<ObservedRouteAttempt>,
    terminal: Option<SessionTerminal>,
    terminal_at: Option<SystemTime>,
    websocket: Option<WebSocketSessionEvidence>,
    stream: Option<transmog_core::stream::StreamState>,
    stream_history: transmog_core::stream::StreamHistory,
}

impl SessionSnapshot {
    /// Effective category used consistently by traffic queries and inspectors.
    pub fn state(&self) -> &'static str {
        if matches!(
            self.terminal,
            Some(SessionTerminal::Failed(ExchangeFailure {
                kind: ExchangeFailureKind::Cancelled,
                ..
            }))
        ) {
            return "cancelled";
        }
        if let Some(stream) = &self.stream {
            if stream.status.is_active() {
                return if self.imported {
                    "open-at-capture"
                } else {
                    "active"
                };
            }
            return match stream.status {
                transmog_core::stream::StreamStatus::Failed => "failed",
                transmog_core::stream::StreamStatus::Interrupted => "interrupted",
                _ => "completed",
            };
        }
        match self.terminal {
            Some(SessionTerminal::Completed(_)) => "completed",
            Some(SessionTerminal::Failed(_)) => "failed",
            None => {
                if self.imported {
                    "incomplete"
                } else {
                    "active"
                }
            }
        }
    }
}

impl MutableSession {
    fn live_websocket(&self) -> bool {
        !self.imported
            && (self.stream.as_ref().is_some_and(|stream| {
                stream.protocol == transmog_core::stream::StreamProtocol::WebSocket
                    && stream.status.is_active()
            }) || self.metadata.websocket_requested && self.terminal.is_none())
    }
    fn is_active(&self) -> bool {
        self.stream
            .as_ref()
            .map_or(self.terminal.is_none(), |stream| stream.status.is_active())
    }
    fn snapshot(&self) -> SessionSnapshot {
        self.snapshot_with_content(
            true,
            std::time::Instant::now(),
            transmog_core::stream::observed_millis(),
        )
    }

    fn snapshot_with_content(
        &self,
        content: bool,
        now: std::time::Instant,
        cutoff_millis: u64,
    ) -> SessionSnapshot {
        SessionSnapshot {
            request_number: self.request_number,
            performance: self.performance.clone(),
            imported: self.imported,
            exchange_id: self.metadata.exchange_id,
            metadata: Arc::clone(&self.metadata),
            last_sequence: self.last_sequence,
            sequence_loss: self.sequence_loss,
            request_heads: self.request_heads.clone(),
            response_heads: self.response_heads.clone(),
            bodies: self
                .bodies
                .iter()
                .map(|body| BodySnapshot {
                    boundary: body.boundary,
                    observed_bytes: body.observed_bytes,
                    retained_prefix: if content {
                        body.retained_prefix.clone().freeze()
                    } else {
                        bytes::Bytes::new()
                    },
                    truncated: body.truncated,
                    trailers: body.trailers.clone(),
                })
                .collect(),
            initialization_diagnostics: self.initialization_diagnostics.clone(),
            hook_effects: self.hook_effects.clone(),
            route_selection: self.route_selection.clone(),
            route_attempts: self.route_attempts.clone(),
            terminal: self.terminal.clone(),
            terminal_at: self.terminal_at,
            websocket: self.websocket.clone(),
            stream: self.stream.clone(),
            stream_history: if content {
                self.stream_history.snapshot(!self.imported)
            } else {
                self.stream_history
                    .metadata_snapshot(!self.imported, now, cutoff_millis)
            },
        }
    }
}

#[derive(Debug, Default)]
struct CatalogState {
    borrowed: HashMap<ExchangeId, views::Reference>,
    pins: HashMap<ExchangeId, usize>,
    retain_stream_content: [bool; 2],
    max_live_entries: Option<NonZeroUsize>,
    imported_count: usize,
    dismissed: HashSet<ExchangeId>,
    dismissal_versions: HashMap<ExchangeId, u64>,
    next_dismissal: u64,
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
    /// Applies body privacy choices to subsequently retained client/server messages.
    /// Connection state and item counts remain available without content.
    pub fn set_stream_content_retention(&self, client: bool, server: bool) {
        self.lock_state().retain_stream_content = [client, server];
    }
    /// Creates an empty catalog with explicit finite limits.
    pub fn new(limits: SessionLimits) -> Self {
        let (deltas, _) = broadcast::channel(limits.subscriber_capacity.get());
        Self {
            inner: Arc::new(CatalogInner {
                limits,
                state: Mutex::new(CatalogState {
                    max_live_entries: limits.max_sessions,
                    ..CatalogState::default()
                }),
                deltas,
            }),
        }
    }

    /// Current optional live-entry count limit.
    pub fn live_entry_limit(&self) -> Option<NonZeroUsize> {
        self.lock_state().max_live_entries
    }

    /// Changes live-entry retention and drops oldest completed entries immediately.
    /// Imported entries and active requests are preserved. None removes the limit.
    pub fn set_live_entry_limit(&self, maximum: Option<NonZeroUsize>) {
        let mut state = self.lock_state();
        state.max_live_entries = maximum;
        trim_live_entries(&mut state);
        drop(state);
        let _ = self.inner.deltas.send(CatalogDelta {
            exchange_id: ExchangeId(0),
            sequence: 0,
            terminal: true,
            change: CatalogChange::Metadata,
        });
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
            let Some(admission) = state.next_admission.checked_add(1) else {
                state.counters.admission_rejected =
                    state.counters.admission_rejected.saturating_add(1);
                return CatalogApply::AdmissionRejected;
            };
            while state.max_live_entries.is_some_and(|maximum| {
                state.by_id.len().saturating_sub(state.imported_count) >= maximum.get()
            }) {
                if !evict_oldest_terminal(&mut state) {
                    break;
                }
            }
            let ObserverEventKind::ExchangeStarted { metadata } = &event.kind else {
                unreachable!();
            };
            state.next_admission = admission;
            state.order.insert(admission, exchange_id);
            state.by_id.insert(
                exchange_id,
                MutableSession {
                    run_ended: false,
                    request_number: admission,
                    performance: transmog_core::performance::PerformanceEvidence::default(),
                    imported: false,
                    metadata: Arc::clone(metadata),
                    last_sequence: 0,
                    sequence_loss: 0,
                    request_heads: Vec::with_capacity(2),
                    response_heads: Vec::with_capacity(2),
                    bodies: Vec::with_capacity(4),
                    retained_body_bytes: 0,
                    initialization_diagnostics: Vec::new(),
                    hook_effects: Vec::new(),
                    route_selection: None,
                    route_attempts: Vec::new(),
                    terminal: None,
                    terminal_at: None,
                    websocket: None,
                    stream: None,
                    stream_history: transmog_core::stream::StreamHistory::default(),
                },
            );
        }

        let Some(session) = state.by_id.get(&exchange_id) else {
            state.counters.unknown_exchange_events =
                state.counters.unknown_exchange_events.saturating_add(1);
            return CatalogApply::UnknownExchange;
        };
        if session.imported {
            return CatalogApply::PostTerminal;
        }
        if session.run_ended {
            return CatalogApply::PostTerminal;
        }
        let (last_sequence, terminal) = { (session.last_sequence, session.terminal.is_some()) };
        if event.sequence <= last_sequence {
            state.counters.stale_events = state.counters.stale_events.saturating_add(1);
            return CatalogApply::Stale;
        }
        if terminal
            && !matches!(
                event.kind,
                ObserverEventKind::StreamState(_) | ObserverEventKind::StreamMessage(_)
            )
        {
            state.counters.post_terminal_events =
                state.counters.post_terminal_events.saturating_add(1);
            return CatalogApply::PostTerminal;
        }
        if event.sequence > last_sequence.saturating_add(1) {
            let missing = event.sequence - last_sequence - 1;
            state.counters.sequence_gaps = state.counters.sequence_gaps.saturating_add(missing);
            if let Some(session) = state.by_id.get_mut(&exchange_id) {
                session.sequence_loss = session.sequence_loss.saturating_add(missing);
            }
        }

        let Some(delta) = update_session(&mut state, event, self.inner.limits) else {
            return CatalogApply::UnknownExchange;
        };
        drop(state);
        let _ = self.inner.deltas.send(delta);
        CatalogApply::Applied
    }

    /// Finalizes live evidence only after its runtime has ended. Returned local
    /// terminal observations are also written to any active native capture.
    pub(crate) fn finish_live(&self) -> Vec<ObserverEvent> {
        let mut state = self.lock_state();
        let mut events = Vec::new();
        let mut deltas = Vec::new();
        for session in state
            .by_id
            .values_mut()
            .filter(|session| !session.imported && !session.run_ended)
        {
            let mut kinds = Vec::new();
            if let Some(stream) = &session.stream
                && stream.status.is_active()
            {
                // Stream state precedes the HTTP terminal record: capture
                // sequencing retires an exchange after its final HTTP event.
                kinds.push(ObserverEventKind::StreamState(
                    transmog_core::stream::StreamState::now(
                        stream.protocol,
                        transmog_core::stream::StreamStatus::Interrupted,
                        Some("Proxy stopped before the stream closed.".into()),
                    ),
                ));
            }
            if session.terminal.is_none() {
                kinds.push(ObserverEventKind::Failed(ExchangeFailure {
                    metadata: Arc::clone(&session.metadata),
                    stage: ExchangeStage::Terminal,
                    kind: ExchangeFailureKind::Shutdown,
                    request_committed: session
                        .request_heads
                        .iter()
                        .any(|head| head.boundary == ExchangeBoundary::UpstreamRequest),
                    response_committed: session
                        .response_heads
                        .iter()
                        .any(|head| head.boundary == ExchangeBoundary::ClientResponse),
                    message: "Proxy stopped before this request recorded a completion.".into(),
                }));
            }
            session.run_ended = true;
            if kinds.is_empty() {
                continue;
            }
            for kind in kinds {
                session.last_sequence = session.last_sequence.saturating_add(1);
                events.push(ObserverEvent {
                    exchange_id: session.metadata.exchange_id,
                    sequence: session.last_sequence,
                    kind: kind.clone(),
                });
                apply_kind(session, kind, self.inner.limits, &mut 0);
            }
            deltas.push(CatalogDelta {
                exchange_id: session.metadata.exchange_id,
                sequence: session.last_sequence,
                terminal: true,
                change: CatalogChange::Terminal,
            });
        }
        trim_live_entries(&mut state);
        drop(state);
        for delta in deltas {
            let _ = self.inner.deltas.send(delta);
        }
        events
    }

    /// Gets one immutable snapshot by exchange ID.
    pub fn get(&self, exchange_id: ExchangeId) -> Option<SessionSnapshot> {
        let state = self.lock_state();
        if state.dismissed.contains(&exchange_id) {
            return None;
        }
        if let Some(reference) = state.borrowed.get(&exchange_id) {
            return reference.snapshot(true);
        }
        state.by_id.get(&exchange_id).map(MutableSession::snapshot)
    }

    /// Projects only borrowed retained heads while holding the catalog lock.
    /// The callback must be bounded native code and must not perform I/O or re-enter this catalog.
    pub fn with_heads<T>(
        &self,
        exchange_id: ExchangeId,
        project: impl FnOnce(SessionHeads<'_>) -> T,
    ) -> Option<T> {
        let state = self.lock_state();
        if state.dismissed.contains(&exchange_id) {
            return None;
        }
        if let Some(reference) = state.borrowed.get(&exchange_id) {
            let snapshot = reference.snapshot(false)?;
            return Some(project(SessionHeads::from(&snapshot)));
        }
        state.by_id.get(&exchange_id).map(|entry| {
            project(SessionHeads {
                metadata: &entry.metadata,
                request_number: entry.request_number,
                imported: entry.imported,
                request_heads: &entry.request_heads,
                response_heads: &entry.response_heads,
            })
        })
    }

    /// Up to 4,096 visible retained identities in admission order, between exclusive bounds.
    /// Reverse traversal supports nearest-earlier lookup without cloning unrelated evidence.
    pub fn ordered_ids(
        &self,
        after: Option<u64>,
        before: Option<u64>,
        reverse: bool,
        limit: usize,
    ) -> Vec<(u64, ExchangeId)> {
        use std::ops::Bound::{Excluded, Unbounded};
        if self.has_references() {
            let mut rows = self
                .view_metadata()
                .into_iter()
                .filter(|snapshot| {
                    after.is_none_or(|number| snapshot.request_number > number)
                        && before.is_none_or(|number| snapshot.request_number < number)
                })
                .map(|snapshot| (snapshot.request_number, snapshot.exchange_id))
                .collect::<Vec<_>>();
            if reverse {
                rows.reverse();
            }
            return rows.into_iter().take(limit.min(4096)).collect();
        }
        if after
            .zip(before)
            .is_some_and(|(after, before)| after >= before)
        {
            return Vec::new();
        }
        let state = self.lock_state();
        let range = state.order.range((
            after.map_or(Unbounded, Excluded),
            before.map_or(Unbounded, Excluded),
        ));
        let mut result = Vec::new();
        let mut push = |(&number, &id)| {
            if !state.dismissed.contains(&id) {
                result.push((number, id));
            }
            result.len() < limit.min(4096)
        };
        if limit != 0 {
            if reverse {
                for entry in range.rev() {
                    if !push(entry) {
                        break;
                    }
                }
            } else {
                for entry in range {
                    if !push(entry) {
                        break;
                    }
                }
            }
        }
        result
    }

    /// Reads a retained entry's revision without cloning headers or stream history.
    pub fn sequence(&self, exchange_id: ExchangeId) -> Option<u64> {
        let state = self.lock_state();
        if state.dismissed.contains(&exchange_id) {
            return None;
        }
        if let Some(reference) = state.borrowed.get(&exchange_id) {
            return reference
                .snapshot(false)
                .map(|snapshot| snapshot.last_sequence);
        }
        state
            .by_id
            .get(&exchange_id)
            .map(|entry| entry.last_sequence)
    }

    /// Atomically adds immutable saved sessions without evicting live evidence.
    /// Saved entries have a separate 100,000-entry bound and support ordinary
    /// queries, selection, dismissal and Undo. Observer events cannot change them.
    ///
    /// # Errors
    /// Returns an import bound or ID collision error before publishing any entry.
    pub fn import_sessions(&self, sessions: Vec<SessionSnapshot>) -> Result<(), CatalogError> {
        self.import_sessions_with(sessions, || {})
    }

    /// Publishes a saved batch after preparing its body sources and provenance.
    /// The callback runs after validation, while readers are excluded, and must
    /// not re-enter this catalog. Notifications follow the complete publication.
    ///
    /// # Errors
    /// Returns an import bound or collision error without invoking the callback.
    pub fn import_sessions_with(
        &self,
        sessions: Vec<SessionSnapshot>,
        prepare: impl FnOnce(),
    ) -> Result<(), CatalogError> {
        self.try_import_sessions_with(sessions, || {
            prepare();
            Ok(())
        })
    }

    /// Publishes a saved batch with a final fallible preparation step under the
    /// catalog lock. The callback must check cancellation before changing any
    /// external state and must not re-enter this catalog.
    ///
    /// # Errors
    /// Returns a bound or collision error without calling `prepare`, or its
    /// error without publishing any sessions or notifications.
    pub fn try_import_sessions_with(
        &self,
        sessions: Vec<SessionSnapshot>,
        prepare: impl FnOnce() -> Result<(), CatalogError>,
    ) -> Result<(), CatalogError> {
        let mut state = self.lock_state();
        let mut ids = HashSet::new();
        if state.imported_count.saturating_add(sessions.len()) > 100_000
            || sessions.iter().any(|session| {
                state.by_id.contains_key(&session.exchange_id)
                    || state.borrowed.contains_key(&session.exchange_id)
                    || !ids.insert(session.exchange_id)
            })
        {
            return Err(CatalogError::ImportLimit);
        }
        let count =
            u64::try_from(sessions.len()).map_err(|_| CatalogError::RequestNumberExhausted)?;
        state
            .next_admission
            .checked_add(count)
            .ok_or(CatalogError::RequestNumberExhausted)?;
        prepare()?;
        for snapshot in sessions {
            state.next_admission += 1;
            let admission = state.next_admission;
            let id = snapshot.exchange_id;
            state.order.insert(admission, id);
            state.imported_count += 1;
            let bodies = snapshot
                .bodies
                .into_iter()
                .map(|body| MutableBody {
                    boundary: body.boundary,
                    observed_bytes: body.observed_bytes,
                    retained_prefix: BytesMut::new(),
                    truncated: body.truncated,
                    trailers: body.trailers,
                })
                .collect();
            state.by_id.insert(
                id,
                MutableSession {
                    run_ended: true,
                    request_number: admission,
                    imported: true,
                    performance: snapshot.performance,
                    metadata: snapshot.metadata,
                    last_sequence: snapshot.last_sequence,
                    sequence_loss: snapshot.sequence_loss,
                    request_heads: snapshot.request_heads,
                    response_heads: snapshot.response_heads,
                    bodies,
                    retained_body_bytes: 0,
                    initialization_diagnostics: snapshot.initialization_diagnostics,
                    hook_effects: snapshot.hook_effects,
                    route_selection: snapshot.route_selection,
                    route_attempts: snapshot.route_attempts,
                    terminal: snapshot.terminal,
                    terminal_at: snapshot.terminal_at,
                    websocket: snapshot.websocket,
                    stream: snapshot.stream,
                    stream_history: snapshot.stream_history,
                },
            );
        }
        drop(state);
        self.notify_view_change(&ids.into_iter().collect::<Vec<_>>());
        Ok(())
    }

    /// Retained identities, including dismissed evidence available for Undo.
    pub fn retained_ids(&self) -> Vec<ExchangeId> {
        {
            let state = self.lock_state();
            state
                .by_id
                .keys()
                .chain(state.borrowed.keys())
                .copied()
                .collect()
        }
    }

    /// Open or connecting live `WebSockets` that remain visible in Traffic.
    pub fn live_websocket_count(&self) -> usize {
        let state = self.lock_state();
        state
            .by_id
            .iter()
            .filter(|(id, session)| !state.dismissed.contains(id) && session.live_websocket())
            .count()
    }

    /// Removes entries from the managed view, preserving open live `WebSockets` unless explicitly terminated.
    /// The bounded catalog retains them for Undo until ordinary eviction.
    pub fn dismiss(
        &self,
        ids: &[ExchangeId],
        terminated_websockets: &[ExchangeId],
    ) -> Vec<ExchangeId> {
        let mut state = self.lock_state();
        let terminated = terminated_websockets
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let changed = ids
            .iter()
            .copied()
            .filter(|id| {
                state
                    .by_id
                    .get(id)
                    .is_some_and(|session| !session.live_websocket() || terminated.contains(id))
                    || state.borrowed.contains_key(id)
            })
            .collect::<Vec<_>>();
        let changed = changed
            .into_iter()
            .filter(|id| state.dismissed.insert(*id))
            .collect::<Vec<_>>();
        track_dismissal(&mut state, &changed);
        drop(state);
        self.notify_view_change(&changed);
        changed
    }

    /// Removes every currently visible entry outside a selected ID set. This
    /// operates across the catalog, independent of filters and page size, and
    /// retains evidence for Undo without stopping requests or deleting files.
    pub fn dismiss_unselected(
        &self,
        selected: &HashSet<ExchangeId>,
        terminated_websockets: &[ExchangeId],
    ) -> Vec<ExchangeId> {
        if self.has_references() {
            let ids = self
                .retained_ids()
                .into_iter()
                .filter(|id| !selected.contains(id))
                .collect::<Vec<_>>();
            return self.dismiss(&ids, terminated_websockets);
        }
        let mut state = self.lock_state();
        let terminated = terminated_websockets
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let changed = state
            .by_id
            .iter()
            .filter(|(id, session)| !session.live_websocket() || terminated.contains(id))
            .map(|(id, _)| *id)
            .filter(|id| !selected.contains(id) && !state.dismissed.contains(id))
            .collect::<Vec<_>>();
        state.dismissed.extend(changed.iter().copied());
        track_dismissal(&mut state, &changed);
        drop(state);
        self.notify_view_change(&changed);
        changed
    }

    /// Clears every retained entry, including evidence from earlier removals.
    /// Renew dismissal identities so earlier expiry tasks cannot delete this Undo.
    pub fn dismiss_all(&self, terminated_websockets: &[ExchangeId]) -> Vec<ExchangeId> {
        if self.has_references() {
            let ids = self.retained_ids();
            return self.dismiss(&ids, terminated_websockets);
        }
        let mut state = self.lock_state();
        let terminated = terminated_websockets
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let ids = state
            .by_id
            .iter()
            .filter(|(id, session)| !session.live_websocket() || terminated.contains(id))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        state.dismissed.extend(ids.iter().copied());
        track_dismissal(&mut state, &ids);
        drop(state);
        self.notify_view_change(&ids);
        ids
    }

    /// Permanently releases dismissed evidence after Undo expires.
    pub fn forget_dismissed(&self, ids: &[ExchangeId]) -> Vec<ExchangeId> {
        let mut state = self.lock_state();
        forget_dismissed(&mut state, ids)
    }

    /// Snapshot dismissal identities so a stale expiry cannot erase later work.
    pub fn dismissal_versions(&self, ids: &[ExchangeId]) -> Vec<(ExchangeId, u64)> {
        let state = self.lock_state();
        ids.iter()
            .filter_map(|id| {
                state
                    .dismissal_versions
                    .get(id)
                    .map(|version| (*id, *version))
            })
            .collect()
    }

    /// Release only dismissals which still belong to this Undo operation.
    pub fn forget_dismissed_versions(&self, versions: &[(ExchangeId, u64)]) -> Vec<ExchangeId> {
        let mut state = self.lock_state();
        let ids = versions
            .iter()
            .filter_map(|(id, version)| {
                (state.dismissal_versions.get(id) == Some(version)).then_some(*id)
            })
            .collect::<Vec<_>>();
        forget_dismissed(&mut state, &ids)
    }

    /// Counts retained header bytes without cloning headers during Clear.
    pub fn retained_header_bytes(&self, ids: &[ExchangeId]) -> u64 {
        let state = self.lock_state();
        ids.iter()
            .filter_map(|id| state.by_id.get(id))
            .flat_map(|session| {
                session
                    .request_heads
                    .iter()
                    .map(|h| &h.head.headers)
                    .chain(session.response_heads.iter().map(|h| &h.head.headers))
            })
            .flat_map(transmog_core::HeaderBlock::iter)
            .fold(0_u64, |sum, field| {
                sum.saturating_add((field.name().len() + field.value().len()) as u64)
            })
    }

    /// Releases all in-memory evidence when the owning desktop session closes.
    pub fn discard_all(&self) -> Vec<ExchangeId> {
        let references = std::mem::take(&mut self.lock_state().borrowed);
        let borrowed_ids = references.keys().copied().collect::<Vec<_>>();
        drop(references);
        let mut state = self.lock_state();
        let ids = state
            .by_id
            .keys()
            .filter(|id| !state.pins.contains_key(id))
            .copied()
            .chain(borrowed_ids)
            .collect::<Vec<_>>();
        let pinned = state.pins.keys().copied().collect::<HashSet<_>>();
        state.by_id.retain(|id, _| pinned.contains(id));
        state.dismissed.extend(pinned.iter().copied());
        state.order.retain(|_, id| pinned.contains(id));
        state.dismissed.retain(|id| !ids.contains(id));
        state.dismissal_versions.clear();
        state.imported_count = 0;
        ids
    }

    /// Restores dismissed entries which have not since been evicted.
    pub fn restore_dismissed(&self, ids: &[ExchangeId]) -> Vec<ExchangeId> {
        let mut state = self.lock_state();
        let changed = ids
            .iter()
            .copied()
            .filter(|id| state.dismissed.remove(id))
            .collect::<Vec<_>>();
        for id in &changed {
            state.dismissal_versions.remove(id);
        }
        drop(state);
        self.notify_view_change(&changed);
        changed
    }

    fn notify_view_change(&self, ids: &[ExchangeId]) {
        if let Some(id) = ids.first() {
            let _ = self.inner.deltas.send(CatalogDelta {
                exchange_id: *id,
                sequence: 0,
                terminal: false,
                change: CatalogChange::Metadata,
            });
        }
    }

    /// Pages authoritative state in stable admission order.
    pub fn query(&self, query: &CatalogQuery) -> CatalogPage {
        if self.has_references() {
            return self.query_view(query, false);
        }
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
                if state.dismissed.contains(exchange_id) {
                    return None;
                }
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

    /// Returns the newest matching bounded window in admission order.
    ///
    /// This is intended for live traffic surfaces that follow the tail rather
    /// than expose cursor-based historical pagination.
    pub fn query_latest(&self, query: &CatalogQuery) -> CatalogPage {
        if self.has_references() {
            return self.query_view(query, true);
        }
        let state = self.lock_state();
        let limit = query
            .limit
            .map_or(self.inner.limits.max_page_size.get(), NonZeroUsize::get)
            .min(self.inner.limits.max_page_size.get());
        let mut selected = state
            .order
            .iter()
            .rev()
            .filter_map(|(_, exchange_id)| {
                if state.dismissed.contains(exchange_id) {
                    return None;
                }
                let session = state.by_id.get(exchange_id)?;
                matches_filter(session, &query.filter).then_some(session)
            })
            .take(limit)
            .collect::<Vec<_>>();
        selected.reverse();
        CatalogPage {
            sessions: selected.into_iter().map(MutableSession::snapshot).collect(),
            next: None,
        }
    }

    /// Returns monotonic pressure and loss counters.
    pub fn counters(&self) -> CatalogCounters {
        self.lock_state().counters
    }

    /// Projects all retained exchanges into lightweight owned read models.
    ///
    /// Membership and admission order are selected at the start. Each entry is
    /// snapshotted separately; entries removed before their turn are omitted.
    /// Projection runs outside the writer lock, so formatting a large traffic
    /// list cannot stall observer ingestion. Only one temporary snapshot is
    /// alive at a time, and the callback may query this catalog.
    pub fn project_retained<T>(&self, mut project: impl FnMut(&SessionSnapshot) -> T) -> Vec<T> {
        if self.has_references() {
            return self
                .view_metadata()
                .into_iter()
                .filter_map(|snapshot| self.get(snapshot.exchange_id))
                .map(|snapshot| project(&snapshot))
                .collect();
        }
        let ids = {
            let state = self.lock_state();
            state
                .order
                .values()
                .filter(|id| !state.dismissed.contains(id))
                .copied()
                .collect::<Vec<_>>()
        };
        ids.into_iter()
            .filter_map(|id| self.get(id))
            .map(|snapshot| project(&snapshot))
            .collect()
    }

    /// Atomically copies retained metadata at one lifecycle/time boundary.
    /// Body prefixes and message collections are omitted; formatting happens after unlocking.
    pub fn retained_metadata(&self) -> Vec<SessionSnapshot> {
        if self.has_references() {
            return self.view_metadata();
        }
        let state = self.lock_state();
        let now = std::time::Instant::now();
        let cutoff_millis = transmog_core::stream::observed_millis();
        state
            .order
            .values()
            .filter(|id| !state.dismissed.contains(id))
            .filter_map(|id| {
                state
                    .by_id
                    .get(id)
                    .map(|session| session.snapshot_with_content(false, now, cutoff_millis))
            })
            .collect()
    }

    /// Maximum number of rows returned by a presentation query.
    pub fn page_size_limit(&self) -> usize {
        self.inner.limits.max_page_size.get()
    }

    /// Attaches terminal `WebSocket` relay evidence to its originating exchange.
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
            terminal: !session.is_active(),
            change: CatalogChange::Metadata,
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

fn trim_live_entries(state: &mut CatalogState) {
    while state.max_live_entries.is_some_and(|maximum| {
        state.by_id.len().saturating_sub(state.imported_count) > maximum.get()
    }) {
        if !evict_oldest_terminal(state) {
            break;
        }
    }
}

fn evict_oldest_terminal(state: &mut CatalogState) -> bool {
    let candidate = state.order.iter().find_map(|(admission, exchange_id)| {
        state
            .by_id
            .get(exchange_id)
            .filter(|session| !session.imported)
            .filter(|_| !state.pins.contains_key(exchange_id))
            .filter(|session| !session.is_active())
            .map(|_| (*admission, *exchange_id))
    });
    let Some((admission, exchange_id)) = candidate else {
        return false;
    };
    state.order.remove(&admission);
    state.by_id.remove(&exchange_id);
    state.dismissed.remove(&exchange_id);
    state.dismissal_versions.remove(&exchange_id);
    state.counters.evicted = state.counters.evicted.saturating_add(1);
    true
}

fn update_session(
    state: &mut CatalogState,
    mut event: ObserverEvent,
    limits: SessionLimits,
) -> Option<CatalogDelta> {
    let mut detail_dropped = 0_u64;
    retain_stream_content(&mut event.kind, state.retain_stream_content);
    let session = state.by_id.get_mut(&event.exchange_id)?;
    session.last_sequence = event.sequence;
    let change = CatalogChange::from_kind(&event.kind);
    apply_kind(session, event.kind, limits, &mut detail_dropped);
    let delta = CatalogDelta {
        exchange_id: event.exchange_id,
        sequence: session.last_sequence,
        terminal: !session.is_active(),
        change,
    };
    state.counters.detail_records_dropped = state
        .counters
        .detail_records_dropped
        .saturating_add(detail_dropped);
    trim_live_entries(state);
    Some(delta)
}

fn retain_stream_content(kind: &mut ObserverEventKind, retention: [bool; 2]) {
    if let ObserverEventKind::StreamMessage(message) = kind
        && !retention[usize::from(message.sender != "client")]
    {
        message.truncated |= !message.payload.is_empty() || message.raw.is_some();
        message.payload.clear();
        message.raw = None;
        message.event = None;
        message.event_id = None;
    }
}

fn apply_kind(
    session: &mut MutableSession,
    kind: ObserverEventKind,
    limits: SessionLimits,
    detail_dropped: &mut u64,
) {
    match kind {
        ObserverEventKind::StreamMessage(message) => {
            session.stream_history.push(session.last_sequence, message);
        }
        ObserverEventKind::StreamState(stream) => {
            session
                .stream_history
                .observe_state(&stream, !session.imported);
            session.stream = Some(stream);
        }
        ObserverEventKind::Performance(evidence) => session.performance.merge(&evidence),
        ObserverEventKind::ExchangeStarted { .. } | ObserverEventKind::BodyCompleted { .. } => {}
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
            session.terminal_at = Some(SystemTime::now());
            if let Some(stream) = &mut session.stream
                && stream.protocol == transmog_core::stream::StreamProtocol::Sse
                && stream.status.is_active()
            {
                stream.status = transmog_core::stream::StreamStatus::Closed;
                stream.at_millis = transmog_core::stream::observed_millis();
                session
                    .stream_history
                    .observe_state(stream, !session.imported);
            }
        }
        ObserverEventKind::Failed(failure) => {
            let interrupted = matches!(
                failure.kind,
                ExchangeFailureKind::Cancelled | ExchangeFailureKind::Shutdown
            );
            session.terminal = Some(SessionTerminal::Failed(failure));
            session.terminal_at = Some(SystemTime::now());
            if let Some(stream) = &mut session.stream
                && stream.status.is_active()
                && (interrupted || stream.protocol == transmog_core::stream::StreamProtocol::Sse)
            {
                stream.status = if interrupted {
                    transmog_core::stream::StreamStatus::Interrupted
                } else {
                    transmog_core::stream::StreamStatus::Failed
                };
                stream.at_millis = transmog_core::stream::observed_millis();
                session
                    .stream_history
                    .observe_state(stream, !session.imported);
            }
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
        .is_some_and(|terminal| terminal == session.is_active())
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

fn track_dismissal(state: &mut CatalogState, ids: &[ExchangeId]) {
    if ids.is_empty() {
        return;
    }
    state.next_dismissal = state.next_dismissal.wrapping_add(1).max(1);
    for id in ids {
        state.dismissal_versions.insert(*id, state.next_dismissal);
    }
}
fn forget_dismissed(state: &mut CatalogState, ids: &[ExchangeId]) -> Vec<ExchangeId> {
    let removed = ids
        .iter()
        .copied()
        .filter(|id| state.dismissed.contains(id) && !state.pins.contains_key(id))
        .collect::<HashSet<_>>();
    let references = removed
        .iter()
        .filter_map(|id| state.borrowed.remove(id))
        .collect::<Vec<_>>();
    // Reference drops can release a parent catalog; no parent ever calls its child.
    drop(references);
    for id in &removed {
        if let Some(session) = state.by_id.remove(id) {
            state.imported_count = state
                .imported_count
                .saturating_sub(usize::from(session.imported));
        }
        state.dismissed.remove(id);
        state.dismissal_versions.remove(id);
    }
    state.order.retain(|_, id| !removed.contains(id));
    removed.into_iter().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_pins_leave_room_for_the_main_catalog_to_rotate() {
        let root = SessionCatalog::new(limits(2));
        start(&root, 1, "example.test");
        start(&root, 2, "example.test");
        let view = SessionCatalog::new(limits(2));
        view.reference_exchanges(&root, &[ExchangeId(1)], Arc::new(|_| {}))
            .unwrap();
        assert!(
            view.reference_exchanges(&root, &[ExchangeId(2)], Arc::new(|_| {}))
                .is_err()
        );
        assert_eq!(root.lock_state().pins.len(), 1);
    }
    #[test]
    fn private_views_share_one_exchange_and_survive_main_clear_until_last_release() {
        let root = SessionCatalog::new(limits(8));
        start(&root, 1, "example.test");
        let a = SessionCatalog::new(limits(8));
        let b = SessionCatalog::new(limits(8));
        let released = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = released.clone();
        let release = std::sync::Arc::new(move |ids: Vec<ExchangeId>| {
            counter.fetch_add(ids.len(), std::sync::atomic::Ordering::SeqCst);
        });
        a.reference_exchanges(&root, &[ExchangeId(1)], release.clone())
            .unwrap();
        b.reference_exchanges(&root, &[ExchangeId(1)], release)
            .unwrap();
        assert_eq!(root.retained_ids().len(), 1);
        assert_eq!(
            a.get(ExchangeId(1)).unwrap().exchange_id,
            b.get(ExchangeId(1)).unwrap().exchange_id
        );
        root.dismiss_all(&[ExchangeId(1)]);
        assert_eq!(
            root.forget_dismissed(&[ExchangeId(1)]),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        assert!(root.get(ExchangeId(1)).is_none());
        assert!(a.get(ExchangeId(1)).is_some());
        a.discard_all();
        assert!(b.get(ExchangeId(1)).is_some());
        assert_eq!(released.load(std::sync::atomic::Ordering::SeqCst), 0);
        b.discard_all();
        assert_eq!(released.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            root.retained_ids(),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
    }

    #[tokio::test]
    async fn catalog_deltas_classify_message_interests_without_carrying_values() {
        let catalog = SessionCatalog::new(limits(3));
        let mut subscription = catalog.subscribe().unwrap();
        start(&catalog, 1, "example.test");
        let kinds = [
            (
                ObserverEventKind::RequestHeadObserved {
                    boundary: ExchangeBoundary::ClientRequest,
                    head: head("POST"),
                },
                CatalogChange::Head(ExchangeBoundary::ClientRequest),
            ),
            (
                ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 1,
                    sample: None,
                    truncated: false,
                }),
                CatalogChange::Body(ExchangeBoundary::ClientRequest),
            ),
            (
                ObserverEventKind::BodyCompleted {
                    boundary: ExchangeBoundary::ClientRequest,
                },
                CatalogChange::BodyCompleted(ExchangeBoundary::ClientRequest),
            ),
            (
                ObserverEventKind::Performance(
                    transmog_core::performance::PerformanceEvidence::default(),
                ),
                CatalogChange::Metadata,
            ),
        ];
        let SubscriptionEvent::Delta(started) = subscription.recv().await.unwrap() else {
            panic!("delta expected")
        };
        assert_eq!(started.change, CatalogChange::Metadata);
        for (index, (kind, expected)) in kinds.into_iter().enumerate() {
            catalog.apply(event(1, index as u64 + 2, kind));
            let SubscriptionEvent::Delta(delta) = subscription.recv().await.unwrap() else {
                panic!("delta expected")
            };
            assert_eq!(delta.change, expected);
        }
        fail(&catalog, 1, 6);
        let SubscriptionEvent::Delta(terminal) = subscription.recv().await.unwrap() else {
            panic!("delta expected")
        };
        assert_eq!(terminal.change, CatalogChange::Terminal);
    }

    #[test]
    fn request_numbers_survive_completion_undo_and_clear_without_reuse() {
        let catalog = SessionCatalog::new(limits(2));
        start(&catalog, 90, "one.test");
        start(&catalog, 2, "two.test");
        fail(&catalog, 2, 2);
        fail(&catalog, 90, 2);
        assert_eq!(catalog.get(ExchangeId(90)).unwrap().request_number, 1);
        assert_eq!(catalog.get(ExchangeId(2)).unwrap().request_number, 2);
        catalog.dismiss_all(&[]);
        catalog.restore_dismissed(&[ExchangeId(90)]);
        assert_eq!(catalog.get(ExchangeId(90)).unwrap().request_number, 1);
        catalog.dismiss_all(&[]);
        catalog.forget_dismissed(&[ExchangeId(90), ExchangeId(2)]);
        assert_eq!(
            catalog.apply(event(
                90,
                3,
                ObserverEventKind::StreamState(transmog_core::stream::StreamState::now(
                    transmog_core::stream::StreamProtocol::Sse,
                    transmog_core::stream::StreamStatus::Closed,
                    None
                ))
            )),
            CatalogApply::UnknownExchange
        );
        assert!(catalog.get(ExchangeId(90)).is_none());
        start(&catalog, 3, "three.test");
        assert_eq!(catalog.get(ExchangeId(3)).unwrap().request_number, 3);
    }

    #[test]
    fn borrowed_head_projection_and_ordered_identity_pages_exclude_dismissed_evidence() {
        let catalog = SessionCatalog::new(limits(8));
        for id in [90, 2, 50] {
            start(&catalog, id, "one.test");
        }
        assert_eq!(
            catalog.ordered_ids(None, None, false, 2),
            [(1, ExchangeId(90)), (2, ExchangeId(2))]
        );
        assert_eq!(
            catalog.ordered_ids(None, Some(3), true, 2),
            [(2, ExchangeId(2)), (1, ExchangeId(90))]
        );
        assert_eq!(
            catalog.ordered_ids(Some(1), Some(3), false, 2),
            [(2, ExchangeId(2))]
        );
        assert_eq!(
            catalog.ordered_ids(Some(3), Some(3), false, 2),
            [] as [(u64, transmog_core::intercept::ExchangeId); 0]
        );
        assert_eq!(
            catalog.ordered_ids(None, None, false, 0),
            [] as [(u64, transmog_core::intercept::ExchangeId); 0]
        );
        assert_eq!(
            catalog.with_heads(ExchangeId(90), |heads| (
                heads.request_number,
                heads.imported
            )),
            Some((1, false))
        );
        catalog.dismiss(&[ExchangeId(2)], &[]);
        assert!(catalog.with_heads(ExchangeId(2), |_| ()).is_none());
        assert_eq!(
            catalog.ordered_ids(None, None, false, 4),
            [(1, ExchangeId(90)), (3, ExchangeId(50))]
        );
    }

    #[test]
    fn request_number_exhaustion_and_canceled_import_are_atomic() {
        let source = SessionCatalog::new(limits(3));
        start(&source, 20, "one.test");
        start(&source, 10, "two.test");
        let batch = source.project_retained(Clone::clone);
        let catalog = SessionCatalog::new(limits(3));
        catalog.lock_state().next_admission = u64::MAX - 1;
        assert_eq!(
            catalog.import_sessions(batch.clone()),
            Err(CatalogError::RequestNumberExhausted)
        );
        assert_eq!(catalog.lock_state().next_admission, u64::MAX - 1);
        assert_eq!(
            catalog.project_retained(|row| row.exchange_id),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        assert_eq!(
            catalog.try_import_sessions_with(vec![batch[0].clone()], || Err(
                CatalogError::ImportCanceled
            )),
            Err(CatalogError::ImportCanceled)
        );
        assert_eq!(catalog.lock_state().next_admission, u64::MAX - 1);
        catalog.import_sessions(vec![batch[0].clone()]).unwrap();
        assert_eq!(
            catalog.get(ExchangeId(20)).unwrap().request_number,
            u64::MAX
        );
        assert_eq!(
            catalog.apply(event(
                99,
                1,
                ObserverEventKind::ExchangeStarted {
                    metadata: metadata(99, "rejected.test")
                }
            )),
            CatalogApply::AdmissionRejected
        );
        assert!(catalog.get(ExchangeId(99)).is_none());
        assert_eq!(catalog.counters().admission_rejected, 1);
    }

    #[test]
    fn live_and_imported_requests_share_one_numbering_namespace() {
        let source = SessionCatalog::new(limits(3));
        start(&source, 20, "one.test");
        start(&source, 10, "two.test");
        let catalog = SessionCatalog::new(limits(3));
        start(&catalog, 90, "live.test");
        catalog
            .import_sessions(source.project_retained(Clone::clone))
            .unwrap();
        fail(&catalog, 90, 2);
        start(&catalog, 99, "later.test");
        assert_eq!(
            catalog.project_retained(|row| (row.exchange_id.0, row.request_number)),
            [(90, 1), (20, 2), (10, 3), (99, 4)]
        );
    }

    #[test]
    fn clear_includes_previous_removals_and_stale_expiry_cannot_release_new_undo() {
        let catalog = SessionCatalog::new(limits(3));
        start(&catalog, 1, "placeholder.invalid");
        start(&catalog, 2, "placeholder.invalid");
        catalog.dismiss(&[ExchangeId(1)], &[]);
        let old = catalog.dismissal_versions(&[ExchangeId(1)]);
        assert_eq!(catalog.dismiss_all(&[]).len(), 2);
        assert_eq!(
            catalog.forget_dismissed_versions(&old),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        let clear = catalog.dismissal_versions(&[ExchangeId(1), ExchangeId(2)]);
        catalog.restore_dismissed(&[ExchangeId(1)]);
        catalog.dismiss(&[ExchangeId(1)], &[]);
        assert_eq!(
            catalog.forget_dismissed_versions(&clear),
            vec![ExchangeId(2)]
        );
        catalog.restore_dismissed(&[ExchangeId(1)]);
        assert!(catalog.get(ExchangeId(1)).is_some());
        assert!(catalog.get(ExchangeId(2)).is_none());
    }
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

    #[test]
    fn slow_projection_allows_ingestion_and_preserves_initial_admission_order() {
        use std::{sync::mpsc, thread, time::Duration};

        let catalog = SessionCatalog::new(SessionLimits::default());
        start(&catalog, 1, "one.test");
        start(&catalog, 2, "two.test");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let reader_catalog = catalog.clone();
        let reader = thread::spawn(move || {
            reader_catalog.project_retained(|snapshot| {
                if snapshot.exchange_id == ExchangeId(1) {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                (snapshot.exchange_id, snapshot.terminal.is_some())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (applied_tx, applied_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            fail(&catalog, 2, 2);
            start(&catalog, 3, "three.test");
            applied_tx.send(()).unwrap();
        });
        let progressed = applied_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        // Always release the reader, even when testing an implementation that
        // incorrectly holds the writer lock across the projection callback.
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        let rows = reader.join().unwrap();
        assert!(progressed, "traffic projection blocked observer ingestion");
        assert_eq!(rows, [(ExchangeId(1), false), (ExchangeId(2), true)]);
    }

    #[test]
    fn pending_websockets_survive_clear_forget_expiry_and_live_retention() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(limits(1));
        let mut opening = (*metadata(1, "opening.test")).clone();
        opening.websocket_requested = true;
        catalog.apply(event(
            1,
            1,
            ObserverEventKind::ExchangeStarted {
                metadata: Arc::new(opening),
            },
        ));
        start(&catalog, 2, "ordinary.test");
        assert_eq!(
            catalog.dismiss(&[ExchangeId(1)], &[]),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        assert_eq!(
            catalog.dismiss_unselected(&HashSet::from([ExchangeId(2)]), &[]),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        let removed = catalog.dismiss_all(&[]);
        assert_eq!(removed, vec![ExchangeId(2)]);
        let expiry = catalog.dismissal_versions(&removed);
        // Non-undoable Clear and expired Undo use these same permanent release paths.
        assert_eq!(catalog.forget_dismissed(&removed), vec![ExchangeId(2)]);
        assert_eq!(
            catalog.forget_dismissed_versions(&expiry),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        assert!(catalog.get(ExchangeId(1)).is_some());
        assert_eq!(catalog.live_websocket_count(), 1);
        start(&catalog, 3, "later.test");
        fail(&catalog, 3, 2);
        assert!(catalog.get(ExchangeId(3)).is_none());
        for (sequence, status) in [(2, StreamStatus::Connecting), (3, StreamStatus::Open)] {
            assert_eq!(
                catalog.apply(event(
                    1,
                    sequence,
                    ObserverEventKind::StreamState(StreamState::now(
                        StreamProtocol::WebSocket,
                        status,
                        None
                    ))
                )),
                CatalogApply::Applied
            );
            assert!(catalog.get(ExchangeId(1)).is_some());
        }
        // Imported open-at-capture evidence never protects or controls a live connection.
        let imported = SessionCatalog::new(limits(1));
        imported
            .import_sessions(catalog.project_retained(Clone::clone))
            .unwrap();
        assert_eq!(imported.live_websocket_count(), 0);
        assert_eq!(imported.dismiss_all(&[]), vec![ExchangeId(1)]);
        assert_eq!(
            imported.forget_dismissed(&[ExchangeId(1)]),
            vec![ExchangeId(1)]
        );
        // Explicit runtime-confirmed termination still permits removal while observations lag.
        assert_eq!(catalog.dismiss_all(&[ExchangeId(1)]), vec![ExchangeId(1)]);
        assert_eq!(
            catalog.forget_dismissed(&[ExchangeId(1)]),
            vec![ExchangeId(1)]
        );
    }

    #[test]
    fn failed_websocket_opening_is_removable_without_termination() {
        let catalog = SessionCatalog::new(limits(1));
        let mut opening = (*metadata(1, "opening.test")).clone();
        opening.websocket_requested = true;
        catalog.apply(event(
            1,
            1,
            ObserverEventKind::ExchangeStarted {
                metadata: Arc::new(opening),
            },
        ));
        fail(&catalog, 1, 2);
        assert_eq!(catalog.live_websocket_count(), 0);
        assert_eq!(catalog.dismiss_all(&[]), vec![ExchangeId(1)]);
    }

    #[test]
    fn removal_preserves_open_websockets_and_only_allows_explicitly_terminated_ones() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(limits(10));
        for id in 1..=3 {
            start(&catalog, id, "live.test");
        }
        for id in 1..=2 {
            catalog.apply(event(
                id,
                2,
                ObserverEventKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Open,
                    None,
                )),
            ));
        }
        assert_eq!(
            catalog.dismiss(&[ExchangeId(1), ExchangeId(3)], &[]),
            vec![ExchangeId(3)]
        );
        assert_eq!(catalog.live_websocket_count(), 2);
        catalog.restore_dismissed(&[ExchangeId(3)]);
        assert_eq!(
            catalog.dismiss_unselected(&HashSet::new(), &[]),
            vec![ExchangeId(3)]
        );
        assert_eq!(catalog.dismiss_all(&[]), vec![ExchangeId(3)]);
        // A connection that opened after termination selection stays visible.
        let removed = catalog.dismiss_all(&[ExchangeId(1)]);
        assert!(removed.contains(&ExchangeId(1)));
        assert!(!removed.contains(&ExchangeId(2)));
        assert_eq!(catalog.live_websocket_count(), 1);
        catalog.apply(event(
            2,
            3,
            ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::WebSocket,
                StreamStatus::Closed,
                None,
            )),
        ));
        assert!(catalog.dismiss_all(&[]).contains(&ExchangeId(2)));
    }

    #[test]
    fn dismissed_active_entries_stay_hidden_until_undo_or_eviction() {
        let catalog = SessionCatalog::new(limits(2));
        start(&catalog, 1, "one.test");
        start(&catalog, 2, "two.test");
        assert_eq!(catalog.dismiss(&[ExchangeId(1)], &[]), [ExchangeId(1)]);
        fail(&catalog, 1, 2);
        assert!(catalog.get(ExchangeId(1)).is_none());
        assert_eq!(
            catalog.project_retained(|session| session.metadata.exchange_id),
            [ExchangeId(2)]
        );
        assert_eq!(catalog.restore_dismissed(&[ExchangeId(1)]), [ExchangeId(1)]);
        assert!(catalog.get(ExchangeId(1)).unwrap().terminal.is_some());
        catalog.dismiss(&[ExchangeId(1)], &[]);
        start(&catalog, 3, "three.test");
        assert_eq!(catalog.get(ExchangeId(3)).unwrap().request_number, 3);
        assert_eq!(
            catalog.restore_dismissed(&[ExchangeId(1)]),
            [] as [transmog_core::intercept::ExchangeId; 0]
        );
        assert!(catalog.lock_state().dismissed.is_empty());
    }

    fn limits(max_sessions: usize) -> SessionLimits {
        SessionLimits {
            max_sessions: NonZeroUsize::new(max_sessions),
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
                client_identity: transmog_core::ClientIdentity::default(),
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
    fn canceled_preparation_publishes_no_sessions() {
        let source = SessionCatalog::new(limits(1));
        start(&source, 77, "saved.test");
        fail(&source, 77, 2);
        let saved = source.get(ExchangeId(77)).unwrap();
        let catalog = SessionCatalog::new(limits(1));
        assert_eq!(
            catalog.try_import_sessions_with(vec![saved.clone()], || {
                Err(CatalogError::ImportCanceled)
            }),
            Err(CatalogError::ImportCanceled)
        );
        assert!(catalog.get(ExchangeId(77)).is_none());
        assert_eq!(catalog.lock_state().imported_count, 0);
        catalog.import_sessions(vec![saved]).unwrap();
        assert!(catalog.get(ExchangeId(77)).unwrap().imported);
    }

    #[test]
    fn ending_a_run_closes_only_live_entries_and_rejects_late_updates() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(SessionLimits {
            subscriber_capacity: NonZeroUsize::new(8).unwrap(),
            ..limits(10)
        });
        for id in 1..=3 {
            start(&catalog, id, "live.test");
        }
        catalog.apply(event(
            2,
            2,
            ObserverEventKind::Completed(CompletedExchange {
                metadata: metadata(2, "live.test"),
                request_head: head("GET"),
                response_head: ResponseHead {
                    status: 101,
                    headers: HeaderBlock::default(),
                    source_version: transmog_core::HttpLegVersion::Http1,
                },
            }),
        ));
        let completed_at = catalog.get(ExchangeId(2)).unwrap().terminal_at;
        // A completed HTTP handshake can still have an open WebSocket.
        catalog.apply(event(
            2,
            3,
            ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::WebSocket,
                StreamStatus::Open,
                None,
            )),
        ));
        catalog.apply(event(
            3,
            2,
            ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::Sse,
                StreamStatus::Open,
                None,
            )),
        ));
        let saved = SessionCatalog::new(limits(10));
        start(&saved, 4, "saved.test");
        catalog
            .import_sessions(vec![saved.get(ExchangeId(4)).unwrap()])
            .unwrap();
        let mut updates = catalog.inner.deltas.subscribe();
        let events = catalog.finish_live();
        assert_eq!(events.len(), 4); // Two HTTP failures and two interrupted streams.
        assert!(updates.try_recv().unwrap().terminal);
        let request = catalog.get(ExchangeId(1)).unwrap();
        assert_eq!(request.state(), "failed");
        assert!(request.terminal_at.is_some());
        assert!(matches!(
            request.terminal,
            Some(SessionTerminal::Failed(ExchangeFailure {
                kind: ExchangeFailureKind::Shutdown,
                ..
            }))
        ));
        let websocket = catalog.get(ExchangeId(2)).unwrap();
        assert_eq!(websocket.state(), "interrupted");
        assert_eq!(
            websocket.terminal_at, completed_at,
            "stream closure must preserve HTTP duration"
        );
        assert_eq!(catalog.get(ExchangeId(3)).unwrap().state(), "interrupted");
        let imported = catalog.get(ExchangeId(4)).unwrap();
        assert_eq!(imported.state(), "incomplete");
        assert_eq!(imported.terminal_at, None);
        assert!(catalog.finish_live().is_empty());
        assert_eq!(
            catalog.get(ExchangeId(1)).unwrap().terminal_at,
            request.terminal_at
        );
        assert_eq!(
            catalog.apply(event(
                2,
                100,
                ObserverEventKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Open,
                    None
                ))
            )),
            CatalogApply::PostTerminal
        );
        start(&catalog, 5, "next-run.test");
        assert_eq!(catalog.get(ExchangeId(5)).unwrap().state(), "active");
    }

    #[test]
    fn caller_cancellation_is_distinct_from_proxy_shutdown_and_transport_failure() {
        for (kind, expected) in [
            (ExchangeFailureKind::Cancelled, "cancelled"),
            (ExchangeFailureKind::Shutdown, "failed"),
            (ExchangeFailureKind::Upstream, "failed"),
        ] {
            let catalog = SessionCatalog::new(limits(10));
            start(&catalog, 1, "cancelled.test");
            catalog.apply(event(
                1,
                2,
                ObserverEventKind::Failed(ExchangeFailure {
                    metadata: metadata(1, "cancelled.test"),
                    stage: ExchangeStage::Terminal,
                    kind,
                    request_committed: false,
                    response_committed: false,
                    message: "Recorded outcome".into(),
                }),
            ));
            let snapshot = catalog.get(ExchangeId(1)).unwrap();
            assert_eq!(snapshot.state(), expected);
            let imported = SessionCatalog::new(limits(10));
            imported.import_sessions(vec![snapshot]).unwrap();
            assert_eq!(imported.get(ExchangeId(1)).unwrap().state(), expected);
        }
    }

    #[test]
    fn cancellation_ends_a_connecting_application_stream_without_waiting_for_stop() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        for protocol in [StreamProtocol::WebSocket, StreamProtocol::Sse] {
            let catalog = SessionCatalog::new(limits(10));
            start(&catalog, 1, "cancelled.test");
            catalog.apply(event(
                1,
                2,
                ObserverEventKind::StreamState(StreamState::now(
                    protocol,
                    StreamStatus::Connecting,
                    None,
                )),
            ));
            catalog.apply(event(
                1,
                3,
                ObserverEventKind::Failed(ExchangeFailure {
                    metadata: metadata(1, "cancelled.test"),
                    stage: ExchangeStage::Terminal,
                    kind: ExchangeFailureKind::Cancelled,
                    request_committed: false,
                    response_committed: false,
                    message: "Request handling was cancelled before a response was returned."
                        .into(),
                }),
            ));
            let snapshot = catalog.get(ExchangeId(1)).unwrap();
            assert_eq!(snapshot.state(), "cancelled");
            assert_eq!(snapshot.stream.unwrap().status, StreamStatus::Interrupted);
            assert!(snapshot.terminal_at.is_some());
        }
    }

    #[test]
    fn imports_are_atomic_immutable_and_survive_live_eviction() {
        let source = SessionCatalog::new(limits(1));
        start(&source, 77, "saved.test");
        fail(&source, 77, 2);
        let saved = source.get(ExchangeId(77)).unwrap();
        let catalog = SessionCatalog::new(limits(1));
        catalog.import_sessions(vec![saved.clone()]).unwrap();
        assert!(catalog.get(ExchangeId(77)).unwrap().imported);
        let mut prepared = false;
        assert_eq!(
            catalog.import_sessions_with(vec![saved], || prepared = true),
            Err(CatalogError::ImportLimit)
        );
        assert!(!prepared);
        assert_eq!(
            catalog.apply(event(
                77,
                3,
                ObserverEventKind::RequestHeadFinalized(head("POST"))
            )),
            CatalogApply::PostTerminal
        );
        start(&catalog, 1, "live.test");
        fail(&catalog, 1, 2);
        start(&catalog, 2, "new.test");
        assert!(catalog.get(ExchangeId(1)).is_none());
        assert!(catalog.get(ExchangeId(77)).is_some());
        assert_eq!(
            catalog.dismiss(&[ExchangeId(77)], &[]),
            vec![ExchangeId(77)]
        );
        assert_eq!(catalog.query(&CatalogQuery::default()).sessions.len(), 1);
        catalog.restore_dismissed(&[ExchangeId(77)]);
        assert_eq!(
            catalog
                .project_retained(|session| session.exchange_id)
                .len(),
            2
        );
    }

    #[test]
    fn default_catalog_has_no_entry_cap_and_configurable_limits_evict_oldest() {
        let catalog = SessionCatalog::new(SessionLimits::default());
        for id in 1..=10_005 {
            start(&catalog, id, "many.test");
            fail(&catalog, id, 2);
        }
        assert_eq!(
            catalog
                .project_retained(|session| session.exchange_id)
                .len(),
            10_005
        );
        catalog.set_live_entry_limit(NonZeroUsize::new(2));
        assert_eq!(
            catalog.project_retained(|session| session.exchange_id),
            vec![ExchangeId(10_004), ExchangeId(10_005)]
        );
        catalog.set_live_entry_limit(None);
        start(&catalog, 10_006, "unlimited.test");
        fail(&catalog, 10_006, 2);
        assert_eq!(
            catalog
                .project_retained(|session| session.exchange_id)
                .len(),
            3
        );
    }

    #[test]
    fn websocket_remains_active_after_handshake_until_relay_closes() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(limits(1));
        start(&catalog, 1, "socket.test");
        let state = |status| {
            ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::WebSocket,
                status,
                None,
            ))
        };
        catalog.apply(event(1, 2, state(StreamStatus::Connecting)));
        catalog.apply(event(
            1,
            3,
            ObserverEventKind::Completed(CompletedExchange {
                metadata: metadata(1, "socket.test"),
                request_head: head("GET"),
                response_head: ResponseHead {
                    status: 101,
                    headers: HeaderBlock::default(),
                    source_version: HttpLegVersion::Http1,
                },
            }),
        ));
        assert_eq!(
            catalog.apply(event(1, 4, state(StreamStatus::Open))),
            CatalogApply::Applied
        );
        assert_eq!(catalog.get(ExchangeId(1)).unwrap().state(), "active");
        start(&catalog, 2, "other.test");
        assert!(
            catalog.get(ExchangeId(1)).is_some(),
            "open socket was evicted as completed HTTP"
        );
        assert_eq!(
            catalog.apply(event(1, 5, state(StreamStatus::Closed))),
            CatalogApply::Applied
        );
        assert!(
            catalog.get(ExchangeId(1)).is_none(),
            "closed socket should now be evictable"
        );
    }

    #[test]
    fn stream_privacy_preserves_lifetime_counts_without_retaining_content() {
        use transmog_core::stream::StreamMessage;
        let catalog = SessionCatalog::new(SessionLimits::default());
        start(&catalog, 1, "stream.test");
        let mut message = StreamMessage::new("server", "event", Bytes::from_static(b"private"));
        message.raw = Some("data: private\n\n".into());
        message.event_id = Some("private-id".into());
        catalog.apply(event(1, 2, ObserverEventKind::StreamMessage(message)));
        let snapshot = catalog.get(ExchangeId(1)).unwrap();
        let retained = &snapshot.stream_history.items[0].message;
        assert!(
            retained.payload.is_empty() && retained.raw.is_none() && retained.event_id.is_none()
        );
        assert!(retained.truncated);
        assert_eq!(snapshot.stream_history.server_bytes, 7);
        catalog.set_stream_content_retention(true, false);
        catalog.apply(event(
            1,
            3,
            ObserverEventKind::StreamMessage(StreamMessage::new(
                "client",
                "text",
                Bytes::from_static(b"retained"),
            )),
        ));
        assert_eq!(
            catalog.get(ExchangeId(1)).unwrap().stream_history.items[1]
                .message
                .payload,
            b"retained"[..]
        );
    }

    #[test]
    fn large_stream_histories_remain_complete_and_active() {
        use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(SessionLimits::default());
        catalog.set_stream_content_retention(true, true);
        let payload = Bytes::from(vec![b'x'; 512 * 1024]);
        for id in 1..=10 {
            start(&catalog, id, "stream.test");
            catalog.apply(event(
                id,
                2,
                ObserverEventKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Open,
                    None,
                )),
            ));
            for sequence in 3..=1102 {
                catalog.apply(event(
                    id,
                    sequence,
                    ObserverEventKind::StreamMessage(StreamMessage::new(
                        "server",
                        "binary",
                        payload.clone(),
                    )),
                ));
            }
        }
        let snapshots = catalog.project_retained(|s| (s.state(), s.stream_history.clone()));
        assert!(snapshots.iter().all(|(state, h)| *state == "active"
            && h.observed == 1100
            && h.items.len() == 1100
            && h.items[0].message.payload == payload
            && !h.items[0].message.truncated));
        assert!(
            snapshots
                .iter()
                .map(|(_, h)| h.retained_bytes)
                .sum::<usize>()
                > 64 * 1024 * 1024
        );
        assert!(snapshots.iter().all(|(_, h)| h.evicted == 0));
        assert!(
            snapshots
                .iter()
                .all(|(_, h)| h.observed == h.items.len() as u64 + h.evicted)
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
            CatalogApply::Applied
        );
        assert_eq!(catalog.counters().admission_rejected, 0);
        fail(&catalog, 1, 2);
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
            max_sessions: NonZeroUsize::new(4),
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
                    sensitive_headers: false,
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
        let credential = headers
            .iter()
            .find(|field| field.name() == b"authorization")
            .unwrap();
        assert!(credential.is_redacted());
        assert_eq!(credential.value_bytes(), b"Bearer secret".len());
        assert_eq!(credential.value(), b"");
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
            max_sessions: NonZeroUsize::new(EXCHANGES),
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
    #[test]
    fn atomic_metadata_snapshot_freezes_lifecycle_and_omits_content_collections() {
        use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
        let catalog = SessionCatalog::new(limits(3));
        catalog.set_stream_content_retention(true, true);
        for id in 1..=2 {
            start(&catalog, id, "snapshot.test");
            catalog.apply(event(
                id,
                2,
                ObserverEventKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Open,
                    None,
                )),
            ));
            catalog.apply(event(
                id,
                3,
                ObserverEventKind::StreamMessage(StreamMessage::new(
                    "server",
                    "text",
                    Bytes::from_static(b"private message"),
                )),
            ));
            catalog.apply(event(
                id,
                4,
                ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 3,
                    sample: Some(Bytes::from_static(b"abc")),
                    truncated: false,
                }),
            ));
        }
        let snapshot = catalog.retained_metadata();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(
            snapshot[0].stream_history.observed_until,
            snapshot[1].stream_history.observed_until
        );
        assert!(snapshot.iter().all(|row| {
            row.stream_history.items.is_empty()
                && row.stream_history.observed == 1
                && row.stream_history.server_bytes == 15
                && row
                    .bodies
                    .iter()
                    .all(|body| body.retained_prefix.is_empty())
        }));
        catalog.apply(event(
            1,
            5,
            ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::WebSocket,
                StreamStatus::Closed,
                None,
            )),
        ));
        catalog.dismiss(&[ExchangeId(2)], &[ExchangeId(2)]);
        assert!(snapshot.iter().all(|row| row.state() == "active"));
        assert_eq!(catalog.retained_metadata().len(), 1);
        assert_eq!(
            catalog
                .get(ExchangeId(1))
                .unwrap()
                .stream_history
                .items
                .len(),
            1
        );
        assert!(
            !catalog.get(ExchangeId(1)).unwrap().bodies[0]
                .retained_prefix
                .is_empty()
        );
    }
    #[test]
    fn http_terminal_paths_freeze_established_stream_lifetimes() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        for (protocol, failure_kind) in [
            (StreamProtocol::Sse, None),
            (StreamProtocol::Sse, Some(ExchangeFailureKind::Upstream)),
            (StreamProtocol::Sse, Some(ExchangeFailureKind::Cancelled)),
            (StreamProtocol::Sse, Some(ExchangeFailureKind::Shutdown)),
            (
                StreamProtocol::WebSocket,
                Some(ExchangeFailureKind::Cancelled),
            ),
            (
                StreamProtocol::WebSocket,
                Some(ExchangeFailureKind::Shutdown),
            ),
        ] {
            let catalog = SessionCatalog::new(limits(3));
            start(&catalog, 1, "terminal.test");
            catalog.apply(event(
                1,
                2,
                ObserverEventKind::StreamState(StreamState::now(
                    protocol,
                    StreamStatus::Open,
                    None,
                )),
            ));
            let terminal = failure_kind.map_or_else(
                || {
                    ObserverEventKind::Completed(CompletedExchange {
                        metadata: metadata(1, "terminal.test"),
                        request_head: head("GET"),
                        response_head: ResponseHead {
                            status: 200,
                            headers: HeaderBlock::default(),
                            source_version: HttpLegVersion::Http2,
                        },
                    })
                },
                |kind| {
                    ObserverEventKind::Failed(ExchangeFailure {
                        metadata: metadata(1, "terminal.test"),
                        stage: ExchangeStage::Terminal,
                        kind,
                        request_committed: true,
                        response_committed: true,
                        message: "terminal fixture".into(),
                    })
                },
            );
            catalog.apply(event(1, 3, terminal));
            let snapshot = catalog.get(ExchangeId(1)).unwrap();
            let stream = snapshot.stream.as_ref().unwrap();
            assert!(!stream.status.is_active());
            assert_eq!(snapshot.stream_history.ended_at, Some(stream.at_millis));
            let elapsed = snapshot.stream_history.elapsed_millis;
            assert!(elapsed.is_some());
            let state = catalog.lock_state();
            let history = &state.by_id[&ExchangeId(1)].stream_history;
            let later = std::time::Instant::now()
                .checked_add(std::time::Duration::from_mins(1))
                .unwrap();
            assert_eq!(
                history
                    .metadata_snapshot(true, later, u64::MAX)
                    .elapsed_millis,
                elapsed
            );
            drop(state);
            assert_eq!(
                catalog
                    .get(ExchangeId(1))
                    .unwrap()
                    .stream_history
                    .elapsed_millis,
                elapsed
            );
        }
    }
}
