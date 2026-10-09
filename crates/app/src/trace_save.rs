//! Streaming snapshots of the retained Traffic workspace, with original sources.
use crate::{AppError, Application, BodyStore, ErrorCategory, traces::TraceEntry};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use transmog_capture::{
    CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter, CapturedHeader,
};
use transmog_core::{HeaderBlock, observe::ExchangeBoundary};
use transmog_session::{SessionSnapshot, SessionTerminal};

/// Explicit save choices; collection is always opt-in.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSaveOptions {
    /// Omit sensitive header values from this copy; retained evidence is unchanged.
    #[serde(default)]
    pub redact_sensitive_headers: bool,
    /// Collect network configuration from this computer when saving.
    #[serde(default)]
    pub include_network_context: bool,
}
/// Result of saving the whole retained workspace.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSaveResult {
    /// User-selected destination.
    pub destination: PathBuf,
    /// Number of saved exchanges, across every page and filter.
    pub entries: usize,
    /// Size of the finished file.
    pub bytes: u64,
    /// Number of unavailable or incomplete body boundaries.
    pub incomplete_bodies: usize,
}

pub(crate) async fn save(
    application: Application,
    destination: PathBuf,
    options: TraceSaveOptions,
) -> Result<TraceSaveResult, AppError> {
    let network = if options.include_network_context {
        Some(transmog_network::context::collect_for("trace-save").await)
    } else {
        None
    };
    tokio::task::spawn_blocking(move || {
        write(
            &application,
            destination,
            network.as_ref(),
            options.redact_sensitive_headers,
        )
    })
    .await
    .map_err(|_| error("Trace save worker failed"))?
}
fn write(
    application: &Application,
    destination: PathBuf,
    network: Option<&transmog_network::context::NetworkContext>,
    redact: bool,
) -> Result<TraceSaveResult, AppError> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(error("Choose an absolute trace destination"));
    }
    let name = destination.to_string_lossy().to_ascii_lowercase();
    if !destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tmcap"))
        && !name.ends_with(".tmcap.gz")
    {
        return Err(error("Trace filename must end in .tmcap or .tmcap.gz"));
    }
    let store = application
        .body_store
        .as_ref()
        .ok_or_else(|| error("Body retention is not configured"))?;
    store
        .flush()
        .map_err(|_| error("Body metadata is temporarily unavailable"))?;
    let sessions = application.service.catalog().project_retained(|session| {
        let mut snapshot = session.clone();
        for body in &mut snapshot.bodies {
            body.retained_prefix = bytes::Bytes::new();
        }
        snapshot
    });
    let parent = destination
        .parent()
        .ok_or_else(|| error("Choose a trace destination"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| error("Trace output could not be created"))?;
    let incomplete = write_records(
        temporary.as_file_mut(),
        application,
        store,
        &sessions,
        network,
        redact,
    )?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| error("Trace could not be flushed"))?;
    if destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        let mut compressed = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| error("Compressed trace could not be created"))?;
        temporary
            .as_file_mut()
            .rewind()
            .map_err(|_| error("Trace could not be reread"))?;
        let mut gzip =
            flate2::write::GzEncoder::new(compressed.as_file_mut(), flate2::Compression::default());
        std::io::copy(temporary.as_file_mut(), &mut gzip)
            .map_err(|_| error("Trace compression failed"))?;
        gzip.finish()
            .map_err(|_| error("Trace compression could not finish"))?;
        compressed
            .as_file()
            .sync_all()
            .map_err(|_| error("Compressed trace could not be flushed"))?;
        temporary = compressed;
    }
    let bytes = temporary
        .as_file()
        .metadata()
        .map_err(|_| error("Trace size is unavailable"))?
        .len();
    // The host's native Save as dialog owns overwrite consent. Failed writes
    // leave an existing destination intact; only the finished temporary replaces it.
    temporary
        .persist(&destination)
        .map_err(|_| error("Trace could not be saved at this destination"))?;
    Ok(TraceSaveResult {
        destination,
        entries: sessions.len(),
        bytes,
        incomplete_bodies: incomplete,
    })
}
use std::io::Seek;
fn write_records(
    output: &mut std::fs::File,
    application: &Application,
    store: &BodyStore,
    sessions: &[SessionSnapshot],
    network: Option<&transmog_network::context::NetworkContext>,
    redact: bool,
) -> Result<usize, AppError> {
    let mut writer = CaptureWriter::new(
        output,
        CaptureLimits {
            max_file_bytes: 4 * 1024 * 1024 * 1024,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: 10_000_000,
        },
    )
    .map_err(|_| error("Trace writer could not start"))?;
    append(
        &mut writer,
        0,
        0,
        CaptureRecordKind::Unknown {
            kind: "trace-metadata".into(),
            payload: serde_json::json!({"application":"Transmog", "version":env!("CARGO_PKG_VERSION"),"savedAt":millis(SystemTime::now()),"networkContext":network,"sensitiveHeadersRedacted":redact}),
        },
    )?;
    let source_ids = sessions
        .iter()
        .filter_map(|session| {
            application
                .traces
                .entry(&format!("{:032x}", session.exchange_id.0))
                .map(|entry| entry.trace_id)
        })
        .collect::<std::collections::HashSet<_>>();
    for source in application
        .traces
        .list()
        .into_iter()
        .filter(|source| source_ids.contains(&source.id))
    {
        append(
            &mut writer,
            0,
            0,
            CaptureRecordKind::Unknown {
                kind: "trace-source".into(),
                payload: serde_json::to_value(source)
                    .map_err(|_| error("Source metadata could not be saved"))?,
            },
        )?;
    }
    if sessions.iter().any(|session| !session.imported) {
        append(
            &mut writer,
            0,
            0,
            CaptureRecordKind::Unknown {
                kind: "trace-source".into(),
                payload: serde_json::json!({"id":"live","name":"Live traffic","format":"live","path":"","sessions":sessions.iter().filter(|session|!session.imported).count(),"importedAt":millis(SystemTime::now()),"context":{"networkContext":network},"notes":["Network configuration, when included, was collected when this workspace was saved."]}),
            },
        )?;
    }
    let mut incomplete = 0;
    for session in sessions {
        incomplete += write_session(&mut writer, application, store, session, redact)?;
    }
    writer
        .seal()
        .map_err(|_| error("Trace could not be sealed"))?;
    Ok(incomplete)
}
fn append<W: Write>(
    writer: &mut CaptureWriter<W>,
    id: u128,
    sequence: u64,
    kind: CaptureRecordKind,
) -> Result<(), AppError> {
    writer
        .append(&CaptureRecord {
            exchange_id: id,
            sequence,
            kind,
        })
        .map_err(|_| error("Trace output exceeded its limit or could not be written"))
}
#[allow(clippy::too_many_lines)]
fn write_session<W: Write>(
    writer: &mut CaptureWriter<W>,
    application: &Application,
    store: &BodyStore,
    session: &SessionSnapshot,
    redact: bool,
) -> Result<usize, AppError> {
    let id = session.exchange_id.0;
    let mut sequence = 0;
    let mut emit = |mut kind| {
        if redact {
            crate::export_privacy::redact(&mut kind);
        }
        sequence += 1;
        append(writer, id, sequence, kind)
    };
    emit(CaptureRecordKind::ExchangeStarted {
        client_addr: session.metadata.client_addr.to_string(),
        client_identity: session.metadata.client_identity.clone(),
        listener_addr: session.metadata.listener_addr.to_string(),
        authority: session
            .metadata
            .original_target
            .as_target()
            .authority
            .clone(),
        started_unix_nanos: session
            .metadata
            .started_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    })?;
    let entry = application
        .traces
        .entry(&format!("{id:032x}"))
        .unwrap_or_else(|| TraceEntry {
            trace_id: "live".into(),
            original_id: format!("{id:032x}"),
            raw_headers: None,
            timings: std::collections::BTreeMap::default(),
            protocol_known: !session.performance.protocols.is_empty(),
            target_known: true,
            diagnostics: vec![],
        });
    emit(CaptureRecordKind::Unknown {
        kind: "entry-provenance".into(),
        payload: serde_json::json!({"source":entry,"terminalUnixMillis":session.terminal_at.map(millis)}),
    })?;
    for head in &session.request_heads {
        let target = &head.head.target;
        emit(CaptureRecordKind::RequestHead {
            boundary: head.boundary,
            method: head.head.method.clone(),
            target: format!(
                "{}://{}{}{}",
                target.scheme,
                target.authority,
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}"))
            ),
            headers: headers(&head.head.headers),
        })?;
    }
    for head in &session.response_heads {
        emit(CaptureRecordKind::ResponseHead {
            boundary: head.boundary,
            status: head.head.status,
            headers: headers(&head.head.headers),
        })?;
    }
    emit(CaptureRecordKind::Performance(session.performance.clone()))?;
    let mut incomplete = 0;
    for boundary in [
        ExchangeBoundary::ClientRequest,
        ExchangeBoundary::UpstreamRequest,
        ExchangeBoundary::UpstreamResponse,
        ExchangeBoundary::ClientResponse,
    ] {
        let observed = session.bodies.iter().find(|body| body.boundary == boundary);
        let metadata = store
            .metadata(session.exchange_id)
            .into_iter()
            .find(|body| body.boundary == crate::inspector::boundary(boundary));
        if metadata.is_none() && observed.is_none() {
            continue;
        }
        if let Ok(mut body) = store.open_complete(session.exchange_id, boundary) {
            let expected = body.metadata().retained_bytes;
            let length_known = body.metadata().length_known;
            let mut copied = 0u64;
            let mut buffer = vec![0u8; 128 * 1024];
            loop {
                let count = body
                    .read(&mut buffer)
                    .map_err(|_| error("Retained body changed or could not be read"))?;
                if count == 0 {
                    break;
                }
                copied = copied.saturating_add(count as u64);
                if copied > expected {
                    return Err(error("Retained body changed while saving"));
                }
                emit(CaptureRecordKind::BodySegment {
                    boundary,
                    byte_count: count,
                    bytes: Some(buffer[..count].to_vec()),
                    truncated: false,
                })?;
            }
            if length_known && copied != expected {
                return Err(error("Retained body is incomplete"));
            }
            if copied == 0 {
                emit(CaptureRecordKind::BodySegment {
                    boundary,
                    byte_count: 0,
                    bytes: Some(vec![]),
                    truncated: false,
                })?;
            }
        } else {
            incomplete += 1;
            let count = metadata.as_ref().map_or_else(
                || observed.map_or(0, |body| body.observed_bytes),
                |body| body.observed_bytes,
            );
            emit(CaptureRecordKind::BodySegment {
                boundary,
                byte_count: usize::try_from(count)
                    .map_err(|_| error("Body length exceeds this platform's limit"))?,
                bytes: None,
                truncated: true,
            })?;
        }
        if let Some(trailers) = observed.and_then(|body| body.trailers.as_ref()) {
            emit(CaptureRecordKind::Trailers {
                boundary,
                headers: headers(trailers),
            })?;
        }
    }
    if session.sequence_loss > 0 {
        emit(CaptureRecordKind::Loss {
            first_missing_sequence: 0,
            count: session.sequence_loss,
            reason: "original capture reported sequence loss".into(),
        })?;
    }
    emit(match &session.terminal {
        Some(SessionTerminal::Completed(_)) => CaptureRecordKind::Completed,
        Some(SessionTerminal::Failed(failure)) => CaptureRecordKind::Failed {
            category: format!("{:?}", failure.kind),
            message: failure.message.clone(),
        },
        None => CaptureRecordKind::Failed {
            category: "incomplete".into(),
            message: "This exchange was still active when the trace was saved.".into(),
        },
    })?;
    Ok(incomplete)
}
fn headers(block: &HeaderBlock) -> Vec<CapturedHeader> {
    block
        .iter()
        .map(|field| CapturedHeader {
            name: field.name().to_vec(),
            value: (!field.is_redacted()).then(|| field.value().to_vec()),
            original_value_bytes: field.original_value_bytes(),
        })
        .collect()
}
fn millis(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
fn error(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}
