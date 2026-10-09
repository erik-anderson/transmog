//! Frozen captured responses for a render-only native browser window.
use crate::{
    AppError, Application, CapturedPageDiagnostics, CapturedPageOptions, CapturedPageScope,
    ErrorCategory, inspector::parse_session_id,
};
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
    files: HashMap<(String, String), Vec<ResourceVariant>>,
    diagnostics: CapturedPageDiagnostics,
    _root: tempfile::TempDir,
}
struct ResourceVariant {
    resource: CapturedResource,
    entry_id: String,
    body_sha256: Option<String>,
    vary: Option<Vec<(String, Option<String>)>>,
    primary: bool,
}
impl CapturedPage {
    /// Compatibility lookup for method/URL only, in deterministic nearest order.
    pub fn resource(&self, method: &str, url: &str) -> Option<CapturedResource> {
        let variants = self.files.get(&(method.to_owned(), canonical_url(url)?))?;
        variants
            .iter()
            .find(|variant| variant.primary)
            .or_else(|| variants.first())
            .map(|variant| variant.resource.clone())
    }
    /// Local diagnostics, independent of browser and capture lifetimes.
    pub fn diagnostics(&self) -> CapturedPageDiagnostics {
        self.diagnostics.clone()
    }
    /// Resolves an intercepted request against frozen body/Vary signatures.
    /// This never performs I/O or reaches an origin. Missing signatures fail closed.
    pub fn resolve(
        &self,
        method: &str,
        url: &str,
        headers: Option<&[(String, String)]>,
        body_sha256: Option<&str>,
        document: bool,
    ) -> Option<CapturedResource> {
        if url.len() > 16 * 1024 || method.len() > 32 {
            self.diagnostics.request(
                method,
                url,
                None,
                "Request URL or method exceeds the preview limit",
            );
            return None;
        }
        let variants = canonical_url(url).and_then(|url| self.files.get(&(method.to_owned(), url)));
        let selected = variants.and_then(|variants| {
            if document
                && method == "GET"
                && canonical_url(url).as_deref() == Some(&self.url)
                && let Some(primary) = variants.iter().find(|variant| variant.primary)
            {
                return Some((primary, "Selected captured HTML document"));
            }
            variants
                .iter()
                .find(|variant| {
                    body_sha256.is_some()
                        && variant.body_sha256.as_deref() == body_sha256
                        && variant.vary.as_ref().is_some_and(|vary| {
                            vary.iter().all(|(name, value)| {
                                headers.is_some_and(|headers| header_value(headers, name) == *value)
                                    || headers.is_none() && vary.is_empty()
                            })
                        })
                })
                .map(|variant| {
                    (
                        variant,
                        "Nearest captured response matching request body and Vary headers",
                    )
                })
        });
        let reason = selected.map_or_else(
            || {
                if variants.is_none() {
                    "No captured response for this method and URL"
                } else if body_sha256.is_none() {
                    "Request body could not be inspected or exceeds the preview limit"
                } else {
                    "No captured request body/Vary variant matched"
                }
            },
            |(_, reason)| reason,
        );
        self.diagnostics.request(
            method,
            url,
            selected.map(|(variant, _)| variant.entry_id.clone()),
            reason,
        );
        selected.map(|(variant, _)| variant.resource.clone())
    }
}
fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    let values = headers
        .iter()
        .filter(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim_matches([' ', '\t']))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| {
        values.join(if name.eq_ignore_ascii_case("cookie") {
            "; "
        } else {
            ", "
        })
    })
}
fn vary_signature(
    response: &transmog_core::HeaderBlock,
    request: &transmog_core::HeaderBlock,
) -> Option<Vec<(String, Option<String>)>> {
    let mut names = Vec::new();
    for field in response.iter().filter(|field| field.name_eq("vary")) {
        if field.is_redacted() {
            return None;
        }
        for name in std::str::from_utf8(field.value())
            .ok()?
            .split(',')
            .map(str::trim)
        {
            if name.is_empty() || name.eq_ignore_ascii_case("accept-encoding") {
                continue;
            }
            if name == "*"
                || name.len() > 128
                || http::HeaderName::from_bytes(name.as_bytes()).is_err()
            {
                return None;
            }
            let name = name.to_ascii_lowercase();
            if !names.contains(&name) {
                names.push(name);
            }
            if names.len() > 32 {
                return None;
            }
        }
    }
    let mut signature = Vec::new();
    let mut total = 0;
    for name in names {
        let mut values = Vec::new();
        for field in request.iter().filter(|field| field.name_eq(&name)) {
            if field.is_redacted() {
                return None;
            }
            let value = std::str::from_utf8(field.value()).ok()?;
            total += value.len();
            if total > 64 * 1024 {
                return None;
            }
            values.push(value.trim_matches([' ', '\t']).to_owned());
        }
        let value =
            (!values.is_empty()).then(|| values.join(if name == "cookie" { "; " } else { ", " }));
        signature.push((name, value));
    }
    Some(signature)
}

struct Candidate {
    id: ExchangeId,
    method: String,
    url: String,
    head: ResponseHead,
    time: u64,
    reason: Option<String>,
    vary: Option<Vec<(String, Option<String>)>>,
    source: String,
}
pub(crate) async fn prepare(
    application: Application,
    id: String,
    options: CapturedPageOptions,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<CapturedPage, AppError> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(build(&application, &id, &options, &canceled))
    })
    .await
    .map_err(|_| error("Captured page preparation failed"))?
}
async fn build(
    application: &Application,
    id: &str,
    options: &CapturedPageOptions,
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
    let mut candidates = candidates(application, source.as_deref(), options.scope);
    candidates.sort_by_key(|candidate| {
        (
            candidate.id != numeric,
            candidate.time.abs_diff(anchor),
            candidate.id.0,
        )
    });
    let root = tempfile::Builder::new()
        .prefix("transmog-captured-page-")
        .tempdir()
        .map_err(|_| error("Captured page storage is unavailable"))?;
    let mut scene = preparation::SceneBuilder::new(numeric, root);
    for candidate in candidates {
        scene.add(application, store, candidate, canceled).await?;
    }
    if canceled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(error("Captured page preview canceled"));
    }
    let source_name = source
        .as_deref()
        .and_then(|id| application.traces.metadata(id))
        .map_or_else(|| "Live traffic".into(), |metadata| metadata.name);
    scene.finish(options.scope, source_name)
}
#[path = "captured_page_build.rs"]
mod preparation;

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

fn candidates(
    application: &Application,
    source: Option<&str>,
    scope: CapturedPageScope,
) -> Vec<Candidate> {
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
            if scope == CapturedPageScope::OriginalTrace
                && application
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
                vary: vary_signature(&response.head.headers, &request.head.headers),
                source: application
                    .traces
                    .source_id(&format!("{:032x}", snapshot.exchange_id.0))
                    .and_then(|id| application.traces.metadata(&id))
                    .map_or_else(|| "Live traffic".into(), |metadata| metadata.name),
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
    fn variant_fixture(path: &std::path::Path, extra: bool) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let mut rows = vec![
            (
                1,
                "GET",
                "https://variants.invalid/page",
                "",
                "",
                "text/html",
                "<h1>Selected document</h1>",
                "",
            ),
            (
                2,
                "POST",
                "https://variants.invalid/api",
                "alpha",
                "X-Theme: light\r\n",
                "application/json",
                "alpha response",
                "Vary: X-Theme\r\n",
            ),
            (
                3,
                "POST",
                "https://variants.invalid/api",
                "beta",
                "X-Theme: dark\r\n",
                "application/json",
                "beta response",
                "Vary: X-Theme\r\n",
            ),
            (
                4,
                "GET",
                "https://variants.invalid/star",
                "",
                "",
                "text/plain",
                "unmatchable",
                "Vary: *\r\n",
            ),
        ];
        if extra {
            rows.push((
                5,
                "GET",
                "https://variants.invalid/extra.js",
                "",
                "",
                "text/javascript",
                "/* other trace */",
                "",
            ));
        }
        for (id, method, url, body, headers, mime, response, vary) in rows {
            for (name, value) in [
                (
                    format!("raw/{id}_c.txt"),
                    format!(
                        "{method} {url} HTTP/1.1\r\n{headers}Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    ),
                ),
                (
                    format!("raw/{id}_s.txt"),
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n{vary}Content-Length: {}\r\n\r\n{response}",
                        response.len()
                    ),
                ),
            ] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(value.as_bytes()).unwrap();
            }
        }
        zip.finish().unwrap();
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One scene qualifies variant matching, source isolation and bounded diagnostics together.
    async fn body_vary_variants_source_scope_and_local_diagnostics_fail_closed() {
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("original.saz");
        let second = root.path().join("additional.saz");
        variant_fixture(&first, false);
        variant_fixture(&second, true);
        let app = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        })
        .unwrap();
        let original = app
            .import_trace(
                TraceImportRequest {
                    path: first,
                    operation_id: "one".into(),
                    max_file_bytes: 1024 * 1024,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        app.import_trace(
            TraceImportRequest {
                path: second,
                operation_id: "two".into(),
                max_file_bytes: 1024 * 1024,
            },
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        let id = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions
            .into_iter()
            .find(|row| row.path == "/page" && row.trace_id.as_deref() == Some(&original.trace.id))
            .unwrap()
            .id;
        let scene = app
            .prepare_captured_page(id.clone(), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let empty = format!("{:x}", Sha256::digest([]));
        let beta = format!("{:x}", Sha256::digest(b"beta"));
        let alpha = format!("{:x}", Sha256::digest(b"alpha"));
        let headers = vec![("x-theme".into(), "dark".into())];
        let response = scene
            .resolve(
                "POST",
                "https://variants.invalid/api",
                Some(&headers),
                Some(&beta),
                false,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(response.body_path).unwrap(),
            "beta response"
        );
        assert!(
            scene
                .resolve(
                    "POST",
                    "https://variants.invalid/api",
                    Some(&headers),
                    Some(&alpha),
                    false
                )
                .is_none()
        );
        assert!(
            scene
                .resolve(
                    "POST",
                    "https://variants.invalid/api",
                    None,
                    Some(&beta),
                    false
                )
                .is_none()
        );
        assert!(
            scene
                .resolve(
                    "GET",
                    "https://variants.invalid/extra.js",
                    Some(&[]),
                    Some(&empty),
                    false
                )
                .is_none()
        );
        assert!(
            scene
                .resolve(
                    "GET",
                    "https://variants.invalid/star",
                    Some(&[]),
                    Some(&empty),
                    false
                )
                .is_none()
        );
        assert!(
            scene
                .resolve("GET", "https://variants.invalid/page", None, None, true)
                .is_some()
        );
        let report = scene.diagnostics().snapshot();
        assert_eq!((report.hits, report.misses), (2, 4));
        assert_eq!(report.skipped, 1);
        assert_eq!(report.requests.len(), 6);
        assert!(
            report
                .resources
                .iter()
                .any(|row| row.decision.contains("Vary headers unavailable"))
        );
        let all = app
            .prepare_captured_page_with_options(
                id,
                CapturedPageOptions {
                    scope: CapturedPageScope::AllLoaded,
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert!(
            all.resolve(
                "GET",
                "https://variants.invalid/extra.js",
                Some(&[]),
                Some(&empty),
                false
            )
            .is_some()
        );
        assert_eq!(
            all.diagnostics().snapshot().scope,
            CapturedPageScope::AllLoaded
        );
        for _ in 0..300 {
            all.resolve(
                "GET",
                "https://missing.invalid/",
                Some(&[]),
                Some(&empty),
                false,
            );
        }
        let report = all.diagnostics().snapshot();
        assert_eq!(report.misses, 300);
        assert_eq!(report.requests.len(), 256);
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
