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
    let document: Value =
        serde_json::from_reader(std::io::BufReader::new(CancelReader { reader, canceled }))
            .map_err(|_| {
                if canceled.load(Ordering::Acquire) {
                    unavailable("Trace import canceled")
                } else {
                    invalid("The trace is not valid JSON")
                }
            })?;
    if canceled.load(Ordering::Acquire) {
        return Err(unavailable("Trace import canceled"));
    }
    match format {
        crate::TraceFormat::Har => har(&document, prefix, trace_id, canceled, progress),
        crate::TraceFormat::Netlog => {
            netlog::import(&document, prefix, trace_id, canceled, progress)
        }
        _ => Err(invalid("Unsupported JSON trace")),
    }
}

fn empty(format: &'static str, context: Value) -> ImportData {
    ImportData {
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
    let capture_context = without_fields(log, &["entries"]);
    let mut result = empty("har", capture_context);
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
        for format in ["har", "netlog"] {
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
}
