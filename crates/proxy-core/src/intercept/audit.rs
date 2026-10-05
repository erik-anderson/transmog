use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use crate::{
    CanonicalResponse, HeaderBlock, LocalStreamingResponse, RequestHead, ResponseHead, Target,
};

use super::{BodyPlan, BodyRepresentation, HookAbort};

/// Stable identity assigned to one interceptor registration.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InterceptorId(Arc<str>);

impl InterceptorId {
    /// Creates an identity from a caller-owned stable name.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    /// Returns the stable identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One interceptor's immutable identity within a configured chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterceptorIdentity {
    /// Stable caller-defined identifier.
    pub id: InterceptorId,
    /// Operator-facing display name.
    pub name: Arc<str>,
    /// Zero-based position in request order.
    pub chain_position: usize,
}

/// Hook phase that produced an auditable traffic effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookPhase {
    /// Request-head processing.
    RequestHead,
    /// Request-body plan selection.
    RequestBody,
    /// Response-head processing.
    ResponseHead,
    /// Response-body plan selection.
    ResponseBody,
}

/// Redacted structural header difference.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeaderChanges {
    /// Header names introduced by the hook.
    pub added: Vec<String>,
    /// Header names removed by the hook.
    pub removed: Vec<String>,
    /// Header names whose ordered values changed.
    pub changed: Vec<String>,
}

impl HeaderChanges {
    /// Whether the two header blocks were structurally identical.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Redacted request-head difference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestHeadChanges {
    /// Whether the method changed.
    pub method_changed: bool,
    /// Whether any normalized target component changed.
    pub target_changed: bool,
    /// Header-name-level changes without credential values.
    pub headers: HeaderChanges,
}

impl RequestHeadChanges {
    /// Whether the replacement was byte-for-byte equivalent at the canonical level.
    pub fn is_empty(&self) -> bool {
        !self.method_changed && !self.target_changed && self.headers.is_empty()
    }
}

/// Redacted response-head difference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseHeadChanges {
    /// Whether the status changed.
    pub status_changed: bool,
    /// Header-name-level changes without credential values.
    pub headers: HeaderChanges,
}

impl ResponseHeadChanges {
    /// Whether the replacement was byte-for-byte equivalent at the canonical level.
    pub fn is_empty(&self) -> bool {
        !self.status_changed && self.headers.is_empty()
    }
}

/// Kind of body plan selected by an interceptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyPlanKind {
    /// Stateful streaming transformation.
    Transform,
    /// Explicitly bounded whole-body handler.
    Buffer,
    /// Bounded replacement body.
    Replace,
    /// Body removal.
    Discard,
}

/// Traffic-affecting action attributed to one interceptor callback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookEffectAction {
    /// A request head was replaced.
    ReplaceRequestHead(RequestHeadChanges),
    /// A request was explicitly rerouted.
    Reroute {
        /// Redacted changes to the logical request.
        changes: RequestHeadChanges,
        /// Authorized logical target requested by the hook.
        target: Target,
    },
    /// A response head was replaced.
    ReplaceResponseHead(ResponseHeadChanges),
    /// The hook supplied a local response.
    Respond {
        /// Response status.
        status: u16,
        /// Number of bounded response frames supplied by the hook.
        body_frames: usize,
        /// Total response data bytes supplied by the hook.
        body_bytes: usize,
    },
    /// The hook aborted the exchange.
    Abort(HookAbort),
    /// A non-pass-through body plan was selected.
    BodyPlan {
        /// Plan kind.
        kind: BodyPlanKind,
        /// Byte representation requested by the plan.
        representation: BodyRepresentation,
        /// Complete-body bound for buffering plans.
        buffer_limit: Option<usize>,
        /// Data bytes in an immediate replacement body.
        replacement_bytes: Option<usize>,
    },
}

/// One immutable, ordered hook-effect audit record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookEffect {
    /// Monotonic sequence within this exchange's hook-effect trail.
    pub sequence: u64,
    /// Interceptor that produced the action.
    pub interceptor: InterceptorIdentity,
    /// Callback phase.
    pub phase: HookPhase,
    /// Applied action and its redacted structural summary.
    pub action: HookEffectAction,
    /// Whether the returned replacement changed canonical traffic state.
    pub changed: bool,
}

#[derive(Debug, Default)]
struct HookAuditState {
    records: Vec<HookEffect>,
    published: usize,
}

/// Exchange-local authoritative hook-effect trail.
///
/// Its maximum record count is finite: each configured interceptor can produce
/// at most one record in each of the four hook phases.
#[derive(Clone, Debug, Default)]
pub struct HookAuditTrail {
    state: Arc<Mutex<HookAuditState>>,
}

impl HookAuditTrail {
    pub(crate) fn record(
        &self,
        interceptor: InterceptorIdentity,
        phase: HookPhase,
        action: HookEffectAction,
        changed: bool,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sequence = u64::try_from(state.records.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        state.records.push(HookEffect {
            sequence,
            interceptor,
            phase,
            action,
            changed,
        });
    }

    /// Returns a stable snapshot of every effect recorded so far.
    pub fn snapshot(&self) -> Vec<HookEffect> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .records
            .clone()
    }

    /// Marks and returns records not previously published to lifecycle observers.
    pub fn take_unpublished(&self) -> Vec<HookEffect> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let records = state.records[state.published..].to_vec();
        state.published = state.records.len();
        records
    }
}

pub(crate) fn request_changes(before: &RequestHead, after: &RequestHead) -> RequestHeadChanges {
    RequestHeadChanges {
        method_changed: before.method != after.method,
        target_changed: before.target != after.target,
        headers: header_changes(&before.headers, &after.headers),
    }
}

pub(crate) fn response_changes(before: &ResponseHead, after: &ResponseHead) -> ResponseHeadChanges {
    ResponseHeadChanges {
        status_changed: before.status != after.status,
        headers: header_changes(&before.headers, &after.headers),
    }
}

pub(crate) fn response_summary(response: &CanonicalResponse) -> HookEffectAction {
    let body_bytes = response
        .body
        .iter()
        .filter_map(|frame| match frame {
            crate::BodyFrame::Data(bytes) => Some(bytes.len()),
            crate::BodyFrame::Trailers(_) => None,
        })
        .fold(0usize, usize::saturating_add);
    HookEffectAction::Respond {
        status: response.head.status,
        body_frames: response.body.len(),
        body_bytes,
    }
}

pub(crate) fn streaming_response_summary(response: &LocalStreamingResponse) -> HookEffectAction {
    HookEffectAction::Respond {
        status: response.head.status,
        body_frames: 0,
        body_bytes: response
            .body_length
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0),
    }
}

pub(crate) fn body_plan_effect(
    plan: &BodyPlan,
    representation: BodyRepresentation,
) -> Option<HookEffectAction> {
    let (kind, buffer_limit, replacement_bytes) = match plan {
        BodyPlan::PassThrough => return None,
        BodyPlan::Transform(_) => (BodyPlanKind::Transform, None, None),
        BodyPlan::Buffer { limit, .. } => (BodyPlanKind::Buffer, Some(limit.get()), None),
        BodyPlan::Replace(body) => (BodyPlanKind::Replace, None, Some(body.data().len())),
        BodyPlan::Discard => (BodyPlanKind::Discard, None, None),
    };
    Some(HookEffectAction::BodyPlan {
        kind,
        representation,
        buffer_limit,
        replacement_bytes,
    })
}

fn header_changes(before: &HeaderBlock, after: &HeaderBlock) -> HeaderChanges {
    let before = grouped_headers(before);
    let after = grouped_headers(after);
    let names = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = HeaderChanges::default();
    for name in names {
        match (before.get(&name), after.get(&name)) {
            (None, Some(_)) => changes.added.push(name),
            (Some(_), None) => changes.removed.push(name),
            (Some(before), Some(after)) if before != after => changes.changed.push(name),
            _ => {}
        }
    }
    changes
}

fn grouped_headers(headers: &HeaderBlock) -> BTreeMap<String, Vec<Vec<u8>>> {
    let mut grouped = BTreeMap::<String, Vec<Vec<u8>>>::new();
    for field in headers.iter() {
        let name = String::from_utf8_lossy(field.name()).to_ascii_lowercase();
        grouped
            .entry(name)
            .or_default()
            .push(field.value().to_vec());
    }
    grouped
}
