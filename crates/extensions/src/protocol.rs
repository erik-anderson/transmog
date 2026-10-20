//! Private same-build worker IPC, independently versioned from package/API contracts.
use crate::{Direction, Error, Header, Manifest, VisualizationDocument, invalid};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Same-build worker protocol revision.
pub const HOST_PROTOCOL_VERSION: u32 = 4;
/// Absolute framing ceiling, checked before allocating any incoming JSON.
pub const MAX_HOST_FRAME_BYTES: usize = 12 * 1024 * 1024;
/// Maximum raw payload bytes in one initialization chunk.
pub const PAYLOAD_CHUNK_BYTES: usize = 128 * 1024;

/// Unpredictable per-launch IPC token, never supplied to extension JavaScript.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct HostToken(pub [u8; 32]);

/// Host-validated body source capability. Its ID is not a path or a cross-instance grant.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyReference {
    /// Instance-scoped source identity.
    pub source_id: String,
    /// Canonical exact retained length.
    pub length: String,
    /// Complete, capturing, truncated, evicted or another explicit retention state.
    pub availability: String,
}
/// Host-owned HTTP decoded source, with enclosing provenance in the encoded body.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecodedBody {
    /// Separately granted decoded bytes, or the original capability for identity.
    pub body: BodyReference,
    /// Entire frozen encoded entity; compressed offsets are never presented as exact.
    pub parent: crate::Span,
    /// Canonical content codings in sender application order; empty means identity.
    pub codings: Vec<String>,
}
/// Finite read-only decode outcomes; no captured bytes or header values in errors.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DecodeFailure {
    /// Body retention is incomplete or still in progress.
    Incomplete,
    /// Exact content-coding metadata is missing, inconsistent or redacted.
    MetadataUnavailable,
    /// The declared coding is unsupported.
    Unsupported,
    /// Encoded bytes failed codec validation.
    Malformed,
    /// The codec or retained decoded-source budget was exhausted.
    Limited,
}
/// Result resumed into extension code after an explicitly requested HTTP decode.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HttpDecodeResult {
    /// A validated, separately identified representation, with exact lengths.
    Ready {
        /// Decoded source and its encoded parent.
        decoded: DecodedBody,
    },
    /// Keep the original encoded representation available and report the reason.
    Unavailable {
        /// Stable outcome, distinct from unavailable original bytes.
        reason: DecodeFailure,
    },
}
/// Projected message input; includes only this contribution's granted headers/body handle.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageInput {
    /// Opaque identity for this exact message boundary and evidence revision.
    pub evidence_id: String,
    /// Ordered, exactly projected header occurrences.
    pub headers: Vec<Header>,
    /// Body capability, absent for body access `none`.
    pub body: Option<BodyReference>,
    /// Request method when observed.
    pub method: Option<String>,
    /// Endpoint path without its query.
    pub path: Option<String>,
    /// Observed response status when available.
    pub status: Option<u16>,
}
/// One generation-bound invocation input.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizationInput {
    /// Exact active UI instance.
    pub instance_id: String,
    /// Opaque stable exchange identity across revisions, context and live updates.
    pub exchange_id: String,
    /// Exact immutable workspace admission order, shared with context-page ordering.
    pub request_number: String,
    /// Canonical monotonically changed generation.
    pub generation: String,
    /// Selected display side.
    pub direction: Direction,
    /// Request evidence, if available.
    pub request: Option<MessageInput>,
    /// Response evidence, if available.
    pub response: Option<MessageInput>,
}
/// Bounded source read requested as a continuation, with no arbitrary host callback.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadRequest {
    /// Granted source identity.
    pub source_id: String,
    /// Exact source offset.
    pub offset: String,
    /// At most 64 KiB in one requested page.
    pub max_bytes: usize,
}
/// A verified source page returned by the broker.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BytePage {
    /// The requested source.
    pub source_id: String,
    /// Exact returned offset.
    pub offset: String,
    /// Bounded page bytes, never an entire unbounded body.
    pub bytes: Vec<u8>,
}
/// Position within the host-verified conversation, never an arbitrary exchange selector.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ContextPosition {
    /// Earlier retained exchanges.
    Before,
    /// Later retained exchanges, only when explicitly declared.
    After,
}
/// Bounded conversation read requested by an active parser.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextRequest {
    /// Earlier or declared later evidence.
    pub position: ContextPosition,
    /// Opaque continuation issued to this active instance; absent for the first page.
    pub cursor: Option<String>,
    /// At most 32 exchanges and no more than the manifest's total context limit.
    pub limit: usize,
}
impl ContextRequest {
    /// Checks the maximum declaration before the host looks up any related traffic.
    /// # Errors
    /// Rejects undeclared lookahead, invalid cursors and excessive page sizes.
    pub fn validate(&self, context: &crate::Context) -> Result<(), crate::Error> {
        if self.limit == 0
            || self.limit > 32
            || self.limit > context.max_exchanges
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| !crate::valid_id(cursor))
        {
            return Err(crate::Error::Limit);
        }
        if self.position == ContextPosition::After && !context.look_ahead {
            return Err(crate::Error::AccessDenied);
        }
        Ok(())
    }
}
/// One projected related exchange with a lossless host ordering key.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextExchange {
    /// Stable workspace admission order within this capture.
    pub order: String,
    /// The same exact per-side header/body grant as the selected input.
    pub input: VisualizationInput,
}
/// Availability of a bounded historical context page.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ContextStatus {
    /// The retained range was exhausted normally.
    Complete,
    /// More retained evidence can be requested through the returned cursor.
    More,
    /// Conversation identity or its retained anchor is unavailable.
    Unavailable,
    /// A declared or host budget was reached.
    Limited,
}
/// Host-owned result of a conversation read; it never grants networking or arbitrary lookup.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextPage {
    /// Ordering key of the selected exchange.
    pub anchor_order: String,
    /// Related exchanges in ascending order within this page.
    pub exchanges: Vec<ContextExchange>,
    /// A cursor for the same direction in this instance, if more evidence remains.
    pub next_cursor: Option<String>,
    /// Availability independent of the parser's own status.
    pub status: ContextStatus,
}
/// Metadata for one admitted native live update.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TrafficPhase {
    /// Newly observed request or response headers.
    Head,
    /// Coalesced body growth, only when explicitly requested.
    Body,
    /// Body completion, only for a declared body input.
    BodyCompleted,
    /// HTTP completion or failure; no body bytes are implied.
    Ended,
}
/// Metadata for one admitted native live update.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficEvent {
    /// Canonical subscription delivery sequence.
    pub sequence: String,
    /// Opaque retained exchange identity.
    pub exchange_id: String,
    /// Changed side.
    pub direction: Direction,
    /// Native lifecycle classification, independent of retained byte availability.
    pub phase: TrafficPhase,
    /// Exact evidence revision.
    pub revision: String,
    /// Projected scoped input for this update.
    pub input: VisualizationInput,
}
/// One synchronous bounded parser operation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Invocation {
    /// Parse a selected message snapshot.
    Visualize {
        /// Projected immutable input.
        input: Box<VisualizationInput>,
    },
    /// Resume a parser with requested retained byte pages.
    Resume {
        /// Worker-local continuation token.
        token: String,
        /// Bounded broker pages.
        pages: Vec<BytePage>,
    },
    /// Resume only the pending, declared conversation read.
    ResumeContext {
        /// Exact pending parser token.
        token: String,
        /// Projected host page within the selected capture/conversation.
        page: ContextPage,
    },
    /// Resume only an explicitly pending, declared HTTP decode.
    ResumeDecoded {
        /// Exact parser continuation token.
        token: String,
        /// Host-controlled representation or explicit failure.
        result: HttpDecodeResult,
    },
    /// Expand a deferred document node.
    Expand {
        /// Worker-local expansion token.
        token: String,
    },
    /// Feed one admitted opt-in live batch.
    Traffic {
        /// Ordered updates.
        events: Vec<TrafficEvent>,
        /// Canonical count of lost/coalesced-away capture hints whose matching set is uncertain.
        missed_hints: String,
    },
}
/// Pure parser output or a finite request for declared retained bytes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum VisualizationStep {
    /// Complete or explicitly partial host-owned view.
    Document {
        /// Validated document.
        document: VisualizationDocument,
    },
    /// Request bounded data without granting ambient I/O.
    NeedInput {
        /// Exact authorized read requests.
        reads: Vec<ReadRequest>,
        /// Instance-local continuation identity.
        token: String,
    },
    /// Request related retained traffic through the host's native scope checks.
    NeedContext {
        /// Bounded direction and instance-owned cursor.
        request: ContextRequest,
        /// Exact parser continuation token.
        token: String,
    },
    /// Decode a granted complete body under its declared content-encoding dependency.
    NeedDecoded {
        /// Exact original body capability; never a header or an arbitrary source.
        source_id: String,
        /// Exact parser continuation token.
        token: String,
    },
}
/// Redaction-safe failure classification.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// Source/import/manifest validation failed.
    Validation,
    /// Parser or module initialization threw.
    Exception,
    /// Process containment/launch unavailable.
    Unavailable,
    /// Invocation deadline exceeded.
    Timeout,
    /// Resource quota exhausted.
    Limit,
    /// Authentication/framing/ordering failed.
    Protocol,
}
/// Failure text is host-controlled and never includes captured input or source snippets.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    /// Stable category.
    pub kind: FailureKind,
    /// Plain static explanation.
    pub message: String,
}
impl Failure {
    /// Constructs a bounded host-controlled diagnostic.
    pub fn new(kind: FailureKind, message: &str) -> Self {
        Self {
            kind,
            message: message.chars().take(256).collect(),
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Failure {}

/// Authenticated request envelope; initialization code uploads are chunked.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HostRequest {
    /// Establish immutable declarations before receiving source bytes.
    Initialize {
        /// Independent IPC revision.
        protocol_version: u32,
        /// Per-process token.
        token: HostToken,
        /// Validated exact metadata.
        manifest: Manifest,
        /// Selected contribution, not every package entry.
        visualizer_id: String,
    },
    /// Upload one bounded payload chunk.
    Payload {
        /// Per-process token.
        token: HostToken,
        /// Declared package member.
        path: String,
        /// Exact next offset.
        offset: String,
        /// Bounded bytes in base64.
        bytes_base64: String,
    },
    /// Verify complete source inventory and initialize the isolated parser.
    Start {
        /// Per-process token.
        token: HostToken,
    },
    /// Invoke a declared operation.
    Invoke {
        /// Per-process token.
        token: HostToken,
        /// Monotonic request identity.
        request_id: u64,
        /// Bounded pure operation.
        invocation: Invocation,
    },
}
/// Authenticated response envelope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HostReply {
    /// Source verification and initialization succeeded.
    Ready {
        /// Per-process token.
        token: HostToken,
    },
    /// One exact matching operation result.
    Result {
        /// Per-process token.
        token: HostToken,
        /// Matching request identity.
        request_id: u64,
        /// Parser result or safe error.
        result: Result<VisualizationStep, Failure>,
    },
    /// Initialization failed before a reusable runtime exists.
    Fatal {
        /// Token received during initialization, if available.
        token: Option<HostToken>,
        /// Bounded safe failure.
        failure: Failure,
    },
}

/// Reads a length-prefixed frame, rejecting oversized lengths before allocation.
/// # Errors
/// Returns framing, transport or JSON failures.
pub fn read_frame<T: serde::de::DeserializeOwned>(input: &mut impl Read) -> Result<T, Error> {
    let mut prefix = [0_u8; 4];
    input.read_exact(&mut prefix)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_HOST_FRAME_BYTES {
        return Err(invalid("Invalid extension IPC frame length"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
/// Writes one bounded authenticated JSON frame.
/// # Errors
/// Rejects oversized output before writing its prefix.
pub fn write_frame<T: Serialize>(output: &mut impl Write, value: &T) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.is_empty() || bytes.len() > MAX_HOST_FRAME_BYTES {
        return Err(Error::Limit);
    }
    output.write_all(
        &u32::try_from(bytes.len())
            .map_err(|_| Error::Limit)?
            .to_le_bytes(),
    )?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
