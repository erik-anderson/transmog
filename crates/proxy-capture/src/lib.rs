#![deny(missing_docs)]

//! Append-oriented, checksummed native capture storage.
//!
//! The durable schema is independent from live control messages. A valid
//! prefix remains readable after a crash-truncated final record.

mod circular;
pub use circular::CircularCapture;
mod encoding;
pub use encoding::{CaptureEncoding, CapturePassword, FrameCodec};

use std::{
    collections::{BTreeSet, HashMap},
    io::{self, Read, Seek, SeekFrom, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use transmog_core::{
    ClientIdentity, HeaderBlock,
    intercept::{HookEffectAction, HookPhase},
    observe::{ExchangeBoundary, ObserverEvent, ObserverEventKind},
};

const MAGIC: [u8; 9] = encoding::MAGIC;
const FRAME_HEADER_BYTES: usize = 8;

/// Default retained request-body limit (25 decimal MB); file budgets still apply.
pub const DEFAULT_REQUEST_BODY_CAPTURE_BYTES: u64 = 25_000_000;

/// Durable record schema revision, independent of the container header version.
pub const CAPTURE_FORMAT_REVISION: u32 = 3;

/// Writer/recovery limits; artifact size and record count default to no maximum.
#[derive(Clone, Copy, Debug)]
pub struct CaptureLimits {
    /// Maximum complete artifact size including framing.
    pub max_file_bytes: u64,
    /// Maximum serialized record payload.
    pub max_record_bytes: usize,
    /// Maximum number of records accepted during recovery.
    pub max_records: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: u64::MAX,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: usize::MAX,
        }
    }
}

impl CaptureLimits {
    /// Validates all limits before I/O begins.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidLimits`] when any limit cannot contain a
    /// header and one nonempty record.
    pub fn validate(self) -> Result<Self, CaptureError> {
        if self.max_file_bytes <= MAGIC.len() as u64 + FRAME_HEADER_BYTES as u64
            || self.max_record_bytes == 0
            || self.max_record_bytes > u32::MAX as usize
            || self.max_records == 0
        {
            return Err(CaptureError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Capture-specific redaction and retention policy.
#[derive(Clone, Debug)]
pub struct CapturePolicy {
    request_header_names: BTreeSet<String>,
    response_header_names: BTreeSet<String>,
    /// Whether observer body samples are retained.
    pub retain_body_samples: bool,
    /// Maximum retained bytes per request boundary; None removes that cap.
    pub request_body_limit: Option<u64>,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            request_header_names: ["authorization", "proxy-authorization", "cookie"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            response_header_names: ["set-cookie"].into_iter().map(str::to_owned).collect(),
            retain_body_samples: false,
            request_body_limit: Some(DEFAULT_REQUEST_BODY_CAPTURE_BYTES),
        }
    }
}

impl CapturePolicy {
    /// Retains sensitive fields for an explicitly configured local capture.
    #[must_use]
    pub fn retain_sensitive_headers(mut self) -> Self {
        self.request_header_names.clear();
        self.response_header_names.clear();
        self
    }
    /// Adds a case-insensitive request header name to redact.
    pub fn redact_request_header(&mut self, name: impl Into<String>) {
        self.request_header_names
            .insert(name.into().to_ascii_lowercase());
    }

    /// Adds a case-insensitive response header name to redact.
    pub fn redact_response_header(&mut self, name: impl Into<String>) {
        self.response_header_names
            .insert(name.into().to_ascii_lowercase());
    }
}

/// Streaming retention accounting shared by native capture consumers.
/// File size and record limits remain enforced by the writer in Unlimited mode.
#[derive(Debug, Default)]
pub struct CaptureBodyRetention {
    request_bytes: HashMap<(u128, bool), u64>,
}
impl CaptureBodyRetention {
    /// Releases accounting for exchanges evicted before their terminal event.
    pub fn retain_exchanges(&mut self, mut retained: impl FnMut(u128) -> bool) {
        self.request_bytes.retain(|(id, _), _| retained(*id));
    }

    /// Clips request samples to the configured prefix without altering observed
    /// byte counts. Terminal records release per-exchange accounting.
    pub fn apply(&mut self, record: &mut CaptureRecord, policy: &CapturePolicy) {
        match &mut record.kind {
            CaptureRecordKind::BodySegment {
                boundary,
                byte_count,
                bytes,
                truncated,
            } if matches!(
                boundary,
                ExchangeBoundary::ClientRequest | ExchangeBoundary::UpstreamRequest
            ) =>
            {
                if let (Some(limit), Some(sample)) = (policy.request_body_limit, bytes) {
                    let used = self
                        .request_bytes
                        .entry((
                            record.exchange_id,
                            *boundary == ExchangeBoundary::UpstreamRequest,
                        ))
                        .or_default();
                    let remaining = limit.saturating_sub(*used);
                    let take = usize::try_from(remaining)
                        .unwrap_or(usize::MAX)
                        .min(sample.len());
                    sample.truncate(take);
                    *used = used.saturating_add(take as u64);
                    *truncated |= take < *byte_count;
                }
            }
            CaptureRecordKind::Completed | CaptureRecordKind::Failed { .. } => {
                self.request_bytes.remove(&(record.exchange_id, false));
                self.request_bytes.remove(&(record.exchange_id, true));
            }
            _ => {}
        }
    }
}

/// Durable duplicate-preserving header field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapturedHeader {
    /// Header name.
    pub name: Vec<u8>,
    /// Header value, or `None` when policy redacted it.
    pub value: Option<Vec<u8>>,
    /// Original value size; unavailable when the source did not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_value_bytes: Option<usize>,
}

/// Durable record payload. New readers preserve unknown kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureRecordKind {
    /// Bounded measured milestone, protocol and transport evidence.
    Performance(transmog_core::performance::PerformanceEvidence),
    /// Exchange lifecycle started.
    ExchangeStarted {
        /// Client peer address.
        client_addr: String,
        /// Best-effort caller identity captured at connection accept time.
        client_identity: ClientIdentity,
        /// Listener address.
        listener_addr: String,
        /// Original target authority.
        authority: String,
        /// Start timestamp in Unix nanoseconds.
        started_unix_nanos: u128,
    },
    /// Request head at a logical boundary.
    RequestHead {
        /// Logical boundary.
        boundary: ExchangeBoundary,
        /// HTTP method.
        method: String,
        /// Normalized URI components.
        target: String,
        /// Redacted ordered fields.
        headers: Vec<CapturedHeader>,
    },
    /// Response head at a logical boundary.
    ResponseHead {
        /// Logical boundary.
        boundary: ExchangeBoundary,
        /// Numeric status.
        status: u16,
        /// Redacted ordered fields.
        headers: Vec<CapturedHeader>,
    },
    /// One observed body segment.
    BodySegment {
        /// Logical boundary.
        boundary: ExchangeBoundary,
        /// Original segment length.
        byte_count: usize,
        /// Retained bytes under policy.
        bytes: Option<Vec<u8>>,
        /// Whether observed data was incomplete.
        truncated: bool,
    },
    /// Terminal trailers.
    Trailers {
        /// Logical boundary.
        boundary: ExchangeBoundary,
        /// Redacted ordered fields.
        headers: Vec<CapturedHeader>,
    },
    /// Centrally attributed hook effect.
    HookEffect {
        /// Stable hook ID.
        hook_id: String,
        /// Display name at capture time.
        hook_name: String,
        /// Hook phase.
        phase: String,
        /// Stable action category.
        action: String,
        /// Whether canonical state changed.
        changed: bool,
    },
    /// Selected route policy.
    RouteSelected {
        /// Stable policy ID.
        policy_id: String,
        /// Redacted reason.
        reason: String,
    },
    /// Concrete upstream attempt.
    RouteAttempt {
        /// Protocol name.
        protocol: String,
        /// Stable outcome category.
        outcome: String,
    },
    /// Explicit missing-event marker.
    Loss {
        /// First missing source sequence.
        first_missing_sequence: u64,
        /// Number of missing source events.
        count: u64,
        /// Stable loss reason.
        reason: String,
    },
    /// Successful terminal state.
    Completed,
    /// Failed terminal state.
    Failed {
        /// Stable failure category.
        category: String,
        /// Redacted operator message.
        message: String,
    },
    /// Optional hook was skipped during initialization.
    HookInitializationSkipped {
        /// Hook display name.
        name: String,
        /// Redacted reason.
        message: String,
    },
    /// Final marker for a cleanly closed artifact.
    Seal {
        /// Number of preceding records.
        record_count: u64,
    },
    /// Forward-compatible record unknown to this build.
    Unknown {
        /// Durable kind name.
        kind: String,
        /// Preserved JSON payload.
        payload: Value,
    },
}

/// One durable source-sequenced record.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureRecord {
    /// Source observer sequence; zero is reserved for artifact-level records.
    pub sequence: u64,
    /// Exchange ID, or zero for artifact-level records.
    pub exchange_id: u128,
    /// Durable payload.
    pub kind: CaptureRecordKind,
}

#[derive(Deserialize, Serialize)]
struct StoredRecord {
    revision: u32,
    sequence: u64,
    exchange_id: u128,
    kind: String,
    payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body_index: Option<StoredBodyIndex>,
}

impl StoredRecord {
    fn load_metadata(self) -> Result<(CaptureRecord, Option<StoredBodyIndex>), CaptureError> {
        let index = self.body_index;
        let record = load(self)?;
        match record.kind {
            CaptureRecordKind::BodySegment { bytes: Some(_), .. } => {
                return Err(CaptureError::InvalidMagic);
            }
            CaptureRecordKind::BodySegment { bytes: None, .. } => {}
            _ if index.is_some() => return Err(CaptureError::InvalidMagic),
            _ => {}
        }
        Ok((record, index))
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct StoredBodyIndex {
    frame_bytes: u64,
    bytes: usize,
    digest: [u8; 32],
}

impl StoredBodyIndex {
    fn validate(self, maximum: usize) -> Result<(), CaptureError> {
        if self.bytes > maximum
            || self.frame_bytes < FRAME_HEADER_BYTES as u64
            || self.frame_bytes
                > FRAME_HEADER_BYTES as u64 + FrameCodec::encoded_bound(maximum) as u64
        {
            return Err(CaptureError::RecordTooLarge {
                actual: self.bytes,
                limit: maximum,
            });
        }
        Ok(())
    }
    fn decode(
        self,
        codec: &FrameCodec,
        encoded: &[u8],
        index: u64,
        maximum: usize,
        record_index: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        let length = u32::from_le_bytes(
            encoded[..4]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        ) as usize;
        let crc = u32::from_le_bytes(
            encoded[4..8]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        );
        if length != encoded.len() - FRAME_HEADER_BYTES || crc32fast::hash(&encoded[8..]) != crc {
            return Err(CaptureError::ChecksumMismatch { record_index });
        }
        let bytes = codec.decode_body(&encoded[8..], index, maximum)?;
        if bytes.len() != self.bytes || <[u8; 32]>::from(Sha256::digest(&bytes)) != self.digest {
            return Err(CaptureError::AuthenticationFailed);
        }
        Ok(bytes)
    }
}

#[derive(Deserialize, Serialize)]
struct StartedPayload {
    client_addr: String,
    client_identity: ClientIdentity,
    listener_addr: String,
    authority: String,
    started_unix_nanos: u128,
}

#[derive(Deserialize, Serialize)]
struct RequestHeadPayload {
    boundary: String,
    method: String,
    target: String,
    headers: Vec<CapturedHeader>,
}

#[derive(Deserialize, Serialize)]
struct ResponseHeadPayload {
    boundary: String,
    status: u16,
    headers: Vec<CapturedHeader>,
}

#[derive(Deserialize, Serialize)]
struct BodyPayload {
    boundary: String,
    byte_count: usize,
    bytes: Option<Vec<u8>>,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
struct TrailersPayload {
    boundary: String,
    headers: Vec<CapturedHeader>,
}

#[derive(Deserialize, Serialize)]
struct HookPayload {
    hook_id: String,
    hook_name: String,
    phase: String,
    action: String,
    changed: bool,
}

#[derive(Deserialize, Serialize)]
struct RoutePayload {
    policy_id: String,
    reason: String,
}

#[derive(Deserialize, Serialize)]
struct AttemptPayload {
    protocol: String,
    outcome: String,
}

#[derive(Deserialize, Serialize)]
struct LossPayload {
    first_missing_sequence: u64,
    count: u64,
    reason: String,
}

#[derive(Deserialize, Serialize)]
struct FailurePayload {
    category: String,
    message: String,
}

#[derive(Deserialize, Serialize)]
struct SkippedPayload {
    name: String,
    message: String,
}

#[derive(Deserialize, Serialize)]
struct SealPayload {
    record_count: u64,
}

/// Append-only writer over a file, socket, or other streaming destination.
pub struct CaptureWriter<W> {
    output: W,
    limits: CaptureLimits,
    bytes_written: u64,
    records_written: u64,
    sealed: bool,
    codec: FrameCodec,
}

impl<W: Write> CaptureWriter<W> {
    /// Writes a new native artifact preamble.
    ///
    /// # Errors
    ///
    /// Returns a typed limit or I/O error.
    pub fn new(output: W, limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::with_encoding(output, limits, &CaptureEncoding::default())
    }

    /// Creates a compressed, optionally encrypted capture with an agile header.
    ///
    /// # Errors
    /// Returns invalid bounds, cryptographic initialization or output failures.
    pub fn with_encoding(
        mut output: W,
        limits: CaptureLimits,
        options: &CaptureEncoding,
    ) -> Result<Self, CaptureError> {
        let limits = limits.validate()?;
        let (codec, bytes_written) = FrameCodec::create(&mut output, options)?;
        if bytes_written > limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        Ok(Self {
            output,
            limits,
            bytes_written,
            records_written: 0,
            sealed: false,
            codec,
        })
    }

    /// Appends and flushes one checksummed record.
    ///
    /// # Errors
    ///
    /// Returns a typed serialization, limit, sealed-state, or I/O error.
    pub fn append(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        if self.sealed {
            return Err(CaptureError::AlreadySealed);
        }
        if let CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes: Some(bytes),
            truncated,
        } = &record.kind
        {
            // Raw body frames remain bounded independently of metadata.
            // Each fragment keeps its observer sequence; file order determines
            // body order. Only the final fragment carries unretained counts.
            let part_bytes = self.limits.max_record_bytes.saturating_sub(1024);
            if bytes.len() > part_bytes && part_bytes > 0 {
                let mut counted = 0;
                for part in bytes.chunks(part_bytes) {
                    let last = counted + part.len() == bytes.len();
                    let fragment = CaptureRecord {
                        sequence: record.sequence,
                        exchange_id: record.exchange_id,
                        kind: CaptureRecordKind::BodySegment {
                            boundary: *boundary,
                            byte_count: if last {
                                byte_count.saturating_sub(counted)
                            } else {
                                part.len()
                            },
                            bytes: Some(part.to_vec()),
                            truncated: *truncated,
                        },
                    };
                    self.append_record(&fragment)?;
                    counted += part.len();
                }
                return Ok(());
            }
            if bytes.len() > self.limits.max_record_bytes {
                return Err(CaptureError::RecordTooLarge {
                    actual: bytes.len(),
                    limit: self.limits.max_record_bytes,
                });
            }
        }
        self.append_record(record)
    }

    fn append_record(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        // Reserve the final record for a seal so a healthy writer always creates
        // an artifact readable under the same recovery limits.
        if self.records_written >= self.limits.max_records.saturating_sub(1) as u64 {
            return Err(CaptureError::RecordCountExceeded);
        }
        if let CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes: Some(bytes),
            truncated,
        } = &record.kind
        {
            let body = self.codec.encode_body(bytes, self.records_written)?;
            let metadata = CaptureRecord {
                sequence: record.sequence,
                exchange_id: record.exchange_id,
                kind: CaptureRecordKind::BodySegment {
                    boundary: *boundary,
                    byte_count: *byte_count,
                    bytes: None,
                    truncated: *truncated,
                },
            };
            let mut stored = store(&metadata)?;
            stored.body_index = Some(StoredBodyIndex {
                frame_bytes: FRAME_HEADER_BYTES as u64 + body.len() as u64,
                bytes: bytes.len(),
                digest: Sha256::digest(bytes).into(),
            });
            let payload = serde_json::to_vec(&stored)?;
            if payload.len() > self.limits.max_record_bytes {
                return Err(CaptureError::RecordTooLarge {
                    actual: payload.len(),
                    limit: self.limits.max_record_bytes,
                });
            }
            let metadata = self.codec.encode(&payload, self.records_written)?;
            self.write_encoded(&[&metadata, &body])?;
        } else {
            let stored = store(record)?;
            let payload = serde_json::to_vec(&stored)?;
            self.write_payload(&payload)?;
        }
        self.records_written = self.records_written.saturating_add(1);
        Ok(())
    }

    /// Appends a final marker and flushes the destination.
    ///
    /// # Errors
    ///
    /// Returns a typed serialization, limit, sealed-state, or I/O error.
    pub fn seal(&mut self) -> Result<(), CaptureError> {
        if self.sealed {
            return Err(CaptureError::AlreadySealed);
        }
        let record = CaptureRecord {
            sequence: 0,
            exchange_id: 0,
            kind: CaptureRecordKind::Seal {
                record_count: self.records_written,
            },
        };
        let payload = serde_json::to_vec(&store(&record)?)?;
        self.write_payload(&payload)?;
        self.sealed = true;
        self.output.flush()?;
        Ok(())
    }

    /// Total bytes committed to the destination.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns the wrapped output without sealing it.
    pub fn into_inner(self) -> W {
        self.output
    }

    fn write_payload(&mut self, payload: &[u8]) -> Result<(), CaptureError> {
        if payload.len() > self.limits.max_record_bytes {
            return Err(CaptureError::RecordTooLarge {
                actual: payload.len(),
                limit: self.limits.max_record_bytes,
            });
        }
        let payload = self.codec.encode(payload, self.records_written)?;
        self.write_encoded(&[&payload])
    }

    fn write_encoded(&mut self, payloads: &[&[u8]]) -> Result<(), CaptureError> {
        let framed = payloads.iter().try_fold(0_u64, |total, payload| {
            total
                .checked_add(FRAME_HEADER_BYTES as u64 + payload.len() as u64)
                .ok_or(CaptureError::QuotaExceeded)
        })?;
        let next = self
            .bytes_written
            .checked_add(framed)
            .ok_or(CaptureError::QuotaExceeded)?;
        if next > self.limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        // Poison until the entire metadata/payload pair is committed; retries must never reuse a nonce.
        self.sealed = true;
        for payload in payloads {
            let length =
                u32::try_from(payload.len()).map_err(|_| CaptureError::RecordTooLarge {
                    actual: payload.len(),
                    limit: u32::MAX as usize,
                })?;
            self.output.write_all(&length.to_le_bytes())?;
            self.output
                .write_all(&crc32fast::hash(payload).to_le_bytes())?;
            self.output.write_all(payload)?;
        }
        self.output.flush()?;
        self.bytes_written = next;
        self.sealed = false;
        Ok(())
    }
}

/// Result of scanning a native artifact through its last valid frame.
#[derive(Clone, Debug)]
pub struct RecoveredCapture {
    /// Decoded records, including a seal when present.
    pub records: Vec<CaptureRecord>,
    /// Bytes comprising the valid prefix.
    pub valid_bytes: u64,
    /// Whether the input ended during a frame.
    pub truncated_tail: bool,
    /// Whether a valid final seal was present and consistent.
    pub sealed: bool,
}

/// Deterministic high-level artifact summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureSummary {
    /// All decoded records, including a seal.
    pub records: usize,
    /// Distinct nonzero exchange identifiers.
    pub exchanges: usize,
    /// Explicit loss markers.
    pub loss_markers: usize,
    /// Retained body bytes.
    pub retained_body_bytes: u64,
    /// Whether the artifact ended in a valid seal.
    pub sealed: bool,
    /// Whether recovery found a partial tail.
    pub truncated_tail: bool,
}

impl RecoveredCapture {
    /// Computes a deterministic, bounded summary.
    pub fn summary(&self) -> CaptureSummary {
        let exchanges = self
            .records
            .iter()
            .filter_map(|record| (record.exchange_id != 0).then_some(record.exchange_id))
            .collect::<BTreeSet<_>>()
            .len();
        let loss_markers = self
            .records
            .iter()
            .filter(|record| matches!(record.kind, CaptureRecordKind::Loss { .. }))
            .count();
        let retained_body_bytes = self
            .records
            .iter()
            .filter_map(|record| match &record.kind {
                CaptureRecordKind::BodySegment {
                    bytes: Some(bytes), ..
                } => Some(bytes.len() as u64),
                _ => None,
            })
            .fold(0_u64, u64::saturating_add);
        CaptureSummary {
            records: self.records.len(),
            exchanges,
            loss_markers,
            retained_body_bytes,
            sealed: self.sealed,
            truncated_tail: self.truncated_tail,
        }
    }
}

/// Report returned by a capture exporter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportReport {
    /// Records written.
    pub records: usize,
    /// Bytes written when known by the exporter.
    pub bytes: u64,
}

/// Adapter boundary for derived capture formats.
pub trait CaptureExporter {
    /// Exporter-specific failure.
    type Error;

    /// Writes a recovered native capture.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`] for serialization, format, or destination
    /// failures.
    fn export(&mut self, capture: &RecoveredCapture) -> Result<ExportReport, Self::Error>;
}

/// Streaming newline-delimited JSON exporter for scripting and diagnostics.
pub struct JsonLinesExporter<W> {
    output: W,
    bytes_written: u64,
}

impl<W: Write> JsonLinesExporter<W> {
    /// Wraps an output without writing a preamble.
    pub fn new(output: W) -> Self {
        Self {
            output,
            bytes_written: 0,
        }
    }

    /// Returns the wrapped output.
    pub fn into_inner(self) -> W {
        self.output
    }
}

impl<W: Write> CaptureExporter for JsonLinesExporter<W> {
    type Error = CaptureError;

    fn export(&mut self, capture: &RecoveredCapture) -> Result<ExportReport, Self::Error> {
        let starting_bytes = self.bytes_written;
        for record in &capture.records {
            let payload = serde_json::to_vec(&store(record)?)?;
            self.output.write_all(&payload)?;
            self.output.write_all(b"\n")?;
            self.bytes_written = self
                .bytes_written
                .saturating_add(payload.len() as u64)
                .saturating_add(1);
        }
        self.output.flush()?;
        Ok(ExportReport {
            records: capture.records.len(),
            bytes: self.bytes_written.saturating_sub(starting_bytes),
        })
    }
}

/// Reads the valid prefix of an artifact.
///
/// A partial final header or payload is reported as a recoverable truncated
/// tail. A complete frame with a bad checksum or malformed payload is rejected.
///
/// # Errors
///
/// Returns a typed format, checksum, serialization, bound, or I/O error.
pub fn recover<R: Read>(input: R, limits: CaptureLimits) -> Result<RecoveredCapture, CaptureError> {
    recover_with_password(input, limits, None)
}

/// Recover a bounded capture using a transient password.
///
/// # Errors
/// Returns password, authentication, format, resource or I/O failures.
pub fn recover_with_password<R: Read>(
    input: R,
    limits: CaptureLimits,
    password: Option<&CapturePassword>,
) -> Result<RecoveredCapture, CaptureError> {
    let mut reader = CaptureReader::with_password(input, limits, password)?;
    let mut records = Vec::new();
    while let Some(frame) = reader.read_next()? {
        records.push(frame.record);
    }
    Ok(RecoveredCapture {
        records,
        valid_bytes: reader.valid_bytes(),
        truncated_tail: reader.truncated_tail(),
        sealed: reader.sealed(),
    })
}

/// Authenticated metadata for a separately stored body payload.
#[derive(Clone, Copy, Debug)]
pub struct CaptureBodyIndex {
    /// Captured (expanded) body byte count.
    pub bytes: usize,
    /// Expected digest, verified when the payload is first read.
    pub digest: [u8; 32],
}

/// One checked native frame with its stable on-disk location.
#[derive(Clone, Debug)]
pub struct CaptureFrame {
    /// Offset of its frame header, suitable for a later bounded re-read.
    pub offset: u64,
    /// Zero-based physical frame index used for authenticated random reads.
    pub index: u64,
    /// Total header and payload bytes.
    pub frame_bytes: u64,
    /// Decoding context for an independently indexed payload.
    pub codec: FrameCodec,
    /// Validated durable record.
    pub record: CaptureRecord,
    /// Separate payload descriptor; indexed reads leave body bytes unloaded.
    pub body: Option<CaptureBodyIndex>,
}

/// Streaming native reader retaining at most one bounded frame payload.
pub struct CaptureReader<R> {
    input: R,
    limits: CaptureLimits,
    records: usize,
    valid_bytes: u64,
    truncated_tail: bool,
    sealed: bool,
    done: bool,
    codec: FrameCodec,
    index_base: u64,
}

impl<R: Read> CaptureReader<R> {
    /// Validates a native preamble and finite limits.
    ///
    /// # Errors
    /// Returns a format, truncated-preamble or I/O error.
    pub fn new(input: R, limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::with_password(input, limits, None)
    }

    /// Opens a native capture, checking the password before accepting records.
    ///
    /// # Errors
    /// Returns a missing/wrong password, unsupported encoding or preamble error.
    pub fn with_password(
        mut input: R,
        limits: CaptureLimits,
        password: Option<&CapturePassword>,
    ) -> Result<Self, CaptureError> {
        let limits = limits.validate()?;
        let mut magic = [0_u8; MAGIC.len()];
        input.read_exact(&mut magic).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                CaptureError::TruncatedPreamble
            } else {
                CaptureError::Io(error)
            }
        })?;
        if magic != MAGIC {
            return Err(CaptureError::InvalidMagic);
        }
        let (codec, valid_bytes) = FrameCodec::read(&mut input, password)?;
        if valid_bytes > limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        Ok(Self {
            input,
            limits,
            records: 0,
            valid_bytes,
            truncated_tail: false,
            sealed: false,
            done: false,
            codec,
            index_base: 0,
        })
    }

    /// Reads one complete checksummed record or the recoverable end of input.
    ///
    /// # Errors
    /// Returns complete-frame corruption, serialization, resource or I/O errors.
    pub fn read_next(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        self.read_next_using(|input, bytes| {
            let length = usize::try_from(bytes).map_err(|_| CaptureError::QuotaExceeded)?;
            let mut payload = vec![0; length];
            input.read_exact(&mut payload)?;
            Ok(Some(payload))
        })
    }

    fn check_frame_bytes(&self, bytes: u64) -> Result<(), CaptureError> {
        if self
            .valid_bytes
            .checked_add(bytes)
            .is_none_or(|next| next > self.limits.max_file_bytes)
        {
            return Err(CaptureError::QuotaExceeded);
        }
        Ok(())
    }

    fn read_next_using(
        &mut self,
        mut read_body: impl FnMut(&mut R, u64) -> Result<Option<Vec<u8>>, CaptureError>,
    ) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.done {
            return Ok(None);
        }
        let mut header = [0_u8; FRAME_HEADER_BYTES];
        let read = read_until_eof(&mut self.input, &mut header)?;
        if read != header.len() {
            self.done = true;
            self.truncated_tail = read != 0;
            return Ok(None);
        }
        let length = u32::from_le_bytes(
            header[..4]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        ) as usize;
        let expected_crc = u32::from_le_bytes(
            header[4..]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        );
        if length > FrameCodec::encoded_bound(self.limits.max_record_bytes) {
            return Err(CaptureError::RecordTooLarge {
                actual: length,
                limit: self.limits.max_record_bytes,
            });
        }
        if self.records >= self.limits.max_records {
            return Err(CaptureError::RecordCountExceeded);
        }
        let mut frame_bytes = FRAME_HEADER_BYTES as u64 + length as u64;
        self.check_frame_bytes(frame_bytes)?;
        let mut payload = vec![0_u8; length];
        if read_until_eof(&mut self.input, &mut payload)? != length {
            self.done = true;
            self.truncated_tail = true;
            return Ok(None);
        }
        if crc32fast::hash(&payload) != expected_crc {
            return Err(CaptureError::ChecksumMismatch {
                record_index: self.records,
            });
        }
        let index = self.index_base + self.records as u64;
        let payload = self
            .codec
            .decode(&payload, index, self.limits.max_record_bytes)?;
        let (mut record, body_index) =
            serde_json::from_slice::<StoredRecord>(&payload)?.load_metadata()?;
        if let Some(body) = body_index {
            body.validate(self.limits.max_record_bytes)?;
            frame_bytes = frame_bytes
                .checked_add(body.frame_bytes)
                .ok_or(CaptureError::QuotaExceeded)?;
            self.check_frame_bytes(frame_bytes)?;
            let encoded = match read_body(&mut self.input, body.frame_bytes) {
                Err(CaptureError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.done = true;
                    self.truncated_tail = true;
                    return Ok(None);
                }
                result => result?,
            };
            if let Some(encoded) = encoded {
                let bytes = body.decode(
                    &self.codec,
                    &encoded,
                    index,
                    self.limits.max_record_bytes,
                    self.records,
                )?;
                if let CaptureRecordKind::BodySegment { bytes: target, .. } = &mut record.kind {
                    *target = Some(bytes);
                }
            }
        }
        self.sealed = matches!(&record.kind, CaptureRecordKind::Seal { record_count } if *record_count == self.records as u64);
        self.records += 1;
        let offset = self.valid_bytes;
        self.valid_bytes += frame_bytes;
        Ok(Some(CaptureFrame {
            offset,
            index,
            frame_bytes,
            codec: self.codec.clone(),
            record,
            body: body_index.map(|body| CaptureBodyIndex {
                bytes: body.bytes,
                digest: body.digest,
            }),
        }))
    }

    /// Decoding context for later independent authenticated frame reads.
    pub fn codec(&self) -> FrameCodec {
        self.codec.clone()
    }

    /// Length of the fully validated native prefix.
    pub fn valid_bytes(&self) -> u64 {
        self.valid_bytes
    }
    /// Whether input ended inside a frame.
    pub fn truncated_tail(&self) -> bool {
        self.truncated_tail
    }
    /// Whether the last valid frame is a consistent seal.
    pub fn sealed(&self) -> bool {
        self.sealed && !self.truncated_tail
    }
}

impl<R: Read + Seek> CaptureReader<R> {
    /// Reads only record metadata, seeking over separately encoded body bytes.
    /// The skipped payload's
    /// checksum, authentication tag and digest are checked by an on-demand read.
    ///
    /// # Errors
    /// Returns metadata corruption, invalid bounds or I/O errors. An incomplete
    /// final metadata/body pair is a recoverable truncated tail.
    pub fn read_next_indexed(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        let position = self.input.stream_position()?;
        let end = self.input.seek(SeekFrom::End(0))?;
        self.input.seek(SeekFrom::Start(position))?;
        self.read_next_using(|input, bytes| {
            let next = input
                .stream_position()?
                .checked_add(bytes)
                .ok_or(CaptureError::QuotaExceeded)?;
            if next > end {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            input.seek(SeekFrom::Start(next))?;
            Ok(None)
        })
    }
}

/// Re-read one compressed/encrypted frame with the original context and physical index.
///
/// # Errors
/// Rejects changed ciphertext, reordered frames, malformed data or exceeded bounds.
pub fn read_indexed_frame_with_codec(
    input: impl Read,
    frame_bytes: u64,
    limits: CaptureLimits,
    codec: &FrameCodec,
    index: u64,
) -> Result<CaptureRecord, CaptureError> {
    let mut reader = CaptureReader {
        input: input.take(frame_bytes),
        limits: limits.validate()?,
        records: 0,
        valid_bytes: 0,
        truncated_tail: false,
        sealed: false,
        done: false,
        codec: codec.clone(),
        index_base: index,
    };
    let frame = reader.read_next()?.ok_or(CaptureError::TruncatedPreamble)?;
    if frame.frame_bytes != frame_bytes {
        return Err(CaptureError::InvalidMagic);
    }
    Ok(frame.record)
}

/// Converts one already-redacted core observer event into the durable schema.
pub fn record_from_observer(
    event: &ObserverEvent,
    policy: &CapturePolicy,
) -> Option<CaptureRecord> {
    let request_headers =
        |headers: &HeaderBlock| captured_headers(headers, &policy.request_header_names);
    let response_headers =
        |headers: &HeaderBlock| captured_headers(headers, &policy.response_header_names);
    let kind = match &event.kind {
        ObserverEventKind::Performance(evidence) => {
            CaptureRecordKind::Performance(evidence.clone())
        }
        ObserverEventKind::ExchangeStarted { metadata } => CaptureRecordKind::ExchangeStarted {
            client_addr: metadata.client_addr.to_string(),
            client_identity: metadata.client_identity.clone(),
            listener_addr: metadata.listener_addr.to_string(),
            authority: metadata.original_target.as_target().authority.clone(),
            started_unix_nanos: unix_nanos(metadata.started_at),
        },
        ObserverEventKind::RequestHeadObserved { boundary, head } => {
            CaptureRecordKind::RequestHead {
                boundary: *boundary,
                method: head.method.clone(),
                target: display_target(&head.target),
                headers: request_headers(&head.headers),
            }
        }
        ObserverEventKind::ResponseHeadObserved { boundary, head } => {
            CaptureRecordKind::ResponseHead {
                boundary: *boundary,
                status: head.status,
                headers: response_headers(&head.headers),
            }
        }
        ObserverEventKind::HookEffect(effect) => CaptureRecordKind::HookEffect {
            hook_id: effect.interceptor.id.as_str().to_owned(),
            hook_name: effect.interceptor.name.to_string(),
            phase: hook_phase(effect.phase).to_owned(),
            action: hook_action(&effect.action).to_owned(),
            changed: effect.changed,
        },
        ObserverEventKind::BodyChunk(chunk) => CaptureRecordKind::BodySegment {
            boundary: chunk.boundary,
            byte_count: chunk.byte_count,
            bytes: policy
                .retain_body_samples
                .then(|| chunk.sample.as_ref().map(|bytes| bytes.to_vec()))
                .flatten(),
            truncated: chunk.truncated || chunk.sample.is_none(),
        },
        ObserverEventKind::BodyTrailers(trailers) => CaptureRecordKind::Trailers {
            boundary: trailers.boundary,
            headers: if matches!(
                trailers.boundary,
                ExchangeBoundary::ClientRequest | ExchangeBoundary::UpstreamRequest
            ) {
                request_headers(&trailers.trailers)
            } else {
                response_headers(&trailers.trailers)
            },
        },
        ObserverEventKind::BodyCompleted {
            boundary: completed_boundary,
        } => CaptureRecordKind::Unknown {
            kind: "body-completed".into(),
            payload: serde_json::json!({"boundary": boundary(*completed_boundary)}),
        },
        ObserverEventKind::RouteSelected { policy_id, reason } => {
            CaptureRecordKind::RouteSelected {
                policy_id: policy_id.to_string(),
                reason: reason.to_string(),
            }
        }
        ObserverEventKind::RouteAttempt(attempt) => CaptureRecordKind::RouteAttempt {
            protocol: format!("{:?}", attempt.protocol),
            outcome: attempt.outcome.to_string(),
        },
        ObserverEventKind::Completed(_) => CaptureRecordKind::Completed,
        ObserverEventKind::Failed(failure) => CaptureRecordKind::Failed {
            category: format!("{:?}", failure.kind),
            message: failure.message.clone(),
        },
        ObserverEventKind::HookInitializationSkipped(diagnostic) => {
            CaptureRecordKind::HookInitializationSkipped {
                name: diagnostic.name.to_string(),
                message: diagnostic.message.clone(),
            }
        }
        ObserverEventKind::RequestHeadFinalized(_)
        | ObserverEventKind::ResponseHeadFinalized(_) => {
            return None;
        }
    };
    Some(CaptureRecord {
        sequence: event.sequence,
        exchange_id: event.exchange_id.0,
        kind,
    })
}

/// Constructs an explicit observer-delivery loss marker.
pub fn loss_record(
    exchange_id: u128,
    first_missing_sequence: u64,
    count: u64,
    reason: impl Into<String>,
) -> CaptureRecord {
    CaptureRecord {
        sequence: first_missing_sequence,
        exchange_id,
        kind: CaptureRecordKind::Loss {
            first_missing_sequence,
            count,
            reason: reason.into(),
        },
    }
}

fn captured_headers(headers: &HeaderBlock, redacted: &BTreeSet<String>) -> Vec<CapturedHeader> {
    headers
        .iter()
        .map(|field| CapturedHeader {
            name: field.name().to_vec(),
            original_value_bytes: field.original_value_bytes(),
            value: (!field.is_redacted()
                && !redacted.contains(&String::from_utf8_lossy(field.name()).to_ascii_lowercase()))
            .then(|| field.value().to_vec()),
        })
        .collect()
}

fn display_target(target: &transmog_core::Target) -> String {
    let mut value = format!("{}://{}{}", target.scheme, target.authority, target.path);
    if let Some(query) = &target.query {
        value.push('?');
        value.push_str(query);
    }
    value
}

fn unix_nanos(value: SystemTime) -> u128 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

const fn boundary(value: ExchangeBoundary) -> &'static str {
    match value {
        ExchangeBoundary::ClientRequest => "client-request",
        ExchangeBoundary::UpstreamRequest => "upstream-request",
        ExchangeBoundary::UpstreamResponse => "upstream-response",
        ExchangeBoundary::ClientResponse => "client-response",
    }
}

fn parse_boundary(value: &str) -> Result<ExchangeBoundary, CaptureError> {
    match value {
        "client-request" => Ok(ExchangeBoundary::ClientRequest),
        "upstream-request" => Ok(ExchangeBoundary::UpstreamRequest),
        "upstream-response" => Ok(ExchangeBoundary::UpstreamResponse),
        "client-response" => Ok(ExchangeBoundary::ClientResponse),
        _ => Err(CaptureError::MalformedRecord("unknown boundary")),
    }
}

const fn hook_phase(value: HookPhase) -> &'static str {
    match value {
        HookPhase::RequestHead => "request-head",
        HookPhase::RequestBody => "request-body",
        HookPhase::ResponseHead => "response-head",
        HookPhase::ResponseBody => "response-body",
    }
}

const fn hook_action(value: &HookEffectAction) -> &'static str {
    match value {
        HookEffectAction::ReplaceRequestHead(_) => "replace-request-head",
        HookEffectAction::Reroute { .. } => "reroute",
        HookEffectAction::ReplaceResponseHead(_) => "replace-response-head",
        HookEffectAction::Respond { .. } => "respond",
        HookEffectAction::Abort(_) => "abort",
        HookEffectAction::BodyPlan { .. } => "body-plan",
    }
}

fn json<T: Serialize>(value: T) -> Result<Value, CaptureError> {
    Ok(serde_json::to_value(value)?)
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CaptureError> {
    Ok(serde_json::from_value(value)?)
}

#[allow(clippy::too_many_lines)]
fn store(record: &CaptureRecord) -> Result<StoredRecord, CaptureError> {
    let (kind, payload) = match &record.kind {
        CaptureRecordKind::Performance(evidence) => {
            ("performance", serde_json::to_value(evidence)?)
        }
        CaptureRecordKind::ExchangeStarted {
            client_addr,
            client_identity,
            listener_addr,
            authority,
            started_unix_nanos,
        } => (
            "exchange-started",
            json(StartedPayload {
                client_addr: client_addr.clone(),
                client_identity: client_identity.clone(),
                listener_addr: listener_addr.clone(),
                authority: authority.clone(),
                started_unix_nanos: *started_unix_nanos,
            })?,
        ),
        CaptureRecordKind::RequestHead {
            boundary: edge,
            method,
            target,
            headers,
        } => (
            "request-head",
            json(RequestHeadPayload {
                boundary: boundary(*edge).to_owned(),
                method: method.clone(),
                target: target.clone(),
                headers: headers.clone(),
            })?,
        ),
        CaptureRecordKind::ResponseHead {
            boundary: edge,
            status,
            headers,
        } => (
            "response-head",
            json(ResponseHeadPayload {
                boundary: boundary(*edge).to_owned(),
                status: *status,
                headers: headers.clone(),
            })?,
        ),
        CaptureRecordKind::BodySegment {
            boundary: edge,
            byte_count,
            bytes,
            truncated,
        } => (
            "body-segment",
            json(BodyPayload {
                boundary: boundary(*edge).to_owned(),
                byte_count: *byte_count,
                bytes: bytes.clone(),
                truncated: *truncated,
            })?,
        ),
        CaptureRecordKind::Trailers {
            boundary: edge,
            headers,
        } => (
            "trailers",
            json(TrailersPayload {
                boundary: boundary(*edge).to_owned(),
                headers: headers.clone(),
            })?,
        ),
        CaptureRecordKind::HookEffect {
            hook_id,
            hook_name,
            phase,
            action,
            changed,
        } => (
            "hook-effect",
            json(HookPayload {
                hook_id: hook_id.clone(),
                hook_name: hook_name.clone(),
                phase: phase.clone(),
                action: action.clone(),
                changed: *changed,
            })?,
        ),
        CaptureRecordKind::RouteSelected { policy_id, reason } => (
            "route-selected",
            json(RoutePayload {
                policy_id: policy_id.clone(),
                reason: reason.clone(),
            })?,
        ),
        CaptureRecordKind::RouteAttempt { protocol, outcome } => (
            "route-attempt",
            json(AttemptPayload {
                protocol: protocol.clone(),
                outcome: outcome.clone(),
            })?,
        ),
        CaptureRecordKind::Loss {
            first_missing_sequence,
            count,
            reason,
        } => (
            "loss",
            json(LossPayload {
                first_missing_sequence: *first_missing_sequence,
                count: *count,
                reason: reason.clone(),
            })?,
        ),
        CaptureRecordKind::Completed => ("completed", Value::Null),
        CaptureRecordKind::Failed { category, message } => (
            "failed",
            json(FailurePayload {
                category: category.clone(),
                message: message.clone(),
            })?,
        ),
        CaptureRecordKind::HookInitializationSkipped { name, message } => (
            "hook-initialization-skipped",
            json(SkippedPayload {
                name: name.clone(),
                message: message.clone(),
            })?,
        ),
        CaptureRecordKind::Seal { record_count } => (
            "seal",
            json(SealPayload {
                record_count: *record_count,
            })?,
        ),
        CaptureRecordKind::Unknown { kind, payload } => (kind.as_str(), payload.clone()),
    };
    Ok(StoredRecord {
        revision: CAPTURE_FORMAT_REVISION,
        sequence: record.sequence,
        exchange_id: record.exchange_id,
        kind: kind.to_owned(),
        payload,
        body_index: None,
    })
}

#[allow(clippy::too_many_lines)]
fn load(record: StoredRecord) -> Result<CaptureRecord, CaptureError> {
    if record.revision != CAPTURE_FORMAT_REVISION {
        return Err(CaptureError::UnsupportedRevision(record.revision));
    }
    let kind = match record.kind.as_str() {
        "performance" => CaptureRecordKind::Performance(decode(record.payload)?),
        "exchange-started" => {
            let value: StartedPayload = decode(record.payload)?;
            CaptureRecordKind::ExchangeStarted {
                client_addr: value.client_addr,
                client_identity: value.client_identity,
                listener_addr: value.listener_addr,
                authority: value.authority,
                started_unix_nanos: value.started_unix_nanos,
            }
        }
        "request-head" => {
            let value: RequestHeadPayload = decode(record.payload)?;
            CaptureRecordKind::RequestHead {
                boundary: parse_boundary(&value.boundary)?,
                method: value.method,
                target: value.target,
                headers: value.headers,
            }
        }
        "response-head" => {
            let value: ResponseHeadPayload = decode(record.payload)?;
            CaptureRecordKind::ResponseHead {
                boundary: parse_boundary(&value.boundary)?,
                status: value.status,
                headers: value.headers,
            }
        }
        "body-segment" => {
            let value: BodyPayload = decode(record.payload)?;
            CaptureRecordKind::BodySegment {
                boundary: parse_boundary(&value.boundary)?,
                byte_count: value.byte_count,
                bytes: value.bytes,
                truncated: value.truncated,
            }
        }
        "trailers" => {
            let value: TrailersPayload = decode(record.payload)?;
            CaptureRecordKind::Trailers {
                boundary: parse_boundary(&value.boundary)?,
                headers: value.headers,
            }
        }
        "hook-effect" => {
            let value: HookPayload = decode(record.payload)?;
            CaptureRecordKind::HookEffect {
                hook_id: value.hook_id,
                hook_name: value.hook_name,
                phase: value.phase,
                action: value.action,
                changed: value.changed,
            }
        }
        "route-selected" => {
            let value: RoutePayload = decode(record.payload)?;
            CaptureRecordKind::RouteSelected {
                policy_id: value.policy_id,
                reason: value.reason,
            }
        }
        "route-attempt" => {
            let value: AttemptPayload = decode(record.payload)?;
            CaptureRecordKind::RouteAttempt {
                protocol: value.protocol,
                outcome: value.outcome,
            }
        }
        "loss" => {
            let value: LossPayload = decode(record.payload)?;
            CaptureRecordKind::Loss {
                first_missing_sequence: value.first_missing_sequence,
                count: value.count,
                reason: value.reason,
            }
        }
        "completed" => CaptureRecordKind::Completed,
        "failed" => {
            let value: FailurePayload = decode(record.payload)?;
            CaptureRecordKind::Failed {
                category: value.category,
                message: value.message,
            }
        }
        "hook-initialization-skipped" => {
            let value: SkippedPayload = decode(record.payload)?;
            CaptureRecordKind::HookInitializationSkipped {
                name: value.name,
                message: value.message,
            }
        }
        "seal" => {
            let value: SealPayload = decode(record.payload)?;
            CaptureRecordKind::Seal {
                record_count: value.record_count,
            }
        }
        _ => CaptureRecordKind::Unknown {
            kind: record.kind,
            payload: record.payload,
        },
    };
    Ok(CaptureRecord {
        sequence: record.sequence,
        exchange_id: record.exchange_id,
        kind,
    })
}

fn read_until_eof<R: Read>(input: &mut R, destination: &mut [u8]) -> io::Result<usize> {
    let mut offset = 0;
    while offset < destination.len() {
        match input.read(&mut destination[offset..])? {
            0 => break,
            count => offset += count,
        }
    }
    Ok(offset)
}

/// Native capture failure.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// Encrypted capture needs an explicitly supplied transient password.
    #[error("This capture requires a password")]
    PasswordRequired,
    /// Header key check failed before any traffic was published.
    #[error("The capture password is incorrect or its encrypted header is damaged")]
    InvalidPassword,
    /// Cipher, KDF or compression parameters are not supported.
    #[error("The capture uses an unsupported encoding or encryption algorithm")]
    UnsupportedEncoding,
    /// Authenticated ciphertext or frame ordering changed.
    #[error("The capture failed authentication")]
    AuthenticationFailed,
    /// Redacted cryptographic failure.
    #[error("Capture cryptography failed: {0}")]
    Crypto(&'static str),
    /// Limits were zero or internally inconsistent.
    #[error("capture limits are invalid")]
    InvalidLimits,
    /// Preamble was incomplete.
    #[error("capture preamble is truncated")]
    TruncatedPreamble,
    /// Magic or revision marker was not recognized.
    #[error("capture magic is invalid")]
    InvalidMagic,
    /// Payload exceeded the configured record bound.
    #[error("capture record contains {actual} bytes, exceeding limit {limit}")]
    RecordTooLarge {
        /// Actual bytes.
        actual: usize,
        /// Configured bound.
        limit: usize,
    },
    /// Artifact quota would be exceeded.
    #[error("capture artifact quota exceeded")]
    QuotaExceeded,
    /// Recovery record count would exceed its finite bound.
    #[error("capture record-count limit exceeded")]
    RecordCountExceeded,
    /// A complete record failed its checksum.
    #[error("capture record {record_index} failed its checksum")]
    ChecksumMismatch {
        /// Zero-based record index.
        record_index: usize,
    },
    /// Record used a future incompatible schema revision.
    #[error("capture revision {0} is not supported")]
    UnsupportedRevision(u32),
    /// Known record payload was structurally invalid.
    #[error("malformed capture record: {0}")]
    MalformedRecord(&'static str),
    /// Artifact was already sealed.
    #[error("capture artifact is already sealed")]
    AlreadySealed,
    /// Underlying stream failed.
    #[error("capture I/O failed: {0}")]
    Io(#[from] io::Error),
    /// Record JSON failed.
    #[error("capture serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use transmog_core::{
        HeaderField, HttpLegVersion, RequestHead, Target, intercept::ExchangeId,
        observe::ObserverEventKind,
    };

    use super::*;

    #[test]
    fn outdated_revisions_are_rejected_and_performance_round_trips() {
        for revision in [1, 2] {
            let mut old = store(&completed(3)).unwrap();
            old.revision = revision;
            assert!(matches!(
                load(old),
                Err(CaptureError::UnsupportedRevision(_))
            ));
        }
        let evidence = transmog_core::performance::PerformanceEvidence {
            points: vec![transmog_core::performance::TimingPoint {
                milestone: transmog_core::performance::Milestone::ClientConnected,
                unix_millis: 1_800_000_000_000,
                offset_micros: -123,
            }],
            ..Default::default()
        };
        let record = CaptureRecord {
            sequence: 1,
            exchange_id: 7,
            kind: CaptureRecordKind::Performance(evidence),
        };
        let recovered = recover(
            Cursor::new(artifact(std::slice::from_ref(&record), true)),
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(recovered.records[0], record);
        assert!(recovered.sealed);
    }

    fn completed(sequence: u64) -> CaptureRecord {
        CaptureRecord {
            sequence,
            exchange_id: 7,
            kind: CaptureRecordKind::Completed,
        }
    }

    fn artifact(records: &[CaptureRecord], seal: bool) -> Vec<u8> {
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        for record in records {
            writer.append(record).unwrap();
        }
        if seal {
            writer.seal().unwrap();
        }
        writer.into_inner()
    }

    #[test]
    fn streaming_offsets_can_reread_exact_frames_and_reject_changes() {
        let bytes = artifact(&[completed(1), completed(2)], true);
        let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        let first = reader.read_next().unwrap().unwrap();
        let second = reader.read_next().unwrap().unwrap();
        assert_eq!(second.offset, first.offset + first.frame_bytes);
        let mut cursor = Cursor::new(&bytes);
        cursor.set_position(second.offset);
        assert_eq!(
            read_indexed_frame_with_codec(
                cursor,
                second.frame_bytes,
                CaptureLimits::default(),
                &second.codec,
                second.index
            )
            .unwrap(),
            completed(2)
        );
        let mut changed = bytes.clone();
        let last = usize::try_from(second.offset + second.frame_bytes - 1).unwrap();
        changed[last] ^= 255;
        let mut cursor = Cursor::new(changed);
        cursor.set_position(second.offset);
        assert!(
            read_indexed_frame_with_codec(
                cursor,
                second.frame_bytes,
                CaptureLimits::default(),
                &second.codec,
                second.index
            )
            .is_err()
        );
        reader.read_next().unwrap().unwrap();
        assert!(reader.sealed());
        assert!(reader.read_next().unwrap().is_none());
        let mut partial = bytes;
        partial.extend_from_slice(&[1, 2, 3]);
        let recovered = recover(Cursor::new(partial), CaptureLimits::default()).unwrap();
        assert!(recovered.truncated_tail);
        assert!(!recovered.sealed);
    }

    struct ShortWriter {
        bytes: Vec<u8>,
        quantum: usize,
    }

    impl Write for ShortWriter {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            let count = input.len().min(self.quantum);
            self.bytes.extend_from_slice(&input[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn arbitrary_short_writes_round_trip_and_seal() {
        let output = ShortWriter {
            bytes: Vec::new(),
            quantum: 1,
        };
        let mut writer = CaptureWriter::new(output, CaptureLimits::default()).unwrap();
        writer.append(&completed(1)).unwrap();
        writer.seal().unwrap();
        assert_eq!(
            writer.append(&completed(2)).unwrap_err().to_string(),
            "capture artifact is already sealed"
        );
        let recovered = recover(
            Cursor::new(writer.into_inner().bytes),
            CaptureLimits::default(),
        )
        .unwrap();
        assert!(recovered.sealed);
        assert!(!recovered.truncated_tail);
        assert_eq!(recovered.records.len(), 2);
    }

    #[test]
    fn every_truncated_tail_recovers_only_a_valid_prefix() {
        let bytes = artifact(&[completed(1), completed(2)], true);
        let preamble = usize::try_from(
            CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default())
                .unwrap()
                .valid_bytes(),
        )
        .unwrap();
        for length in preamble..bytes.len() {
            let recovered = recover(&bytes[..length], CaptureLimits::default()).unwrap();
            assert!(recovered.valid_bytes <= length as u64);
            assert!(!recovered.sealed);
            if length > preamble {
                assert!(recovered.truncated_tail || recovered.valid_bytes == length as u64);
            }
        }
    }

    #[test]
    fn default_writer_crosses_four_gib_without_a_file_ceiling() {
        let mut writer = CaptureWriter::new(std::io::sink(), CaptureLimits::default()).unwrap();
        // Resume accounting just before the former ceiling without allocating a multi-GiB fixture.
        writer.bytes_written = 4 * 1024 * 1024 * 1024 - 1;
        writer.append(&completed(1)).unwrap();
        writer.seal().unwrap();
        assert!(writer.bytes_written() > 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn corruption_quota_and_record_bounds_fail_closed() {
        let mut bytes = artifact(&[completed(1)], false);
        *bytes.last_mut().unwrap() ^= 0x80;
        assert!(matches!(
            recover(&bytes[..], CaptureLimits::default()),
            Err(CaptureError::ChecksumMismatch { record_index: 0 })
        ));

        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        writer.limits.max_file_bytes = writer.bytes_written() + 1;
        assert!(matches!(
            writer.append(&completed(1)),
            Err(CaptureError::QuotaExceeded)
        ));
    }

    #[test]
    fn unknown_records_are_preserved() {
        let record = CaptureRecord {
            sequence: 1,
            exchange_id: 4,
            kind: CaptureRecordKind::Unknown {
                kind: "future-kind".to_owned(),
                payload: serde_json::json!({"future": true}),
            },
        };
        let recovered = recover(
            &artifact(std::slice::from_ref(&record), false)[..],
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(recovered.records, vec![record]);
    }

    #[test]
    fn explicit_loss_records_are_durable() {
        let record = loss_record(9, 10, 3, "observer-queue-saturated");
        let recovered = recover(
            &artifact(std::slice::from_ref(&record), false)[..],
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(recovered.records, vec![record]);
    }

    #[test]
    fn caller_process_identity_round_trips_in_streaming_capture() {
        let record = CaptureRecord {
            sequence: 1,
            exchange_id: 7,
            kind: CaptureRecordKind::ExchangeStarted {
                client_addr: "127.0.0.1:50000".to_owned(),
                client_identity: ClientIdentity::LocalProcess {
                    pid: 4242,
                    name: Some("browser.exe".to_owned()),
                },
                listener_addr: "127.0.0.1:8080".to_owned(),
                authority: "example.test".to_owned(),
                started_unix_nanos: 10,
            },
        };
        let recovered = recover(
            &artifact(std::slice::from_ref(&record), false)[..],
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(recovered.records, vec![record]);
    }

    #[test]
    fn observer_conversion_redacts_again_and_ignores_nonfinalized_heads() {
        let mut headers = HeaderBlock::new();
        headers.push(HeaderField::try_new("authorization", b"secret".to_vec()).unwrap());
        headers.push(HeaderField::try_new("x-visible", b"yes".to_vec()).unwrap());
        let head = RequestHead {
            method: "GET".to_owned(),
            target: Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/".to_owned(),
                query: None,
            },
            headers,
            source_version: HttpLegVersion::Http2,
        };
        let event = ObserverEvent {
            exchange_id: ExchangeId(7),
            sequence: 2,
            kind: ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::ClientRequest,
                head: head.clone(),
            },
        };
        let record = record_from_observer(&event, &CapturePolicy::default()).unwrap();
        let CaptureRecordKind::RequestHead { headers, .. } = record.kind else {
            panic!("expected request head");
        };
        assert!(
            headers
                .iter()
                .any(|field| { field.name == b"authorization" && field.value.is_none() })
        );
        assert!(headers.iter().any(|field| {
            field.name == b"x-visible" && field.value.as_deref() == Some(&b"yes"[..])
        }));

        let nonfinalized = ObserverEvent {
            exchange_id: ExchangeId(7),
            sequence: 3,
            kind: ObserverEventKind::RequestHeadFinalized(head),
        };
        assert!(record_from_observer(&nonfinalized, &CapturePolicy::default()).is_none());
    }

    #[test]
    fn body_completion_observation_survives_native_capture_encoding() {
        let event = ObserverEvent {
            exchange_id: ExchangeId(7),
            sequence: 1,
            kind: ObserverEventKind::BodyCompleted {
                boundary: ExchangeBoundary::ClientRequest,
            },
        };
        let record = record_from_observer(&event, &CapturePolicy::default()).unwrap();
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        writer.append(&record).unwrap();
        writer.seal().unwrap();
        let saved = recover(&writer.into_inner()[..], CaptureLimits::default()).unwrap();
        assert_eq!(saved.records[0], record);
        assert!(
            matches!(&record.kind, CaptureRecordKind::Unknown { kind, payload }
            if kind == "body-completed" && payload["boundary"] == "client-request")
        );
    }

    #[test]
    fn large_body_frames_split_without_losing_bytes_counts_or_sequence() {
        let limits = CaptureLimits {
            max_record_bytes: 4096,
            ..CaptureLimits::default()
        };
        let bytes = vec![255; 32 * 1024];
        let record = CaptureRecord {
            exchange_id: 7,
            sequence: 2,
            kind: CaptureRecordKind::BodySegment {
                boundary: ExchangeBoundary::ClientRequest,
                byte_count: 50_000,
                bytes: Some(bytes.clone()),
                truncated: true,
            },
        };
        let mut writer = CaptureWriter::new(Vec::new(), limits).unwrap();
        writer.append(&record).unwrap();
        writer.seal().unwrap();
        let recovered = recover(&writer.into_inner()[..], limits).unwrap();
        assert!(recovered.sealed);
        let mut exact = Vec::new();
        let mut observed = 0;
        let mut parts = 0;
        for record in recovered.records {
            if let CaptureRecordKind::BodySegment {
                bytes: Some(bytes),
                byte_count,
                truncated,
                ..
            } = record.kind
            {
                assert_eq!(record.sequence, 2);
                assert!(truncated);
                exact.extend(bytes);
                observed += byte_count;
                parts += 1;
            }
        }
        assert!(parts > 1);
        assert_eq!(exact, bytes);
        assert_eq!(observed, 50_000);
        let limits = CaptureLimits {
            max_records: 2,
            ..limits
        };
        let mut writer = CaptureWriter::new(Vec::new(), limits).unwrap();
        writer.append(&completed(1)).unwrap();
        assert!(matches!(
            writer.append(&completed(2)),
            Err(CaptureError::RecordCountExceeded)
        ));
        writer.seal().unwrap();
        assert!(recover(&writer.into_inner()[..], limits).unwrap().sealed);
    }

    #[test]
    fn request_sample_limit_preserves_counts_boundaries_and_unlimited() {
        let mut policy = CapturePolicy {
            retain_body_samples: true,
            request_body_limit: Some(5),
            ..CapturePolicy::default()
        };
        let mut retention = CaptureBodyRetention::default();
        let segment = |boundary, bytes: Vec<u8>| CaptureRecord {
            sequence: 1,
            exchange_id: 7,
            kind: CaptureRecordKind::BodySegment {
                boundary,
                byte_count: bytes.len(),
                bytes: Some(bytes),
                truncated: false,
            },
        };
        let mut client = segment(ExchangeBoundary::ClientRequest, vec![1; 3]);
        retention.apply(&mut client, &policy);
        let mut next = segment(ExchangeBoundary::ClientRequest, vec![2; 4]);
        retention.apply(&mut next, &policy);
        assert!(
            matches!(&next.kind,CaptureRecordKind::BodySegment {byte_count:4,bytes:Some(bytes),truncated:true,..} if bytes==&vec![2;2])
        );
        let mut upstream = segment(ExchangeBoundary::UpstreamRequest, vec![3; 5]);
        retention.apply(&mut upstream, &policy);
        assert!(
            matches!(&upstream.kind,CaptureRecordKind::BodySegment {bytes:Some(bytes),truncated:false,..} if bytes.len()==5)
        );
        let mut response = segment(ExchangeBoundary::ClientResponse, vec![4; 8]);
        retention.apply(&mut response, &policy);
        assert!(
            matches!(&response.kind,CaptureRecordKind::BodySegment {bytes:Some(bytes),truncated:false,..} if bytes.len()==8)
        );
        let mut end = completed(2);
        retention.apply(&mut end, &policy);
        assert!(retention.request_bytes.is_empty());
        policy.request_body_limit = None;
        let mut unlimited = segment(ExchangeBoundary::ClientRequest, vec![5; 20]);
        retention.apply(&mut unlimited, &policy);
        assert!(
            matches!(&unlimited.kind,CaptureRecordKind::BodySegment {bytes:Some(bytes),truncated:false,..} if bytes.len()==20)
        );
    }

    #[test]
    fn evicted_requests_release_accounting_without_losing_active_limits() {
        let policy = CapturePolicy {
            retain_body_samples: true,
            request_body_limit: Some(2),
            ..CapturePolicy::default()
        };
        let mut retention = CaptureBodyRetention::default();
        for id in 1..=100 {
            let mut sample = CaptureRecord {
                exchange_id: id,
                sequence: 1,
                kind: CaptureRecordKind::BodySegment {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 2,
                    bytes: Some(vec![1; 2]),
                    truncated: false,
                },
            };
            retention.apply(&mut sample, &policy);
        }
        retention.retain_exchanges(|id| id == 100);
        assert_eq!(retention.request_bytes.len(), 1);
        let mut sample = CaptureRecord {
            exchange_id: 100,
            sequence: 2,
            kind: CaptureRecordKind::BodySegment {
                boundary: ExchangeBoundary::ClientRequest,
                byte_count: 2,
                bytes: Some(vec![2; 2]),
                truncated: false,
            },
        };
        retention.apply(&mut sample, &policy);
        assert!(
            matches!(&sample.kind, CaptureRecordKind::BodySegment {bytes: Some(bytes), truncated: true, ..} if bytes.is_empty())
        );
    }

    #[test]
    fn json_lines_export_is_streaming_and_summary_is_deterministic() {
        let capture = recover(
            &artifact(&[completed(1), loss_record(7, 2, 1, "gap")], true)[..],
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(
            capture.summary(),
            CaptureSummary {
                records: 3,
                exchanges: 1,
                loss_markers: 1,
                retained_body_bytes: 0,
                sealed: true,
                truncated_tail: false,
            }
        );
        let mut exporter = JsonLinesExporter::new(Vec::new());
        let report = exporter.export(&capture).unwrap();
        let output = exporter.into_inner();
        assert_eq!(report.records, 3);
        assert_eq!(report.bytes, output.len() as u64);
        assert_eq!(
            String::from_utf8(output.clone()).unwrap().lines().count(),
            3
        );
        for line in output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let _: serde_json::Value = serde_json::from_slice(line).unwrap();
        }
    }
}
