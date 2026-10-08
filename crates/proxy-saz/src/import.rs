use std::{
    collections::{BTreeMap, HashSet},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
};

use http::Uri;
use quick_xml::{Reader, events::Event};
use transmog_core::{HeaderBlock, HeaderField, HttpLegVersion, RequestHead, ResponseHead, Target};
use zip::{CompressionMethod, ZipArchive};

use crate::SazError;

/// Finite bounds for an untrusted session archive.
#[derive(Clone, Copy, Debug)]
pub struct SazImportLimits {
    /// Maximum compressed input length.
    pub max_archive_bytes: u64,
    /// Maximum metadata-indexed sessions.
    pub max_sessions: usize,
    /// Maximum ZIP members, including unrecognized members.
    pub max_members: usize,
    /// Maximum central-directory bytes before ZIP parsing can allocate its index.
    pub max_directory_bytes: u64,
    /// Maximum uncompressed member size.
    pub max_member_bytes: u64,
    /// Maximum sum of declared uncompressed member sizes.
    pub max_total_bytes: u64,
    /// Maximum HTTP head length before its body.
    pub max_head_bytes: usize,
    /// Maximum XML or namespaced JSON metadata length.
    pub max_metadata_bytes: usize,
}

impl Default for SazImportLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 4 * 1024 * 1024 * 1024,
            max_sessions: 100_000,
            max_members: 500_000,
            max_directory_bytes: 64 * 1024 * 1024,
            max_member_bytes: 1024 * 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024 * 1024,
            max_head_bytes: 1024 * 1024,
            max_metadata_bytes: 1024 * 1024,
        }
    }
}

/// Lazy body location inside one immutable ZIP member.
#[derive(Clone, Debug)]
pub struct ArchiveBody {
    /// Central-directory member index.
    pub member: usize,
    /// Exact byte offset immediately after the HTTP head.
    pub offset: u64,
    /// Remaining member bytes, before removing HTTP chunk framing.
    pub wire_bytes: u64,
    /// Whether the stored body uses HTTP chunk framing.
    pub chunked: bool,
    /// Whether source metadata explicitly declares omitted body bytes.
    pub dropped: bool,
}

/// Parsed head with original bytes and a body which has not been materialized.
#[derive(Clone, Debug)]
pub struct ArchiveMessage {
    /// Original start line, including the source protocol and response reason.
    pub start_line: String,
    /// Ordered, duplicate-preserving header fields.
    pub headers: HeaderBlock,
    /// Complete original head, including its terminal empty line.
    pub raw_head: Vec<u8>,
    /// Lazy original body reference.
    pub body: ArchiveBody,
}

impl ArchiveMessage {
    /// Resolves a captured request target, including classic origin-form SAZ.
    ///
    /// # Errors
    /// Returns malformed methods, targets, protocols, or ambiguous Host fields.
    pub fn request_head(&self, metadata: &ArchiveMetadata) -> Result<RequestHead, SazError> {
        let parts = self.start_line.split_ascii_whitespace().collect::<Vec<_>>();
        if parts.len() != 3 || http::Method::from_bytes(parts[0].as_bytes()).is_err() {
            return Err(SazError::InvalidArchive);
        }
        let protocol = protocol(parts[2])?;
        let address = if parts[1].starts_with("http://") || parts[1].starts_with("https://") {
            parts[1].to_owned()
        } else if parts[0] == "CONNECT" {
            format!("https://{}/", parts[1])
        } else {
            let hosts = self.headers.values("host").collect::<Vec<_>>();
            if hosts.len() != 1 || !parts[1].starts_with('/') {
                return Err(SazError::InvalidArchive);
            }
            let host = std::str::from_utf8(hosts[0]).map_err(|_| SazError::InvalidArchive)?;
            if !metadata.scheme_recorded {
                return Err(SazError::InvalidArchive);
            }
            let scheme = if metadata.bit_flags & 1 != 0 {
                "https"
            } else {
                "http"
            };
            format!("{scheme}://{host}{}", parts[1])
        };
        let target = parse_target(&address)?;
        Ok(RequestHead {
            method: parts[0].into(),
            target,
            headers: self.headers.clone(),
            source_version: protocol,
        })
    }

    /// Converts an HTTP response head without losing its original start line.
    ///
    /// # Errors
    /// Returns invalid protocol or status syntax.
    pub fn response_head(&self) -> Result<ResponseHead, SazError> {
        let mut parts = self.start_line.splitn(3, ' ');
        let version = protocol(parts.next().ok_or(SazError::InvalidArchive)?)?;
        let status = parts
            .next()
            .and_then(|status| status.parse::<u16>().ok())
            .filter(|status| (100..=999).contains(status))
            .ok_or(SazError::InvalidArchive)?;
        Ok(ResponseHead {
            status,
            headers: self.headers.clone(),
            source_version: version,
        })
    }
}

/// Original metadata, retained as data without executing or interpreting flags.
#[derive(Clone, Debug, Default)]
pub struct ArchiveMetadata {
    /// Fiddler's documented session bit flags.
    pub bit_flags: u64,
    /// Whether metadata recorded the scheme bit, rather than leaving it unknown.
    pub scheme_recorded: bool,
    /// Duplicate-free `SessionFlag` name/value pairs.
    pub flags: BTreeMap<String, String>,
    /// Original `SessionTimers` fields; missing/sentinel values stay explicit.
    pub timers: BTreeMap<String, String>,
    /// Newer `SessionMetrics` attributes and scalar element values.
    pub metrics: BTreeMap<String, String>,
}

/// One saved session with independently optional message evidence.
#[derive(Clone, Debug)]
pub struct ArchiveSession {
    /// Original archive session number, independent of viewer IDs.
    pub source_id: String,
    /// Request, if present and parseable.
    pub request: Option<ArchiveMessage>,
    /// Response, if present and parseable.
    pub response: Option<ArchiveMessage>,
    /// Original metadata, including latency evidence and client information.
    pub metadata: ArchiveMetadata,
}

/// An actionable per-session issue without captured credentials or bodies.
#[derive(Clone, Debug)]
pub struct SazIssue {
    /// Original session number, or archive-wide context.
    pub source_id: String,
    /// Bounded diagnosis.
    pub message: &'static str,
}

/// Lightweight result of indexing heads and metadata without extracting bodies.
#[derive(Clone, Debug)]
pub struct SazIndex {
    /// Parsed sessions, including incomplete ones.
    pub sessions: Vec<ArchiveSession>,
    /// Missing or malformed evidence; capped independently of archive size.
    pub issues: Vec<SazIssue>,
    /// Count of issues omitted from the bounded issue list.
    pub additional_issues: usize,
    /// Optional Transmog trace context, preserved independently of this machine.
    pub trace_metadata: Option<serde_json::Value>,
}

/// Seekable, cloneable archive handle. Its reader must clone an independent
/// cursor over the same immutable bytes. No archive member is extracted by name.
#[derive(Clone, Debug)]
pub struct SazArchive<R> {
    archive: ZipArchive<R>,
    limits: SazImportLimits,
}

#[derive(Default)]
struct Parts {
    request: Option<usize>,
    response: Option<usize>,
    metadata: Option<usize>,
}

impl<R: Read + Seek + Clone> SazArchive<R> {
    /// Opens a ZIP/ZIP64 archive using finite directory and inflated-size bounds.
    ///
    /// # Errors
    /// Returns invalid ZIPs, resource overflow, unsafe/duplicate names,
    /// encryption, or unsupported compression. Never extracts arbitrary paths.
    pub fn open(mut reader: R, limits: SazImportLimits) -> Result<Self, SazError> {
        if limits.max_archive_bytes == 0
            || limits.max_sessions == 0
            || limits.max_members == 0
            || limits.max_directory_bytes == 0
            || limits.max_head_bytes == 0
            || limits.max_metadata_bytes == 0
            || limits.max_member_bytes == 0
            || limits.max_total_bytes == 0
        {
            return Err(SazError::InvalidLimits);
        }
        if reader.seek(SeekFrom::End(0))? > limits.max_archive_bytes {
            return Err(SazError::ImportLimitExceeded);
        }
        check_directory_bounds(&mut reader, limits)?;
        reader.rewind()?;
        let mut archive = ZipArchive::new(reader)?;
        if archive.len() > limits.max_members {
            return Err(SazError::ImportLimitExceeded);
        }
        let mut names = HashSet::new();
        let mut total = 0_u64;
        for index in 0..archive.len() {
            let member = archive.by_index_raw(index)?;
            let name = safe_name(member.name())?;
            if !names.insert(name) {
                return Err(SazError::UnsafeMember);
            }
            if member.encrypted()
                || !matches!(
                    member.compression(),
                    CompressionMethod::Stored | CompressionMethod::Deflated
                )
            {
                return Err(SazError::UnsupportedMember);
            }
            total = total
                .checked_add(member.size())
                .ok_or(SazError::ImportLimitExceeded)?;
            if member.size() > limits.max_member_bytes || total > limits.max_total_bytes {
                return Err(SazError::ImportLimitExceeded);
            }
        }
        Ok(Self { archive, limits })
    }

    /// Indexes requests, responses and metadata with progress and cancellation.
    ///
    /// # Errors
    /// Returns archive/resource/cancellation errors. Malformed individual
    /// sessions are reported while the rest remain usable.
    pub fn index(
        &mut self,
        mut progress: impl FnMut(usize, usize),
        canceled: impl Fn() -> bool,
    ) -> Result<SazIndex, SazError> {
        let mut parts: BTreeMap<u64, Parts> = BTreeMap::new();
        let mut trace_metadata = None;
        let members = self.archive.len();
        for index in 0..members {
            if canceled() {
                return Err(SazError::Canceled);
            }
            let name = safe_name(self.archive.by_index_raw(index)?.name())?;
            if name == "transmog/trace-metadata.json" {
                let bytes = self.read_metadata(index)?;
                trace_metadata = Some(serde_json::from_slice(&bytes)?);
            } else if let Some((id, kind)) = session_member(&name) {
                let session = parts.entry(id).or_default();
                let slot = match kind {
                    'c' => &mut session.request,
                    's' => &mut session.response,
                    'm' => &mut session.metadata,
                    _ => continue,
                };
                if slot.replace(index).is_some() {
                    return Err(SazError::UnsafeMember);
                }
                if parts.len() > self.limits.max_sessions {
                    return Err(SazError::ImportLimitExceeded);
                }
            }
            progress(index + 1, members + parts.len());
        }
        let count = parts.len();
        let mut result = SazIndex {
            sessions: Vec::with_capacity(count),
            issues: Vec::new(),
            additional_issues: 0,
            trace_metadata,
        };
        for (position, (id, parts)) in parts.into_iter().enumerate() {
            if canceled() {
                return Err(SazError::Canceled);
            }
            let source_id = id.to_string();
            let metadata = parts
                .metadata
                .map(|index| {
                    self.read_metadata(index)
                        .and_then(|bytes| parse_metadata(&bytes))
                })
                .transpose();
            let metadata = if let Ok(metadata) = metadata {
                metadata.unwrap_or_default()
            } else {
                issue(
                    &mut result,
                    &source_id,
                    "Session metadata could not be parsed",
                );
                ArchiveMetadata::default()
            };
            let request = self.message_or_issue(
                parts.request,
                &metadata,
                true,
                false,
                &source_id,
                &mut result,
            );
            let head_response = request
                .as_ref()
                .is_some_and(|message| message.start_line.starts_with("HEAD "));
            let response = self.message_or_issue(
                parts.response,
                &metadata,
                false,
                head_response,
                &source_id,
                &mut result,
            );
            if request.is_some() || response.is_some() {
                result.sessions.push(ArchiveSession {
                    source_id,
                    request,
                    response,
                    metadata,
                });
            }
            progress(members + position + 1, members + count);
        }
        Ok(result)
    }

    /// Streams original content bytes on demand, removing HTTP chunk framing.
    /// Content-Encoding is preserved. Reading through EOF verifies ZIP CRC.
    ///
    /// # Errors
    /// Returns missing/dropped body, framing, checksum, cancellation or limits.
    pub fn copy_body(
        &mut self,
        body: &ArchiveBody,
        output: &mut impl Write,
        canceled: impl Fn() -> bool,
    ) -> Result<u64, SazError> {
        if body.dropped {
            return Err(SazError::InvalidArchive);
        }
        let member = self.archive.by_index(body.member)?;
        if body.offset > member.size() || member.size() - body.offset != body.wire_bytes {
            return Err(SazError::InvalidArchive);
        }
        let mut reader = BufReader::new(member);
        let skipped = std::io::copy(&mut reader.by_ref().take(body.offset), &mut std::io::sink())?;
        if skipped != body.offset {
            return Err(SazError::InvalidArchive);
        }
        if body.chunked {
            copy_chunked(&mut reader, output, self.limits.max_member_bytes, canceled)
        } else {
            copy_bounded(&mut reader, output, body.wire_bytes, canceled)
        }
    }

    fn read_metadata(&mut self, index: usize) -> Result<Vec<u8>, SazError> {
        let member = self.archive.by_index(index)?;
        if member.size() > self.limits.max_metadata_bytes as u64 {
            return Err(SazError::ImportLimitExceeded);
        }
        let mut bytes = Vec::new();
        member
            .take(self.limits.max_metadata_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > self.limits.max_metadata_bytes {
            return Err(SazError::ImportLimitExceeded);
        }
        Ok(bytes)
    }

    fn message_or_issue(
        &mut self,
        index: Option<usize>,
        metadata: &ArchiveMetadata,
        request: bool,
        head_response: bool,
        source_id: &str,
        result: &mut SazIndex,
    ) -> Option<ArchiveMessage> {
        let Some(index) = index else {
            issue(
                result,
                source_id,
                if request {
                    "Request was not saved"
                } else {
                    "Response was not saved"
                },
            );
            return None;
        };
        let message = self
            .read_message(index, metadata, request, head_response)
            .and_then(|message| {
                if request {
                    message.request_head(metadata)?;
                } else {
                    message.response_head()?;
                }
                Ok(message)
            });
        if let Ok(message) = message {
            Some(message)
        } else {
            issue(
                result,
                source_id,
                if request {
                    "Request head could not be parsed"
                } else {
                    "Response head could not be parsed"
                },
            );
            None
        }
    }

    fn read_message(
        &mut self,
        index: usize,
        metadata: &ArchiveMetadata,
        request: bool,
        head_response: bool,
    ) -> Result<ArchiveMessage, SazError> {
        let member = self.archive.by_index(index)?;
        let size = member.size();
        let mut reader = BufReader::new(member.take(self.limits.max_head_bytes as u64 + 1));
        let mut bytes = Vec::new();
        loop {
            let before = bytes.len();
            if reader.read_until(b'\n', &mut bytes)? == 0
                || bytes.len() > self.limits.max_head_bytes
            {
                return Err(SazError::InvalidArchive);
            }
            if matches!(&bytes[before..], b"\r\n" | b"\n") {
                break;
            }
        }
        let line_end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or(SazError::InvalidArchive)?;
        let line = std::str::from_utf8(&bytes[..line_end])
            .map_err(|_| SazError::InvalidArchive)?
            .trim_end_matches('\r')
            .to_owned();
        let mut raw_headers = vec![httparse::EMPTY_HEADER; 1024];
        let httparse::Status::Complete((_, fields)) =
            httparse::parse_headers(&bytes[line_end + 1..], &mut raw_headers)
                .map_err(|_| SazError::InvalidArchive)?
        else {
            return Err(SazError::InvalidArchive);
        };
        let headers = HeaderBlock::from_fields(
            fields
                .iter()
                .map(|field| {
                    HeaderField::try_new(field.name.as_bytes(), field.value)
                        .map_err(|_| SazError::InvalidArchive)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let chunked = chunked(&headers)?;
        let offset = bytes.len() as u64;
        let wire_bytes = size.checked_sub(offset).ok_or(SazError::InvalidArchive)?;
        let dropped = metadata.bit_flags & if request { 1_048_576 } else { 131_072 } != 0
            || metadata
                .flags
                .get(if request {
                    "log-drop-request-body"
                } else {
                    "log-drop-response-body"
                })
                .is_some_and(|value| !value.eq_ignore_ascii_case("false"))
            || (!head_response || wire_bytes != 0)
                && declared_body_mismatch(&headers, wire_bytes, chunked, &line, request);
        Ok(ArchiveMessage {
            start_line: line,
            headers,
            raw_head: bytes,
            body: ArchiveBody {
                member: index,
                offset,
                wire_bytes,
                chunked,
                dropped,
            },
        })
    }
}

fn safe_name(name: &str) -> Result<String, SazError> {
    let name = name.replace('\\', "/").to_ascii_lowercase();
    if name.len() > 1024
        || name.starts_with('/')
        || name.contains(':')
        || name.contains('\0')
        || name.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(SazError::UnsafeMember);
    }
    Ok(name)
}

// Bound allocations before ZipArchive constructs an index from untrusted counts.
fn check_directory_bounds(
    reader: &mut (impl Read + Seek),
    limits: SazImportLimits,
) -> Result<(), SazError> {
    let length = reader.seek(SeekFrom::End(0))?;
    let tail_length =
        usize::try_from(length.min(65_557)).map_err(|_| SazError::ImportLimitExceeded)?;
    reader.seek(SeekFrom::Start(length - tail_length as u64))?;
    let mut tail = vec![0_u8; tail_length];
    reader.read_exact(&mut tail)?;
    let end = tail
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(position, window)| {
            if window != b"PK\x05\x06" || position + 22 > tail.len() {
                return None;
            }
            let comment = u16::from_le_bytes([tail[position + 20], tail[position + 21]]) as usize;
            (position + 22 + comment == tail.len()).then_some(position)
        })
        .ok_or(SazError::InvalidArchive)?;
    let comment = u16::from_le_bytes(
        tail[end + 20..end + 22]
            .try_into()
            .map_err(|_| SazError::InvalidArchive)?,
    ) as usize;
    if end + 22 + comment != tail.len() || tail[end + 4..end + 8] != [0, 0, 0, 0] {
        return Err(SazError::InvalidArchive);
    }
    let mut entries = u64::from(u16::from_le_bytes(
        tail[end + 10..end + 12]
            .try_into()
            .map_err(|_| SazError::InvalidArchive)?,
    ));
    let mut directory_bytes = u64::from(u32::from_le_bytes(
        tail[end + 12..end + 16]
            .try_into()
            .map_err(|_| SazError::InvalidArchive)?,
    ));
    if entries == u64::from(u16::MAX) || directory_bytes == u64::from(u32::MAX) {
        if end < 20 || &tail[end - 20..end - 16] != b"PK\x06\x07" {
            return Err(SazError::InvalidArchive);
        }
        let offset = u64::from_le_bytes(
            tail[end - 12..end - 4]
                .try_into()
                .map_err(|_| SazError::InvalidArchive)?,
        );
        if offset.saturating_add(56) > length {
            return Err(SazError::InvalidArchive);
        }
        reader.seek(SeekFrom::Start(offset))?;
        let mut record = [0_u8; 56];
        reader.read_exact(&mut record)?;
        if &record[..4] != b"PK\x06\x06" || record[16..24] != [0; 8] {
            return Err(SazError::InvalidArchive);
        }
        entries = u64::from_le_bytes(
            record[32..40]
                .try_into()
                .map_err(|_| SazError::InvalidArchive)?,
        );
        directory_bytes = u64::from_le_bytes(
            record[40..48]
                .try_into()
                .map_err(|_| SazError::InvalidArchive)?,
        );
    }
    if entries > limits.max_members as u64
        || directory_bytes > limits.max_directory_bytes
        || directory_bytes > length
    {
        return Err(SazError::ImportLimitExceeded);
    }
    Ok(())
}

fn session_member(name: &str) -> Option<(u64, char)> {
    let raw = name.strip_prefix("raw/")?;
    let (number, suffix) = raw.split_once('_')?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let kind = match suffix {
        "c.txt" => 'c',
        "s.txt" => 's',
        "m.xml" => 'm',
        _ => return None,
    };
    Some((number.parse().ok()?, kind))
}

fn protocol(value: &str) -> Result<HttpLegVersion, SazError> {
    match value {
        "HTTP/1.0" | "HTTP/1.1" => Ok(HttpLegVersion::Http1),
        "HTTP/2" | "HTTP/2.0" => Ok(HttpLegVersion::Http2),
        "HTTP/3" | "HTTP/3.0" => Ok(HttpLegVersion::Http3),
        _ => Err(SazError::InvalidArchive),
    }
}

fn parse_target(value: &str) -> Result<Target, SazError> {
    let uri: Uri = value.parse().map_err(|_| SazError::InvalidArchive)?;
    let scheme = uri
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"))
        .ok_or(SazError::InvalidArchive)?;
    let authority = uri.authority().ok_or(SazError::InvalidArchive)?;
    if authority.as_str().contains('@') {
        return Err(SazError::InvalidArchive);
    }
    Ok(Target {
        scheme: scheme.into(),
        authority: authority.as_str().into(),
        host: authority.host().trim_matches(['[', ']']).into(),
        port: authority
            .port_u16()
            .unwrap_or(if scheme == "https" { 443 } else { 80 }),
        path: if uri.path().is_empty() {
            "/".into()
        } else {
            uri.path().into()
        },
        query: uri.query().map(str::to_owned),
    })
}

fn issue(result: &mut SazIndex, source_id: &str, message: &'static str) {
    if result.issues.len() < 256 {
        result.issues.push(SazIssue {
            source_id: source_id.into(),
            message,
        });
    } else {
        result.additional_issues = result.additional_issues.saturating_add(1);
    }
}

fn parse_metadata(bytes: &[u8]) -> Result<ArchiveMetadata, SazError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut result = ArchiveMetadata::default();
    let mut stack: Vec<String> = Vec::new();
    let mut count = 0_usize;
    let mut version = quick_xml::XmlVersion::default();
    loop {
        match reader.read_event().map_err(|_| SazError::InvalidArchive)? {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let (element, empty) = match event {
                    Event::Start(element) => (element, false),
                    Event::Empty(element) => (element, true),
                    _ => unreachable!(),
                };
                count += 1;
                if count > 8192 || stack.len() > 64 {
                    return Err(SazError::ImportLimitExceeded);
                }
                let name = element.name().as_ref().to_owned();
                let mut attributes = BTreeMap::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| SazError::InvalidArchive)?;
                    let key = attribute.key.as_ref().to_owned();
                    let value = attribute
                        .normalized_value(version)
                        .map_err(|_| SazError::InvalidArchive)?
                        .into_owned();
                    if key.len() > 255 || value.len() > 16 * 1024 || attributes.len() > 256 {
                        return Err(SazError::ImportLimitExceeded);
                    }
                    attributes.insert(key, value);
                }
                apply_metadata_attributes(&mut result, &name, attributes)?;
                if !empty {
                    stack.push(name);
                }
            }
            Event::End(_) => {
                stack.pop().ok_or(SazError::InvalidArchive)?;
            }
            Event::Text(value) if stack.iter().any(|name| name == "SessionMetrics") => {
                let value = value.xml10_content();
                if value.len() > 16 * 1024 {
                    return Err(SazError::ImportLimitExceeded);
                }
                if let Some(name) = stack.last() {
                    let metric = result.metrics.entry(name.clone()).or_default();
                    if metric.len().saturating_add(value.len()) > 16 * 1024 {
                        return Err(SazError::ImportLimitExceeded);
                    }
                    metric.push_str(&value);
                }
            }
            Event::DocType(_) => return Err(SazError::InvalidArchive),
            Event::Decl(declaration) => {
                version = declaration
                    .xml_version()
                    .map_err(|_| SazError::InvalidArchive)?;
            }
            Event::GeneralRef(reference) => {
                let value = if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|_| SazError::InvalidArchive)?
                {
                    character.to_string()
                } else {
                    quick_xml::escape::resolve_xml_entity(reference.as_ref())
                        .ok_or(SazError::InvalidArchive)?
                        .to_owned()
                };
                if stack.iter().any(|name| name == "SessionMetrics")
                    && let Some(name) = stack.last()
                {
                    let metric = result.metrics.entry(name.clone()).or_default();
                    if metric.len().saturating_add(value.len()) > 16 * 1024 {
                        return Err(SazError::ImportLimitExceeded);
                    }
                    metric.push_str(&value);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(SazError::InvalidArchive);
    }
    Ok(result)
}

fn apply_metadata_attributes(
    result: &mut ArchiveMetadata,
    name: &str,
    mut attributes: BTreeMap<String, String>,
) -> Result<(), SazError> {
    match name {
        "Session" => {
            if let Some(value) = attributes.get("BitFlags") {
                result.bit_flags = value
                    .strip_prefix("0x")
                    .map_or_else(
                        || value.parse().ok(),
                        |hex| u64::from_str_radix(hex, 16).ok(),
                    )
                    .ok_or(SazError::InvalidArchive)?;
                result.scheme_recorded = true;
            }
        }
        "SessionTimers" => result.timers.extend(attributes),
        "SessionFlag" => {
            let key = attributes.remove("N").ok_or(SazError::InvalidArchive)?;
            let value = attributes.remove("V").ok_or(SazError::InvalidArchive)?;
            if result.flags.insert(key, value).is_some() {
                return Err(SazError::InvalidArchive);
            }
        }
        "SessionMetrics" => result.metrics.extend(attributes),
        _ => {}
    }
    Ok(())
}

fn chunked(headers: &HeaderBlock) -> Result<bool, SazError> {
    let values = headers.values("transfer-encoding").collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(false);
    }
    if values.len() != 1 || !values[0].eq_ignore_ascii_case(b"chunked") {
        return Err(SazError::InvalidArchive);
    }
    Ok(true)
}

fn declared_body_mismatch(
    headers: &HeaderBlock,
    bytes: u64,
    chunked: bool,
    line: &str,
    request: bool,
) -> bool {
    let lengths = headers
        .values("content-length")
        .map(|value| {
            std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .collect::<Vec<_>>();
    if lengths.iter().any(Option::is_none) || lengths.windows(2).any(|pair| pair[0] != pair[1]) {
        return true;
    }
    if chunked {
        return !lengths.is_empty();
    }
    // 304 and bodyless statuses may describe a cached entity's length.
    let status = line
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok());
    if !request
        && status.is_some_and(|status| (100..200).contains(&status) || matches!(status, 204 | 304))
        && bytes == 0
    {
        return false;
    }
    lengths.first().is_some_and(|length| *length != Some(bytes))
}

fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected: u64,
    canceled: impl Fn() -> bool,
) -> Result<u64, SazError> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        if canceled() {
            return Err(SazError::Canceled);
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|total| *total <= expected)
            .ok_or(SazError::ImportLimitExceeded)?;
        writer.write_all(&buffer[..count])?;
    }
    if total != expected {
        return Err(SazError::InvalidArchive);
    }
    Ok(total)
}

fn copy_chunked(
    reader: &mut impl BufRead,
    output: &mut impl Write,
    limit: u64,
    canceled: impl Fn() -> bool,
) -> Result<u64, SazError> {
    let mut total = 0_u64;
    loop {
        if canceled() {
            return Err(SazError::Canceled);
        }
        let line = bounded_line(reader, 8192)?;
        let length = line
            .split(|byte| *byte == b';')
            .next()
            .ok_or(SazError::InvalidArchive)?;
        let length = std::str::from_utf8(length)
            .map_err(|_| SazError::InvalidArchive)?
            .trim();
        if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SazError::InvalidArchive);
        }
        let count = u64::from_str_radix(length, 16).map_err(|_| SazError::InvalidArchive)?;
        if count == 0 {
            let mut trailers = Vec::new();
            loop {
                let line = bounded_line(reader, 64 * 1024)?;
                trailers.extend_from_slice(&line);
                if trailers.len() > 64 * 1024 {
                    return Err(SazError::ImportLimitExceeded);
                }
                if line == b"\r\n" {
                    break;
                }
            }
            let mut fields = [httparse::EMPTY_HEADER; 256];
            if !matches!(
                httparse::parse_headers(&trailers, &mut fields),
                Ok(httparse::Status::Complete(_))
            ) {
                return Err(SazError::InvalidArchive);
            }
            let mut extra = [0_u8; 1];
            if reader.read(&mut extra)? != 0 {
                return Err(SazError::InvalidArchive);
            }
            return Ok(total);
        }
        total = total
            .checked_add(count)
            .filter(|total| *total <= limit)
            .ok_or(SazError::ImportLimitExceeded)?;
        let mut remaining = count;
        let mut buffer = [0_u8; 16 * 1024];
        while remaining > 0 {
            if canceled() {
                return Err(SazError::Canceled);
            }
            let count = usize::try_from(remaining.min(buffer.len() as u64))
                .map_err(|_| SazError::ImportLimitExceeded)?;
            reader.read_exact(&mut buffer[..count])?;
            output.write_all(&buffer[..count])?;
            remaining -= count as u64;
        }
        let mut ending = [0_u8; 2];
        reader.read_exact(&mut ending)?;
        if ending != *b"\r\n" {
            return Err(SazError::InvalidArchive);
        }
    }
}

fn bounded_line(reader: &mut impl BufRead, limit: usize) -> Result<Vec<u8>, SazError> {
    let mut line = Vec::new();
    reader.take(limit as u64 + 1).read_until(b'\n', &mut line)?;
    if line.len() > limit || !line.ends_with(b"\r\n") {
        return Err(SazError::InvalidArchive);
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn zip(members: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in members {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn index(bytes: Vec<u8>) -> (SazArchive<Cursor<Vec<u8>>>, SazIndex) {
        let mut archive = SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
        let result = archive.index(|_, _| {}, || false).unwrap();
        (archive, result)
    }

    #[test]
    fn stored_and_compressed_classic_archives_preserve_headers_binary_and_timings() {
        let request = b"POST /path?q=1 HTTP/1.1\r\nHost: example.invalid\r\nAuthorization: Bearer original\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nContent-Length: 3\r\n\r\n\0\xffa";
        let response =
            b"HTTP/1.0 404 Custom reason\r\nContent-Encoding: gzip\r\n\r\n\x1f\x8b\0\xff";
        let metadata = br#"<Session SID="7" BitFlags="1"><SessionTimers ClientConnected="2026-10-08T23:23:35.338-07:00" DNSTime="12" ServerConnected="0001-01-01T00:00:00"/><SessionFlags><SessionFlag N="x-clientIP" V="192.0.2.25"/><SessionFlag N="ui-comments" V="&lt;script&gt;inert&lt;/script&gt;"/></SessionFlags><SessionMetrics RequestDuration="17"><TTFB>2&#48;</TTFB></SessionMetrics></Session>"#;
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let (mut archive, result) = index(zip(
                &[
                    ("raw/0007_c.txt", request),
                    ("raw/0007_s.txt", response),
                    ("raw/0007_m.xml", metadata),
                ],
                method,
            ));
            assert!(result.issues.is_empty());
            let session = &result.sessions[0];
            let head = session
                .request
                .as_ref()
                .unwrap()
                .request_head(&session.metadata)
                .unwrap();
            assert_eq!(head.target.scheme, "https");
            assert_eq!(head.target.query.as_deref(), Some("q=1"));
            assert_eq!(
                head.headers.values("x-duplicate").collect::<Vec<_>>(),
                [b"one".as_slice(), b"two".as_slice()]
            );
            assert_eq!(
                head.headers.values("authorization").next(),
                Some(b"Bearer original".as_slice())
            );
            assert_eq!(
                session.metadata.flags["ui-comments"],
                "<script>inert</script>"
            );
            assert_eq!(session.metadata.timers["DNSTime"], "12");
            assert_eq!(session.metadata.metrics["TTFB"], "20");
            assert_eq!(
                session.response.as_ref().unwrap().start_line,
                "HTTP/1.0 404 Custom reason"
            );
            let mut bytes = Vec::new();
            archive
                .copy_body(&session.request.as_ref().unwrap().body, &mut bytes, || {
                    false
                })
                .unwrap();
            assert_eq!(bytes, b"\0\xffa");
            bytes.clear();
            archive
                .copy_body(&session.response.as_ref().unwrap().body, &mut bytes, || {
                    false
                })
                .unwrap();
            assert_eq!(bytes, b"\x1f\x8b\0\xff");
        }
    }

    #[test]
    fn chunk_extensions_trailers_and_incomplete_framing_are_explicit() {
        let request = b"POST http://example.invalid/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2;example=yes\r\na\0\r\n1\r\n\xff\r\n0\r\nX-Trailer: captured\r\n\r\n";
        let (mut archive, result) = index(zip(
            &[("raw/1_c.txt", request)],
            CompressionMethod::Deflated,
        ));
        assert_eq!(result.issues[0].message, "Response was not saved");
        let mut bytes = Vec::new();
        assert_eq!(
            archive
                .copy_body(
                    &result.sessions[0].request.as_ref().unwrap().body,
                    &mut bytes,
                    || false
                )
                .unwrap(),
            3
        );
        assert_eq!(bytes, b"a\0\xff");
        for body in [
            b"2\r\nx\r\n".as_slice(),
            b"+2\r\nxx\r\n0\r\n\r\n".as_slice(),
            b"0\r\n\r\nextra".as_slice(),
        ] {
            let mut output = Vec::new();
            assert!(copy_chunked(&mut Cursor::new(body), &mut output, 1024, || false).is_err());
        }
        let metadata = br#"<Session BitFlags="1048576"/>"#;
        let (mut archive, result) = index(zip(
            &[
                (
                    "raw/1_c.txt",
                    b"POST http://example.invalid/ HTTP/1.1\r\nContent-Length: 9\r\n\r\n",
                ),
                ("raw/1_m.xml", metadata),
            ],
            CompressionMethod::Stored,
        ));
        let body = &result.sessions[0].request.as_ref().unwrap().body;
        assert!(body.dropped);
        assert!(archive.copy_body(body, &mut Vec::new(), || false).is_err());
    }

    #[test]
    fn bodyless_responses_are_complete_and_unknown_scheme_is_not_guessed() {
        let (_, result) = index(zip(
            &[
                (
                    "raw/1_c.txt",
                    b"HEAD https://example.invalid/ HTTP/1.1\r\n\r\n",
                ),
                (
                    "raw/1_s.txt",
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n",
                ),
                (
                    "raw/2_c.txt",
                    b"GET / HTTP/1.1\r\nHost: example.invalid\r\n\r\n",
                ),
            ],
            CompressionMethod::Stored,
        ));
        assert!(!result.sessions[0].response.as_ref().unwrap().body.dropped);
        assert_eq!(result.sessions.len(), 1);
        assert!(result.issues.iter().any(|issue| issue.source_id == "2"));
    }

    #[test]
    fn malformed_metadata_and_members_do_not_erase_other_saved_sessions() {
        let metadata = br#"<!DOCTYPE Session [<!ENTITY steal SYSTEM "file:///private">]><Session BitFlags="1"/>"#;
        let (_, result) = index(zip(
            &[
                (
                    "raw/1_c.txt",
                    b"GET https://example.invalid/ HTTP/1.1\r\n\r\n",
                ),
                ("raw/1_m.xml", metadata),
                ("raw/2_c.txt", b"garbage"),
                ("raw/2_s.txt", b"HTTP/1.1 503 Not ready\r\n\r\n"),
            ],
            CompressionMethod::Stored,
        ));
        assert_eq!(result.sessions.len(), 2);
        assert!(result.sessions[1].request.is_none());
        assert!(result.sessions[1].response.is_some());
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.message == "Session metadata could not be parsed")
        );
        assert!(parse_metadata(b"<Session><SessionFlags><SessionFlag N=\"x\" V=\"1\"/><SessionFlag N=\"x\" V=\"2\"/></SessionFlags></Session>").is_err());
    }

    #[test]
    fn unsafe_names_directory_counts_and_cancellation_fail_before_processing() {
        for names in [
            vec!["../outside.txt"],
            vec!["raw/1_c.txt", "RAW/1_C.TXT"],
            vec!["C:/outside.txt"],
        ] {
            let members = names
                .iter()
                .map(|name| (*name, b"data".as_slice()))
                .collect::<Vec<_>>();
            assert!(matches!(
                SazArchive::open(
                    Cursor::new(zip(&members, CompressionMethod::Stored)),
                    SazImportLimits::default()
                ),
                Err(SazError::UnsafeMember)
            ));
        }
        let bytes = zip(
            &[(
                "raw/1_c.txt",
                b"GET http://example.invalid/ HTTP/1.1\r\n\r\n",
            )],
            CompressionMethod::Stored,
        );
        let mut changed = bytes.clone();
        let end = changed.len() - 22;
        changed[end + 10..end + 12].copy_from_slice(&1000_u16.to_le_bytes());
        assert!(matches!(
            SazArchive::open(
                Cursor::new(changed),
                SazImportLimits {
                    max_members: 10,
                    ..SazImportLimits::default()
                }
            ),
            Err(SazError::ImportLimitExceeded)
        ));
        let mut archive = SazArchive::open(Cursor::new(bytes), SazImportLimits::default()).unwrap();
        assert!(matches!(
            archive.index(|_, _| {}, || true),
            Err(SazError::Canceled)
        ));
    }

    #[derive(Clone)]
    struct CountedReader {
        bytes: Cursor<Arc<[u8]>>,
        read: Arc<AtomicUsize>,
    }
    impl Read for CountedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.read(buffer)?;
            self.read.fetch_add(count, Ordering::Relaxed);
            Ok(count)
        }
    }
    impl Seek for CountedReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.bytes.seek(position)
        }
    }

    #[test]
    fn indexing_reads_headers_without_materializing_large_binary_bodies() {
        let body = vec![0xaa; 1024 * 1024];
        let mut message = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\n\r\n".to_vec();
        message.extend_from_slice(&body);
        let bytes = zip(&[("raw/1_s.txt", &message)], CompressionMethod::Stored);
        let read = Arc::new(AtomicUsize::new(0));
        let reader = CountedReader {
            bytes: Cursor::new(Arc::from(bytes)),
            read: read.clone(),
        };
        let mut archive = SazArchive::open(reader, SazImportLimits::default()).unwrap();
        let result = archive.index(|_, _| {}, || false).unwrap();
        assert!(read.load(Ordering::Relaxed) < 200 * 1024);
        let mut output = Vec::new();
        archive
            .copy_body(
                &result.sessions[0].response.as_ref().unwrap().body,
                &mut output,
                || false,
            )
            .unwrap();
        assert_eq!(output, body);
    }

    #[test]
    fn zip64_directory_records_are_bounded_and_supported() {
        let mut bytes = zip(
            &[(
                "raw/1_c.txt",
                b"GET http://example.invalid/ HTTP/1.1\r\n\r\n",
            )],
            CompressionMethod::Stored,
        );
        let start = bytes.len() - 22;
        let mut end = bytes.split_off(start);
        let count = u64::from(u16::from_le_bytes([end[10], end[11]]));
        let size = u64::from(u32::from_le_bytes(end[12..16].try_into().unwrap()));
        let offset = u64::from(u32::from_le_bytes(end[16..20].try_into().unwrap()));
        bytes.extend_from_slice(b"PK\x06\x06");
        bytes.extend_from_slice(&44_u64.to_le_bytes());
        bytes.extend_from_slice(&45_u16.to_le_bytes());
        bytes.extend_from_slice(&45_u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(b"PK\x06\x07");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&(start as u64).to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        end[8..12].fill(0xff);
        end[12..20].fill(0xff);
        bytes.extend_from_slice(&end);
        let (_, result) = index(bytes.clone());
        assert_eq!(result.sessions.len(), 1);
        assert!(matches!(
            SazArchive::open(
                Cursor::new(bytes),
                SazImportLimits {
                    max_directory_bytes: 1,
                    ..SazImportLimits::default()
                }
            ),
            Err(SazError::ImportLimitExceeded)
        ));
    }

    #[test]
    fn body_crc_is_verified_when_a_lazy_body_is_read() {
        let mut message = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        message.extend_from_slice(&vec![0xaa; 100_000]);
        let mut bytes = zip(&[("raw/1_s.txt", &message)], CompressionMethod::Stored);
        // Damage body bytes past the header prefetch while keeping ZIP metadata.
        let position = bytes
            .windows(16)
            .position(|bytes| bytes == [0xaa; 16])
            .unwrap()
            + 50_000;
        bytes[position] ^= 1;
        let (mut archive, result) = index(bytes);
        assert!(result.sessions[0].response.is_some());
        assert!(
            archive
                .copy_body(
                    &result.sessions[0].response.as_ref().unwrap().body,
                    &mut Vec::new(),
                    || false
                )
                .is_err()
        );
    }
}
