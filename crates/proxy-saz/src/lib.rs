#![deny(missing_docs)]

//! Bounded Session Archive Zip import and export adapters.
//!
//! Strict mode emits only the conventional OPC content-types member, an explicit
//! `raw/` directory, three `raw/<id>_{c,s,m}` files per HTTP exchange, and
//! `raw/<id>_w.txt` frame lists for established `WebSockets`.
//! Missing heads use labeled compatibility placeholders without altering evidence.
//! The native format remains authoritative because SAZ cannot represent every
//! proxy boundary or hook effect.

use std::{
    collections::BTreeMap,
    io::{Seek, Write},
};

use serde::Serialize;
use thiserror::Error;
use transmog_capture::{
    CaptureExporter, CaptureRecordKind, CapturedHeader, ExportReport, RecoveredCapture,
};
use transmog_core::observe::ExchangeBoundary;
use zip::{
    CompressionMethod, ZipWriter,
    write::{FileOptions, SimpleFileOptions},
};

mod connections;
mod evidence;
mod import;
mod metadata;
mod tls;
mod websocket;
#[cfg(test)]
mod websocket_tests;
mod xml_text;
pub use evidence::{FailureEvidence, SessionEvidence};
pub use import::{
    ArchiveBody, ArchiveMessage, ArchiveMetadata, ArchiveSession, SazArchive, SazImportLimits,
    SazIndex, SazIssue,
};
pub use tls::{tls_alps, tls_http3_alps, tls_negotiation_details, tls_network_profile};

const CONTENT_TYPES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    "<Default Extension=\"txt\" ContentType=\"text/plain\"/>",
    "<Default Extension=\"xml\" ContentType=\"text/xml\"/>",
    "</Types>"
);

/// SAZ compatibility profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SazMode {
    /// Emit conventional SAZ members only.
    #[default]
    Strict,
    /// Also emit a namespaced fidelity manifest.
    Extended,
}

/// Finite SAZ conversion limits.
#[derive(Clone, Copy, Debug)]
pub struct SazLimits {
    /// Maximum exchanges scanned from one native capture.
    pub max_sessions: usize,
    /// Maximum retained request or response body per exchange.
    pub max_body_bytes_per_direction: usize,
    /// Maximum ZIP members emitted.
    pub max_entries: usize,
}

impl Default for SazLimits {
    fn default() -> Self {
        Self {
            max_sessions: 1_000_000,
            max_body_bytes_per_direction: 256 * 1024 * 1024,
            max_entries: 4_000_003,
        }
    }
}

impl SazLimits {
    fn validate(self) -> Result<Self, SazError> {
        if self.max_sessions == 0 || self.max_body_bytes_per_direction == 0 || self.max_entries < 5
        {
            return Err(SazError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Detailed conversion result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SazReport {
    /// Sessions emitted, including sessions with placeholder heads.
    pub sessions: usize,
    /// Sessions with an explicitly labeled placeholder for an uncaptured head.
    pub placeholder_sessions: usize,
    /// Generated CONNECT connection-context rows, independent of captured exchanges.
    pub connection_sessions: usize,
    /// Emitted sessions whose retained body was incomplete.
    pub incomplete_bodies: usize,
    /// ZIP members emitted.
    pub entries: usize,
    /// Final archive length.
    pub bytes: u64,
}

/// SAZ exporter requiring a seekable destination for ZIP finalization.
pub struct SazExporter<W> {
    output: Option<W>,
    mode: SazMode,
    limits: SazLimits,
    report: Option<SazReport>,
    password: Option<transmog_capture::CapturePassword>,
}

impl<W: Write + Seek> SazExporter<W> {
    /// Creates a strict compatibility exporter.
    ///
    /// # Errors
    ///
    /// Returns [`SazError::InvalidLimits`] for zero or inconsistent bounds.
    pub fn new(output: W, mode: SazMode, limits: SazLimits) -> Result<Self, SazError> {
        Ok(Self {
            output: Some(output),
            mode,
            limits: limits.validate()?,
            report: None,
            password: None,
        })
    }

    /// Opt in to AES-256 encryption of every file member in the new archive.
    ///
    /// # Errors
    /// Rejects empty passwords before writing any archive bytes.
    pub fn encrypted(
        mut self,
        password: transmog_capture::CapturePassword,
    ) -> Result<Self, SazError> {
        if password.bytes().is_empty() {
            return Err(SazError::InvalidPassword);
        }
        self.password = Some(password);
        Ok(self)
    }

    fn file_options(&self) -> FileOptions<'_, ()> {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        self.password.as_ref().map_or(options, |password| {
            options.with_aes_encryption_bytes(zip::AesMode::Aes256, password.bytes())
        })
    }

    /// Returns the report after a successful export.
    pub fn report(&self) -> Option<SazReport> {
        self.report
    }

    /// Returns the finalized destination.
    ///
    /// # Errors
    ///
    /// Returns [`SazError::NotExported`] before a successful export.
    pub fn into_inner(self) -> Result<W, SazError> {
        if self.report.is_none() {
            return Err(SazError::NotExported);
        }
        self.output.ok_or(SazError::NotExported)
    }
}

impl<W: Write + Seek> CaptureExporter for SazExporter<W> {
    type Error = SazError;

    fn export(&mut self, capture: &RecoveredCapture) -> Result<ExportReport, Self::Error> {
        self.export_with_progress(capture, |_, _| {})
    }
}

impl<W: Write + Seek> SazExporter<W> {
    /// Exports every exchange, reporting completed sessions while writing.
    /// Missing heads use labeled compatibility placeholders, never captured evidence.
    ///
    /// # Errors
    /// Returns the same storage, framing and limit errors as [`CaptureExporter::export`].
    #[allow(clippy::too_many_lines)] // Keep the ZIP transaction and its finalization in one owned pipeline.
    pub fn export_with_progress(
        &mut self,
        capture: &RecoveredCapture,
        progress: impl Fn(usize, usize),
    ) -> Result<ExportReport, SazError> {
        if self.report.is_some() {
            return Err(SazError::AlreadyExported);
        }
        let mut output = self.output.take().ok_or(SazError::AlreadyExported)?;
        let starting_position = output.stream_position()?;
        let sessions = collect_sessions(capture, self.limits)?;
        let connections = connections::summaries(&sessions);
        let total = sessions.len().saturating_add(connections.len());
        if total > self.limits.max_sessions {
            return Err(SazError::SessionLimitExceeded);
        }
        let mut ordered = sessions
            .into_iter()
            .map(|(id, session)| (Some(id), session))
            .chain(connections.into_iter().map(|session| (None, session)))
            .collect::<Vec<_>>();
        ordered.sort_by_key(|(_, session)| {
            let time = ordering_time(session);
            (
                time.is_none(),
                time.unwrap_or_default(),
                u8::from(time.is_some() && session.kind == SessionKind::Captured),
            )
        });
        progress(0, total);
        let mut writer = ZipWriter::new(output);
        let options = self.file_options();
        write_member(
            &mut writer,
            "[Content_Types].xml",
            CONTENT_TYPES.as_bytes(),
            options,
        )?;
        // Fiddler validates this directory entry before scanning request files;
        // file names with a raw/ prefix do not satisfy that archive check.
        writer.add_directory("raw/", SimpleFileOptions::default())?;
        let mut report = SazReport {
            entries: 2,
            ..SazReport::default()
        };
        report.entries += write_trace_context(
            &mut writer,
            capture,
            options,
            self.limits.max_entries.saturating_sub(report.entries),
        )?;
        if self.mode == SazMode::Extended {
            report.entries += evidence::write_sources(
                &mut writer,
                capture,
                options,
                self.limits.max_entries.saturating_sub(report.entries),
            )?;
        }
        let mut manifest = Vec::new();
        for (exchange_id, session) in &mut ordered {
            if exchange_id.is_some() && (session.request.is_none() || session.response.is_none()) {
                report.placeholder_sessions = report.placeholder_sessions.saturating_add(1);
            }
            let session_id = report.sessions.saturating_add(1);
            let incomplete = write_http_session(
                &mut writer,
                session_id,
                session,
                options,
                self.limits.max_entries.saturating_sub(report.entries),
            )?;
            report.sessions += 1;
            report.entries += 3;
            if websocket::established(session) {
                if report.entries >= self.limits.max_entries {
                    return Err(SazError::EntryLimitExceeded);
                }
                writer.start_file(format!("raw/{session_id}_w.txt"), options)?;
                websocket::write_messages(&mut writer, &session.websocket_messages)?;
                report.entries += 1;
            }
            if let Some(exchange_id) = exchange_id {
                if self.mode == SazMode::Extended {
                    report.entries += evidence::write_session(
                        &mut writer,
                        session,
                        *exchange_id,
                        session_id,
                        options,
                        self.limits.max_entries.saturating_sub(report.entries),
                    )?;
                }
                report.incomplete_bodies += usize::from(incomplete);
                manifest.push(ManifestSession {
                    saz_id: session_id,
                    native_exchange_id: exchange_id.to_string(),
                    incomplete,
                });
            } else {
                report.connection_sessions += 1;
            }
            progress(report.sessions, total);
        }
        if self.mode == SazMode::Extended {
            if report.entries >= self.limits.max_entries {
                return Err(SazError::EntryLimitExceeded);
            }
            let manifest = serde_json::to_vec_pretty(&Manifest {
                format: "transmog-saz-extension-v1",
                source_sealed: capture.sealed,
                source_truncated_tail: capture.truncated_tail,
                sessions: &manifest,
            })?;
            if manifest.len() > 4 * 1024 * 1024 {
                return Err(SazError::MetadataLimitExceeded);
            }
            write_member(&mut writer, "transmog/manifest.json", &manifest, options)?;
            report.entries = report.entries.saturating_add(1);
        }
        let mut output = writer.finish()?;
        output.flush()?;
        report.bytes = output.stream_position()?.saturating_sub(starting_position);
        self.output = Some(output);
        self.report = Some(report);
        Ok(ExportReport {
            records: report.sessions,
            bytes: report.bytes,
        })
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent stream, body completeness and loss evidence.
struct Session {
    stream: Option<transmog_core::stream::StreamProtocol>,
    websocket_messages: Vec<transmog_core::stream::StreamMessage>,
    websocket_omitted: usize,
    kind: SessionKind,
    evidence: SessionEvidence,
    started_unix_nanos: Option<u128>,
    ordering_unix_nanos: Option<u128>,
    request: Option<CapturedRequest>,
    response: Option<CapturedResponse>,
    request_body: Vec<u8>,
    response_body: Vec<u8>,
    request_body_incomplete: bool,
    response_body_incomplete: bool,
    request_body_completion: BodyCompletion,
    response_body_completion: BodyCompletion,
    loss: bool,
    terminal: TerminalState,
    request_representation: Option<serde_json::Value>,
    response_representation: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SessionKind {
    #[default]
    Captured,
    ConnectionContext,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TerminalState {
    #[default]
    Open,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum BodyCompletion {
    #[default]
    Pending,
    Complete,
}

struct CapturedRequest {
    method: String,
    target: String,
    headers: Vec<CapturedHeader>,
}

struct CapturedResponse {
    status: u16,
    headers: Vec<CapturedHeader>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_sealed: bool,
    source_truncated_tail: bool,
    sessions: &'a [ManifestSession],
}

#[derive(Serialize)]
struct ManifestSession {
    saz_id: usize,
    native_exchange_id: String,
    incomplete: bool,
}

#[allow(clippy::too_many_lines)]
fn collect_sessions(
    capture: &RecoveredCapture,
    limits: SazLimits,
) -> Result<BTreeMap<u128, Session>, SazError> {
    let mut sessions = BTreeMap::<u128, Session>::new();
    for record in &capture.records {
        if record.exchange_id == 0 {
            continue;
        }
        if !sessions.contains_key(&record.exchange_id) && sessions.len() >= limits.max_sessions {
            return Err(SazError::SessionLimitExceeded);
        }
        let session = sessions.entry(record.exchange_id).or_default();
        match &record.kind {
            CaptureRecordKind::StreamState(state) => {
                session.stream = Some(state.protocol);
            }
            CaptureRecordKind::StreamMessage(message) => {
                if matches!(message.kind.as_str(), "event" | "heartbeat" | "fields")
                    || session.stream == Some(transmog_core::stream::StreamProtocol::Sse)
                {
                    session.stream = Some(transmog_core::stream::StreamProtocol::Sse);
                } else {
                    session
                        .stream
                        .get_or_insert(transmog_core::stream::StreamProtocol::WebSocket);
                    session.websocket_messages.push(message.clone());
                }
            }
            CaptureRecordKind::ExchangeStarted {
                client_addr,
                client_identity,
                started_unix_nanos,
                ..
            } => {
                session.evidence.client_addr = Some(client_addr.clone());
                session.evidence.client_identity = Some(client_identity.clone());
                session.started_unix_nanos = Some(*started_unix_nanos);
            }
            CaptureRecordKind::Performance(performance) => {
                session.evidence.performance.merge(performance);
            }
            CaptureRecordKind::Unknown { kind, payload } if kind == "entry-provenance" => {
                session.evidence.provenance = Some(payload.clone());
            }
            CaptureRecordKind::Unknown { kind, payload } if kind == "request-order" => {
                if payload["version"].as_u64() == Some(1) {
                    session.evidence.request_order = payload["ordinal"].as_str().map(str::to_owned);
                }
            }
            CaptureRecordKind::Unknown { kind, payload } if kind == "body-representation" => {
                match payload["boundary"].as_str() {
                    Some("client-request") => {
                        session.request_representation = Some(payload.clone());
                    }
                    Some("client-response") => {
                        session.response_representation = Some(payload.clone());
                    }
                    _ => {}
                }
            }
            CaptureRecordKind::Unknown { kind, payload } if kind == "body-completed" => {
                match payload["boundary"].as_str() {
                    Some("client-request") => {
                        session.request_body_completion = BodyCompletion::Complete;
                    }
                    Some("client-response") => {
                        session.response_body_completion = BodyCompletion::Complete;
                    }
                    Some("upstream-request" | "upstream-response") => {}
                    _ => return Err(SazError::InvalidArchive),
                }
            }
            CaptureRecordKind::Trailers {
                boundary: ExchangeBoundary::ClientRequest,
                headers,
            } => session.evidence.request_trailers.clone_from(headers),
            CaptureRecordKind::Trailers {
                boundary: ExchangeBoundary::ClientResponse,
                headers,
            } => session.evidence.response_trailers.clone_from(headers),
            CaptureRecordKind::RequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                method,
                target,
                headers,
            } => {
                session.request = Some(CapturedRequest {
                    method: method.clone(),
                    target: target.clone(),
                    headers: headers.clone(),
                });
            }
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status,
                headers,
            } => {
                session.response = Some(CapturedResponse {
                    status: *status,
                    headers: headers.clone(),
                });
            }
            CaptureRecordKind::BodySegment {
                boundary,
                byte_count,
                bytes,
                truncated,
            } => {
                let (body, incomplete, completed) = match boundary {
                    ExchangeBoundary::ClientRequest => (
                        &mut session.request_body,
                        &mut session.request_body_incomplete,
                        session.request_body_completion,
                    ),
                    ExchangeBoundary::ClientResponse => (
                        &mut session.response_body,
                        &mut session.response_body_incomplete,
                        session.response_body_completion,
                    ),
                    ExchangeBoundary::UpstreamRequest | ExchangeBoundary::UpstreamResponse => {
                        continue;
                    }
                };
                *incomplete |= completed == BodyCompletion::Complete;
                let Some(bytes) = bytes else {
                    *incomplete |= *truncated || *byte_count > 0;
                    continue;
                };
                *incomplete |= *truncated || bytes.len() != *byte_count;
                let next = body
                    .len()
                    .checked_add(bytes.len())
                    .ok_or(SazError::BodyLimitExceeded)?;
                if next > limits.max_body_bytes_per_direction {
                    return Err(SazError::BodyLimitExceeded);
                }
                body.extend_from_slice(bytes);
            }
            CaptureRecordKind::Loss { .. } => session.loss = true,
            CaptureRecordKind::Completed => session.terminal = TerminalState::Completed,
            CaptureRecordKind::Failed { category, message } => {
                session.terminal = TerminalState::Failed;
                session.evidence.failure = Some(FailureEvidence {
                    category: category.clone(),
                    message: message.clone(),
                });
            }
            _ => {}
        }
    }
    for session in sessions.values_mut() {
        websocket::validate(session)?;
        if let (Some(head), Some(representation)) =
            (&mut session.request, &session.request_representation)
        {
            apply_representation(
                &mut head.headers,
                representation,
                session.request_body.len(),
            );
        }
        if let (Some(head), Some(representation)) =
            (&mut session.response, &session.response_representation)
        {
            apply_representation(
                &mut head.headers,
                representation,
                session.response_body.len(),
            );
        }
    }
    Ok(sessions)
}

fn apply_representation(
    headers: &mut Vec<CapturedHeader>,
    representation: &serde_json::Value,
    bytes: usize,
) {
    headers.retain(|field| {
        ![
            b"content-encoding".as_slice(),
            b"content-type",
            b"content-length",
            b"transfer-encoding",
        ]
        .iter()
        .any(|name| field.name.eq_ignore_ascii_case(name))
    });
    let mut add = |name: &str, value: String| {
        headers.push(CapturedHeader {
            name: name.as_bytes().to_vec(),
            value: Some(value.into_bytes()),
            original_value_bytes: None,
        });
    };
    if let Some(media) = representation["mediaType"].as_str() {
        add(
            "Content-Type",
            representation["charset"].as_str().map_or_else(
                || media.into(),
                |charset| format!("{media}; charset={charset}"),
            ),
        );
    }
    if let Some(codings) = representation["contentCodings"].as_array() {
        for coding in codings {
            if let Some(value) = coding.as_str() {
                add("Content-Encoding", value.into());
            }
        }
    }
    add("Content-Length", bytes.to_string());
}

fn body_finished(
    representation: Option<&serde_json::Value>,
    completion: BodyCompletion,
    terminal: TerminalState,
    retained_bytes: usize,
) -> bool {
    representation.map_or(
        completion == BodyCompletion::Complete || terminal == TerminalState::Completed,
        |saved| {
            saved["complete"].as_bool() == Some(true)
                && saved["retainedBytes"].as_u64() == u64::try_from(retained_bytes).ok()
        },
    )
}

fn render_request(
    request: &CapturedRequest,
    body: &[u8],
    trailers: &[CapturedHeader],
    protocol: &str,
) -> Result<Vec<u8>, SazError> {
    validate_line_component(request.method.as_bytes())?;
    validate_line_component(request.target.as_bytes())?;
    let target = if request.method == "CONNECT" {
        let uri: http::Uri = request
            .target
            .parse()
            .map_err(|_| SazError::UnsafeWireField)?;
        let authority = uri.authority().ok_or(SazError::UnsafeWireField)?;
        format!(
            "{}:{}",
            authority.host(),
            authority.port_u16().unwrap_or(443)
        )
    } else {
        request.target.clone()
    };
    let mut output = Vec::new();
    output.extend_from_slice(request.method.as_bytes());
    output.push(b' ');
    output.extend_from_slice(target.as_bytes());
    output.extend_from_slice(format!(" {protocol}\r\n").as_bytes());
    render_entity(&mut output, &request.headers, body, trailers)?;
    Ok(output)
}

fn render_response(
    response: &CapturedResponse,
    body: &[u8],
    trailers: &[CapturedHeader],
    protocol: &str,
    reason: &str,
) -> Result<Vec<u8>, SazError> {
    if !(100..=999).contains(&response.status) {
        return Err(SazError::UnsafeWireField);
    }
    let mut output = format!("{protocol} {} {reason}\r\n", response.status).into_bytes();
    if reason.contains(['\r', '\n']) {
        return Err(SazError::UnsafeWireField);
    }
    let headers = cache_headers_first(&response.headers);
    render_entity(&mut output, &headers, body, trailers)?;
    Ok(output)
}

fn cache_headers_first(headers: &[CapturedHeader]) -> std::borrow::Cow<'_, [CapturedHeader]> {
    let positions = headers
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            (field.name.eq_ignore_ascii_case(b"cache-control")
                || field.name.eq_ignore_ascii_case(b"expires"))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut expiry_seen = false;
    let needs_ordering = positions.iter().any(|&index| {
        if headers[index].name.eq_ignore_ascii_case(b"expires") {
            expiry_seen = true;
            false
        } else {
            expiry_seen
        }
    });
    if !needs_ordering {
        return std::borrow::Cow::Borrowed(headers);
    }
    let ordered = positions
        .iter()
        .filter(|&&index| headers[index].name.eq_ignore_ascii_case(b"cache-control"))
        .chain(
            positions
                .iter()
                .filter(|&&index| headers[index].name.eq_ignore_ascii_case(b"expires")),
        )
        .map(|&index| headers[index].clone());
    let mut result = headers.to_vec();
    for (index, field) in positions.iter().copied().zip(ordered) {
        result[index] = field;
    }
    std::borrow::Cow::Owned(result)
}

fn render_headers(output: &mut Vec<u8>, headers: &[CapturedHeader]) -> Result<(), SazError> {
    for header in headers {
        let Some(value) = &header.value else {
            continue;
        };
        if header.name.contains(&b'\r')
            || header.name.contains(&b'\n')
            || value.contains(&b'\r')
            || value.contains(&b'\n')
        {
            return Err(SazError::UnsafeWireField);
        }
        output.extend_from_slice(&header.name);
        output.extend_from_slice(b": ");
        output.extend_from_slice(value);
        output.extend_from_slice(b"\r\n");
    }
    Ok(())
}

fn validate_line_component(value: &[u8]) -> Result<(), SazError> {
    if value.is_empty() || value.contains(&b'\r') || value.contains(&b'\n') {
        return Err(SazError::UnsafeWireField);
    }
    Ok(())
}

fn render_entity(
    output: &mut Vec<u8>,
    headers: &[CapturedHeader],
    body: &[u8],
    trailers: &[CapturedHeader],
) -> Result<(), SazError> {
    // Native segments contain entity bytes, never HTTP chunk framing. Normalize
    // framing only when captured Transfer-Encoding or actual trailers require it.
    let framed = !trailers.is_empty()
        || headers
            .iter()
            .any(|field| field.name.eq_ignore_ascii_case(b"transfer-encoding"));
    if !framed {
        render_headers(output, headers)?;
        output.extend_from_slice(b"\r\n");
        output.extend_from_slice(body);
        return Ok(());
    }
    let fields = headers
        .iter()
        .filter(|field| {
            ![
                b"transfer-encoding".as_slice(),
                b"content-length",
                b"trailer",
            ]
            .iter()
            .any(|name| field.name.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect::<Vec<_>>();
    render_headers(output, &fields)?;
    output.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
    if !body.is_empty() {
        output.extend_from_slice(format!("{:X}\r\n", body.len()).as_bytes());
        output.extend_from_slice(body);
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(b"0\r\n");
    render_headers(output, trailers)?;
    output.extend_from_slice(b"\r\n");
    Ok(())
}

const fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

fn connection_reason(session: &Session) -> &'static str {
    use transmog_core::performance::TransportOutcome;

    match session
        .evidence
        .performance
        .transports
        .iter()
        .find(|transport| transport.leg == "upstream")
        .map(|transport| transport.outcome)
        .unwrap_or_default()
    {
        TransportOutcome::Connected => "Connection Established",
        TransportOutcome::DnsFailed => "Transmog - DNS Lookup Failed",
        TransportOutcome::TcpFailed => "Transmog - Connection Failed",
        TransportOutcome::TlsFailed => "Transmog - TLS Handshake Failed",
        TransportOutcome::PolicyRejected => "Transmog - Connection Rejected",
        TransportOutcome::Unavailable => "Transmog - Upstream outcome unavailable",
    }
}

fn response_reason<'a>(
    session: &'a Session,
    response: &CapturedResponse,
    websocket_handshake: bool,
    diagnostic_reason: &'a str,
) -> &'a str {
    if session.response.is_none() {
        diagnostic_reason
    } else if websocket_handshake {
        "Switching Protocols"
    } else if session.kind == SessionKind::ConnectionContext {
        connection_reason(session)
    } else if session
        .request
        .as_ref()
        .is_some_and(|request| request.method == "CONNECT")
        && response.status == 200
        && metadata::reason(session).is_none_or(|reason| reason == "OK")
    {
        "Connection Established"
    } else {
        metadata::reason(session).unwrap_or_else(|| reason_phrase(response.status))
    }
}

fn write_http_session<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    id: usize,
    session: &Session,
    options: FileOptions<'_, ()>,
    remaining: usize,
) -> Result<bool, SazError> {
    if remaining < 3 {
        return Err(SazError::EntryLimitExceeded);
    }
    let placeholder_request = CapturedRequest {
        method: "GET".into(),
        target: "http://transmog.invalid/uncaptured-request".into(),
        headers: vec![
            placeholder_header("Request headers were not captured"),
            length_header(session.request_body.len()),
        ],
    };
    let diagnostic = response_diagnostic(session);
    let placeholder_response = CapturedResponse {
        status: diagnostic.0,
        headers: vec![
            placeholder_header("Response headers were not captured"),
            connections::header(
                "X-Transmog-Generated",
                "SAZ diagnostic; not a server response",
            ),
        ],
    };
    let request = session.request.as_ref().unwrap_or(&placeholder_request);
    let websocket_request = websocket::request(session, request);
    let request = websocket_request.as_ref().unwrap_or(request);
    let request_incomplete = session.request_body_incomplete
        || session.loss
        || !body_finished(
            session.request_representation.as_ref(),
            session.request_body_completion,
            session.terminal,
            session.request_body.len(),
        );
    let response_incomplete = session.response_body_incomplete
        || session.loss
        || !body_finished(
            session.response_representation.as_ref(),
            session.response_body_completion,
            session.terminal,
            session.response_body.len(),
        );
    let request_wire = render_request(
        request,
        &session.request_body,
        &session.evidence.request_trailers,
        &metadata::protocol(session, "client-request"),
    )?;
    // Preserve retained bytes even when their head is missing. Only an empty,
    // uncaptured response gets explanatory text for ordinary Fiddler inspectors.
    let response_body = if session.response.is_none() && session.response_body.is_empty() {
        diagnostic.2.as_bytes()
    } else {
        &session.response_body
    };
    let mut placeholder_response = placeholder_response;
    placeholder_response
        .headers
        .push(length_header(response_body.len()));
    if session.response_body.is_empty() {
        placeholder_response.headers.push(CapturedHeader {
            name: b"Content-Type".to_vec(),
            value: Some(b"text/plain; charset=utf-8".to_vec()),
            original_value_bytes: None,
        });
    }
    let response = session.response.as_ref().unwrap_or(&placeholder_response);
    let opaque_response = connections::opaque_response(session, response);
    let response = opaque_response.as_ref().unwrap_or(response);
    let websocket_response = websocket::response(session, response);
    let response = websocket_response.as_ref().unwrap_or(response);
    let reason = response_reason(
        session,
        response,
        websocket_response.is_some(),
        diagnostic.1,
    );
    let response_wire = render_response(
        response,
        response_body,
        &session.evidence.response_trailers,
        &metadata::protocol(session, "client-response"),
        reason,
    )?;
    let metadata = metadata::render(session, id, request_incomplete, response_incomplete);
    write_member(writer, &format!("raw/{id}_c.txt"), &request_wire, options)?;
    write_member(writer, &format!("raw/{id}_s.txt"), &response_wire, options)?;
    write_member(
        writer,
        &format!("raw/{id}_m.xml"),
        metadata.as_bytes(),
        options,
    )?;
    Ok(request_incomplete || response_incomplete)
}

fn ordering_time(session: &Session) -> Option<u128> {
    session
        .started_unix_nanos
        .or_else(|| {
            session
                .evidence
                .performance
                .points
                .iter()
                .find(|point| {
                    point.milestone == transmog_core::performance::Milestone::RequestHeaders
                })
                .map(|point| u128::from(point.unix_millis) * 1_000_000)
        })
        .or(session.ordering_unix_nanos)
}

fn response_diagnostic(session: &Session) -> (u16, &'static str, String) {
    use transmog_core::performance::TransportOutcome;
    let outcome = session
        .evidence
        .performance
        .transports
        .iter()
        .rev()
        .find(|transport| transport.leg == "upstream")
        .map(|transport| transport.outcome);
    let (status, reason, explanation) = match outcome {
        Some(TransportOutcome::DnsFailed) => (
            502,
            "Transmog - DNS Lookup Failed",
            "DNS lookup failed while opening the upstream connection.",
        ),
        Some(TransportOutcome::TcpFailed) => (
            502,
            "Transmog - Connection Failed",
            "The upstream TCP connection could not be opened.",
        ),
        Some(TransportOutcome::TlsFailed) => (
            502,
            "Transmog - TLS Handshake Failed",
            "Upstream TLS negotiation or certificate verification failed.",
        ),
        _ if pending_at_save(session) => (
            599,
            "Transmog - Request still active at save",
            "This exchange was still active when the trace was saved.",
        ),
        _ => (
            599,
            "Transmog response not captured",
            "Response headers were not captured.",
        ),
    };
    let target = session
        .request
        .as_ref()
        .map_or("Unavailable", |request| request.target.as_str());
    let mut body = format!("[Transmog] {explanation}\r\nTarget: {target}\r\n");
    if let Some(failure) = &session.evidence.failure {
        use std::fmt::Write as _;
        let _ = write!(
            body,
            "Recorded failure ({}): {}\r\n",
            failure.category, failure.message
        );
    }
    body.push_str(
        "\r\nThis is not a server response. This diagnostic was generated for SAZ export.\r\n",
    );
    (status, reason, body)
}

fn pending_at_save(session: &Session) -> bool {
    let message = "This exchange was still active when the trace was saved.";
    session
        .evidence
        .failure
        .as_ref()
        .is_some_and(|failure| failure.category == "incomplete" || failure.message == message)
        || session
            .evidence
            .provenance
            .as_ref()
            .and_then(|entry| entry["source"]["diagnostics"].as_array())
            .is_some_and(|notes| {
                notes.iter().any(|note| {
                    note.as_str() == Some(&format!("Saved failure (incomplete): {message}"))
                })
            })
}

fn placeholder_header(message: &str) -> CapturedHeader {
    CapturedHeader {
        name: b"X-Transmog-Placeholder".to_vec(),
        value: Some(message.as_bytes().to_vec()),
        original_value_bytes: None,
    }
}

fn length_header(bytes: usize) -> CapturedHeader {
    CapturedHeader {
        name: b"Content-Length".to_vec(),
        value: Some(bytes.to_string().into_bytes()),
        original_value_bytes: None,
    }
}

fn write_trace_context<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    capture: &RecoveredCapture,
    options: FileOptions<'_, ()>,
    remaining: usize,
) -> Result<usize, SazError> {
    let metadata = capture
        .records
        .iter()
        .rev()
        .find_map(|record| match &record.kind {
            CaptureRecordKind::Unknown { kind, payload }
                if record.exchange_id == 0 && kind == "trace-metadata" =>
            {
                Some(payload)
            }
            _ => None,
        });
    let Some(metadata) = metadata else {
        return Ok(0);
    };
    if remaining == 0 {
        return Err(SazError::EntryLimitExceeded);
    }
    let bytes = serde_json::to_vec(metadata)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(SazError::MetadataLimitExceeded);
    }
    write_member(writer, "transmog/trace-metadata.json", &bytes, options)?;
    Ok(1)
}

fn write_member<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    name: &str,
    bytes: &[u8],
    options: FileOptions<'_, ()>,
) -> Result<(), SazError> {
    writer.start_file(name, options)?;
    writer.write_all(bytes)?;
    Ok(())
}

/// SAZ conversion failure.
#[derive(Debug, Error)]
pub enum SazError {
    /// Encrypted archive requires a transient password.
    #[error("This SAZ archive requires a password")]
    PasswordRequired,
    /// Supplied password was rejected by the ZIP encryption reader.
    #[error("The SAZ password is incorrect")]
    InvalidPassword,
    /// Archive member names collide or escape the archive namespace.
    #[error("SAZ contains an unsafe or duplicate member name")]
    UnsafeMember,
    /// Archive content uses an unsupported compression or encryption method.
    #[error("SAZ member compression or encryption is unsupported")]
    UnsupportedMember,
    /// Malformed HTTP framing or session metadata.
    #[error("SAZ contains malformed HTTP or metadata")]
    InvalidArchive,
    /// A finite import byte or metadata bound was exceeded.
    #[error("SAZ import resource limit exceeded")]
    ImportLimitExceeded,
    /// Caller canceled the bounded import.
    #[error("SAZ import canceled")]
    Canceled,
    /// Limits were zero or inconsistent.
    #[error("SAZ limits are invalid")]
    InvalidLimits,
    /// Exporter was already consumed.
    #[error("SAZ exporter has already produced an archive")]
    AlreadyExported,
    /// Destination was requested before export.
    #[error("SAZ exporter has not produced an archive")]
    NotExported,
    /// Session count exceeded its finite bound.
    #[error("SAZ session limit exceeded")]
    SessionLimitExceeded,
    /// ZIP member count exceeded its finite bound.
    #[error("SAZ entry limit exceeded")]
    EntryLimitExceeded,
    /// Capture context exceeds the bounded importer-compatible metadata size.
    #[error("SAZ trace metadata exceeds its four MiB limit")]
    MetadataLimitExceeded,
    /// Retained body bytes exceeded the per-direction bound.
    #[error("SAZ body limit exceeded")]
    BodyLimitExceeded,
    /// Captured values could inject or corrupt raw HTTP wire rendering.
    #[error("SAZ capture contains an unsafe HTTP wire field")]
    UnsafeWireField,
    /// ZIP writer failed.
    #[error("SAZ ZIP failure: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Destination I/O failed.
    #[error("SAZ I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// Extended manifest serialization failed.
    #[error("SAZ manifest failure: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use transmog_capture::CaptureRecord;

    use super::*;

    #[test]
    fn connect_exports_an_authority_form_destination_instead_of_a_url() {
        for (target, authority) in [
            ("https://example.test:8443/", "example.test:8443"),
            ("https://[::1]:443/", "[::1]:443"),
            ("https://example.test/", "example.test:443"),
        ] {
            let request = CapturedRequest {
                method: "CONNECT".into(),
                target: target.into(),
                headers: Vec::new(),
            };
            let bytes = render_request(&request, &[], &[], "HTTP/1.1").unwrap();
            assert!(bytes.starts_with(format!("CONNECT {authority} HTTP/1.1\r\n").as_bytes()));
        }
    }

    #[test]
    fn encrypted_saz_round_trips_and_produces_interop_fixtures() {
        let password = transmog_capture::CapturePassword::new("Transmog-interop-password".into());
        let capture = capture(true, true);
        for (name, mode) in [("strict", SazMode::Strict), ("extended", SazMode::Extended)] {
            let mut exporter =
                SazExporter::new(Cursor::new(Vec::new()), mode, SazLimits::default())
                    .unwrap()
                    .encrypted(password.clone())
                    .unwrap();
            exporter.export(&capture).unwrap();
            let bytes = exporter.into_inner().unwrap().into_inner();
            assert!(matches!(
                SazArchive::open(Cursor::new(bytes.clone()), SazImportLimits::default()),
                Err(SazError::PasswordRequired)
            ));
            let mut wrong = SazArchive::with_password(
                Cursor::new(bytes.clone()),
                SazImportLimits::default(),
                Some(transmog_capture::CapturePassword::new("incorrect".into())),
            )
            .unwrap();
            assert!(matches!(
                wrong.index(|_, _| {}, || false),
                Err(SazError::InvalidPassword)
            ));
            let mut archive = SazArchive::with_password(
                Cursor::new(bytes.clone()),
                SazImportLimits::default(),
                Some(password.clone()),
            )
            .unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            assert_eq!(index.sessions.len(), 1);
            let mut body = Vec::new();
            archive
                .copy_body(
                    &index.sessions[0].request.as_ref().unwrap().body,
                    &mut body,
                    || false,
                )
                .unwrap();
            assert_eq!(body, b"req");
            if let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR") {
                let root = std::path::PathBuf::from(directory);
                std::fs::write(root.join(format!("{name}-encrypted.saz")), bytes).unwrap();
                std::fs::write(
                    root.join(format!("{name}-plain.saz")),
                    export(mode, &capture).0,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn external_zipcrypto_imports_exact_binary_without_extraction() {
        let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR") else {
            return;
        };
        let bytes =
            std::fs::read(std::path::PathBuf::from(directory).join("external-zipcrypto.saz"))
                .unwrap();
        let mut archive = SazArchive::with_password(
            Cursor::new(bytes),
            SazImportLimits::default(),
            Some(transmog_capture::CapturePassword::new(
                "Transmog-interop-password".into(),
            )),
        )
        .unwrap();
        let index = archive.index(|_, _| {}, || false).unwrap();
        let mut body = Vec::new();
        archive
            .copy_body(
                &index.sessions[0].request.as_ref().unwrap().body,
                &mut body,
                || false,
            )
            .unwrap();
        assert_eq!(body.len(), 262_144);
        assert!(
            body.iter()
                .enumerate()
                .all(|(index, byte)| usize::from(*byte) == index % 256)
        );
    }

    #[test]
    fn aes_128_192_and_256_archives_import_without_extraction() {
        for mode in [
            zip::AesMode::Aes128,
            zip::AesMode::Aes192,
            zip::AesMode::Aes256,
        ] {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file(
                    "raw/1_c.txt",
                    SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Deflated)
                        .with_aes_encryption(mode, "password"),
                )
                .unwrap();
            writer
                .write_all(
                    b"POST https://example.test/upload HTTP/1.1\r\nContent-Length: 3\r\n\r\nreq",
                )
                .unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            let mut archive = SazArchive::with_password(
                Cursor::new(bytes),
                SazImportLimits::default(),
                Some(transmog_capture::CapturePassword::new("password".into())),
            )
            .unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            let mut body = Vec::new();
            archive
                .copy_body(
                    &index.sessions[0].request.as_ref().unwrap().body,
                    &mut body,
                    || false,
                )
                .unwrap();
            assert_eq!(body, b"req");
        }
    }

    fn capture(include_response: bool, include_body: bool) -> RecoveredCapture {
        let headers = vec![
            CapturedHeader {
                name: b"host".to_vec(),
                value: Some(b"example.test".to_vec()),
                original_value_bytes: None,
            },
            CapturedHeader {
                name: b"authorization".to_vec(),
                value: None,
                original_value_bytes: None,
            },
        ];
        let mut records = vec![CaptureRecord {
            sequence: 1,
            exchange_id: 42,
            kind: CaptureRecordKind::RequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                method: "POST".to_owned(),
                target: "https://example.test/upload".to_owned(),
                headers: headers.clone(),
            },
        }];
        if include_body {
            records.push(CaptureRecord {
                sequence: 2,
                exchange_id: 42,
                kind: CaptureRecordKind::BodySegment {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 3,
                    bytes: Some(b"req".to_vec()),
                    truncated: false,
                },
            });
        } else {
            records.push(CaptureRecord {
                sequence: 2,
                exchange_id: 42,
                kind: CaptureRecordKind::BodySegment {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 3,
                    bytes: None,
                    truncated: true,
                },
            });
        }
        if include_response {
            records.extend([
                CaptureRecord {
                    sequence: 3,
                    exchange_id: 42,
                    kind: CaptureRecordKind::ResponseHead {
                        boundary: ExchangeBoundary::ClientResponse,
                        status: 200,
                        headers: vec![CapturedHeader {
                            name: b"content-type".to_vec(),
                            value: Some(b"text/plain".to_vec()),
                            original_value_bytes: None,
                        }],
                    },
                },
                CaptureRecord {
                    sequence: 4,
                    exchange_id: 42,
                    kind: CaptureRecordKind::BodySegment {
                        boundary: ExchangeBoundary::ClientResponse,
                        byte_count: 3,
                        bytes: Some(b"res".to_vec()),
                        truncated: false,
                    },
                },
                CaptureRecord {
                    sequence: 5,
                    exchange_id: 42,
                    kind: CaptureRecordKind::Completed,
                },
            ]);
        }
        RecoveredCapture {
            records,
            valid_bytes: 0,
            truncated_tail: false,
            sealed: true,
        }
    }

    fn export(mode: SazMode, capture: &RecoveredCapture) -> (Vec<u8>, SazReport) {
        let mut exporter =
            SazExporter::new(Cursor::new(Vec::new()), mode, SazLimits::default()).unwrap();
        exporter.export(capture).unwrap();
        let report = exporter.report().unwrap();
        let bytes = exporter.into_inner().unwrap().into_inner();
        (bytes, report)
    }

    fn member(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
        let mut file = archive.by_name(name).unwrap();
        let mut value = Vec::new();
        file.read_to_end(&mut value).unwrap();
        value
    }

    #[test]
    fn recorded_process_identity_survives_both_saz_profiles() {
        use transmog_core::ClientIdentity;
        for identity in [
            ClientIdentity::LocalProcess {
                pid: 42,
                name: Some("browser & helper.exe".into()),
            },
            ClientIdentity::LocalProcess {
                pid: 42,
                name: None,
            },
            ClientIdentity::LocalUnknown,
            ClientIdentity::Remote,
        ] {
            for mode in [SazMode::Strict, SazMode::Extended] {
                let mut source = capture(true, true);
                source.records.insert(
                    0,
                    CaptureRecord {
                        sequence: 0,
                        exchange_id: 42,
                        kind: CaptureRecordKind::ExchangeStarted {
                            client_addr: "127.0.0.1:1234".into(),
                            client_identity: identity.clone(),
                            listener_addr: "127.0.0.1:8888".into(),
                            authority: "example.test".into(),
                            started_unix_nanos: 1_800_000_000_000_000_000,
                        },
                    },
                );
                let (bytes, _) = export(mode, &source);
                let mut archive =
                    SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
                let index = archive.index(|_, _| {}, || false).unwrap();
                let session = &index.sessions[0];
                let expected = if let ClientIdentity::LocalProcess { pid, name } = &identity {
                    assert_eq!(
                        session.metadata.flags["x-processinfo"],
                        format!("{}:{pid}", name.as_deref().unwrap_or_default())
                    );
                    Some(identity.clone())
                } else {
                    assert!(!session.metadata.flags.contains_key("x-processinfo"));
                    None
                };
                assert_eq!(session.metadata.client_identity(), expected);
                if mode == SazMode::Extended {
                    assert_eq!(
                        session.evidence.as_ref().unwrap().client_identity.as_ref(),
                        Some(&identity)
                    );
                }
            }
        }
    }

    #[test]
    fn cache_control_precedes_expires_without_changing_values_or_extended_headers() {
        let mut source = capture(true, true);
        if let CaptureRecordKind::ResponseHead { headers, .. } = &mut source.records[2].kind {
            headers.extend([
                connections::header("Expires", "Thu, 01 Jan 1970 00:00:00 GMT"),
                connections::header("Cache-Control", "private, must-revalidate, max-age=0"),
                connections::header(
                    "Set-Cookie",
                    "fixture=value; Expires=Sun, 11 Oct 2026 00:00:00 GMT",
                ),
                connections::header("Cache-Control", "no-transform"),
            ]);
        }
        for mode in [SazMode::Strict, SazMode::Extended] {
            let (bytes, _) = export(mode, &source);
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let raw = String::from_utf8(member(&mut archive, "raw/1_s.txt")).unwrap();
            assert!(
                raw.find("Cache-Control: private").unwrap() < raw.find("Expires: Thu").unwrap()
            );
            assert!(
                raw.find("Cache-Control: no-transform").unwrap()
                    < raw.find("Expires: Thu").unwrap()
            );
            assert!(raw.contains("Set-Cookie: fixture=value; Expires=Sun"));
            if mode == SazMode::Extended {
                let evidence: SessionEvidence =
                    serde_json::from_slice(&member(&mut archive, "transmog/session-1.json"))
                        .unwrap();
                let fields = evidence.response_headers.unwrap();
                assert_eq!(fields[1].name, b"Expires");
                assert_eq!(fields[2].name, b"Cache-Control");
            }
        }
    }

    #[test]
    fn generated_connections_use_http_tunnel_flags_and_no_fiddler_attribution() {
        for mode in [SazMode::Strict, SazMode::Extended] {
            let mut source = capture(true, true);
            source.records.push(CaptureRecord {
                sequence: 6,
                exchange_id: 42,
                kind: CaptureRecordKind::Performance(connections::tests::evidence()),
            });
            let (bytes, report) = export(mode, &source);
            assert_eq!(report.sessions, 2);
            assert_eq!(report.connection_sessions, 1);
            let mut archive =
                SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            let summary = &index.sessions[1];
            assert_eq!(
                summary.request.as_ref().unwrap().start_line,
                "CONNECT example.test:443 HTTP/1.1"
            );
            assert_eq!(
                summary.response.as_ref().unwrap().start_line,
                "HTTP/1.1 200 Connection Established"
            );
            assert_eq!(summary.metadata.bit_flags, 0x2400);
            assert_eq!(
                summary.metadata.flags["x-transmog-connection-context"],
                "true"
            );
            assert!(!summary.request.as_ref().unwrap().body.dropped);
            assert!(!summary.response.as_ref().unwrap().body.dropped);
            assert!(
                summary
                    .response
                    .as_ref()
                    .unwrap()
                    .headers
                    .values("Content-Type")
                    .next()
                    .is_none()
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Verify wire ordering, metadata, manifest association and headers together.
    fn connection_attempts_are_separate_chronological_rows_with_recorded_outcomes() {
        use transmog_core::performance::{
            ConnectionTiming, Milestone, TimingPoint, TransportOutcome,
        };
        for mode in [SazMode::Strict, SazMode::Extended] {
            let mut source = capture(true, true);
            let mut performance = connections::tests::evidence();
            performance.points.push(TimingPoint {
                milestone: Milestone::RequestHeaders,
                unix_millis: 1_800_000_000_000,
                offset_micros: 0,
            });
            let mut first = performance.transports[1].clone();
            first.setup_timings = vec![ConnectionTiming {
                phase: "tcp".into(),
                began_offset_micros: -3000,
                ended_offset_micros: -2000,
                request_wait_micros: 0,
            }];
            first.sampled_offset_micros = 500;
            first.bytes_read = Some(11);
            first.bytes_written = Some(22);
            let mut failed = first.clone();
            failed.connection_id = "failed-origin".into();
            failed.outcome = TransportOutcome::DnsFailed;
            failed.setup_timings[0].began_offset_micros = -1000;
            failed.setup_timings[0].ended_offset_micros = -500;
            performance.transports[1] = first;
            performance.transports.push(failed);
            source.records.push(CaptureRecord {
                sequence: 6,
                exchange_id: 42,
                kind: CaptureRecordKind::Performance(performance),
            });
            let (bytes, report) = export(mode, &source);
            assert_eq!((report.sessions, report.connection_sessions), (3, 2));
            let mut archive =
                SazArchive::open(Cursor::new(bytes.clone()), SazImportLimits::default()).unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            let first = &index.sessions[0];
            let failed = &index.sessions[1];
            let request = &index.sessions[2];
            assert!(
                first
                    .request
                    .as_ref()
                    .unwrap()
                    .start_line
                    .starts_with("CONNECT ")
            );
            assert!(
                request
                    .request
                    .as_ref()
                    .unwrap()
                    .start_line
                    .starts_with("POST ")
            );
            assert_eq!(
                first.response.as_ref().unwrap().start_line,
                "HTTP/1.1 200 Connection Established"
            );
            assert_eq!(
                failed.response.as_ref().unwrap().start_line,
                "HTTP/1.1 502 Transmog - DNS Lookup Failed"
            );
            assert_eq!(failed.metadata.bit_flags, 0x400);
            for entry in [first, failed] {
                let response = entry.response.as_ref().unwrap();
                assert!(response.headers.values("Content-Type").next().is_none());
                assert!(response.headers.values("EndTime").next().is_none());
                assert_eq!(
                    response.headers.values("ClientToServerBytes").next(),
                    Some(b"22".as_slice())
                );
                assert_eq!(
                    response.headers.values("ServerToClientBytes").next(),
                    Some(b"11".as_slice())
                );
                assert_eq!(
                    response.headers.values("Transmog-Time-Zone").next(),
                    Some(b"UTC".as_slice())
                );
            }
            assert!(
                first.metadata.timers["ClientBeginRequest"]
                    < failed.metadata.timers["ClientBeginRequest"]
            );
            assert!(
                failed.metadata.timers["ClientBeginRequest"]
                    < request.metadata.timers["ClientBeginRequest"]
            );
            assert!(
                failed
                    .response
                    .as_ref()
                    .unwrap()
                    .headers
                    .values("Timestamp")
                    .next()
                    .is_some()
            );
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            for id in 1..=2 {
                let raw = String::from_utf8(member(&mut zip, &format!("raw/{id}_s.txt"))).unwrap();
                assert_eq!(raw.matches("== Capturing proxy to origin ==").count(), 1);
            }
            if mode == SazMode::Extended {
                let manifest: serde_json::Value =
                    serde_json::from_slice(&member(&mut zip, "transmog/manifest.json")).unwrap();
                assert_eq!(manifest["sessions"][0]["saz_id"], 3);
                assert!(zip.by_name("transmog/session-3.json").is_ok());
            }
        }
    }

    fn failed_capture(saved: bool) -> RecoveredCapture {
        let mut source = capture(true, true);
        let mut records = Vec::new();
        for mut record in source.records {
            if let CaptureRecordKind::BodySegment { boundary, .. } = record.kind {
                let name = match boundary {
                    ExchangeBoundary::ClientRequest => "client-request",
                    ExchangeBoundary::ClientResponse => "client-response",
                    _ => unreachable!(),
                };
                if saved {
                    records.push(CaptureRecord {
                        exchange_id: 42,
                        sequence: 0,
                        kind: CaptureRecordKind::Unknown {
                            kind: "body-representation".into(),
                            payload: serde_json::json!({
                                "boundary":name,"complete":true,"observedBytes":3,
                                "retainedBytes":3,"mediaType":"text/plain","charset":null,
                                "contentCodings":[]
                            }),
                        },
                    });
                }
                records.push(record);
                if !saved {
                    records.push(CaptureRecord {
                        exchange_id: 42,
                        sequence: 0,
                        kind: CaptureRecordKind::Unknown {
                            kind: "body-completed".into(),
                            payload: serde_json::json!({"boundary":name}),
                        },
                    });
                }
                continue;
            }
            if matches!(record.kind, CaptureRecordKind::Completed) {
                record.kind = CaptureRecordKind::Failed {
                    category: "Body".into(),
                    message: "downstream response-body consumer closed".into(),
                };
            }
            records.push(record);
        }
        for (index, record) in records.iter_mut().enumerate() {
            record.sequence = index as u64 + 1;
        }
        source.records = records;
        source
    }

    #[test]
    fn legacy_failure_categories_and_long_explanations_export_without_field_caps() {
        use transmog_core::intercept::{ExchangeFailureKind, HookAbort};
        let category = format!(
            "{:?}",
            ExchangeFailureKind::HookAborted(HookAbort::Policy("policy explanation ".repeat(100)))
        );
        let message = "拒否された <policy> explanation.\r\n\tNext line & details.\n".repeat(400);
        assert!(category.len() > 512 && message.len() > 8192);
        let mut source = capture(true, true);
        source.records.last_mut().unwrap().kind = CaptureRecordKind::Failed {
            category: category.clone(),
            message: message.clone(),
        };
        for mode in [SazMode::Strict, SazMode::Extended] {
            let (bytes, report) = export(mode, &source);
            assert_eq!(report.sessions, 1);
            let mut archive =
                SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            assert_eq!(
                index.sessions[0].metadata.flags["x-transmog-failure-category"],
                category
            );
            assert_eq!(
                index.sessions[0].metadata.flags["x-transmog-failure-message"],
                message
            );
            if let Some(evidence) = &index.sessions[0].evidence {
                let failure = evidence.failure.as_ref().unwrap();
                assert_eq!(failure.category, category);
                assert_eq!(failure.message, message);
            }
        }
    }

    #[test]
    fn failed_exchanges_keep_complete_bodies_in_both_saz_profiles() {
        for (evidence, saved) in [("saved", true), ("recorded", false)] {
            for (profile, mode) in [("strict", SazMode::Strict), ("extended", SazMode::Extended)] {
                let (bytes, report) = export(mode, &failed_capture(saved));
                assert_eq!(report.incomplete_bodies, 0, "{profile}/{evidence}");
                if let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR") {
                    std::fs::write(
                        std::path::PathBuf::from(directory)
                            .join(format!("{profile}-failed-{evidence}-plain.saz")),
                        &bytes,
                    )
                    .unwrap();
                }
                let mut archive =
                    SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
                let index = archive.index(|_, _| {}, || false).unwrap();
                let session = &index.sessions[0];
                assert_eq!(session.metadata.flags["x-transmog-terminal"], "failed");
                for (message, expected) in [
                    (session.request.as_ref().unwrap(), b"req"),
                    (session.response.as_ref().unwrap(), b"res"),
                ] {
                    assert!(!message.body.dropped, "{profile}/{evidence}");
                    let mut retained = Vec::new();
                    archive
                        .copy_body(&message.body, &mut retained, || false)
                        .unwrap();
                    assert_eq!(retained, expected);
                }
            }
        }
    }

    #[test]
    fn completion_evidence_does_not_hide_incomplete_bodies() {
        for case in [
            "wrong-length",
            "unfinished",
            "truncated",
            "loss",
            "omitted",
            "no-marker",
            "late-data",
        ] {
            let mut source = failed_capture(!matches!(case, "no-marker" | "late-data"));
            if case == "no-marker" {
                source.records.retain(|record| {
                    !matches!(&record.kind,
                        CaptureRecordKind::Unknown { kind, payload }
                            if kind == "body-completed" && payload["boundary"] == "client-response"
                    )
                });
            }
            for record in &mut source.records {
                match &mut record.kind {
                    CaptureRecordKind::Unknown { kind, payload }
                        if kind == "body-representation"
                            && payload["boundary"] == "client-response" =>
                    {
                        if case == "wrong-length" {
                            payload["retainedBytes"] = serde_json::json!(4);
                        } else if case == "unfinished" {
                            payload["complete"] = serde_json::json!(false);
                        }
                    }
                    CaptureRecordKind::BodySegment {
                        boundary: ExchangeBoundary::ClientResponse,
                        bytes,
                        truncated,
                        ..
                    } => {
                        if case == "truncated" {
                            *truncated = true;
                        }
                        if case == "omitted" {
                            *bytes = None;
                        }
                    }
                    _ => {}
                }
            }
            if case == "loss" {
                source.records.push(CaptureRecord {
                    exchange_id: 42,
                    sequence: 9,
                    kind: CaptureRecordKind::Loss {
                        first_missing_sequence: 3,
                        count: 1,
                        reason: "observer delivery gap".into(),
                    },
                });
            } else if case == "late-data" {
                source.records.insert(
                    source.records.len() - 1,
                    CaptureRecord {
                        exchange_id: 42,
                        sequence: 9,
                        kind: CaptureRecordKind::BodySegment {
                            boundary: ExchangeBoundary::ClientResponse,
                            byte_count: 4,
                            bytes: Some(b"late".to_vec()),
                            truncated: false,
                        },
                    },
                );
            }
            for (profile, mode) in [("strict", SazMode::Strict), ("extended", SazMode::Extended)] {
                let (bytes, report) = export(mode, &source);
                assert_eq!(report.incomplete_bodies, 1, "{profile}/{case}");
                if case == "unfinished"
                    && let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR")
                {
                    std::fs::write(
                        std::path::PathBuf::from(directory)
                            .join(format!("{profile}-failed-unfinished-plain.saz")),
                        &bytes,
                    )
                    .unwrap();
                }
                let mut archive =
                    SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
                let index = archive.index(|_, _| {}, || false).unwrap();
                assert!(
                    index.sessions[0].response.as_ref().unwrap().body.dropped,
                    "{profile}/{case}"
                );
                assert_eq!(
                    index.sessions[0].request.as_ref().unwrap().body.dropped,
                    case == "loss",
                    "{profile}/{case}"
                );
            }
        }
    }

    #[test]
    fn classic_metadata_required_dates_preserve_unavailable_and_original_times() {
        use transmog_core::performance::{Milestone, PerformanceEvidence, TimingPoint};

        for case in ["missing", "partial", "imported"] {
            let mut source = capture(true, true);
            if case != "missing" {
                source.records.push(CaptureRecord {
                    sequence: 6,
                    exchange_id: 42,
                    kind: CaptureRecordKind::Performance(PerformanceEvidence {
                        points: vec![
                            TimingPoint {
                                milestone: Milestone::RequestHeaders,
                                unix_millis: 1_800_000_000_000,
                                offset_micros: 0,
                            },
                            TimingPoint {
                                milestone: Milestone::ExchangeDone,
                                unix_millis: 1_800_000_000_015,
                                offset_micros: 15_000,
                            },
                        ],
                        ..PerformanceEvidence::default()
                    }),
                });
            }
            let original = serde_json::json!({
                "ClientConnected": "2026-10-08T23:23:35.3381234-07:00",
                "ClientBeginRequest": "2026-10-08T23:23:35.339-07:00",
                "ServerGotRequest": "2026-10-08T23:23:35.340-07:00",
                "ClientDoneRequest": "0001-01-01T00:00:00",
                "DNSTime": "12",
                "CustomTimer": "original & unavailable"
            });
            if case == "imported" {
                source.records.push(CaptureRecord {
                    sequence: 7,
                    exchange_id: 42,
                    kind: CaptureRecordKind::Unknown {
                        kind: "entry-provenance".into(),
                        payload: serde_json::json!({"source": {"timings": original}}),
                    },
                });
            }
            for (profile, mode) in [("strict", SazMode::Strict), ("extended", SazMode::Extended)] {
                let (bytes, _) = export(mode, &source);
                if let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR") {
                    std::fs::write(
                        std::path::PathBuf::from(directory)
                            .join(format!("{profile}-{case}-plain.saz")),
                        &bytes,
                    )
                    .unwrap();
                }
                let mut archive =
                    SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
                let index = archive.index(|_, _| {}, || false).unwrap();
                let timers = &index.sessions[0].metadata.timers;
                for name in [
                    "ClientConnected",
                    "ClientDoneRequest",
                    "ServerGotRequest",
                    "ServerDoneResponse",
                    "ClientBeginResponse",
                    "ClientDoneResponse",
                ] {
                    let expected = if case == "imported" && original.get(name).is_some() {
                        original[name].as_str().unwrap()
                    } else if case != "missing" && name == "ClientDoneResponse" {
                        "2027-01-15T08:00:00.015Z"
                    } else {
                        "0001-01-01T00:00:00"
                    };
                    assert_eq!(timers[name], expected, "{profile}/{case}/{name}");
                }
                assert!(!timers.contains_key("ServerBeginResponse"));
                assert!(!timers.contains_key("TCPConnectTime"));
                if case == "imported" {
                    for (name, value) in original.as_object().unwrap() {
                        assert_eq!(&timers[name], value.as_str().unwrap());
                    }
                    if mode == SazMode::Extended {
                        assert_eq!(
                            index.sessions[0]
                                .evidence
                                .as_ref()
                                .unwrap()
                                .provenance
                                .as_ref()
                                .unwrap()["source"]["timings"],
                            original
                        );
                    }
                } else if case == "partial" {
                    assert_eq!(timers["ClientBeginRequest"], "2027-01-15T08:00:00.000Z");
                    assert_eq!(timers["GotRequestHeaders"], timers["ClientBeginRequest"]);
                } else {
                    assert_eq!(timers.len(), 6);
                }
            }
        }
    }

    #[test]
    fn original_optional_trace_context_survives_both_saz_profiles() {
        for mode in [SazMode::Strict, SazMode::Extended] {
            let mut source = capture(true, true);
            let context = serde_json::json!({"networkContext":{"platform":"fixture","output":"original adapter context"}});
            source.records.insert(
                0,
                CaptureRecord {
                    sequence: 0,
                    exchange_id: 0,
                    kind: CaptureRecordKind::Unknown {
                        kind: "trace-metadata".into(),
                        payload: context.clone(),
                    },
                },
            );
            let (bytes, _) = export(mode, &source);
            let mut archive =
                crate::SazArchive::open(Cursor::new(bytes), crate::SazImportLimits::default())
                    .unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            assert_eq!(index.trace_metadata, Some(context));
        }
    }

    #[test]
    fn extended_evidence_timers_protocol_reason_and_trailers_round_trip() {
        use transmog_core::performance::{
            Milestone, PerformanceEvidence, ProtocolObservation, TimingPoint,
        };
        let mut source = capture(true, true);
        let performance = PerformanceEvidence {
            points: vec![
                TimingPoint {
                    milestone: Milestone::RequestHeaders,
                    unix_millis: 1_800_000_000_000,
                    offset_micros: 0,
                },
                TimingPoint {
                    milestone: Milestone::ExchangeDone,
                    unix_millis: 1_800_000_000_015,
                    offset_micros: 15500,
                },
            ],
            protocols: vec![
                ProtocolObservation {
                    boundary: "client-request".into(),
                    version: "HTTP/2".into(),
                    reason: None,
                },
                ProtocolObservation {
                    boundary: "client-response".into(),
                    version: "HTTP/1.0".into(),
                    reason: Some("Custom & reason".into()),
                },
            ],
            ..PerformanceEvidence::default()
        };
        let trailer = CapturedHeader {
            name: b"X-Checksum".to_vec(),
            value: Some(b"original".to_vec()),
            original_value_bytes: Some(8),
        };
        source.records.extend([
            CaptureRecord {
                exchange_id: 42,
                sequence: 6,
                kind: CaptureRecordKind::Performance(performance.clone()),
            },
            CaptureRecord {
                exchange_id: 42,
                sequence: 7,
                kind: CaptureRecordKind::Trailers {
                    boundary: ExchangeBoundary::ClientResponse,
                    headers: vec![trailer.clone()],
                },
            },
        ]);
        for mode in [SazMode::Strict, SazMode::Extended] {
            let (bytes, _) = export(mode, &source);
            let mut archive =
                SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
            let index = archive.index(|_, _| {}, || false).unwrap();
            let session = &index.sessions[0];
            assert_eq!(
                session.metadata.flags["x-transmog-original-request-protocol"],
                "HTTP/2"
            );
            assert_eq!(
                session.metadata.timers["ClientDoneResponse"]
                    .rsplit('.')
                    .next(),
                Some("015Z")
            );
            assert_eq!(
                session.response.as_ref().unwrap().start_line,
                "HTTP/1.0 200 Custom & reason"
            );
            let mut body = Vec::new();
            let (length, trailers) = archive
                .copy_body_with_trailers(
                    &session.response.as_ref().unwrap().body,
                    &mut body,
                    || false,
                )
                .unwrap();
            assert_eq!((length, body), (3, b"res".to_vec()));
            assert_eq!(
                trailers.values("X-Checksum").next(),
                Some(b"original".as_slice())
            );
            if mode == SazMode::Extended {
                let evidence = session.evidence.as_ref().unwrap();
                assert_eq!(evidence.performance, performance);
                assert_eq!(evidence.response_trailers, vec![trailer.clone()]);
                assert!(
                    evidence
                        .request_headers
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|field| field.name == b"authorization" && field.value.is_none())
                );
            }
        }
    }

    #[test]
    fn strict_archive_has_conventional_triplet_and_wire_bytes() {
        let source = capture(true, true);
        let (bytes, report) = export(SazMode::Strict, &source);
        let (second, _) = export(SazMode::Strict, &source);
        assert_eq!(bytes, second, "strict SAZ output must be deterministic");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.entries, 5);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 5);
        assert_eq!(
            member(&mut archive, "[Content_Types].xml"),
            CONTENT_TYPES.as_bytes()
        );
        let request = member(&mut archive, "raw/1_c.txt");
        assert_eq!(
            request,
            b"POST https://example.test/upload HTTP/1.1\r\nhost: example.test\r\n\r\nreq"
        );
        let response = member(&mut archive, "raw/1_s.txt");
        assert_eq!(
            response,
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\nres"
        );
        let metadata = String::from_utf8(member(&mut archive, "raw/1_m.xml")).unwrap();
        assert!(!metadata.contains("log-drop-request-body"));
        assert!(!metadata.contains("log-drop-response-body"));
        assert!(archive.by_name("transmog/manifest.json").is_err());
    }

    #[test]
    fn opaque_connect_exports_limited_context_and_dns_failure_without_inventing_tls() {
        use transmog_core::performance::{
            PerformanceEvidence, TransportObservation, TransportOutcome,
        };
        for mode in [SazMode::Strict, SazMode::Extended] {
            for failed in [false, true] {
                let mut source = capture(!failed, true);
                source
                    .records
                    .retain(|record| !matches!(record.kind, CaptureRecordKind::BodySegment { .. }));
                for record in &mut source.records {
                    if let CaptureRecordKind::RequestHead { method, target, .. } = &mut record.kind
                    {
                        *method = "CONNECT".into();
                        *target = "https://example.test:443/".into();
                    }
                }
                source.records.push(CaptureRecord {
                    sequence: 10,
                    exchange_id: 42,
                    kind: CaptureRecordKind::Performance(PerformanceEvidence {
                        transports: vec![TransportObservation {
                            leg: "upstream".into(),
                            connection_id: "opaque".into(),
                            tunnel_destination: Some("example.test:443".into()),
                            peer: (!failed).then(|| "192.0.2.1:443".into()),
                            outcome: if failed {
                                TransportOutcome::DnsFailed
                            } else {
                                TransportOutcome::Connected
                            },
                            bytes_read: Some(123),
                            bytes_written: Some(456),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                });
                if failed {
                    source.records.push(CaptureRecord {
                        sequence: 11,
                        exchange_id: 42,
                        kind: CaptureRecordKind::Failed {
                            category: "Upstream".into(),
                            message: "Recorded name lookup failed".into(),
                        },
                    });
                }
                let (bytes, report) = export(mode, &source);
                assert_eq!(report.sessions, 1);
                assert_eq!(report.connection_sessions, 0);
                let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
                let response_wire = member(&mut zip, "raw/1_s.txt");
                drop(zip);
                let mut archive =
                    SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
                let index = archive.index(|_, _| {}, || false).unwrap();
                let session = &index.sessions[0];
                assert_eq!(
                    session.request.as_ref().unwrap().start_line,
                    "CONNECT example.test:443 HTTP/1.1"
                );
                let response = session.response.as_ref().unwrap();
                assert!(!String::from_utf8_lossy(&response.raw_head).contains("Fiddler"));
                assert!(!String::from_utf8_lossy(&response.raw_head).contains("TLSv"));
                if failed {
                    assert_eq!(
                        response.start_line,
                        "HTTP/1.1 502 Transmog - DNS Lookup Failed"
                    );
                    assert_eq!(session.metadata.bit_flags & 0x1100, 0);
                    assert_eq!(
                        session.metadata.flags["x-transmog-failure-message"],
                        "Recorded name lookup failed"
                    );
                    assert!(
                        String::from_utf8_lossy(&response_wire)
                            .contains("Recorded name lookup failed")
                    );
                } else {
                    assert_eq!(response.start_line, "HTTP/1.1 200 Connection Established");
                    assert_eq!(session.metadata.bit_flags & 0x1001, 0x1000);
                    assert_eq!(session.metadata.tunnel_info["BytesIngress"], "123");
                    assert_eq!(session.metadata.tunnel_info["BytesEgress"], "456");
                    assert!(
                        response
                            .headers
                            .values("Transmog-Tunnel")
                            .next()
                            .unwrap()
                            .starts_with(b"Opaque CONNECT")
                    );
                }
            }
        }
    }

    #[test]
    fn all_profiles_include_explicit_raw_directory_even_without_sessions() {
        for mode in [SazMode::Strict, SazMode::Extended] {
            for include_response in [false, true] {
                let (bytes, report) = export(mode, &capture(include_response, true));
                let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
                let raw = archive
                    .by_name("raw/")
                    .expect("Fiddler requires a raw/ entry");
                assert!(raw.is_dir());
                assert_eq!(raw.size(), 0);
                drop(raw);
                assert_eq!(report.entries, archive.len());
                assert_eq!(
                    report.entries,
                    2 + 3 * report.sessions
                        + usize::from(mode == SazMode::Extended) * (1 + report.sessions)
                );
            }
        }
    }

    #[test]
    fn incomplete_bodies_are_disclosed_and_missing_heads_use_placeholders() {
        let (bytes, report) = export(SazMode::Extended, &capture(true, false));
        assert_eq!(report.incomplete_bodies, 1);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let metadata = String::from_utf8(member(&mut archive, "raw/1_m.xml")).unwrap();
        assert!(metadata.contains("log-drop-request-body"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&member(&mut archive, "transmog/manifest.json")).unwrap();
        assert_eq!(manifest["sessions"][0]["native_exchange_id"], "42");

        let (bytes, report) = export(SazMode::Strict, &capture(false, true));
        assert_eq!(report.sessions, 1);
        assert_eq!(report.placeholder_sessions, 1);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let response = String::from_utf8(member(&mut archive, "raw/1_s.txt")).unwrap();
        assert!(response.starts_with("HTTP/1.1 599 Transmog response not captured\r\n"));
        assert!(response.contains("This is not a server response."));
        let metadata = String::from_utf8(member(&mut archive, "raw/1_m.xml")).unwrap();
        assert!(metadata.contains("ui-comments"));
        assert!(metadata.contains("x-transmog-missing-response-head"));
    }

    #[test]
    fn missing_heads_preserve_neighboring_messages_and_retained_body_bytes() {
        for mode in [SazMode::Strict, SazMode::Extended] {
            for boundary in [
                ExchangeBoundary::ClientRequest,
                ExchangeBoundary::ClientResponse,
            ] {
                let mut capture = capture(true, true);
                capture.records.retain(|record| !matches!(&record.kind,
                    CaptureRecordKind::RequestHead { boundary: head, .. } | CaptureRecordKind::ResponseHead { boundary: head, .. } if *head == boundary));
                let (bytes, report) = export(mode, &capture);
                assert_eq!(report.sessions, 1);
                assert_eq!(report.placeholder_sessions, 1);
                let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
                let request = member(&mut archive, "raw/1_c.txt");
                let response = member(&mut archive, "raw/1_s.txt");
                assert!(request.ends_with(b"req"));
                assert!(response.ends_with(b"res"));
                let missing = if boundary == ExchangeBoundary::ClientRequest {
                    &request
                } else {
                    &response
                };
                let present = if boundary == ExchangeBoundary::ClientRequest {
                    &response
                } else {
                    &request
                };
                assert!(String::from_utf8_lossy(missing).contains("X-Transmog-Placeholder"));
                assert!(!String::from_utf8_lossy(present).contains("X-Transmog-Placeholder"));
                if mode == SazMode::Extended {
                    let evidence: SessionEvidence =
                        serde_json::from_slice(&member(&mut archive, "transmog/session-1.json"))
                            .unwrap();
                    if boundary == ExchangeBoundary::ClientRequest {
                        assert!(evidence.request_headers.is_none());
                    } else {
                        assert!(evidence.response_headers.is_none());
                    }
                }
            }
        }
    }

    #[test]
    fn body_and_entry_limits_fail_before_unbounded_growth() {
        let limits = SazLimits {
            max_sessions: 1,
            max_body_bytes_per_direction: 2,
            max_entries: 5,
        };
        let mut exporter =
            SazExporter::new(Cursor::new(Vec::new()), SazMode::Strict, limits).unwrap();
        assert!(matches!(
            exporter.export(&capture(true, true)),
            Err(SazError::BodyLimitExceeded)
        ));
    }

    #[test]
    fn entry_limits_count_the_raw_directory_and_extended_manifest() {
        let limits = SazLimits {
            max_entries: 5,
            ..SazLimits::default()
        };
        let mut strict =
            SazExporter::new(Cursor::new(Vec::new()), SazMode::Strict, limits).unwrap();
        strict.export(&capture(true, true)).unwrap();
        assert_eq!(strict.report().unwrap().entries, 5);

        let mut extended =
            SazExporter::new(Cursor::new(Vec::new()), SazMode::Extended, limits).unwrap();
        assert!(matches!(
            extended.export(&capture(true, true)),
            Err(SazError::EntryLimitExceeded)
        ));

        assert!(matches!(
            SazExporter::new(
                Cursor::new(Vec::new()),
                SazMode::Strict,
                SazLimits {
                    max_entries: 4,
                    ..limits
                }
            ),
            Err(SazError::InvalidLimits)
        ));
    }

    #[test]
    fn unsafe_raw_http_fields_fail_closed() {
        let mut capture = capture(true, true);
        let CaptureRecordKind::RequestHead { method, .. } = &mut capture.records[0].kind else {
            panic!("expected request head");
        };
        *method = "GET\r\ninjected: true".to_owned();
        let mut exporter = SazExporter::new(
            Cursor::new(Vec::new()),
            SazMode::Strict,
            SazLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            exporter.export(&capture),
            Err(SazError::UnsafeWireField)
        ));
    }
}
