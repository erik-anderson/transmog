//! Private same-build worker IPC, independently versioned from package/API contracts.
use crate::{Direction, Error, Header, Manifest, VisualizationDocument, invalid};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Same-build worker protocol revision.
pub const HOST_PROTOCOL_VERSION: u32 = 1;
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
    /// Exact evidence revision.
    pub revision: String,
    /// Projected scoped input for this update.
    pub input: VisualizationInput,
}
/// One synchronous bounded parser operation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
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
    /// Expand a deferred document node.
    Expand {
        /// Worker-local expansion token.
        token: String,
    },
    /// Feed one admitted opt-in live batch.
    Traffic {
        /// Ordered updates.
        events: Vec<TrafficEvent>,
    },
}
/// Pure parser output or a finite request for declared retained bytes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
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
