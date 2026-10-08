//! Frozen captured responses for a render-only native browser window.
use crate::{AppError, Application, ErrorCategory, inspector::parse_session_id};
use std::fmt::Write as _;
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use transmog_core::{ResponseHead, intercept::ExchangeId, observe::ExchangeBoundary};

const MAX_RESOURCES: usize = 2048;
const MAX_RESOURCE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCENE_BYTES: u64 = 256 * 1024 * 1024;

/// A captured resource already decoded and materialized outside the UI thread.
#[derive(Clone, Debug)]
pub struct CapturedResource {
    /// Actual captured final status, when browser-compatible.
    pub status: u16,
    /// Original or canonical HTTP reason phrase.
    pub reason: String,
    /// Safe browser response headers, with transport framing repaired.
    pub headers: String,
    /// Owned immutable body file, never a caller-selected filesystem path.
    pub body_path: PathBuf,
}
/// Frozen scene. Dropping it removes owned response files after consumers release it.
pub struct CapturedPage {
    /// Original URL used for relative resource resolution and browser origin.
    pub url: String,
    /// Number of response variants available in this scene.
    pub resources: usize,
    /// Unavailable or bounded-out variants; misses still return empty 404s.
    pub skipped: usize,
    files: HashMap<(String, String), CapturedResource>,
    _root: tempfile::TempDir,
}
impl CapturedPage {
    /// Finds an exact method and canonical URL. It never performs network I/O.
    pub fn resource(&self, method: &str, url: &str) -> Option<CapturedResource> {
        self.files
            .get(&(method.to_owned(), canonical_url(url)?))
            .cloned()
    }
}

struct Candidate {
    id: ExchangeId,
    method: String,
    url: String,
    head: ResponseHead,
    time: u64,
    reason: Option<String>,
}
pub(crate) async fn prepare(
    application: Application,
    id: String,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<CapturedPage, AppError> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(build(&application, &id, &canceled)))
        .await
        .map_err(|_| error("Captured page preparation failed"))?
}
async fn build(
    application: &Application,
    id: &str,
    canceled: &std::sync::atomic::AtomicBool,
) -> Result<CapturedPage, AppError> {
    let numeric = ExchangeId(parse_session_id(id)?);
    let selected = application
        .service
        .catalog()
        .get(numeric)
        .ok_or_else(|| error("Selected request is unavailable"))?;
    let store = application
        .body_store
        .as_ref()
        .ok_or_else(|| error("Response retention is unavailable"))?;
    store
        .flush()
        .map_err(|_| error("Response storage is unavailable"))?;
    require_html(store, numeric)?;
    let source = application.traces.source_id(id);
    let anchor = millis(selected.metadata.started_at);
    let mut candidates = candidates(application, source.as_deref());
    candidates.sort_by_key(|candidate| (candidate.id != numeric, candidate.time.abs_diff(anchor)));
    let root = tempfile::Builder::new()
        .prefix("transmog-captured-page-")
        .tempdir()
        .map_err(|_| error("Captured page storage is unavailable"))?;
    let mut files = HashMap::new();
    let mut bytes = 0;
    let mut skipped = 0;
    let mut primary_url = None;
    for candidate in candidates {
        if canceled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(error("Captured page preview canceled"));
        }
        let key = (candidate.method.clone(), candidate.url.clone());
        if files.contains_key(&key) {
            continue;
        }
        if files.len() >= MAX_RESOURCES || bytes >= MAX_SCENE_BYTES {
            skipped += 1;
            continue;
        }
        let path = root.path().join(format!("resource-{}.bin", files.len()));
        let prepared = crate::response_file::prepare(
            &application.service,
            Some(store),
            &format!("{:032x}", candidate.id.0),
            "client-response",
        );
        let result = match prepared {
            Ok(body) => {
                Box::pin(body.write_to(&path, MAX_RESOURCE_BYTES.min(MAX_SCENE_BYTES - bytes)))
                    .await
            }
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(result) => result,
            Err(failure) if candidate.id == numeric => return Err(failure),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if !(200..=599).contains(&candidate.head.status) {
            skipped += 1;
            continue;
        }
        bytes += result.bytes;
        let reason = candidate.reason.unwrap_or_else(|| {
            http::StatusCode::from_u16(candidate.head.status)
                .ok()
                .and_then(|status| status.canonical_reason())
                .unwrap_or("")
                .to_owned()
        });
        let resource = CapturedResource {
            status: candidate.head.status,
            reason,
            headers: browser_headers(&candidate.head, result.bytes),
            body_path: path,
        };
        if candidate.id == numeric {
            primary_url = Some(candidate.url.clone());
            files.insert(("GET".into(), candidate.url.clone()), resource.clone());
        }
        files.insert(key, resource);
    }
    Ok(CapturedPage {
        url: primary_url
            .ok_or_else(|| error("A complete HTML response is required for preview"))?,
        resources: files.len(),
        skipped,
        files,
        _root: root,
    })
}
fn require_html(store: &crate::BodyStore, numeric: ExchangeId) -> Result<(), AppError> {
    let metadata = store
        .metadata(numeric)
        .into_iter()
        .find(|body| body.boundary == "client-response")
        .ok_or_else(|| error("The selected response has no retained body"))?;
    if !matches!(
        metadata.media_type.as_deref(),
        Some("text/html" | "application/xhtml+xml")
    ) {
        return Err(error("Select an HTML response to preview"));
    }
    Ok(())
}

fn candidates(application: &Application, source: Option<&str>) -> Vec<Candidate> {
    application
        .service
        .catalog()
        .project_retained(|snapshot| {
            let request = snapshot
                .request_heads
                .iter()
                .find(|head| head.boundary == ExchangeBoundary::ClientRequest)?;
            let response = snapshot
                .response_heads
                .iter()
                .find(|head| head.boundary == ExchangeBoundary::ClientResponse)?;
            if application
                .traces
                .source_id(&format!("{:032x}", snapshot.exchange_id.0))
                .as_deref()
                != source
            {
                return None;
            }
            let target = &request.head.target;
            let url = canonical_url(&format!(
                "{}://{}{}{}",
                target.scheme,
                target.authority,
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}"))
            ))?;
            Some(Candidate {
                id: snapshot.exchange_id,
                method: request.head.method.clone(),
                url,
                head: response.head.clone(),
                time: millis(snapshot.metadata.started_at),
                reason: snapshot
                    .performance
                    .protocols
                    .iter()
                    .find(|protocol| protocol.boundary == "client-response")
                    .and_then(|protocol| protocol.reason.clone()),
            })
        })
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
}
fn canonical_url(input: &str) -> Option<String> {
    let mut url = url::Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.into())
}
fn browser_headers(head: &ResponseHead, length: u64) -> String {
    let mut out = String::new();
    for header in head.headers.iter() {
        let (Ok(name), Ok(value)) = (
            std::str::from_utf8(header.name()),
            std::str::from_utf8(header.value()),
        ) else {
            continue;
        };
        if header.is_redacted()
            || http::HeaderName::from_bytes(header.name()).is_err()
            || matches!(
                name.to_ascii_lowercase().as_str(),
                "content-encoding"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "keep-alive"
                    | "proxy-authenticate"
                    | "proxy-authorization"
                    | "upgrade"
                    | "cache-control"
                    | "expires"
                    | "age"
                    | "content-disposition"
            )
            || value.contains(['\r', '\n', '\0'])
        {
            continue;
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    write!(out, "Content-Length: {length}\r\nCache-Control: no-store\r\nContent-Security-Policy: worker-src 'none'; frame-src http: https: data: blob:; connect-src http: https:; form-action http: https:; object-src 'none'; base-uri http: https:; sandbox allow-scripts allow-forms allow-same-origin\r\n" ).expect("string write");
    out
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
    AppError::new(ErrorCategory::Unavailable, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, BodyStoreConfig, SessionQueryInput, TraceImportRequest};
    use std::{
        io::{Cursor, Write},
        sync::{Arc, atomic::AtomicBool},
    };

    fn fixture(path: &std::path::Path, caption: &str) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (id, url, mime, body) in [
            (
                1,
                "https://example.invalid/page",
                "text/html",
                format!("<html><h1>{caption}</h1><link rel='stylesheet' href='/style.css'></html>"),
            ),
            (
                2,
                "https://example.invalid/style.css",
                "text/css",
                "h1 {color: green;}".into(),
            ),
            (
                3,
                "https://example.invalid/style.css",
                "text/css",
                "h1 {color: red;}".into(),
            ),
        ] {
            let seconds = if id == 3 { 9 } else { 0 };
            for (name, value) in [
                (
                    format!("raw/{id}_c.txt"),
                    format!("GET {url} HTTP/1.1\r\nHost: example.invalid\r\n\r\n"),
                ),
                (
                    format!("raw/{id}_s.txt"),
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: max-age=3600\r\nContent-Disposition: attachment\r\n\r\n{body}",
                        body.len()
                    ),
                ),
                (
                    format!("raw/{id}_m.xml"),
                    format!(
                        "<Session><SessionTimers ClientBeginRequest='2026-10-08T19:00:0{seconds}Z' ClientDoneResponse='2026-10-08T19:00:0{seconds}.025Z'/></Session>"
                    ),
                ),
            ] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(value.as_bytes()).unwrap();
            }
        }
        std::fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
    }
    #[tokio::test]
    async fn frozen_preview_uses_its_original_source_nearest_variants_and_no_fallback() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.saz");
        let second = root.path().join("second.saz");
        fixture(&first, "First source");
        fixture(&second, "Second source");
        let application = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        })
        .unwrap();
        for (path, key) in [(first, "first"), (second, "second")] {
            application
                .import_trace(
                    TraceImportRequest {
                        path,
                        operation_id: key.into(),
                        max_file_bytes: 1024 * 1024,
                    },
                    Arc::new(|_| {}),
                )
                .await
                .unwrap();
        }
        let rows = application
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        let row = rows
            .iter()
            .find(|row| {
                row.path == "/page"
                    && application
                        .trace_metadata(row.trace_id.as_deref().unwrap())
                        .unwrap()
                        .name
                        == "first.saz"
            })
            .unwrap();
        let scene = application
            .prepare_captured_page(row.id.clone(), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let page = scene
            .resource("GET", "https://example.invalid/page#fragment")
            .unwrap();
        assert!(
            std::fs::read_to_string(page.body_path)
                .unwrap()
                .contains("First source")
        );
        let css = scene
            .resource("GET", "https://EXAMPLE.invalid:443/style.css")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(css.body_path).unwrap(),
            "h1 {color: green;}"
        );
        assert!(scene.resource("GET", "https://missing.invalid/").is_none());
        assert!(
            scene
                .resource("POST", "https://example.invalid/style.css")
                .is_none()
        );
        assert!(scene.resource("GET", "file:///C:/private.txt").is_none());
        assert!(!page.headers.contains("max-age"));
        assert!(!page.headers.contains("attachment"));
        assert!(page.headers.contains("worker-src 'none'"));
        assert!(
            application
                .prepare_captured_page(row.id.clone(), Arc::new(AtomicBool::new(true)))
                .await
                .err()
                .unwrap()
                .message
                .contains("canceled")
        );
    }
    #[test]
    fn canonical_urls_reject_privileged_schemes_and_embedded_credentials() {
        assert!(canonical_url("https://user:password@example.invalid/").is_none());
        assert!(canonical_url("javascript:alert(1)").is_none());
        assert_eq!(
            canonical_url("https://EXAMPLE.invalid:443/path#anchor").unwrap(),
            "https://example.invalid/path"
        );
    }
}
