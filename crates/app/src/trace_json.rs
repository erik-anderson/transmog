//! JSON archives map to the existing traffic, header and original-timing models.
use super::{
    AppError, Arc, AtomicBool, BTreeMap, BodySnapshot, ClientIdentity, Duration, ExchangeBoundary,
    ExchangeId, HashMap, HeaderBlock, HeaderField, HttpLegVersion, ImportData, ImportedBody,
    MAX_HEAD_BYTES, MAX_SESSIONS, Ordering, ResponseHead, SavedBodySource, SessionSnapshot,
    SessionTerminal, SourceReader, SystemTime, TraceEntry, Value, empty_snapshot, invalid,
    metadata, native_request, timestamp, unavailable,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::{Read, Seek, SeekFrom, Write};
use transmog_core::performance::{ProtocolObservation, TransportObservation};

#[path = "trace_netlog.rs"]
mod netlog;
#[path = "trace_netlog_lines.rs"]
mod netlog_lines;

struct CancelReader<'a> {
    reader: SourceReader,
    canceled: &'a AtomicBool,
}
impl Read for CancelReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Trace import canceled"));
        }
        self.reader.read(bytes)
    }
}
fn without_fields(value: &Value, excluded: &[&str]) -> Value {
    Value::Object(
        value
            .as_object()
            .map(|map| {
                map.iter()
                    .filter(|(key, _)| !excluded.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

pub(super) fn import(
    mut reader: SourceReader,
    prefix: u64,
    trace_id: &str,
    format: crate::TraceFormat,
    canceled: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<ImportData, AppError> {
    let mut bom = [0; 3];
    let count = reader
        .read(&mut bom)
        .map_err(|_| invalid("JSON trace is unreadable"))?;
    if count != 3 || bom != [0xef, 0xbb, 0xbf] {
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|_| invalid("JSON trace is unreadable"))?;
    }
    let reader = std::io::BufReader::new(CancelReader { reader, canceled });
    let (document, notes) = if format == crate::TraceFormat::Netlog {
        netlog_lines::read(reader, canceled)?
    } else {
        (
            serde_json::from_reader(reader).map_err(|_| {
                if canceled.load(Ordering::Acquire) {
                    unavailable("Trace import canceled")
                } else {
                    invalid("The trace is not valid JSON")
                }
            })?,
            Vec::new(),
        )
    };
    if canceled.load(Ordering::Acquire) {
        return Err(unavailable("Trace import canceled"));
    }
    match format {
        crate::TraceFormat::Har => har(&document, prefix, trace_id, canceled, progress),
        crate::TraceFormat::Netlog => {
            let mut imported = netlog::import(&document, prefix, trace_id, canceled, progress)?;
            imported.notes.extend(notes);
            Ok(imported)
        }
        _ => Err(invalid("Unsupported JSON trace")),
    }
}

fn empty(format: &'static str, context: Value) -> ImportData {
    ImportData {
        stream_index_bytes: 0,
        index_bytes: 0,
        sources: BTreeMap::new(),
        format,
        sessions: vec![],
        entries: HashMap::new(),
        bodies: vec![],
        context,
        notes: vec![],
        issues: vec![],
    }
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn version(text: &str) -> HttpLegVersion {
    if text.starts_with("HTTP/2") || text == "h2" {
        HttpLegVersion::Http2
    } else if text.starts_with("HTTP/3") || text.starts_with("h3") {
        HttpLegVersion::Http3
    } else {
        HttpLegVersion::Http1
    }
}
fn protocol(
    snapshot: &mut SessionSnapshot,
    boundary: ExchangeBoundary,
    text: &str,
    reason: Option<String>,
) {
    if !text.is_empty() {
        snapshot.performance.protocols.push(ProtocolObservation {
            boundary: crate::inspector::boundary(boundary),
            version: text.into(),
            reason,
        });
    }
}
fn headers(value: &Value) -> Result<HeaderBlock, AppError> {
    let fields = value
        .as_array()
        .ok_or_else(|| invalid("HAR headers must be an array"))?;
    Ok(HeaderBlock::from_fields(
        fields
            .iter()
            .map(|field| {
                let name = field["name"]
                    .as_str()
                    .ok_or_else(|| invalid("HAR header name is missing"))?;
                let value = field["value"]
                    .as_str()
                    .ok_or_else(|| invalid("HAR header value is missing"))?;
                HeaderField::try_new(name, value)
                    .map_err(|_| invalid("HAR contains an invalid header"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}
fn charge(result: &mut ImportData, value: &Value) -> Result<(), AppError> {
    result.index_bytes = result.index_bytes.saturating_add(
        serde_json::to_vec(value)
            .map_err(|_| invalid("Invalid trace metadata"))?
            .len(),
    );
    if result.index_bytes > MAX_HEAD_BYTES || result.sessions.len() >= MAX_SESSIONS {
        return Err(invalid("Trace exceeds the viewer index limit"));
    }
    Ok(())
}

/// Imported JSON body bytes are spooled once and retained by their source lease.
#[derive(Debug)]
struct JsonBody {
    file: Arc<tempfile::NamedTempFile>,
    offset: u64,
    bytes: u64,
}
impl SavedBodySource for JsonBody {
    fn open(&self) -> std::io::Result<Box<dyn Read + Send>> {
        let mut source = SourceReader::new(self.file.reopen()?)?;
        source.seek(SeekFrom::Start(self.offset))?;
        Ok(Box::new(source.take(self.bytes)))
    }
    fn open_at(&self, offset: u64) -> std::io::Result<Box<dyn Read + Send>> {
        if offset > self.bytes {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let mut source = SourceReader::new(self.file.reopen()?)?;
        source.seek(SeekFrom::Start(self.offset + offset))?;
        Ok(Box::new(source.take(self.bytes - offset)))
    }
    fn length(&self) -> Option<u64> {
        Some(self.bytes)
    }
}
struct BodySpool {
    file: tempfile::NamedTempFile,
    pieces: Vec<(usize, u64, u64)>,
}
#[derive(Debug)]
struct PendingBody;
impl SavedBodySource for PendingBody {
    fn open(&self) -> std::io::Result<Box<dyn Read + Send>> {
        Err(std::io::Error::other("Body import is not published"))
    }
    fn length(&self) -> Option<u64> {
        None
    }
}
impl BodySpool {
    fn new() -> Result<Self, AppError> {
        Ok(Self {
            file: tempfile::NamedTempFile::new()
                .map_err(|_| unavailable("Imported body cache could not be created"))?,
            pieces: vec![],
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        result: &mut ImportData,
        snapshot: &mut SessionSnapshot,
        boundary: ExchangeBoundary,
        headers: HeaderBlock,
        response: Option<ResponseHead>,
        bytes: Option<Vec<u8>>,
        observed: u64,
    ) -> Result<(), AppError> {
        let complete = bytes.is_some();
        let bytes = bytes.unwrap_or_default();
        let offset = self
            .file
            .as_file_mut()
            .stream_position()
            .map_err(|_| unavailable("Imported body cache failed"))?;
        self.file
            .write_all(&bytes)
            .map_err(|_| unavailable("Imported body cache is full or unavailable"))?;
        self.pieces
            .push((result.bodies.len(), offset, bytes.len() as u64));
        result.bodies.push(ImportedBody {
            id: snapshot.exchange_id,
            boundary,
            headers,
            response,
            source: Arc::new(PendingBody),
            observed,
            complete,
        });
        snapshot.bodies.push(BodySnapshot {
            boundary,
            observed_bytes: observed,
            retained_prefix: bytes::Bytes::new(),
            truncated: !complete,
            trailers: None,
        });
        Ok(())
    }
    fn finish(self, result: &mut ImportData) {
        let file = Arc::new(self.file);
        for (position, offset, bytes) in self.pieces {
            result.bodies[position].source = Arc::new(JsonBody {
                file: file.clone(),
                offset,
                bytes,
            });
        }
    }
}

#[allow(clippy::too_many_lines)]
fn har(
    document: &Value,
    prefix: u64,
    trace_id: &str,
    canceled: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<ImportData, AppError> {
    let log = document
        .get("log")
        .ok_or_else(|| invalid("HAR log is missing"))?;
    if !string(log, "version").starts_with("1.") {
        return Err(invalid("Unsupported HAR version; expected HAR 1.x"));
    }
    let entries = log["entries"]
        .as_array()
        .ok_or_else(|| invalid("HAR entries must be an array"))?;
    if entries.len() > MAX_SESSIONS {
        return Err(invalid("HAR exceeds the viewer session limit"));
    }
    let capture_context = without_fields(log, &["entries", "_transmogCertificates"]);
    let mut certificates = HashMap::<String, Arc<[u8]>>::new();
    let mut certificate_bytes = 0_usize;
    if let Some(definitions) = log.get("_transmogCertificates") {
        let definitions: Vec<transmog_capture::CertificateDefinition> =
            serde_json::from_value(definitions.clone())
                .map_err(|_| invalid("Invalid HAR public certificate dictionary"))?;
        for definition in definitions {
            definition
                .validate()
                .map_err(|_| invalid("Invalid HAR public certificate"))?;
            certificate_bytes = certificate_bytes.saturating_add(definition.der.len());
            if certificate_bytes > super::MAX_HEAD_BYTES
                || certificates
                    .insert(definition.sha256, definition.der.into())
                    .is_some()
            {
                return Err(invalid("Excessive or duplicate HAR certificates"));
            }
        }
    }
    let mut result = empty("har", capture_context);
    result.index_bytes = certificate_bytes;
    let mut spool = BodySpool::new()?;
    for (position, entry) in entries.iter().enumerate() {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        let mut evidence = without_fields(entry, &["request", "response"]);
        evidence["request"] =
            without_fields(&entry["request"], &["postData", "_transmogBodyBase64"]);
        evidence["response"] = without_fields(&entry["response"], &["content"]);
        charge(&mut result, &evidence)?;
        let id = ExchangeId(u128::from(prefix) << 64 | (position as u128 + 1));
        let req = &entry["request"];
        let resp = &entry["response"];
        let request_headers = headers(&req["headers"])?;
        let mut request = native_request(string(req, "method"), string(req, "url"), vec![])?;
        request.headers = request_headers;
        request.source_version = version(string(req, "httpVersion"));
        let status = resp["status"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| invalid("Invalid HAR response status"))?;
        let response = (status != 0).then(|| ResponseHead {
            status,
            headers: headers(&resp["headers"]).unwrap_or_default(),
            source_version: version(string(resp, "httpVersion")),
        });
        // Validate headers even for failed requests with status 0.
        let response_headers = headers(&resp["headers"])?;
        let started = timestamp(string(entry, "startedDateTime"))
            .ok_or_else(|| invalid("Invalid HAR startedDateTime"))?;
        let duration = entry["time"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| invalid("Invalid HAR elapsed time"))?;
        let ended = started.checked_add(
            Duration::try_from_secs_f64(duration / 1000.0)
                .map_err(|_| invalid("HAR elapsed time exceeds its range"))?,
        );
        let meta = metadata(
            id,
            request.target.clone(),
            "127.0.0.1:0".parse().expect("constant endpoint"),
            ClientIdentity::default(),
            request.source_version,
            started,
        );
        let mut snapshot = empty_snapshot(meta, Some(request.clone()), response.clone(), ended);
        protocol(
            &mut snapshot,
            ExchangeBoundary::ClientRequest,
            string(req, "httpVersion"),
            None,
        );
        protocol(
            &mut snapshot,
            ExchangeBoundary::ClientResponse,
            string(resp, "httpVersion"),
            Some(string(resp, "statusText").into()),
        );
        if let Some(peer) = entry["serverIPAddress"].as_str() {
            snapshot.performance.transports.push(TransportObservation {
                leg: "upstream".into(),
                connection_id: string(entry, "connection").into(),
                peer: Some(peer.into()),
                ..TransportObservation::default()
            });
        }
        if let Some(saved) = entry.get("_transmogPerformance") {
            let mut performance: transmog_core::performance::PerformanceEvidence =
                serde_json::from_value(saved.clone())
                    .map_err(|_| invalid("Invalid HAR transport evidence"))?;
            if !performance.valid() {
                return Err(invalid("HAR transport evidence exceeds its limits"));
            }
            for certificate in performance
                .transports
                .iter_mut()
                .filter_map(|transport| transport.tls.as_mut())
                .flat_map(|tls| tls.server_chain.iter_mut().chain(&mut tls.client_chain))
            {
                if let Some(der) = certificates.get(&certificate.sha256) {
                    certificate.der = der.clone();
                }
            }
            snapshot.performance.merge(&performance);
        }
        if entry.get("_transmogPerformance").is_none()
            && let Some(security) = entry
                .get("_securityDetails")
                .or_else(|| resp.get("_securityDetails"))
        {
            let mut tls = transmog_core::TlsEvidence {
                server_name: Some(request.target.host.clone()),
                ..transmog_core::TlsEvidence::default()
            };
            if let Some(fields) = security.as_object() {
                for (name, value) in fields {
                    if name.len() <= 252 && tls.details.len() < 64 {
                        let text = value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned);
                        if text.len() <= 4096 {
                            tls.details.insert(format!("HAR {name}"), text);
                        }
                    }
                }
            }
            if !tls.details.is_empty() {
                let transport = if let Some(transport) = snapshot.performance.transports.last_mut()
                {
                    transport
                } else {
                    snapshot.performance.transports.push(TransportObservation {
                        leg: "upstream".into(),
                        connection_id: string(entry, "connection").into(),
                        ..TransportObservation::default()
                    });
                    snapshot
                        .performance
                        .transports
                        .last_mut()
                        .expect("inserted transport")
                };
                transport.tls_version = security["protocol"]
                    .as_str()
                    .filter(|value| value.len() <= 512)
                    .map(str::to_owned);
                transport.cipher = security["cipher"]
                    .as_str()
                    .filter(|value| value.len() <= 512)
                    .map(str::to_owned);
                transport.tls = Some(tls);
            }
        }
        let mut timings = BTreeMap::new();
        for key in [
            "startedDateTime",
            "time",
            "pageref",
            "connection",
            "serverIPAddress",
            "comment",
            "cache",
        ] {
            if let Some(value) = entry.get(key) {
                timings.insert(
                    format!("HAR {key}"),
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned),
                );
            }
        }
        if let Some(fields) = entry["timings"].as_object() {
            for (key, value) in fields {
                timings.insert(
                    format!("HAR {key} (ms)"),
                    if value.as_f64() == Some(-1.0) {
                        "Unavailable".into()
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        let request_body = if let Some(text) = req["_transmogBodyBase64"].as_str() {
            Some(
                STANDARD
                    .decode(text)
                    .map_err(|_| invalid("Invalid saved binary request body"))?,
            )
        } else if let Some(text) = req["postData"]["text"].as_str() {
            Some(text.as_bytes().to_vec())
        } else if req["bodySize"].as_i64() == Some(0) {
            Some(vec![])
        } else if string(&req["postData"], "mimeType") == "application/x-www-form-urlencoded" {
            req["postData"]["params"].as_array().map(|fields| {
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                for field in fields {
                    serializer.append_pair(string(field, "name"), string(field, "value"));
                }
                serializer.finish().into_bytes()
            })
        } else {
            None
        };
        // Multipart params do not contain framing bytes; retain metadata, not invented bodies.
        if req.get("postData").is_some() {
            timings.insert("HAR post data metadata".into(), serde_json::json!({"mimeType":req["postData"]["mimeType"],"params":req["postData"]["params"]}).to_string());
        }
        let request_observed = req["bodySize"]
            .as_u64()
            .unwrap_or_else(|| request_body.as_ref().map_or(0, |b| b.len() as u64));
        spool.add(
            &mut result,
            &mut snapshot,
            ExchangeBoundary::ClientRequest,
            request.headers.clone(),
            None,
            request_body,
            request_observed,
        )?;
        let content = &resp["content"];
        let body = match content["text"].as_str() {
            Some(text) if string(content, "encoding") == "base64" => Some(
                STANDARD
                    .decode(text)
                    .map_err(|_| invalid("Invalid HAR base64 response body"))?,
            ),
            Some(text) if string(content, "encoding").is_empty() => Some(text.as_bytes().to_vec()),
            Some(_) => return Err(invalid("Unsupported HAR response content encoding")),
            None if content["size"].as_u64() == Some(0)
                || req["method"] == "HEAD"
                || matches!(status, 204 | 304) =>
            {
                Some(vec![])
            }
            None => None,
        };
        let mut body_headers = response_headers;
        body_headers.remove_all("content-encoding");
        body_headers.remove_all("transfer-encoding");
        body_headers.remove_all("content-length");
        if let Some(bytes) = &body {
            body_headers.push(
                HeaderField::try_new("Content-Length", bytes.len().to_string())
                    .expect("decimal length"),
            );
        }
        if content["text"].is_string() && string(content, "encoding").is_empty() {
            body_headers.remove_all("content-type");
            let mime = string(content, "mimeType")
                .split(';')
                .next()
                .unwrap_or("text/plain");
            body_headers.push(
                HeaderField::try_new("Content-Type", format!("{mime}; charset=utf-8"))
                    .map_err(|_| invalid("Invalid HAR content type"))?,
            );
        }
        let observed = resp["bodySize"]
            .as_u64()
            .unwrap_or_else(|| body.as_ref().map_or(0, |b| b.len() as u64));
        let storage_response = response.clone().map(|mut head| {
            head.headers = body_headers.clone();
            head
        });
        spool.add(
            &mut result,
            &mut snapshot,
            ExchangeBoundary::ClientResponse,
            body_headers,
            storage_response,
            body,
            observed,
        )?;
        result.entries.insert(
            format!("{:032x}", id.0),
            TraceEntry {
                trace_id: trace_id.into(),
                original_id: (position + 1).to_string(),
                raw_headers: None,
                timings,
                protocol_known: !string(req, "httpVersion").is_empty(),
                target_known: true,
                diagnostics: vec![],
            },
        );
        result.sessions.push(snapshot);
        progress(position as u64 + 1, entries.len() as u64);
    }
    spool.finish(&mut result);
    result.notes.push("HAR body text is decoded content. Original headers and wire sizes are preserved; unavailable and multipart-only body data are not reconstructed.".into());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, Application, BodyStoreConfig, TraceImportRequest, TraceSaveOptions};
    use serde_json::json;

    fn application(root: &std::path::Path) -> Application {
        Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.join("cache"))),
            ..AppConfig::default()
        })
        .unwrap()
    }
    fn har_fixture() -> Value {
        json!({"log":{"version":"1.2","creator":{"name":"Placeholder","version":"1"},"entries":[{"startedDateTime":"2026-01-01T00:00:00.000Z","time":6.5,"request":{"method":"GET","url":"https://placeholder.invalid:443/resource?placeholder=value","httpVersion":"HTTP/2","headers":[{"name":"X-Placeholder","value":"placeholder"}],"cookies":[],"queryString":[{"name":"placeholder","value":"value"}],"headersSize":-1,"bodySize":0},"response":{"status":200,"statusText":"","httpVersion":"HTTP/2","headers":[{"name":"Content-Encoding","value":"gzip"},{"name":"Content-Type","value":"application/octet-stream"}],"cookies":[],"headersSize":-1,"bodySize":24,"redirectURL":"","content":{"size":4,"mimeType":"application/octet-stream","encoding":"base64","text":"AAEC/w=="}},"cache":{},"timings":{"send":1,"wait":3.5,"receive":2,"dns":-1},"serverIPAddress":"192.0.2.1","connection":"placeholder"}]}})
    }
    async fn load(application: &Application, path: std::path::PathBuf, operation: &str) -> String {
        let result = application
            .import_trace(
                TraceImportRequest {
                    path,
                    operation_id: operation.into(),
                    password: None,
                    max_file_bytes: u64::MAX,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(result.trace.sessions, 1);
        application
            .session_service()
            .catalog()
            .project_retained(|s| format!("{:032x}", s.exchange_id.0))
            .last()
            .unwrap()
            .clone()
    }
    fn read(application: &Application, id: &str) -> Vec<u8> {
        let id = ExchangeId(u128::from_str_radix(id, 16).unwrap());
        let mut body = application
            .body_store()
            .unwrap()
            .open_complete(id, ExchangeBoundary::ClientResponse)
            .unwrap();
        let mut bytes = vec![];
        body.read_to_end(&mut bytes).unwrap();
        bytes
    }
    #[tokio::test]
    async fn browser_produced_archives_preserve_http_heads_and_response_content() {
        let archives =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser");
        for format in ["har", "netlog", "jsonl"] {
            let root = tempfile::tempdir().unwrap();
            let app = application(root.path());
            app.import_trace(
                TraceImportRequest {
                    path: archives.join(format!("placeholder.{format}")),
                    operation_id: format.into(),
                    password: None,
                    max_file_bytes: u64::MAX,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
            let requests = app
                .session_service()
                .catalog()
                .project_retained(Clone::clone)
                .into_iter()
                .filter(|s| s.metadata.original_target.as_target().host == "placeholder.invalid")
                .collect::<Vec<_>>();
            assert!(requests.len() >= 2);
            for request in requests {
                let head = &request.response_heads.last().unwrap().head;
                assert_eq!(head.status, 200);
                assert_eq!(
                    head.headers.values("x-placeholder").next(),
                    Some(b"placeholder".as_slice())
                );
                let target = request.metadata.original_target.as_target();
                assert_eq!(target.port, 8080);
                let expected = if target.path == "/style.css" {
                    assert_eq!(target.query.as_deref(), Some("placeholder=value"));
                    b"body { color: rgb(10, 20, 30); }".as_slice()
                } else {
                    assert_eq!(target.path, "/");
                    b"<!doctype html><link rel=\"stylesheet\" href=\"/style.css?placeholder=value\"><h1>Placeholder</h1>".as_slice()
                };
                let id = format!("{:032x}", request.exchange_id.0);
                assert_eq!(read(&app, &id), expected, "{format}: {}", target.path);
            }
        }
    }
    #[tokio::test]
    async fn chromium_jsonl_bom_interruption_and_invalid_tail_preserve_atomic_imports() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let source = include_bytes!("../tests/fixtures/browser/placeholder.jsonl");
        let path = root.path().join("browser.data");
        let mut full = vec![0xef, 0xbb, 0xbf];
        full.extend_from_slice(source);
        std::fs::write(&path, &full).unwrap();
        let request = |path: std::path::PathBuf, operation: &str| TraceImportRequest {
            path,
            password: None,
            operation_id: operation.into(),
            max_file_bytes: u64::MAX,
        };
        let complete = app
            .import_trace(request(path, "complete"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert_eq!(complete.trace.sessions, 2);
        assert!(
            complete
                .trace
                .notes
                .iter()
                .all(|note| !note.contains("no end marker"))
        );
        let end = std::str::from_utf8(source)
            .unwrap()
            .rfind("{\"type\":\"end\"}")
            .unwrap();
        let mut interrupted = vec![0xef, 0xbb, 0xbf];
        interrupted.extend_from_slice(&source[..end]);
        interrupted.extend_from_slice(b"{\"type\":\"event\",\"event\":");
        let partial = root.path().join("partial.jsonl");
        std::fs::write(&partial, interrupted).unwrap();
        let recovered = app
            .import_trace(request(partial, "interrupted"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert_eq!(recovered.trace.sessions, 2);
        assert!(
            recovered
                .trace
                .notes
                .iter()
                .any(|note| note.contains("incomplete tail"))
        );
        let retained = app
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .retained_count;
        let bad = root.path().join("invalid.jsonl");
        full.extend_from_slice(b"{\"type\":\"event\",\"event\":{}}\n");
        std::fs::write(&bad, full).unwrap();
        assert!(
            app.import_trace(request(bad, "invalid"), Arc::new(|_| {}))
                .await
                .is_err()
        );
        assert_eq!(
            app.query_sessions(crate::SessionQueryInput::default())
                .unwrap()
                .retained_count,
            retained
        );
        let saved = root.path().join("jsonl.tmcap");
        app.save_traffic_trace(saved.clone(), TraceSaveOptions::default())
            .await
            .unwrap();
        let reopened = application(&root.path().join("reopened"));
        assert_eq!(
            reopened
                .import_trace(request(saved, "native"), Arc::new(|_| {}))
                .await
                .unwrap()
                .trace
                .sessions,
            4
        );
    }

    #[tokio::test]
    async fn clear_and_close_release_imported_sources_and_metadata() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let path = root.path().join("placeholder.har");
        std::fs::write(&path, serde_json::to_vec(&har_fixture()).unwrap()).unwrap();
        let id = load(&app, path.clone(), "clear").await;
        let cleared = app.clear_traffic().await.unwrap();
        assert_eq!(cleared.ids, vec![id.clone()]);
        assert!(cleared.undoable);
        assert_eq!(
            app.remove_traffic_entries(&cleared.ids, true).unwrap(),
            vec![id.clone()]
        );
        app.discard_traffic().unwrap();
        assert!(app.traces.list().is_empty());
        assert!(
            app.body_store()
                .unwrap()
                .metadata(ExchangeId(u128::from_str_radix(&id, 16).unwrap()))
                .is_empty()
        );
        assert!(
            app.session_service()
                .catalog()
                .project_retained(|s| s.exchange_id)
                .is_empty()
        );
        assert!(
            path.exists(),
            "Releasing an imported source must preserve the user's file"
        );
    }
    #[tokio::test]
    async fn compliant_har_preserves_decoded_binary_heads_and_foreign_timers_across_saves() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let path = root.path().join("placeholder.data");
        std::fs::write(&path, serde_json::to_vec(&har_fixture()).unwrap()).unwrap();
        let id = load(&app, path, "har").await;
        assert_eq!(read(&app, &id), [0, 1, 2, 255]);
        Box::pin(
            app.prepare_response_file(&id, "client-response")
                .unwrap()
                .write_to(&root.path().join("placeholder.bin"), u64::MAX),
        )
        .await
        .unwrap();
        let detail = app.session_detail(&id).unwrap();
        assert!(
            detail.responses[0]
                .headers
                .iter()
                .any(|h| h.name == "Content-Encoding" && h.value == "gzip")
        );
        assert_eq!(detail.saved_evidence["HAR dns (ms)"], "Unavailable");
        let native = root.path().join("placeholder.tmcap");
        app.save_traffic_trace(native.clone(), TraceSaveOptions::default())
            .await
            .unwrap();
        let reopened = application(root.path());
        let saved = load(&reopened, native.clone(), "native").await;
        assert_eq!(read(&reopened, &saved), [0, 1, 2, 255]);
        let saz = root.path().join("placeholder.saz");
        app.export_capture(crate::ExportRequest {
            source_password: None,
            password: None,
            redact_sensitive_headers: false,
            source: native,
            destination: saz.clone(),
            format: crate::ExportFormat::SazExtended,
            max_source_bytes: u64::MAX,
        })
        .await
        .unwrap();
        let converted = application(root.path());
        let converted_id = load(&converted, saz, "converted").await;
        let output = root.path().join("converted.bin");
        Box::pin(
            converted
                .prepare_response_file(&converted_id, "client-response")
                .unwrap()
                .write_to(&output, u64::MAX),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(output).unwrap(), [0, 1, 2, 255]);
        let har = root.path().join("placeholder.har");
        app.save_traffic_trace(
            har.clone(),
            TraceSaveOptions {
                format: crate::TraceSaveFormat::Har,
                ..TraceSaveOptions::default()
            },
        )
        .await
        .unwrap();
        let exported: Value = serde_json::from_slice(&std::fs::read(&har).unwrap()).unwrap();
        assert_eq!(exported["log"]["entries"][0]["time"], 6.5);
        assert_eq!(
            exported["log"]["entries"][0]["response"]["content"]["text"],
            "AAEC/w=="
        );
        let reimported = application(root.path());
        let id = load(&reimported, har, "har-again").await;
        assert_eq!(read(&reimported, &id), [0, 1, 2, 255]);
    }
    #[tokio::test]
    async fn netlog_numeric_constants_map_http_heads_errors_and_original_clock() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let path = root.path().join("placeholder.data");
        let log = json!({"constants":{"logSourceType":{"URL_REQUEST":1},"logEventTypes":{"URL_REQUEST_START_JOB":1,"HTTP_TRANSACTION_SEND_REQUEST_HEADERS":2,"HTTP_TRANSACTION_READ_RESPONSE_HEADERS":3,"URL_REQUEST_JOB_BYTES_READ":4},"logEventPhase":{"PHASE_NONE":0,"PHASE_BEGIN":1,"PHASE_END":2},"timeTickOffset":"1767225600000"},"events":[{"source":{"id":1,"type":1},"type":1,"phase":1,"time":"100","params":{"url":"https://placeholder.invalid/resource","method":"GET"}},{"source":{"id":1,"type":1},"type":2,"phase":0,"time":"101","params":{"line":"GET /resource HTTP/1.1","headers":["X-Placeholder: placeholder"]}},{"source":{"id":1,"type":1},"type":3,"phase":0,"time":"102","params":{"headers":["HTTP/1.1 200 Placeholder","Content-Type: text/plain"]}},{"source":{"id":1,"type":1},"type":4,"phase":0,"time":"104","params":{"byte_count":4}}]});
        std::fs::write(&path, serde_json::to_vec(&log).unwrap()).unwrap();
        let id = load(&app, path, "netlog").await;
        let detail = app.session_detail(&id).unwrap();
        assert_eq!(detail.requests[0].method.as_deref(), Some("GET"));
        assert_eq!(detail.responses[0].status, Some(200));
        assert!(detail.performance.points.is_empty());
        assert_eq!(detail.saved_evidence["NetLog elapsed (ms)"], "4");
        assert_eq!(detail.bodies[1].observed_bytes, 4);
    }
    #[tokio::test]
    async fn har_security_summaries_are_visible_without_inventing_certificate_bytes() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let path = root.path().join("summary.har");
        let document = json!({"log":{"version":"1.2","entries":[{"startedDateTime":"2026-10-10T12:00:00Z","time":1,"request":{"method":"GET","url":"https://localhost/test","httpVersion":"HTTP/2","headers":[]},"response":{"status":200,"httpVersion":"HTTP/2","headers":[],"content":{"text":""}},"_securityDetails":{"protocol":"TLS 1.3","issuer":"Recorded issuer","subjectName":"localhost"}}]}});
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let id = load(&app, path, "har-security").await;
        let inspection = app.inspect_certificates(&id).unwrap();
        assert!(inspection.certificates.is_empty());
        assert!(
            inspection.connections[0]
                .fields
                .iter()
                .any(|field| field.label == "HAR issuer" && field.value == "Recorded issuer")
        );
        assert!(
            inspection.connections[0]
                .fields
                .iter()
                .any(|field| field.label == "TLS version" && field.value == "TLS 1.3")
        );
        assert!(
            inspection.connections[0]
                .fields
                .iter()
                .any(|field| field.label == "Recorded verification"
                    && field.value.starts_with("Not recorded"))
        );
    }

    #[tokio::test]
    async fn netlog_maps_nested_certificate_paths_client_counts_and_verification_errors() {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let path = root.path().join("tls-netlog.json");
        let leaf = include_str!("../tests/fixtures/certificates/leaf.pem");
        let issuer = include_str!("../tests/fixtures/certificates/root.pem");
        let source = |id, kind| json!({"id":id,"type":kind});
        let log = json!({"constants":{"logSourceType":{"URL_REQUEST":1,"SOCKET":2,"CERT_VERIFIER_JOB":3},"logEventTypes":{"URL_REQUEST_START_JOB":1,"HTTP_TRANSACTION_SEND_REQUEST_HEADERS":2,"BOUND":3,"SSL_CERTIFICATES_RECEIVED":4,"CERT_VERIFIER_JOB":5,"SSL_CLIENT_CERT_PROVIDED":6,"SSL_HANDSHAKE_ERROR":7},"logEventPhase":{"PHASE_NONE":0,"PHASE_BEGIN":1,"PHASE_END":2}},"events":[
            {"source":source(1,1),"type":1,"phase":1,"time":"1","params":{"url":"https://localhost/test","method":"GET"}},
            {"source":source(1,1),"type":2,"phase":0,"time":"2","params":{"line":"GET /test HTTP/2","headers":["Accept: */*"]}},
            {"source":source(1,1),"type":3,"phase":0,"time":"3","params":{"source_dependency":{"id":2}}},
            {"source":source(2,2),"type":4,"phase":0,"time":"4","params":{"certificates":[leaf]}},
            {"source":source(2,2),"type":6,"phase":0,"time":"5","params":{"cert_count":1}},
            {"source":source(2,2),"type":3,"phase":0,"time":"6","params":{"source_dependency":{"id":3}}},
            {"source":source(3,3),"type":5,"phase":2,"time":"7","params":{"net_error":-202,"cert_status":4,"verified_cert":{"certificates":[leaf,issuer]}}},
            {"source":source(2,2),"type":7,"phase":0,"time":"8","params":{"net_error":-202,"ssl_lib_error":123}}
        ]});
        std::fs::write(&path, serde_json::to_vec(&log).unwrap()).unwrap();
        let id = load(&app, path, "netlog-tls").await;
        let view = app.inspect_certificates(&id).unwrap();
        assert_eq!(view.certificates.len(), 2);
        let connection = &view.connections[0];
        assert_eq!(connection.title, "Browser → server");
        assert_eq!(connection.server_chain.len(), 2);
        assert!(connection.client_chain.is_empty());
        assert!(
            connection
                .fields
                .iter()
                .any(|field| field.label == "Chromium client certificate count"
                    && field.value == "1")
        );
        assert!(
            connection
                .fields
                .iter()
                .any(|field| field.value.contains("Rejected (code -202"))
        );
        assert!(
            connection
                .errors
                .iter()
                .any(|error| error.contains("SSL_HANDSHAKE_ERROR"))
        );
        assert!(
            view.certificates
                .iter()
                .all(|certificate| !certificate.der_base64.is_empty())
        );
    }
}
