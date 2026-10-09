use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use transmog_capture::{CaptureLimits, CaptureReader, CaptureRecordKind, CapturedHeader};
use transmog_core::{
    ClientIdentity, ConnectionId, HeaderBlock, HeaderField, HttpLegVersion, RequestHead,
    ResponseHead, SessionId, SessionMetadata, StreamId, Target,
    intercept::{
        CompletedExchange, ExchangeFailure, ExchangeFailureKind, ExchangeId, ExchangeMetadata,
        ExchangeStage,
    },
    observe::ExchangeBoundary,
};
use transmog_saz::{SazArchive, SazImportLimits};
use transmog_session::{
    ApplicationSessionService, BodySnapshot, ObservedRequestHead, ObservedResponseHead,
    SessionSnapshot, SessionTerminal,
};

use crate::{
    AppError, BodyStore, ErrorCategory,
    body_store::SavedBodySource,
    trace_body::{NativeBodyPiece, NativeBodySource, SazBodySource, SourceReader},
};

const MAX_SESSIONS: usize = 100_000;
const MAX_HEAD_BYTES: usize = 256 * 1024 * 1024;

/// Saved trace import, independent of live proxy capture and settings.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceImportRequest {
    /// Transient password for encrypted native or SAZ traffic.
    #[serde(default)]
    pub password: Option<transmog_capture::CapturePassword>,
    /// User-selected original source file.
    pub path: PathBuf,
    /// Caller-generated operation key used for cancellation and stale results.
    pub operation_id: String,
    /// Finite source byte limit, at most four GiB.
    pub max_file_bytes: u64,
}

/// Import progress for one explicitly named operation.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceImportProgress {
    /// Operation key, independent of request selection.
    pub operation_id: String,
    /// Completed bytes or indexing units.
    pub completed: u64,
    /// Total indexing units or input bytes.
    pub total: u64,
}

/// Per-source trace context, never replaced with the importing machine's state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceMetadataView {
    /// Namespace shared by every entry imported from this source.
    pub id: String,
    /// Human-readable source filename.
    pub name: String,
    /// `saz` or `native`.
    pub format: &'static str,
    /// Source path explicitly selected by the user.
    pub path: PathBuf,
    /// Saved-session count.
    pub sessions: usize,
    /// Import completion time in Unix milliseconds.
    pub imported_at: u64,
    /// Original capture-level metadata, including optional network context.
    pub context: Value,
    /// Bounded source-fidelity and recovery notes.
    pub notes: Vec<String>,
}

/// Completed import report. Source files are unchanged.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceImportResult {
    /// Direct navigation to this source's metadata.
    pub trace: TraceMetadataView,
    /// Bounded missing/malformed evidence descriptions.
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TraceEntry {
    pub trace_id: String,
    pub original_id: String,
    pub raw_headers: Option<String>,
    pub timings: BTreeMap<String, String>,
    pub protocol_known: bool,
    pub target_known: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
struct State {
    index_bytes: usize,
    metadata_bytes: usize,
    metadata: BTreeMap<String, TraceMetadataView>,
    entries: HashMap<String, TraceEntry>,
    operations: HashMap<String, Arc<AtomicBool>>,
}

#[derive(Clone, Default)]
pub(crate) struct TraceRegistry {
    publication: Arc<Mutex<()>>,
    state: Arc<Mutex<State>>,
}
impl TraceRegistry {
    pub(crate) fn source_id(&self, id: &str) -> Option<String> {
        self.lock()
            .entries
            .get(id)
            .map(|entry| entry.trace_id.clone())
    }
    pub(crate) fn entry(&self, id: &str) -> Option<TraceEntry> {
        self.lock().entries.get(id).cloned()
    }
    pub(crate) fn metadata(&self, id: &str) -> Option<TraceMetadataView> {
        self.lock().metadata.get(id).cloned()
    }
    pub(crate) fn list(&self) -> Vec<TraceMetadataView> {
        self.lock().metadata.values().cloned().collect()
    }
    pub(crate) fn cancel(&self, operation_id: &str) {
        if let Some(canceled) = self.lock().operations.get(operation_id) {
            canceled.store(true, Ordering::Release);
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) async fn import(
        &self,
        request: TraceImportRequest,
        service: ApplicationSessionService,
        store: BodyStore,
        progress: Arc<dyn Fn(TraceImportProgress) + Send + Sync>,
    ) -> Result<TraceImportResult, AppError> {
        if request.operation_id.is_empty()
            || request.operation_id.len() > 128
            || request.max_file_bytes == 0
            || request.max_file_bytes > 4 * 1024 * 1024 * 1024
        {
            return Err(invalid("Choose a bounded trace file and import operation"));
        }
        let canceled = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.lock();
            if state.operations.len() >= 4 || state.operations.contains_key(&request.operation_id) {
                return Err(invalid("Another import already uses this operation"));
            }
            state
                .operations
                .insert(request.operation_id.clone(), canceled.clone());
        }
        let registry = self.clone();
        let operation_id = request.operation_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            registry.import_blocking(&request, &service, &store, &canceled, progress.as_ref())
        })
        .await;
        self.lock().operations.remove(&operation_id);
        result.map_err(|_| unavailable("Trace import worker failed"))?
    }

    fn import_blocking(
        &self,
        request: &TraceImportRequest,
        service: &ApplicationSessionService,
        store: &BodyStore,
        canceled: &AtomicBool,
        progress: &(dyn Fn(TraceImportProgress) + Send + Sync),
    ) -> Result<TraceImportResult, AppError> {
        let file = File::open(&request.path)
            .map_err(|_| invalid("The selected trace file is unreadable"))?;
        let bytes = file
            .metadata()
            .map_err(|_| invalid("The selected trace file is unavailable"))?
            .len();
        if bytes > request.max_file_bytes {
            return Err(invalid("The trace exceeds the selected input byte limit"));
        }
        let file = if request
            .path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".tmcap.gz")
        {
            expand_native_gzip(file, request, canceled, progress, bytes)?
        } else {
            file
        };
        let bytes = file
            .metadata()
            .map_err(|_| unavailable("Trace source is unavailable"))?
            .len();
        let reader =
            SourceReader::new(file).map_err(|_| unavailable("Trace source could not be opened"))?;
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)
            .map_err(|_| unavailable("Trace namespace could not be generated"))?;
        let prefix = u64::from_le_bytes(random).max(1);
        let trace_id = format!("trace-{prefix:016x}");
        let name = request.path.file_name().map_or_else(
            || "Capture".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        let report = |completed, total| {
            progress(TraceImportProgress {
                operation_id: request.operation_id.clone(),
                completed,
                total,
            });
        };
        let mut imported = if request
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("saz"))
        {
            import_saz(
                reader,
                prefix,
                &trace_id,
                request.max_file_bytes,
                canceled,
                &report,
            )?
        } else {
            import_native(
                &reader,
                prefix,
                &trace_id,
                bytes,
                canceled,
                &report,
                request.password.as_ref(),
            )?
        };
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        let trace = TraceMetadataView {
            id: trace_id.clone(),
            name,
            format: imported.format,
            path: request.path.clone(),
            sessions: imported.sessions.len(),
            imported_at: millis(SystemTime::now()),
            context: std::mem::take(&mut imported.context),
            notes: bounded_source_notes(
                std::mem::take(&mut imported.notes)
                    .into_iter()
                    .chain(imported.issues.iter().cloned()),
            ),
        };
        self.publish_import(imported, service, store, trace)
    }

    fn publish_import(
        &self,
        imported: ImportData,
        service: &ApplicationSessionService,
        store: &BodyStore,
        trace: TraceMetadataView,
    ) -> Result<TraceImportResult, AppError> {
        let trace_id = trace.id.clone();
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let extra_metadata = std::iter::once(&trace)
            .chain(imported.sources.values())
            .try_fold(0usize, |total, source| {
                serde_json::to_vec(source).map(|bytes| total.saturating_add(bytes.len()))
            })
            .map_err(|_| invalid("Invalid trace source metadata"))?;
        {
            let state = self.lock();
            if state.index_bytes.saturating_add(imported.index_bytes) > MAX_HEAD_BYTES * 2
                || state.metadata_bytes.saturating_add(extra_metadata) > MAX_HEAD_BYTES
                || state
                    .metadata
                    .len()
                    .saturating_add(imported.sources.len())
                    .saturating_add(1)
                    > 10_000
            {
                return Err(invalid(
                    "The viewer's combined trace index limit was reached",
                ));
            }
            if state.metadata.contains_key(&trace.id)
                || imported
                    .sources
                    .keys()
                    .any(|id| state.metadata.contains_key(id))
            {
                return Err(invalid(
                    "Trace source namespace collided; import the file again",
                ));
            }
        }
        // Catalog publication is atomic. All fallible source/index work precedes
        // it, so canceling or a bad file never leaves half an import in Traffic.
        service
            .catalog()
            .import_sessions_with(imported.sessions, || {
                for body in imported.bodies {
                    store.register_saved(
                        body.id,
                        body.boundary,
                        &body.headers,
                        body.response,
                        body.source,
                        body.observed,
                        body.complete,
                    );
                }
                let mut state = self.lock();
                state.index_bytes = state.index_bytes.saturating_add(imported.index_bytes);
                state.metadata_bytes = state.metadata_bytes.saturating_add(extra_metadata);
                state.metadata.extend(imported.sources);
                state.metadata.insert(trace_id, trace.clone());
                state.entries.extend(imported.entries);
            })
            .map_err(|_| invalid("The viewer's imported-entry limit was reached"))?;
        Ok(TraceImportResult {
            trace,
            issues: imported.issues,
        })
    }
}

struct CancellableReader<'a, R> {
    reader: R,
    canceled: &'a AtomicBool,
}
impl<R: std::io::Read> std::io::Read for CancellableReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Trace import canceled"));
        }
        self.reader.read(buffer)
    }
}

fn expand_native_gzip(
    file: File,
    request: &TraceImportRequest,
    canceled: &AtomicBool,
    progress: &(dyn Fn(TraceImportProgress) + Send + Sync),
    bytes: u64,
) -> Result<File, AppError> {
    // Anonymous temporary file disappears with the last saved-body
    // reader. Decompression is bounded, cancellable and CRC checked;
    // bodies remain lazy once the native index is built.
    let mut decoder = flate2::read::MultiGzDecoder::new(CancellableReader {
        reader: file,
        canceled,
    });
    let mut output = tempfile::tempfile()
        .map_err(|_| unavailable("Compressed trace cache could not be created"))?;
    let mut buffer = [0_u8; 16 * 1024];
    let mut expanded = 0_u64;
    loop {
        use std::io::{Read, Seek, Write};
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        let count = decoder.read(&mut buffer).map_err(|_| {
            if canceled.load(Ordering::Acquire) {
                unavailable("Trace import canceled")
            } else {
                invalid("Compressed trace is incomplete or failed its checksum")
            }
        })?;
        if count == 0 {
            break;
        }
        expanded = expanded.saturating_add(count as u64);
        if expanded > request.max_file_bytes {
            return Err(invalid(
                "The expanded trace exceeds the selected input byte limit",
            ));
        }
        output
            .write_all(&buffer[..count])
            .map_err(|_| unavailable("Compressed trace cache could not be written"))?;
        if expanded % (1024 * 1024) < count as u64 {
            progress(TraceImportProgress {
                operation_id: request.operation_id.clone(),
                completed: decoder
                    .get_mut()
                    .reader
                    .stream_position()
                    .unwrap_or(0)
                    .min(bytes),
                total: bytes,
            });
        }
    }
    Ok(output)
}

struct ImportedBody {
    id: ExchangeId,
    boundary: ExchangeBoundary,
    headers: HeaderBlock,
    response: Option<ResponseHead>,
    source: Arc<dyn SavedBodySource>,
    observed: u64,
    complete: bool,
}
struct ImportData {
    index_bytes: usize,
    sources: BTreeMap<String, TraceMetadataView>,
    format: &'static str,
    sessions: Vec<SessionSnapshot>,
    entries: HashMap<String, TraceEntry>,
    bodies: Vec<ImportedBody>,
    context: Value,
    notes: Vec<String>,
    issues: Vec<String>,
}

#[allow(clippy::too_many_lines)]
fn import_saz(
    reader: SourceReader,
    prefix: u64,
    trace_id: &str,
    max_bytes: u64,
    canceled: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<ImportData, AppError> {
    let mut archive = SazArchive::open(
        reader,
        SazImportLimits {
            max_archive_bytes: max_bytes,
            ..SazImportLimits::default()
        },
    )
    .map_err(|error| invalid(&format!("SAZ could not be opened: {error}")))?;
    let index = archive
        .index(
            |done, total| progress(done as u64, total as u64),
            || canceled.load(Ordering::Acquire),
        )
        .map_err(|error| invalid(&format!("SAZ could not be indexed: {error}")))?;
    let mut result = ImportData {
        format: "saz",
        sources: BTreeMap::new(),
        index_bytes: 0,
        sessions: Vec::new(),
        entries: HashMap::new(),
        bodies: Vec::new(),
        context: index.trace_metadata.unwrap_or(Value::Null),
        notes: Vec::new(),
        issues: index
            .issues
            .into_iter()
            .map(|issue| format!("Session {}: {}", issue.source_id, issue.message))
            .collect(),
    };
    if index.additional_issues > 0 {
        result
            .issues
            .push(format!("{} additional issues", index.additional_issues));
    }
    let mut head_bytes = 0_usize;
    let mut omitted_body_issues = 0_usize;
    for source in index.trace_sources {
        head_bytes += serde_json::to_vec(&source)
            .map_err(|_| invalid("Invalid SAZ source"))?
            .len();
        let source = saved_source(source, trace_id)?;
        if result.sources.insert(source.id.clone(), source).is_some() {
            return Err(invalid("Duplicate SAZ source metadata"));
        }
    }
    for (position, mut session) in index.sessions.into_iter().enumerate() {
        let evidence = session.evidence.take();
        if let Some(evidence) = &evidence {
            head_bytes += serde_json::to_vec(evidence)
                .map_err(|_| invalid("Invalid SAZ evidence"))?
                .len();
            if let Some(headers) = &evidence.request_headers
                && let Some(message) = &mut session.request
            {
                message.headers = native_headers(headers.clone())?;
            }
            if let Some(headers) = &evidence.response_headers
                && let Some(message) = &mut session.response
            {
                message.headers = native_headers(headers.clone())?;
            }
        }
        let saved = evidence
            .as_ref()
            .and_then(|evidence| evidence.provenance.clone())
            .map(serde_json::from_value::<SavedEntry>)
            .transpose()
            .map_err(|_| invalid("Invalid SAZ entry association"))?;
        if saved.as_ref().is_some_and(|saved| !saved.valid()) {
            return Err(invalid("SAZ entry association exceeds its limits"));
        }
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        let id = ExchangeId(u128::from(prefix) << 64 | (position as u128 + 1));
        let mut request = session
            .request
            .as_ref()
            .map(|request| request.request_head(&session.metadata))
            .transpose()
            .map_err(|_| invalid("Saved request target is unavailable"))?;
        let mut response = session
            .response
            .as_ref()
            .map(transmog_saz::ArchiveMessage::response_head)
            .transpose()
            .map_err(|_| invalid("Saved response head is unavailable"))?;
        for (boundary, flag, head_version) in [
            (
                "client-request",
                "x-transmog-original-request-protocol",
                request.as_mut().map(|head| &mut head.source_version),
            ),
            (
                "client-response",
                "x-transmog-original-response-protocol",
                response.as_mut().map(|head| &mut head.source_version),
            ),
        ] {
            let version = evidence
                .as_ref()
                .and_then(|evidence| {
                    evidence
                        .performance
                        .protocols
                        .iter()
                        .find(|item| item.boundary == boundary)
                        .map(|item| item.version.as_str())
                })
                .or_else(|| session.metadata.flags.get(flag).map(String::as_str));
            if let (Some(version), Some(head_version)) = (version, head_version) {
                if version.starts_with("HTTP/2") {
                    *head_version = HttpLegVersion::Http2;
                } else if version.starts_with("HTTP/3") {
                    *head_version = HttpLegVersion::Http3;
                }
            }
        }
        let target = request
            .as_ref()
            .map_or_else(unknown_target, |request| request.target.clone());
        let client_ip = session
            .metadata
            .flags
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-clientIP"))
            .and_then(|(_, value)| value.parse::<IpAddr>().ok());
        let client = evidence
            .as_ref()
            .and_then(|evidence| evidence.client_addr.as_ref())
            .and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| {
                client_ip.map_or_else(
                    || "127.0.0.1:0".parse().expect("constant peer"),
                    |ip| SocketAddr::new(ip, 0),
                )
            });
        let started = session
            .metadata
            .timers
            .get("ClientBeginRequest")
            .and_then(|value| timestamp(value))
            .or_else(|| {
                session
                    .metadata
                    .metrics
                    .get("RequestStartedOn")
                    .and_then(|value| timestamp(value))
            })
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let ended = session
            .metadata
            .timers
            .get("ClientDoneResponse")
            .and_then(|value| timestamp(value))
            .or_else(|| {
                session
                    .metadata
                    .metrics
                    .get("SessionFinishedOn")
                    .and_then(|value| timestamp(value))
            });
        let version = request
            .as_ref()
            .map_or(HttpLegVersion::Http1, |request| request.source_version);
        let metadata = metadata(
            id,
            target,
            client,
            if match client.ip() {
                IpAddr::V6(ip) => ip
                    .to_ipv4_mapped()
                    .map_or(ip.is_loopback(), |ip| ip.is_loopback()),
                IpAddr::V4(ip) => ip.is_loopback(),
            } {
                ClientIdentity::default()
            } else {
                ClientIdentity::Remote
            },
            version,
            started,
        );
        let mut snapshot = empty_snapshot(metadata, request.clone(), response.clone(), ended);
        let mut raw_blocks = Vec::new();
        for (message, boundary, response_head) in [
            (session.request, ExchangeBoundary::ClientRequest, None),
            (
                session.response,
                ExchangeBoundary::ClientResponse,
                response.clone(),
            ),
        ] {
            let Some(message) = message else {
                continue;
            };
            let version = if boundary == ExchangeBoundary::ClientRequest {
                message.start_line.split_ascii_whitespace().last()
            } else {
                message.start_line.split_ascii_whitespace().next()
            };
            if let Some(version) = version {
                let flag = if boundary == ExchangeBoundary::ClientRequest {
                    "x-transmog-original-request-protocol"
                } else {
                    "x-transmog-original-response-protocol"
                };
                let version = session
                    .metadata
                    .flags
                    .get(flag)
                    .filter(|value| {
                        matches!(
                            value.as_str(),
                            "HTTP/1.0"
                                | "HTTP/1.1"
                                | "HTTP/2"
                                | "HTTP/2.0"
                                | "HTTP/3"
                                | "HTTP/3.0"
                                | "unavailable"
                        )
                    })
                    .map_or(version, String::as_str);
                if version != "unavailable" {
                    snapshot.performance.protocols.push(
                        transmog_core::performance::ProtocolObservation {
                            boundary: crate::inspector::boundary(boundary),
                            version: version.into(),
                            reason: (boundary == ExchangeBoundary::ClientResponse).then(|| {
                                message
                                    .start_line
                                    .splitn(3, ' ')
                                    .nth(2)
                                    .unwrap_or("")
                                    .to_owned()
                            }),
                        },
                    );
                }
            }
            head_bytes = head_bytes.saturating_add(message.raw_head.len());
            if head_bytes > MAX_HEAD_BYTES {
                return Err(invalid("Saved trace headers exceed the viewer index limit"));
            }
            if let Ok(raw) = std::str::from_utf8(&message.raw_head) {
                raw_blocks.push(raw.trim_end_matches(['\r', '\n']).to_owned());
            }
            let (observed, mut trailers, mut complete) = if message.body.chunked
                && !message.body.dropped
            {
                match archive.copy_body_with_trailers(&message.body, &mut std::io::sink(), || {
                    canceled.load(Ordering::Acquire)
                }) {
                    Ok((bytes, trailers)) => (bytes, Some(trailers), true),
                    Err(_) if canceled.load(Ordering::Acquire) => {
                        return Err(unavailable("Trace import canceled"));
                    }
                    Err(_) => {
                        if result.issues.len() < 512 {
                            result.issues.push(format!(
                                "Session {}: chunked body or checksum unavailable",
                                session.source_id
                            ));
                        } else {
                            omitted_body_issues += 1;
                        }
                        (message.body.wire_bytes, None, false)
                    }
                }
            } else {
                (message.body.wire_bytes, None, !message.body.dropped)
            };
            if let Some(evidence) = &evidence {
                let fields = if boundary == ExchangeBoundary::ClientRequest {
                    &evidence.request_trailers
                } else {
                    &evidence.response_trailers
                };
                if !fields.is_empty() {
                    trailers = Some(native_headers(fields.clone())?);
                }
            }
            complete &= !message.body.dropped;
            if let Some(trailers) = &trailers {
                charge_trailers(&mut head_bytes, trailers)?;
            }
            snapshot.bodies.push(BodySnapshot {
                boundary,
                observed_bytes: observed,
                retained_prefix: bytes::Bytes::new(),
                truncated: !complete,
                trailers,
            });
            result.bodies.push(ImportedBody {
                id,
                boundary,
                headers: message.headers,
                response: response_head,
                observed,
                complete,
                source: Arc::new(SazBodySource::new(
                    archive.clone(),
                    message.body,
                    complete.then_some(observed),
                )),
            });
        }
        if let Some(evidence) = &evidence {
            snapshot.performance.merge(&evidence.performance);
        }
        let mut timings = session.metadata.timers;
        timings.extend(session.metadata.metrics);
        let entry = if let Some(mut saved) = saved {
            let source = source_namespace(trace_id, &saved.source.trace_id);
            if !result.sources.contains_key(&source) {
                return Err(invalid("SAZ entry refers to missing source metadata"));
            }
            saved.source.trace_id = source;
            saved.source
        } else {
            TraceEntry {
                trace_id: trace_id.into(),
                original_id: evidence
                    .as_ref()
                    .and_then(|evidence| evidence.native_exchange_id.clone())
                    .unwrap_or(session.source_id),
                raw_headers: (evidence.is_none()
                    && !session
                        .metadata
                        .flags
                        .keys()
                        .any(|name| name.starts_with("x-transmog-original-"))
                    && !raw_blocks.is_empty())
                .then(|| raw_blocks.join("\r\n\r\n\r\n")),
                timings,
                protocol_known: snapshot
                    .performance
                    .protocols
                    .iter()
                    .any(|item| item.boundary == "client-request"),
                target_known: request.is_some(),
                diagnostics: Vec::new(),
            }
        };
        result.entries.insert(format!("{:032x}", id.0), entry);
        result.sessions.push(snapshot);
    }
    if omitted_body_issues > 0 {
        result.notes.push(format!(
            "{omitted_body_issues} additional chunked body issues omitted."
        ));
    }
    result.index_bytes = head_bytes;
    Ok(result)
}

#[derive(Default)]
struct NativeSession {
    provenance: Option<SavedEntry>,
    performance_bytes: usize,
    performance: transmog_core::performance::PerformanceEvidence,
    client: Option<SocketAddr>,
    caller: ClientIdentity,
    started: Option<SystemTime>,
    requests: Vec<ObservedRequestHead>,
    responses: Vec<ObservedResponseHead>,
    bodies: BTreeMap<String, NativeBody>,
    completed: bool,
    failed: bool,
    loss: u64,
    sequence: u64,
    diagnostics: Vec<String>,
}
#[derive(Default)]
struct NativeBody {
    pieces: Vec<NativeBodyPiece>,
    observed: u64,
    retained: u64,
    incomplete: bool,
    framing_complete: bool,
    trailers: Option<HeaderBlock>,
}

fn import_native(
    reader: &SourceReader,
    prefix: u64,
    trace_id: &str,
    max_bytes: u64,
    canceled: &AtomicBool,
    progress: &dyn Fn(u64, u64),
    password: Option<&transmog_capture::CapturePassword>,
) -> Result<ImportData, AppError> {
    let mut capture = CaptureReader::with_password(
        reader.clone(),
        CaptureLimits {
            max_file_bytes: max_bytes,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: 10_000_000,
        },
        password,
    )
    .map_err(|error| invalid(&error.to_string()))?;
    let mut rows: BTreeMap<u128, NativeSession> = BTreeMap::new();
    let mut context = Value::Null;
    let mut sources = BTreeMap::new();
    let mut budget = 0_usize;
    while let Some(frame) = capture
        .read_next()
        .map_err(|_| invalid("The native capture contains a corrupt or oversized frame"))?
    {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        if frame.record.exchange_id == 0 {
            if let CaptureRecordKind::Unknown { kind, payload } = frame.record.kind {
                let bytes = serde_json::to_vec(&payload)
                    .map_err(|_| invalid("Invalid trace metadata"))?
                    .len();
                budget = budget.saturating_add(bytes);
                if budget > MAX_HEAD_BYTES {
                    return Err(invalid("Trace source metadata exceeds its index limit"));
                }
                if kind == "trace-metadata" {
                    context = payload;
                } else if kind == "trace-source" {
                    let source = saved_source(payload, trace_id)?;
                    if sources.len() >= MAX_SESSIONS
                        || sources.insert(source.id.clone(), source).is_some()
                    {
                        return Err(invalid(
                            "Saved trace contains duplicate or excessive sources",
                        ));
                    }
                }
            }
            progress(capture.valid_bytes(), max_bytes);
            continue;
        }
        let row = rows.entry(frame.record.exchange_id).or_default();
        row.sequence = row.sequence.max(frame.record.sequence);
        apply_native_frame(row, frame, &mut budget)?;
        if rows.len() > MAX_SESSIONS || budget > MAX_HEAD_BYTES {
            return Err(invalid(
                "Native trace metadata exceeds the viewer index limit",
            ));
        }
        progress(capture.valid_bytes(), max_bytes);
    }
    let mut result = ImportData {
        format: "native",
        sources: BTreeMap::new(),
        index_bytes: 0,
        sessions: Vec::new(),
        bodies: Vec::new(),
        entries: HashMap::new(),
        context,
        notes: vec![],
        issues: Vec::new(),
    };
    result.sources = sources;
    result.index_bytes = budget;
    if capture.truncated_tail() {
        result.notes.push(
            "Recovered the valid prefix of an interrupted file. The original file is unchanged."
                .into(),
        );
    }
    if !capture.sealed() {
        result
            .notes
            .push("The source did not have a complete, consistent seal.".into());
    }
    for (position, (original, row)) in rows.into_iter().enumerate() {
        let id = ExchangeId(u128::from(prefix) << 64 | (position as u128 + 1));
        append_native(&mut result, reader, trace_id, id, original, row)?;
    }
    Ok(result)
}

#[allow(clippy::too_many_lines)]
fn apply_native_frame(
    row: &mut NativeSession,
    frame: transmog_capture::CaptureFrame,
    budget: &mut usize,
) -> Result<(), AppError> {
    let old_diagnostics = row.diagnostics.len();
    match frame.record.kind {
        CaptureRecordKind::Unknown { kind, payload } if kind == "entry-provenance" => {
            let bytes = serde_json::to_vec(&payload)
                .map_err(|_| invalid("Invalid entry provenance"))?
                .len();
            let provenance: SavedEntry = serde_json::from_value(payload)
                .map_err(|_| invalid("Invalid saved entry provenance"))?;
            if row.provenance.is_some() || !provenance.valid() {
                return Err(invalid(
                    "Saved entry provenance exceeds its limits or is repeated",
                ));
            }
            *budget = budget.saturating_add(bytes);
            row.provenance = Some(provenance);
        }
        CaptureRecordKind::Performance(evidence) => {
            if !evidence.valid() {
                return Err(invalid(
                    "Saved performance evidence exceeds its limits or has invalid fields",
                ));
            }
            row.performance.merge(&evidence);
            let bytes = serde_json::to_vec(&row.performance)
                .map_err(|_| invalid("Invalid saved performance evidence"))?
                .len();
            *budget = budget
                .saturating_sub(row.performance_bytes)
                .saturating_add(bytes);
            row.performance_bytes = bytes;
        }
        CaptureRecordKind::ExchangeStarted {
            client_addr,
            client_identity,
            started_unix_nanos,
            ..
        } => {
            row.client = client_addr.parse().ok();
            row.caller = client_identity;
            row.started = u64::try_from(started_unix_nanos)
                .ok()
                .and_then(|nanos| SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(nanos)));
        }
        CaptureRecordKind::RequestHead {
            boundary,
            method,
            target,
            headers,
        } => {
            *budget = budget.saturating_add(
                headers
                    .iter()
                    .map(|header| header.name.len() + header.value.as_ref().map_or(0, Vec::len))
                    .sum::<usize>(),
            );
            let head = native_request(&method, &target, headers)?;
            row.requests.retain(|request| request.boundary != boundary);
            row.requests.push(ObservedRequestHead { boundary, head });
        }
        CaptureRecordKind::ResponseHead {
            boundary,
            status,
            headers,
        } => {
            *budget = budget.saturating_add(
                headers
                    .iter()
                    .map(|header| header.name.len() + header.value.as_ref().map_or(0, Vec::len))
                    .sum::<usize>(),
            );
            let head = ResponseHead {
                status,
                headers: native_headers(headers)?,
                source_version: HttpLegVersion::Http1,
            };
            row.responses
                .retain(|response| response.boundary != boundary);
            row.responses.push(ObservedResponseHead { boundary, head });
        }
        CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes,
            truncated,
        } => {
            let body = row
                .bodies
                .entry(crate::inspector::boundary(boundary))
                .or_default();
            body.observed = body.observed.saturating_add(byte_count as u64);
            body.incomplete |= truncated
                || bytes.as_ref().is_none_or(|bytes| bytes.len() != byte_count) && byte_count > 0;
            if let Some(bytes) = bytes
                && !bytes.is_empty()
            {
                body.retained += bytes.len() as u64;
                body.pieces.push(NativeBodyPiece {
                    offset: frame.offset,
                    index: frame.index,
                    codec: frame.codec.clone(),
                    frame_bytes: frame.frame_bytes,
                    digest: Sha256::digest(bytes).into(),
                });
                *budget = budget.saturating_add(std::mem::size_of::<NativeBodyPiece>());
            }
        }
        CaptureRecordKind::Completed => row.completed = true,
        CaptureRecordKind::Failed { category, message } => {
            row.failed = true;
            if row.diagnostics.len() < 64 {
                row.diagnostics
                    .push(format!("Saved failure ({category}): {message}"));
            }
        }
        CaptureRecordKind::Trailers { boundary, headers } => {
            let trailers = native_headers(headers)?;
            charge_trailers(budget, &trailers)?;
            row.bodies
                .entry(crate::inspector::boundary(boundary))
                .or_default()
                .trailers = Some(trailers);
        }
        CaptureRecordKind::HookEffect {
            hook_name,
            phase,
            action,
            changed,
            ..
        } => {
            if row.diagnostics.len() < 64 {
                row.diagnostics.push(format!(
                    "Saved hook {hook_name} / {phase}: {action} (changed: {changed})"
                ));
            }
        }
        CaptureRecordKind::RouteSelected { policy_id, reason } => {
            if row.diagnostics.len() < 64 {
                row.diagnostics
                    .push(format!("Saved route {policy_id}: {reason}"));
            }
        }
        CaptureRecordKind::RouteAttempt { protocol, outcome } => {
            if row.diagnostics.len() < 64 {
                row.diagnostics
                    .push(format!("Saved upstream attempt {protocol}: {outcome}"));
            }
        }
        CaptureRecordKind::Loss { count, .. } => row.loss = row.loss.saturating_add(count),
        _ => {}
    }
    row.diagnostics.truncate(64);
    for value in row.diagnostics.iter_mut().skip(old_diagnostics) {
        *value = value.chars().take(2048).collect();
        *budget = budget.saturating_add(value.len());
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn append_native(
    result: &mut ImportData,
    reader: &SourceReader,
    trace_id: &str,
    id: ExchangeId,
    original: u128,
    mut row: NativeSession,
) -> Result<(), AppError> {
    let protocol_known = row
        .performance
        .protocols
        .iter()
        .any(|item| item.boundary == "client-request");
    let ingress = row
        .performance
        .protocols
        .iter()
        .find(|item| item.boundary == "client-request")
        .map_or(HttpLegVersion::Http1, |item| {
            if item.version.starts_with("HTTP/2") {
                HttpLegVersion::Http2
            } else if item.version.starts_with("HTTP/3") {
                HttpLegVersion::Http3
            } else {
                HttpLegVersion::Http1
            }
        });
    if !protocol_known
        && !result
            .notes
            .iter()
            .any(|note| note.starts_with("Some native entries"))
    {
        result.notes.push("Some native entries predate timing and protocol retention; unavailable fields are left blank.".into());
    }
    let request = row
        .requests
        .iter()
        .find(|head| head.boundary == ExchangeBoundary::ClientRequest)
        .or_else(|| row.requests.first())
        .map(|head| head.head.clone());
    let response = row
        .responses
        .iter()
        .find(|head| head.boundary == ExchangeBoundary::ClientResponse)
        .or_else(|| row.responses.last())
        .map(|head| head.head.clone());
    let target = request
        .as_ref()
        .map_or_else(unknown_target, |head| head.target.clone());
    let metadata = metadata(
        id,
        target,
        row.client
            .unwrap_or_else(|| "127.0.0.1:0".parse().expect("constant peer")),
        row.caller,
        ingress,
        row.started.unwrap_or(SystemTime::UNIX_EPOCH),
    );
    let mut snapshot = empty_snapshot(metadata, request.clone(), response.clone(), None);
    snapshot.request_heads = row.requests;
    snapshot.response_heads = row.responses;
    snapshot.sequence_loss = row.loss;
    snapshot.last_sequence = row.sequence;
    snapshot.terminal_at = row
        .performance
        .points
        .iter()
        .find(|point| point.milestone == transmog_core::performance::Milestone::ExchangeDone)
        .and_then(|point| {
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(point.unix_millis))
        });
    if snapshot.terminal_at.is_none() {
        snapshot.terminal_at = row
            .provenance
            .as_ref()
            .and_then(|saved| saved.terminal_unix_millis)
            .and_then(|millis| SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(millis)));
    }
    snapshot.performance = row.performance;
    if !row.completed || row.failed {
        snapshot.terminal = Some(import_failure(snapshot.metadata.clone()));
    }
    // A completed empty body often has no segment record. Preserve that
    // case, but do not manufacture a complete empty body when a declared
    // entity was omitted from the saved file.
    for (boundary, headers, bodyless) in snapshot
        .request_heads
        .iter()
        .map(|head| (head.boundary, &head.head.headers, false))
        .chain(snapshot.response_heads.iter().map(|head| {
            (
                head.boundary,
                &head.head.headers,
                request
                    .as_ref()
                    .is_some_and(|request| request.method == "HEAD")
                    || (100..200).contains(&head.head.status)
                    || [204, 304].contains(&head.head.status),
            )
        }))
    {
        let body = row
            .bodies
            .entry(crate::inspector::boundary(boundary))
            .or_default();
        let lengths = headers
            .values("content-length")
            .map(|value| {
                std::str::from_utf8(value)
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
            })
            .collect::<Vec<_>>();
        body.framing_complete = bodyless
            || !lengths.is_empty() && lengths.iter().all(|length| *length == Some(body.observed));
        body.incomplete |= !bodyless && !lengths.is_empty() && !body.framing_complete;
    }
    for (name, body) in row.bodies {
        let boundary = parse_boundary(&name)?;
        let response_head = snapshot
            .response_heads
            .iter()
            .find(|head| head.boundary == boundary)
            .map(|head| head.head.clone());
        let headers = response_head
            .as_ref()
            .map(|head| head.headers.clone())
            .or_else(|| {
                snapshot
                    .request_heads
                    .iter()
                    .find(|head| head.boundary == boundary)
                    .map(|head| head.head.headers.clone())
            })
            .unwrap_or_default();
        let complete = (row.completed && !row.failed || body.framing_complete)
            && !body.incomplete
            && row.loss == 0;
        snapshot.bodies.push(BodySnapshot {
            boundary,
            observed_bytes: body.observed,
            retained_prefix: bytes::Bytes::new(),
            truncated: !complete,
            trailers: body.trailers,
        });
        result.bodies.push(ImportedBody {
            id,
            boundary,
            headers,
            response: response_head,
            source: Arc::new(NativeBodySource {
                reader: reader.clone(),
                pieces: Arc::from(body.pieces),
                bytes: body.retained,
            }),
            observed: body.observed,
            complete,
        });
    }
    let entry = if let Some(mut saved) = row.provenance {
        let source = source_namespace(trace_id, &saved.source.trace_id);
        if !result.sources.contains_key(&source) {
            return Err(invalid("Saved entry refers to missing source metadata"));
        }
        saved.source.trace_id = source;
        saved.source
    } else {
        TraceEntry {
            trace_id: trace_id.into(),
            original_id: format!("{original:032x}"),
            raw_headers: None,
            timings: BTreeMap::new(),
            protocol_known,
            target_known: request.is_some(),
            diagnostics: row.diagnostics,
        }
    };
    result.entries.insert(format!("{:032x}", id.0), entry);
    result.sessions.push(snapshot);
    Ok(())
}

fn native_headers(headers: Vec<CapturedHeader>) -> Result<HeaderBlock, AppError> {
    Ok(HeaderBlock::from_fields(
        headers
            .into_iter()
            .map(|header| match header.value {
                Some(value) => HeaderField::try_new(header.name, value),
                None => HeaderField::from_redacted(header.name, header.original_value_bytes),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid("Saved trace contains an invalid header"))?,
    ))
}

fn native_request(
    method: &str,
    target: &str,
    headers: Vec<CapturedHeader>,
) -> Result<RequestHead, AppError> {
    let message = transmog_saz::ArchiveMessage {
        start_line: format!("{method} {target} HTTP/1.1"),
        headers: native_headers(headers)?,
        raw_head: Vec::new(),
        body: transmog_saz::ArchiveBody {
            member: 0,
            offset: 0,
            wire_bytes: 0,
            chunked: false,
            dropped: false,
        },
    };
    message
        .request_head(&transmog_saz::ArchiveMetadata::default())
        .map_err(|_| invalid("Saved trace contains an invalid request target"))
}

fn metadata(
    id: ExchangeId,
    target: Target,
    client: SocketAddr,
    caller: ClientIdentity,
    version: HttpLegVersion,
    started: SystemTime,
) -> Arc<ExchangeMetadata> {
    Arc::new(ExchangeMetadata::from_session_at(
        &SessionMetadata {
            session_id: SessionId(id.0),
            downstream_connection_id: ConnectionId(id.0),
            stream_id: StreamId(id.0),
            client_addr: client,
            client_identity: caller,
            proxy_addr: "127.0.0.1:0".parse().expect("constant listener"),
            ingress_version: version,
            egress_version: None,
        },
        target,
        started,
    ))
}
fn empty_snapshot(
    metadata: Arc<ExchangeMetadata>,
    request: Option<RequestHead>,
    response: Option<ResponseHead>,
    ended: Option<SystemTime>,
) -> SessionSnapshot {
    let terminal = if let (Some(request), Some(response)) = (&request, &response) {
        SessionTerminal::Completed(CompletedExchange {
            metadata: metadata.clone(),
            request_head: request.clone(),
            response_head: response.clone(),
        })
    } else {
        import_failure(metadata.clone())
    };
    SessionSnapshot {
        imported: true,
        performance: transmog_core::performance::PerformanceEvidence::default(),
        exchange_id: metadata.exchange_id,
        metadata,
        last_sequence: 0,
        sequence_loss: 0,
        request_heads: request
            .into_iter()
            .map(|head| ObservedRequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                head,
            })
            .collect(),
        response_heads: response
            .into_iter()
            .map(|head| ObservedResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                head,
            })
            .collect(),
        bodies: Vec::new(),
        initialization_diagnostics: Vec::new(),
        hook_effects: Vec::new(),
        route_selection: None,
        route_attempts: Vec::new(),
        terminal: Some(terminal),
        terminal_at: ended,
        websocket: None,
    }
}
fn import_failure(metadata: Arc<ExchangeMetadata>) -> SessionTerminal {
    SessionTerminal::Failed(ExchangeFailure {
        metadata,
        stage: ExchangeStage::Terminal,
        kind: ExchangeFailureKind::Body,
        request_committed: false,
        response_committed: false,
        message: "Imported session is incomplete".into(),
    })
}
fn unknown_target() -> Target {
    Target {
        scheme: String::new(),
        authority: String::new(),
        host: "Unknown target".into(),
        port: 0,
        path: String::new(),
        query: None,
    }
}
fn parse_boundary(value: &str) -> Result<ExchangeBoundary, AppError> {
    match value {
        "client-request" => Ok(ExchangeBoundary::ClientRequest),
        "upstream-request" => Ok(ExchangeBoundary::UpstreamRequest),
        "client-response" => Ok(ExchangeBoundary::ClientResponse),
        "upstream-response" => Ok(ExchangeBoundary::UpstreamResponse),
        _ => Err(invalid("Unknown saved body boundary")),
    }
}
fn timestamp(value: &str) -> Option<SystemTime> {
    let time =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()?;
    let nanos = u64::try_from(time.unix_timestamp_nanos()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(nanos))
}
fn millis(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn bounded_source_notes(notes: impl Iterator<Item = String>) -> Vec<String> {
    let mut result = Vec::new();
    let mut omitted = 0_usize;
    for note in notes {
        if result.len() < 127 {
            result.push(note.chars().take(2048).collect());
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        result.push(format!("{omitted} additional source notes omitted; inspect individual entries for missing evidence."));
    }
    result
}
fn charge_trailers(budget: &mut usize, trailers: &HeaderBlock) -> Result<(), AppError> {
    for field in trailers.iter() {
        *budget = budget
            .saturating_add(64)
            .saturating_add(field.name().len())
            .saturating_add(field.value().len());
    }
    if *budget > MAX_HEAD_BYTES {
        return Err(invalid(
            "Saved trace trailers exceed the viewer index limit",
        ));
    }
    Ok(())
}

fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedEntry {
    source: TraceEntry,
    terminal_unix_millis: Option<u64>,
}
impl SavedEntry {
    fn valid(&self) -> bool {
        let source = &self.source;
        source.trace_id.len() <= 256
            && source.original_id.len() <= 128
            && source
                .raw_headers
                .as_ref()
                .is_none_or(|head| head.len() <= 2 * 1024 * 1024)
            && source.timings.len() <= 512
            && source
                .timings
                .iter()
                .all(|(name, value)| name.len() <= 256 && value.len() <= 8192)
            && source.diagnostics.len() <= 64
            && source.diagnostics.iter().all(|note| note.len() <= 8192)
            && self
                .terminal_unix_millis
                .is_none_or(|millis| millis <= 253_402_300_799_999)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSource {
    id: String,
    name: String,
    format: String,
    path: PathBuf,
    sessions: usize,
    imported_at: u64,
    context: Value,
    notes: Vec<String>,
}
fn source_namespace(parent: &str, source: &str) -> String {
    format!(
        "{parent}-{:016x}",
        u64::from_le_bytes(
            Sha256::digest(source.as_bytes())[..8]
                .try_into()
                .expect("digest length")
        )
    )
}
fn saved_source(value: Value, parent: &str) -> Result<TraceMetadataView, AppError> {
    let source: SavedSource =
        serde_json::from_value(value).map_err(|_| invalid("Invalid saved trace source"))?;
    if source.id.len() > 256
        || source.name.len() > 1024
        || source.path.as_os_str().len() > 32768
        || source.sessions > MAX_SESSIONS
        || source.notes.len() > 128
        || source.notes.iter().any(|note| note.len() > 8192)
    {
        return Err(invalid("Saved source metadata exceeds its limits"));
    }
    let format = match source.format.as_str() {
        "saz" => "saz",
        "native" => "native",
        "live" => "live",
        _ => return Err(invalid("Unknown saved source format")),
    };
    Ok(TraceMetadataView {
        id: source_namespace(parent, &source.id),
        name: source.name,
        format,
        path: source.path,
        sessions: source.sessions,
        imported_at: source.imported_at,
        context: source.context,
        notes: source.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, Application, BodyStoreConfig, RequestCommandFormat, SessionQueryInput};
    use std::io::{Cursor, Write};
    use transmog_capture::{CaptureRecord, CaptureWriter};

    #[test]
    fn compressed_import_can_cancel_while_consuming_empty_members() {
        use std::io::Read;
        struct CancelAfterRead<'a> {
            input: Cursor<Vec<u8>>,
            canceled: &'a AtomicBool,
        }
        impl Read for CancelAfterRead<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let count = self.input.read(bytes)?;
                self.canceled.store(true, Ordering::Release);
                Ok(count)
            }
        }
        let member = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast())
            .finish()
            .unwrap();
        let canceled = AtomicBool::new(false);
        let reader = CancellableReader {
            reader: CancelAfterRead {
                input: Cursor::new(member.repeat(1024)),
                canceled: &canceled,
            },
            canceled: &canceled,
        };
        let mut decoder = flate2::read::MultiGzDecoder::new(reader);
        assert!(decoder.read(&mut [0_u8; 1]).is_err());
        assert!(canceled.load(Ordering::Acquire));
    }

    fn app(root: &std::path::Path) -> Application {
        Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.join("cache"))),
            ..AppConfig::default()
        })
        .unwrap()
    }
    fn request(path: PathBuf, operation_id: &str) -> TraceImportRequest {
        TraceImportRequest {
            password: None,
            path,
            operation_id: operation_id.into(),
            max_file_bytes: 64 * 1024 * 1024,
        }
    }
    fn native(path: &std::path::Path, bytes: Option<Vec<u8>>, count: usize) {
        let mut writer =
            CaptureWriter::new(File::create(path).unwrap(), CaptureLimits::default()).unwrap();
        let kinds = [
            CaptureRecordKind::ExchangeStarted {
                client_addr: "192.0.2.5:1234".into(),
                client_identity: ClientIdentity::Remote,
                listener_addr: "0.0.0.0:8888".into(),
                authority: "example.invalid".into(),
                started_unix_nanos: 1_800_000_000_000_000_000,
            },
            CaptureRecordKind::RequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                method: "POST".into(),
                target: "https://example.invalid/path".into(),
                headers: vec![
                    CapturedHeader {
                        name: "Content-Length".into(),
                        value: Some(count.to_string().into_bytes()),
                        original_value_bytes: None,
                    },
                    CapturedHeader {
                        name: "Authorization".into(),
                        value: None,
                        original_value_bytes: None,
                    },
                ],
            },
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 204,
                headers: Vec::new(),
            },
            CaptureRecordKind::BodySegment {
                boundary: ExchangeBoundary::ClientRequest,
                byte_count: count,
                bytes,
                truncated: false,
            },
            CaptureRecordKind::Completed,
        ];
        writer
            .append(&CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: CaptureRecordKind::Unknown {
                    kind: "trace-metadata".into(),
                    payload: serde_json::json!({"networkContext":"original machine"}),
                },
            })
            .unwrap();
        for (index, kind) in kinds.into_iter().enumerate() {
            writer
                .append(&CaptureRecord {
                    sequence: index as u64 + 1,
                    exchange_id: 7,
                    kind,
                })
                .unwrap();
        }
        writer.seal().unwrap();
    }
    fn saz(path: &std::path::Path) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, value) in [
            (
                "raw/1_c.txt",
                "POST https://example.invalid/path HTTP/1.1\r\nCookie: secret\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
            ),
            (
                "raw/1_s.txt",
                "HTTP/1.1 404 Custom reason\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello",
            ),
            (
                "raw/1_m.xml",
                "<Session BitFlags='1'><SessionTimers ClientBeginRequest='2026-10-08T10:00:00Z' ClientDoneResponse='2026-10-08T10:00:00.250Z' DNSLookupTime='12'/><SessionFlags><SessionFlag N='x-clientIP' V='192.0.2.8'/></SessionFlags></Session>",
            ),
        ] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(value.as_bytes()).unwrap();
        }
        std::fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
    }

    #[tokio::test]
    async fn measured_native_import_restores_protocols_reason_and_submillisecond_duration() {
        use transmog_core::performance::{
            Milestone, PerformanceEvidence, ProtocolObservation, TimingPoint,
        };
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old.tmcap");
        native(&old, Some(vec![1]), 1);
        let recovered =
            transmog_capture::recover(File::open(&old).unwrap(), CaptureLimits::default()).unwrap();
        let path = root.path().join("measured.tmcap");
        let mut writer =
            CaptureWriter::new(File::create(&path).unwrap(), CaptureLimits::default()).unwrap();
        for record in recovered
            .records
            .iter()
            .filter(|record| !matches!(record.kind, CaptureRecordKind::Seal { .. }))
        {
            writer.append(record).unwrap();
        }
        writer
            .append(&CaptureRecord {
                sequence: 6,
                exchange_id: 7,
                kind: CaptureRecordKind::Performance(PerformanceEvidence {
                    work: Vec::new(),
                    points: vec![
                        TimingPoint {
                            milestone: Milestone::RequestHeaders,
                            unix_millis: 1_800_000_000_000,
                            offset_micros: 0,
                        },
                        TimingPoint {
                            milestone: Milestone::ExchangeDone,
                            unix_millis: 1_800_000_000_000,
                            offset_micros: 550,
                        },
                    ],
                    protocols: vec![
                        ProtocolObservation {
                            boundary: "client-request".into(),
                            version: "HTTP/1.0".into(),
                            reason: None,
                        },
                        ProtocolObservation {
                            boundary: "client-response".into(),
                            version: "HTTP/1.0".into(),
                            reason: Some("Custom phrase".into()),
                        },
                    ],
                    transports: vec![],
                }),
            })
            .unwrap();
        writer.seal().unwrap();
        let application = app(root.path());
        application
            .import_trace(request(path, "measured"), Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = application
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert_eq!(rows[0].protocol, "HTTP/1.0");
        assert_eq!(rows[0].duration_ms, Some(0));
        let detail = application.session_detail(&rows[0].id).unwrap();
        assert_eq!(detail.performance.elapsed_micros(), Some(550));
        assert_eq!(detail.responses[0].protocol, "HTTP/1.0");
        let command = application
            .request_command(&rows[0].id, RequestCommandFormat::Curl)
            .unwrap();
        assert!(command.text.contains("--http1.0"));
        #[cfg(windows)]
        assert!(
            application
                .request_command(&rows[0].id, RequestCommandFormat::Powershell)
                .unwrap()
                .text
                .contains("[Version]'1.0'")
        );
        assert!(
            application
                .copy_all_headers(&rows[0].id)
                .unwrap()
                .contains("HTTP/1.0 204 Custom phrase")
        );
    }

    #[tokio::test]
    async fn redacted_export_omits_raw_secrets_and_preserves_retained_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.saz");
        saz(&source);
        let original = std::fs::read(&source).unwrap();
        let workspace = app(root.path());
        workspace
            .import_trace(request(source.clone(), "privacy"), Arc::new(|_| {}))
            .await
            .unwrap();
        let id = workspace
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions[0]
            .id
            .clone();
        assert!(
            workspace
                .copy_all_headers(&id)
                .unwrap()
                .contains("Cookie: secret")
        );
        let destination = root.path().join("redacted.tmcap");
        workspace
            .save_traffic_trace(
                destination.clone(),
                crate::TraceSaveOptions {
                    password: None,
                    redact_sensitive_headers: true,
                    include_network_context: false,
                },
            )
            .await
            .unwrap();
        assert!(
            !std::fs::read(&destination)
                .unwrap()
                .windows(6)
                .any(|bytes| bytes == b"secret")
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(
            workspace
                .copy_all_headers(&id)
                .unwrap()
                .contains("Cookie: secret")
        );
        let reopened = app(&root.path().join("reopened"));
        reopened
            .import_trace(request(destination, "redacted"), Arc::new(|_| {}))
            .await
            .unwrap();
        let id = reopened
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions[0]
            .id
            .clone();
        assert!(!reopened.copy_all_headers(&id).unwrap().contains("secret"));
        let detail = reopened.session_detail(&id).unwrap();
        assert!(
            detail.requests[0]
                .headers
                .iter()
                .any(|field| field.name == "Cookie" && field.sensitive)
        );
    }

    #[tokio::test]
    async fn saved_merged_workspace_keeps_original_sources_bodies_and_duration() {
        let root = tempfile::tempdir().unwrap();
        let native_path = root.path().join("original.tmcap");
        let saz_path = root.path().join("original.saz");
        native(&native_path, Some(vec![0, 255, 7]), 3);
        saz(&saz_path);
        let workspace = app(root.path());
        workspace
            .import_trace(request(native_path, "native"), Arc::new(|_| {}))
            .await
            .unwrap();
        workspace
            .import_trace(request(saz_path, "saz"), Arc::new(|_| {}))
            .await
            .unwrap();
        let path = root.path().join("merged.tmcap.gz");
        let saved = workspace
            .save_traffic_trace(
                path.clone(),
                crate::TraceSaveOptions {
                    password: None,
                    redact_sensitive_headers: false,
                    include_network_context: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(saved.entries, 2);
        assert_eq!(saved.incomplete_bodies, 0);
        let reopened = app(&root.path().join("reopened"));
        reopened
            .import_trace(request(path, "merged"), Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = reopened
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert_eq!(rows.len(), 2);
        assert_eq!(reopened.trace_metadata_list().len(), 3);
        let native_row = rows
            .iter()
            .find(|row| {
                reopened
                    .trace_metadata(row.trace_id.as_deref().unwrap())
                    .unwrap()
                    .name
                    == "original.tmcap"
            })
            .unwrap();
        let native_context = reopened
            .trace_metadata(native_row.trace_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(native_context.context["networkContext"], "original machine");
        assert_eq!(
            reopened
                .session_detail(&native_row.id)
                .unwrap()
                .original_id
                .as_deref(),
            Some("00000000000000000000000000000007")
        );
        let saz_row = rows
            .iter()
            .find(|row| {
                reopened
                    .trace_metadata(row.trace_id.as_deref().unwrap())
                    .unwrap()
                    .name
                    == "original.saz"
            })
            .unwrap();
        assert_eq!(saz_row.duration_ms, Some(250));
        assert!(
            reopened
                .copy_all_headers(&saz_row.id)
                .unwrap()
                .contains("404 Custom reason")
        );
        assert_eq!(
            reopened.composer_source(&saz_row.id).unwrap().body,
            "616263"
        );
    }

    #[test]
    fn trailer_metadata_is_charged_before_publication() {
        let trailers = HeaderBlock::from_fields(vec![
            HeaderField::try_new(b"X-Trailer".to_vec(), b"value".to_vec()).unwrap(),
        ]);
        let mut budget = MAX_HEAD_BYTES - 4;
        assert!(charge_trailers(&mut budget, &trailers).is_err());
    }
    #[tokio::test]
    async fn warning_heavy_imports_keep_bounded_reopenable_source_notes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("warnings.saz");
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for id in 1..=140 {
            zip.start_file(
                format!("raw/{id}_c.txt"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            write!(
                zip,
                "GET http://example.invalid/{id} HTTP/1.1\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
        }
        std::fs::write(&source, zip.finish().unwrap().into_inner()).unwrap();
        let workspace = app(root.path());
        let imported = workspace
            .import_trace(request(source, "warnings"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert!(imported.trace.notes.len() <= 128);
        assert!(
            imported
                .trace
                .notes
                .last()
                .unwrap()
                .contains("additional source notes omitted")
        );
        let saved = root.path().join("warnings.tmcap");
        workspace
            .save_traffic_trace(saved.clone(), crate::TraceSaveOptions::default())
            .await
            .unwrap();
        let reopened = app(&root.path().join("reopened"));
        reopened
            .import_trace(request(saved, "reopen"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert_eq!(
            reopened.service.catalog().project_retained(|_| ()).len(),
            140
        );
    }

    #[tokio::test]
    async fn native_and_extended_saz_preserve_structured_redacted_trailers() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("original.tmcap");
        native(&source, Some(vec![1, 2, 3]), 3);
        let mut recovered =
            transmog_capture::recover(File::open(&source).unwrap(), CaptureLimits::default())
                .unwrap();
        recovered
            .records
            .retain(|record| !matches!(record.kind, CaptureRecordKind::Seal { .. }));
        recovered.records.push(CaptureRecord {
            exchange_id: 7,
            sequence: 6,
            kind: CaptureRecordKind::Trailers {
                boundary: ExchangeBoundary::ClientRequest,
                headers: vec![CapturedHeader {
                    name: b"Cookie".to_vec(),
                    value: Some(b"secret".to_vec()),
                    original_value_bytes: Some(6),
                }],
            },
        });
        let mut writer =
            CaptureWriter::new(File::create(&source).unwrap(), CaptureLimits::default()).unwrap();
        for record in recovered.records {
            writer.append(&record).unwrap();
        }
        writer.seal().unwrap();
        let workspace = app(root.path());
        workspace
            .import_trace(request(source, "trailers"), Arc::new(|_| {}))
            .await
            .unwrap();
        let saved = root.path().join("saved.tmcap");
        workspace
            .save_traffic_trace(
                saved.clone(),
                crate::TraceSaveOptions {
                    password: None,
                    redact_sensitive_headers: true,
                    ..crate::TraceSaveOptions::default()
                },
            )
            .await
            .unwrap();
        for format in [
            crate::ExportFormat::Native,
            crate::ExportFormat::SazExtended,
        ] {
            let destination = root.path().join(if format == crate::ExportFormat::Native {
                "copy.tmcap"
            } else {
                "copy.saz"
            });
            workspace
                .export_capture(crate::ExportRequest {
                    source: saved.clone(),
                    destination: destination.clone(),
                    format,
                    max_source_bytes: 64 * 1024 * 1024,
                    redact_sensitive_headers: false,
                })
                .await
                .unwrap();
            let reopened = app(&root.path().join(format!("reopened-{format:?}")));
            reopened
                .import_trace(request(destination, "copy"), Arc::new(|_| {}))
                .await
                .unwrap();
            let row = &reopened
                .query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions[0];
            let snapshot = reopened
                .service
                .catalog()
                .get(ExchangeId(u128::from_str_radix(&row.id, 16).unwrap()))
                .unwrap();
            let field = snapshot
                .bodies
                .iter()
                .find(|body| body.boundary == ExchangeBoundary::ClientRequest)
                .unwrap()
                .trailers
                .as_ref()
                .unwrap()
                .iter()
                .next()
                .unwrap();
            assert!(field.is_redacted());
            assert_eq!(field.original_value_bytes(), Some(6));
        }
    }

    #[tokio::test]
    async fn extended_saz_keeps_merged_sources_and_original_entry_ids() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("original.saz");
        saz(&source);
        let workspace = app(root.path());
        workspace
            .import_trace(request(source, "source"), Arc::new(|_| {}))
            .await
            .unwrap();
        let native_path = root.path().join("workspace.tmcap");
        workspace
            .save_traffic_trace(native_path.clone(), crate::TraceSaveOptions::default())
            .await
            .unwrap();
        let destination = root.path().join("extended.saz");
        workspace
            .export_capture(crate::ExportRequest {
                source: native_path,
                destination: destination.clone(),
                format: crate::ExportFormat::SazExtended,
                max_source_bytes: 64 * 1024 * 1024,
                redact_sensitive_headers: true,
            })
            .await
            .unwrap();
        let reopened = app(&root.path().join("reopened"));
        reopened
            .import_trace(request(destination, "extended"), Arc::new(|_| {}))
            .await
            .unwrap();
        let row = &reopened
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions[0];
        let detail = reopened.session_detail(&row.id).unwrap();
        assert_eq!(detail.original_id.as_deref(), Some("1"));
        assert_eq!(
            reopened
                .trace_metadata(row.trace_id.as_deref().unwrap())
                .unwrap()
                .name,
            "original.saz"
        );
        assert_eq!(row.duration_ms, Some(250));
        assert!(
            reopened
                .copy_all_headers(&row.id)
                .unwrap()
                .contains("404 Custom reason")
        );
        assert!(
            !reopened
                .copy_all_headers(&row.id)
                .unwrap()
                .contains("secret")
        );
        assert_eq!(reopened.composer_source(&row.id).unwrap().body, "616263");
    }

    #[tokio::test]
    async fn workspace_save_marks_missing_bodies_excludes_removed_rows_and_preserves_existing_files_on_failure()
     {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.tmcap");
        native(&source, None, 3);
        let workspace = app(root.path());
        workspace
            .import_trace(request(source, "missing"), Arc::new(|_| {}))
            .await
            .unwrap();
        let destination = root.path().join("missing.tmcap");
        let report = workspace
            .save_traffic_trace(
                destination.clone(),
                crate::TraceSaveOptions {
                    password: None,
                    redact_sensitive_headers: false,
                    include_network_context: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.incomplete_bodies, 1);
        let reopened = app(&root.path().join("reopened"));
        reopened
            .import_trace(request(destination, "missing-save"), Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = reopened
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert!(
            reopened
                .request_command(&rows[0].id, RequestCommandFormat::Curl)
                .unwrap()
                .body_file_required
        );
        reopened
            .remove_traffic_entries(&[rows[0].id.clone()], false)
            .unwrap();
        let empty = reopened
            .save_traffic_trace(
                root.path().join("empty.tmcap"),
                crate::TraceSaveOptions {
                    password: None,
                    redact_sensitive_headers: false,
                    include_network_context: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(empty.entries, 0);
        let good = root.path().join("good.tmcap");
        native(&good, Some(vec![0, 255, 7]), 3);
        let workspace = app(&root.path().join("changed"));
        workspace
            .import_trace(request(good.clone(), "changed"), Arc::new(|_| {}))
            .await
            .unwrap();
        let output = root.path().join("existing.tmcap");
        std::fs::write(&output, b"keep original").unwrap();
        std::fs::write(
            &good,
            vec![0u8; usize::try_from(std::fs::metadata(&good).unwrap().len()).unwrap()],
        )
        .unwrap();
        assert!(
            workspace
                .save_traffic_trace(
                    output.clone(),
                    crate::TraceSaveOptions {
                        password: None,
                        redact_sensitive_headers: false,
                        include_network_context: false
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(output).unwrap(), b"keep original");
    }

    #[tokio::test]
    async fn combined_index_limit_rejects_before_publishing_new_entries_or_sources() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.tmcap");
        native(&path, Some(vec![1]), 1);
        let application = app(root.path());
        application.traces.lock().index_bytes = MAX_HEAD_BYTES * 2;
        let rejected = application
            .import_trace(request(path, "full-index"), Arc::new(|_| {}))
            .await
            .unwrap_err();
        assert!(rejected.message.contains("combined trace index limit"));
        assert!(
            application
                .query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(application.trace_metadata_list().is_empty());
    }

    #[tokio::test]
    async fn imported_large_body_loads_as_streamed_composer_source_and_spools_exact_bytes() {
        use std::io::Read;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.tmcap");
        let count = 5 * 1024 * 1024 + 123;
        native(&path, Some(vec![0; count]), count);
        let app = app(root.path());
        app.import_trace(request(path, "large-replay"), Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        let source = app.composer_source(&rows[0].id).unwrap();
        assert!(source.body_available && source.body_streamed);
        assert!(source.body.is_empty());
        assert_eq!(source.body_bytes, Some(count as u64));
        let (mut file, length) = app
            .prepare_request_file(&rows[0].id)
            .unwrap()
            .into_replay_file()
            .await
            .unwrap();
        assert_eq!(length, count as u64);
        let mut bytes = [1; 8192];
        let mut read = 0;
        loop {
            let count = file.read(&mut bytes).unwrap();
            if count == 0 {
                break;
            }
            assert!(bytes[..count].iter().all(|byte| *byte == 0));
            read += count;
        }
        assert_eq!(read, count);
    }

    #[tokio::test]
    async fn native_import_preserves_provenance_unknowns_and_body_sources() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.tmg");
        native(&path, Some(vec![0, 255, 7]), 3);
        let app = app(root.path());
        let first = app
            .import_trace(request(path.clone(), "one"), Arc::new(|_| {}))
            .await
            .unwrap();
        let second = app
            .import_trace(request(path, "two"), Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_ne!(first.trace.id, second.trace.id);
        assert_eq!(rows[0].trace_id.as_deref(), Some(first.trace.id.as_str()));
        assert_eq!(rows[0].duration_ms, None);
        assert_eq!(rows[0].protocol, "Unavailable");
        let detail = app.session_detail(&rows[0].id).unwrap();
        assert_eq!(detail.source_ip.as_deref(), Some("192.0.2.5"));
        assert_eq!(detail.requests[0].headers[1].field_bytes, None);
        assert_eq!(first.trace.context["networkContext"], "original machine");
        assert_eq!(app.composer_source(&rows[0].id).unwrap().body, "00ff07");
        assert!(
            app.request_command(&rows[0].id, RequestCommandFormat::Curl)
                .unwrap()
                .body_file_required
        );
        if cfg!(windows) {
            assert!(
                !app.request_command(&rows[0].id, RequestCommandFormat::Powershell)
                    .unwrap()
                    .body_file_required
            );
        }
        assert!(!root.path().join("cache").exists());
    }

    #[tokio::test]
    async fn encrypted_native_save_reopens_lazily_and_wrong_password_publishes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("plain.tmcap");
        native(&source, Some(b"private payload".to_vec()), 15);
        let original = app(root.path());
        original
            .import_trace(request(source, "source"), Arc::new(|_| {}))
            .await
            .unwrap();
        let encrypted = root.path().join("encrypted.tmcap");
        let password = transmog_capture::CapturePassword::new("secret phrase".into());
        original
            .save_traffic_trace(
                encrypted.clone(),
                crate::TraceSaveOptions {
                    password: Some(password.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let bytes = std::fs::read(&encrypted).unwrap();
        assert!(!bytes.windows(15).any(|part| part == b"private payload"));
        let reopened = app(root.path());
        assert!(
            reopened
                .import_trace(
                    request(encrypted.clone(), "missing-password"),
                    Arc::new(|_| {})
                )
                .await
                .is_err()
        );
        let mut input = request(encrypted.clone(), "wrong-password");
        input.password = Some(transmog_capture::CapturePassword::new("incorrect".into()));
        assert!(
            reopened
                .import_trace(input, Arc::new(|_| {}))
                .await
                .is_err()
        );
        assert!(
            reopened
                .query_sessions(crate::SessionQueryInput::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        let mut input = request(encrypted, "correct-password");
        input.password = Some(password);
        reopened
            .import_trace(input, Arc::new(|_| {}))
            .await
            .unwrap();
        let rows = reopened
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions;
        let id =
            transmog_core::intercept::ExchangeId(u128::from_str_radix(&rows[0].id, 16).unwrap());
        let range = reopened
            .body_store()
            .unwrap()
            .read_range(id, ExchangeBoundary::ClientRequest, 0, 64)
            .unwrap();
        assert_eq!(range.bytes, b"private payload");
        assert!(!root.path().join("cache").exists());
    }

    #[tokio::test]
    async fn compressed_native_is_exact_lazy_and_bad_gzip_never_publishes() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.tmcap");
        let gzip = root.path().join("trace.tmcap.gz");
        native(&path, Some(vec![0, 255, 7]), 3);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&std::fs::read(path).unwrap()).unwrap();
        let mut bytes = encoder.finish().unwrap();
        std::fs::write(&gzip, &bytes).unwrap();
        let app = app(root.path());
        app.import_trace(request(gzip.clone(), "compressed"), Arc::new(|_| {}))
            .await
            .unwrap();
        std::fs::remove_file(&gzip).unwrap();
        let rows = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert_eq!(app.composer_source(&rows[0].id).unwrap().body, "00ff07");
        let last = bytes.len() - 1;
        bytes[last] ^= 255;
        std::fs::write(&gzip, bytes).unwrap();
        assert!(
            app.import_trace(request(gzip, "bad-gzip"), Arc::new(|_| {}))
                .await
                .is_err()
        );
        assert_eq!(app.trace_metadata_list().len(), 1);
    }

    #[tokio::test]
    async fn compressed_native_expansion_is_bounded_before_publication() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let gzip = root.path().join("trace.tmcap.gz");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![0; 128 * 1024]).unwrap();
        std::fs::write(&gzip, encoder.finish().unwrap()).unwrap();
        let app = app(root.path());
        let mut input = request(gzip, "too-large");
        input.max_file_bytes = 2048;
        assert!(
            app.import_trace(input, Arc::new(|_| {}))
                .await
                .unwrap_err()
                .message
                .contains("expanded trace")
        );
        assert!(app.trace_metadata_list().is_empty());
    }

    #[tokio::test]
    async fn missing_native_body_is_not_a_replayable_empty_body() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.tmg");
        native(&path, None, 3);
        let app = app(root.path());
        app.import_trace(request(path, "lost"), Arc::new(|_| {}))
            .await
            .unwrap();
        let id = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions[0]
            .id
            .clone();
        assert!(!app.composer_source(&id).unwrap().body_available);
        let copied = app.copy_all_headers(&id).unwrap();
        assert!(copied.contains("HTTP/[unavailable] 204"));
        assert!(!copied.contains("No Content"));
        let command = app
            .request_command(&id, RequestCommandFormat::Curl)
            .unwrap();
        assert!(command.body_file_required);
        assert!(!command.body_file_available);
    }

    #[tokio::test]
    async fn saz_import_dechunks_for_direct_copy_and_keeps_original_heads_and_timing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.SAZ");
        saz(&path);
        let app = app(root.path());
        let result = app
            .import_trace(request(path, "saz"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        let row = &app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions[0];
        assert_eq!(row.duration_ms, Some(250));
        let command = app
            .request_command(&row.id, RequestCommandFormat::Curl)
            .unwrap();
        assert!(!command.body_file_required);
        assert!(command.text.contains("abc"));
        assert_eq!(app.composer_source(&row.id).unwrap().body, "616263");
        let headers = app.copy_all_headers(&row.id).unwrap();
        assert!(headers.contains("\r\n\r\n\r\nHTTP/1.1 404 Custom reason"));
        assert!(headers.contains("Cookie: secret"));
        assert_eq!(
            app.session_detail(&row.id).unwrap().saved_evidence["DNSLookupTime"],
            "12"
        );
    }

    #[tokio::test]
    async fn cancellation_and_corruption_publish_no_partial_batch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace.tmg");
        native(&path, Some(vec![1, 2, 3]), 3);
        let app = app(root.path());
        let registry = app.traces.clone();
        assert!(
            app.import_trace(
                request(path.clone(), "cancel"),
                Arc::new(move |_| registry.cancel("cancel"))
            )
            .await
            .is_err()
        );
        assert!(app.trace_metadata_list().is_empty());
        assert!(
            app.query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        let mut bytes = std::fs::read(&path).unwrap();
        let end = bytes.len() - 1;
        bytes[end] ^= 255;
        std::fs::write(&path, bytes).unwrap();
        assert!(
            app.import_trace(request(path, "corrupt"), Arc::new(|_| {}))
                .await
                .is_err()
        );
        assert!(app.trace_metadata_list().is_empty());
    }
}
