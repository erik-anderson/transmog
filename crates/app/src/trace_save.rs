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
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSaveOptions {
    /// Optional frozen selection from explicit export review. Omitted saves all retained traffic.
    #[serde(default)]
    pub review_id: Option<String>,
    /// Keep original source notes, incident markers and optional capture context in this copy.
    #[serde(default = "include_context")]
    pub include_original_context: bool,
    /// Native trace, SAZ archive, or standard HTTP Archive.
    #[serde(default)]
    pub format: TraceSaveFormat,
    /// Password for opt-in per-frame AES-256-GCM encryption.
    #[serde(default)]
    pub password: Option<transmog_capture::CapturePassword>,
    /// Omit sensitive header values from this copy; retained evidence is unchanged.
    #[serde(default)]
    pub redact_sensitive_headers: bool,
    /// Collect network configuration from this computer when saving.
    #[serde(default)]
    pub include_network_context: bool,
}
fn include_context() -> bool {
    true
}
impl Default for TraceSaveOptions {
    fn default() -> Self {
        Self {
            review_id: None,
            include_original_context: true,
            format: TraceSaveFormat::default(),
            password: None,
            redact_sensitive_headers: false,
            include_network_context: false,
        }
    }
}
/// Formats offered by Save trace. `NetLog` is import-only.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TraceSaveFormat {
    /// Full-fidelity native capture.
    #[default]
    Native,
    /// HAR 1.2 HTTP exchanges and decoded response bodies.
    Har,
    /// Conventional SAZ for compatibility with other tools.
    SazStrict,
    /// SAZ with original source associations and additional evidence.
    SazExtended,
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
    /// Entries whose missing HTTP heads were labeled with SAZ placeholders.
    pub placeholder_entries: usize,
    /// Additional generated CONNECT rows describing recorded connection evidence.
    pub connection_entries: usize,
    /// Incident markers omitted by the selected privacy/format projection.
    pub omitted_incident_markers: usize,
}

/// Observable save work; zero total denotes an indeterminate phase.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSaveProgress {
    /// Preparing, writing, converting, or finalizing the saved trace.
    pub phase: String,
    /// Entries completed in this phase.
    pub completed: usize,
    /// Entries in this phase, or zero while the amount of work is unknown.
    pub total: usize,
}

pub(crate) type Progress = dyn Fn(TraceSaveProgress) + Send + Sync;

pub(crate) fn notify(progress: &Progress, phase: &str, completed: usize, total: usize) {
    progress(TraceSaveProgress {
        phase: phase.into(),
        completed,
        total,
    });
}

pub(crate) async fn save(
    application: Application,
    destination: PathBuf,
    options: TraceSaveOptions,
    progress: std::sync::Arc<Progress>,
) -> Result<TraceSaveResult, AppError> {
    let expected = match options.format {
        TraceSaveFormat::Native => "tmcap",
        TraceSaveFormat::Har => "har",
        TraceSaveFormat::SazStrict | TraceSaveFormat::SazExtended => "saz",
    };
    if !destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
    {
        return Err(error(
            "The filename extension must match the selected trace format",
        ));
    }
    if options.format == TraceSaveFormat::Har
        && (options.password.is_some() || options.include_network_context)
    {
        return Err(error(
            "HAR has no encryption or network-configuration option; choose TMCap for those options",
        ));
    }
    notify(progress.as_ref(), "preparing", 0, 0);
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
            options,
            progress.as_ref(),
        )
    })
    .await
    .map_err(|_| error("Trace save worker failed"))?
}
#[allow(clippy::too_many_lines)] // Keep each bounded review/save transaction auditable together.
fn write(
    application: &Application,
    destination: PathBuf,
    network: Option<&transmog_network::context::NetworkContext>,
    options: TraceSaveOptions,
    progress: &Progress,
) -> Result<TraceSaveResult, AppError> {
    // Exports are copies. Native overwrite consent must never replace loaded evidence.
    if let Ok(target) = std::fs::canonicalize(&destination)
        && application
            .traces
            .list()
            .iter()
            .any(|source| std::fs::canonicalize(&source.path).is_ok_and(|path| path == target))
    {
        return Err(error(
            "Choose a new filename; an imported original cannot be overwritten",
        ));
    }
    let review = application.export_scope(options.review_id.as_deref())?;
    let redact = options.redact_sensitive_headers;
    let password = options.password;
    let format = options.format;
    let original_sources = application
        .traces
        .list()
        .into_iter()
        .filter(|s| review.as_ref().is_none_or(|r| r.sources.contains(&s.id)))
        .collect::<Vec<_>>();
    let marker_count = original_sources
        .iter()
        .map(|s| {
            s.context
                .get("incidentMarkers")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len)
        })
        .sum::<usize>();
    let omitted_markers = if options.include_original_context
        && matches!(
            format,
            TraceSaveFormat::Native | TraceSaveFormat::SazExtended
        ) {
        0
    } else {
        marker_count
    };
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(error("Choose an absolute trace destination"));
    }
    if format == TraceSaveFormat::Har {
        if password.is_some() || network.is_some() {
            return Err(error(
                "HAR has no encryption or network-configuration option; choose TMCap for those options",
            ));
        }
        let mut result = crate::trace_har::write(
            application,
            destination,
            redact,
            progress,
            review.as_deref().map(|r| &r.ids),
        )?;
        result.omitted_incident_markers = omitted_markers;
        return Ok(result);
    }
    let store = application
        .body_store
        .as_ref()
        .ok_or_else(|| error("Body retention is not configured"))?;
    store
        .flush()
        .map_err(|_| error("Body metadata is temporarily unavailable"))?;
    let sessions = application
        .service
        .catalog()
        .project_retained(|session| {
            if review
                .as_ref()
                .is_some_and(|r| !r.ids.contains(&format!("{:032x}", session.exchange_id.0)))
            {
                return None;
            }
            let mut snapshot = session.clone();
            for body in &mut snapshot.bodies {
                body.retained_prefix = bytes::Bytes::new();
            }
            Some(snapshot)
        })
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if review.as_ref().is_some_and(|review| {
        sessions.len() != review.ids.len() || original_sources.len() != review.sources.len()
    }) {
        return Err(error("Export evidence changed; review the selection again"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| error("Choose a trace destination"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| error("Trace output could not be created"))?;
    let mut native = if matches!(
        format,
        TraceSaveFormat::SazStrict | TraceSaveFormat::SazExtended
    ) {
        Some(tempfile::tempfile().map_err(|_| error("SAZ staging file could not be created"))?)
    } else {
        None
    };
    let incomplete = write_records(
        native.as_mut().unwrap_or_else(|| temporary.as_file_mut()),
        application,
        &sessions,
        network,
        redact,
        &transmog_capture::CaptureEncoding {
            password: password.clone(),
        },
        progress,
        options.include_original_context,
        &original_sources,
    )?;
    let (placeholder_entries, connection_entries) = if let Some(native) = native {
        write_saz(native, temporary.as_file_mut(), format, password, progress)?
    } else {
        (0, 0)
    };
    notify(progress, "finalizing", 0, 0);
    if format != TraceSaveFormat::Native {
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| error("Trace could not be flushed"))?;
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
        placeholder_entries,
        connection_entries,
        omitted_incident_markers: omitted_markers,
    })
}
fn write_saz(
    mut native: std::fs::File,
    output: &mut std::fs::File,
    format: TraceSaveFormat,
    password: Option<transmog_capture::CapturePassword>,
    progress: &Progress,
) -> Result<(usize, usize), AppError> {
    use std::io::Seek as _;
    notify(progress, "converting", 0, 0);
    native
        .rewind()
        .map_err(|_| error("SAZ staging file could not be read"))?;
    let capture = transmog_capture::recover_with_password(
        native,
        CaptureLimits {
            max_file_bytes: u64::MAX,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: usize::MAX,
        },
        password.as_ref(),
    )
    .map_err(AppError::from)?;
    let mode = if format == TraceSaveFormat::SazStrict {
        transmog_saz::SazMode::Strict
    } else {
        transmog_saz::SazMode::Extended
    };
    let mut exporter =
        transmog_saz::SazExporter::new(output, mode, transmog_saz::SazLimits::default())
            .map_err(AppError::from)?;
    if let Some(password) = password {
        exporter = exporter.encrypted(password).map_err(AppError::from)?;
    }
    exporter
        .export_with_progress(&capture, |done, total| {
            notify(progress, "converting", done, total);
        })
        .map_err(AppError::from)?;
    Ok(exporter.report().map_or((0, 0), |report| {
        (report.placeholder_sessions, report.connection_sessions)
    }))
}

#[allow(clippy::too_many_arguments)] // One streaming writer applies explicit export projections.
fn write_records(
    output: &mut std::fs::File,
    application: &Application,
    sessions: &[SessionSnapshot],
    network: Option<&transmog_network::context::NetworkContext>,
    redact: bool,
    encoding: &transmog_capture::CaptureEncoding,
    progress: &Progress,
    include_original_context: bool,
    original_sources: &[crate::traces::TraceMetadataView],
) -> Result<usize, AppError> {
    let store = application
        .body_store
        .as_ref()
        .ok_or_else(|| error("Body retention is not configured"))?;
    let mut writer = CaptureWriter::with_encoding(
        output,
        CaptureLimits {
            max_file_bytes: u64::MAX,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: usize::MAX,
        },
        encoding,
    )
    .map_err(|_| error("Trace writer could not start"))?;
    append(
        &mut writer,
        0,
        0,
        CaptureRecordKind::Unknown {
            kind: "trace-metadata".into(),
            payload: serde_json::json!({"application":"Transmog", "version":env!("CARGO_PKG_VERSION"),"savedAt":millis(SystemTime::now()),"networkContext":network,"sensitiveHeadersRedacted":redact,"exportSelection":{"entries":sessions.len(),"originalContextIncluded":include_original_context}}),
        },
    )?;
    for original in original_sources {
        let mut source = original.clone();
        if !include_original_context {
            source.context = serde_json::Value::Null;
            source.path = std::path::PathBuf::new();
            source.notes=vec!["Original notes, profile/incident and optional context omitted from this exported copy.".into()];
        }
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
    notify(progress, "writing", 0, sessions.len());
    for (position, session) in sessions.iter().enumerate() {
        incomplete += write_session(&mut writer, application, store, session, redact)?;
        notify(progress, "writing", position + 1, sessions.len());
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
            original_number: None,
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
    emit(CaptureRecordKind::Unknown {
        kind: "request-order".into(),
        payload: serde_json::json!({"version":1,"ordinal":session.request_number.to_string()}),
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
    for effect in &session.hook_effects {
        if let transmog_core::intercept::HookEffectAction::Respond {
            snapshot: Some(snapshot),
            ..
        } = &effect.action
        {
            emit(CaptureRecordKind::HookEffect {
                hook_id: effect.interceptor.id.as_str().into(),
                hook_name: effect.interceptor.name.to_string(),
                phase: "request-head".into(),
                action: "respond".into(),
                changed: effect.changed,
                chain_position: effect.interceptor.chain_position,
                response_snapshot: Some(snapshot.clone()),
            })?;
        }
    }
    if let Some(stream) = &session.stream {
        emit(CaptureRecordKind::StreamState(stream.clone()))?;
        for item in &session.stream_history.items {
            emit(CaptureRecordKind::StreamMessage(
                item.read().map_err(|message| error(&message))?,
            ))?;
        }
        let history = &session.stream_history;
        emit(CaptureRecordKind::Unknown {
            kind: "stream-summary".into(),
            payload: serde_json::json!({
                "observed": history.observed, "evicted": history.evicted, "clientBytes": history.client_bytes,
                "serverBytes": history.server_bytes, "openedAt": history.opened_at, "lastActivity": history.last_activity,
            }),
        })?;
    }

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
            let recorded = body.metadata();
            emit(CaptureRecordKind::Unknown {
                kind: "body-representation".into(),
                payload: serde_json::json!({"boundary":crate::inspector::boundary(boundary),"observedBytes":recorded.observed_bytes,"retainedBytes":recorded.retained_bytes,"complete":true,"mediaType":recorded.media_type,"charset":recorded.charset,"contentCodings":recorded.content_codings}),
            })?;
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
            category: failure.kind.category().into(),
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
