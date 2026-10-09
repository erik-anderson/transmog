#![deny(missing_docs)]

//! Bounded Session Archive Zip import and export adapters.
//!
//! Strict mode emits only the conventional OPC content-types member, an explicit
//! `raw/` directory, and three `raw/<id>_{c,s,m}` files per complete HTTP exchange.
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

mod evidence;
mod import;
mod metadata;
pub use evidence::SessionEvidence;
pub use import::{
    ArchiveBody, ArchiveMessage, ArchiveMetadata, ArchiveSession, SazArchive, SazImportLimits,
    SazIndex, SazIssue,
};

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
            max_entries: 3_000_003,
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
    /// Complete sessions emitted.
    pub sessions: usize,
    /// Exchanges omitted because either head was absent.
    pub skipped_incomplete: usize,
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
        if self.report.is_some() {
            return Err(SazError::AlreadyExported);
        }
        let mut output = self.output.take().ok_or(SazError::AlreadyExported)?;
        let starting_position = output.stream_position()?;
        let mut sessions = collect_sessions(capture, self.limits)?;
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
        for (exchange_id, session) in &mut sessions {
            if session.request.is_none() || session.response.is_none() {
                report.skipped_incomplete = report.skipped_incomplete.saturating_add(1);
                continue;
            }
            if report.sessions >= self.limits.max_sessions {
                return Err(SazError::SessionLimitExceeded);
            }
            let session_id = report.sessions.saturating_add(1);
            let incomplete = write_http_session(
                &mut writer,
                session_id,
                session,
                options,
                self.limits.max_entries.saturating_sub(report.entries),
            )?;
            report.sessions = report.sessions.saturating_add(1);
            report.entries = report.entries.saturating_add(3);
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
            report.incomplete_bodies = report
                .incomplete_bodies
                .saturating_add(usize::from(incomplete));
            manifest.push(ManifestSession {
                saz_id: session_id,
                native_exchange_id: exchange_id.to_string(),
                incomplete,
            });
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
struct Session {
    evidence: SessionEvidence,
    started_unix_nanos: Option<u128>,
    request: Option<CapturedRequest>,
    response: Option<CapturedResponse>,
    request_body: Vec<u8>,
    response_body: Vec<u8>,
    request_body_incomplete: bool,
    response_body_incomplete: bool,
    loss: bool,
    terminal: TerminalState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TerminalState {
    #[default]
    Open,
    Completed,
    Failed,
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
            CaptureRecordKind::ExchangeStarted {
                client_addr,
                started_unix_nanos,
                ..
            } => {
                session.evidence.client_addr = Some(client_addr.clone());
                session.started_unix_nanos = Some(*started_unix_nanos);
            }
            CaptureRecordKind::Performance(performance) => {
                session.evidence.performance.merge(performance);
            }
            CaptureRecordKind::Unknown { kind, payload } if kind == "entry-provenance" => {
                session.evidence.provenance = Some(payload.clone());
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
                let (body, incomplete) = match boundary {
                    ExchangeBoundary::ClientRequest => (
                        &mut session.request_body,
                        &mut session.request_body_incomplete,
                    ),
                    ExchangeBoundary::ClientResponse => (
                        &mut session.response_body,
                        &mut session.response_body_incomplete,
                    ),
                    ExchangeBoundary::UpstreamRequest | ExchangeBoundary::UpstreamResponse => {
                        continue;
                    }
                };
                let Some(bytes) = bytes else {
                    *incomplete = *byte_count > 0;
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
            CaptureRecordKind::Failed { .. } => session.terminal = TerminalState::Failed,
            _ => {}
        }
    }
    Ok(sessions)
}

fn render_request(
    request: &CapturedRequest,
    body: &[u8],
    trailers: &[CapturedHeader],
    protocol: &str,
) -> Result<Vec<u8>, SazError> {
    validate_line_component(request.method.as_bytes())?;
    validate_line_component(request.target.as_bytes())?;
    let mut output = Vec::new();
    output.extend_from_slice(request.method.as_bytes());
    output.push(b' ');
    output.extend_from_slice(request.target.as_bytes());
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
    render_entity(&mut output, &response.headers, body, trailers)?;
    Ok(output)
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
    let request = session.request.as_ref().ok_or(SazError::InvalidArchive)?;
    let response = session.response.as_ref().ok_or(SazError::InvalidArchive)?;
    let request_incomplete =
        session.request_body_incomplete || session.loss || session.terminal == TerminalState::Open;
    let response_incomplete = session.response_body_incomplete
        || session.loss
        || session.terminal != TerminalState::Completed;
    let request_wire = render_request(
        request,
        &session.request_body,
        &session.evidence.request_trailers,
        &metadata::protocol(session, "client-request"),
    )?;
    let response_wire = render_response(
        response,
        &session.response_body,
        &session.evidence.response_trailers,
        &metadata::protocol(session, "client-response"),
        metadata::reason(session).unwrap_or_else(|| reason_phrase(response.status)),
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
    fn incomplete_bodies_are_disclosed_and_incomplete_sessions_are_skipped() {
        let (bytes, report) = export(SazMode::Extended, &capture(true, false));
        assert_eq!(report.incomplete_bodies, 1);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let metadata = String::from_utf8(member(&mut archive, "raw/1_m.xml")).unwrap();
        assert!(metadata.contains("log-drop-request-body"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&member(&mut archive, "transmog/manifest.json")).unwrap();
        assert_eq!(manifest["sessions"][0]["native_exchange_id"], "42");

        let (_, report) = export(SazMode::Strict, &capture(false, true));
        assert_eq!(report.sessions, 0);
        assert_eq!(report.skipped_incomplete, 1);
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
