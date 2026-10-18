//! Frozen captured responses for a render-only native browser window.
use crate::{
    AppError, Application, CapturedPageDiagnostics, CapturedPageOptions, CapturedPageScope,
    ErrorCategory, inspector::parse_session_id,
};
use std::fmt::Write as _;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use transmog_core::{ResponseHead, intercept::ExchangeId, observe::ExchangeBoundary};

const MAX_RESOURCES: usize = 2048;
const MAX_RESOURCE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCENE_BYTES: u64 = 256 * 1024 * 1024;

/// A captured resource already decoded and materialized outside the UI thread.
#[derive(Clone, Debug)]
pub struct CapturedResource {
    /// Exact decoded response byte identity.
    pub sha256: String,
    /// Frozen final HTTP status, when browser-compatible.
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
    /// Captured browser identity used for User-Agent-dependent resources.
    pub user_agent: Option<String>,
    /// Number of response variants available in this scene.
    pub resources: usize,
    /// Unavailable or bounded-out variants; misses still return network failures.
    pub skipped: usize,
    files: std::sync::Arc<HashMap<(String, String), Vec<ResourceVariant>>>,
    pub(super) overlay: crate::captured_page_overlay::Overlay,
    pub(super) diagnostics: CapturedPageDiagnostics,
    navigation_time: u64,
    positions: Mutex<HashMap<(String, String), (u64, u128)>>,
    root: std::sync::Arc<tempfile::TempDir>,
}
struct ResourceVariant {
    resource: CapturedResource,
    entry_id: String,
    body_sha256: Option<String>,
    vary: Option<Vec<(String, Option<String>)>>,
    primary: bool,
    user_agent: Option<String>,
    same_process: bool,
    origin: Option<String>,
    referer: Option<String>,
    sec_headers: Vec<(String, String)>,
    time: u64,
    order: u128,
}
impl ResourceVariant {
    fn relaxed_reason(
        &self,
        headers: Option<&[(String, String)]>,
        body_sha256: Option<&str>,
        fallback_ua: Option<String>,
        navigation_time: u64,
    ) -> String {
        let mut reason = String::new();
        let preferences =
            RequestPreferences::new(headers, body_sha256, fallback_ua, navigation_time, None);
        let mut relaxed = Vec::new();
        if body_sha256 != self.body_sha256.as_deref() {
            relaxed.push("body");
        }
        if preferences.user_agent != self.user_agent {
            relaxed.push("User-Agent");
        }
        if preferences.origin != self.origin {
            relaxed.push("Origin");
        }
        if preferences.referer != self.referer {
            relaxed.push("Referer");
        }
        if !self.varies_with(headers) {
            relaxed.push("Vary");
        }
        if self.sec_headers.iter().any(|(name, value)| {
            headers.and_then(|h| header_value(h, name)).as_ref() != Some(value)
        }) {
            relaxed.push("Sec- headers");
        }
        if !relaxed.is_empty() {
            write!(reason, "; relaxed {}", relaxed.join(", ")).expect("string write");
        }
        reason
    }
    fn position(&self) -> (u64, u128) {
        (self.time, self.order)
    }
    fn varies_with(&self, headers: Option<&[(String, String)]>) -> bool {
        self.vary.as_ref().is_some_and(|vary| {
            vary.iter().all(|(name, value)| {
                headers.and_then(|headers| header_value(headers, name)) == *value
            })
        })
    }
}
#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum MatchQuality {
    Matched,
    Unmatched,
}
impl MatchQuality {
    fn from_match(matched: bool) -> Self {
        if matched {
            Self::Matched
        } else {
            Self::Unmatched
        }
    }
}
// Earlier fields take preference; later hints are relaxed first when they conflict.
#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct PreferenceRank {
    body: MatchQuality,
    destination: MatchQuality,
    user_agent: MatchQuality,
    process: MatchQuality,
    context_matches: std::cmp::Reverse<usize>,
    context_mismatches: usize,
    sec_matches: std::cmp::Reverse<usize>,
    vary: MatchQuality,
    before_anchor: bool,
    time_distance: u64,
    capture_order_distance: u128,
}
struct RequestPreferences<'a> {
    headers: Option<&'a [(String, String)]>,
    body: Option<&'a str>,
    user_agent: Option<String>,
    origin: Option<String>,
    referer: Option<String>,
    destination: Option<String>,
    anchor: u64,
    order_anchor: Option<u128>,
}
impl<'a> RequestPreferences<'a> {
    fn new(
        headers: Option<&'a [(String, String)]>,
        body: Option<&'a str>,
        fallback_ua: Option<String>,
        anchor: u64,
        order_anchor: Option<u128>,
    ) -> Self {
        let field = |name| headers.and_then(|headers| header_value(headers, name));
        Self {
            headers,
            body,
            user_agent: field("user-agent").or(fallback_ua),
            origin: field("origin"),
            referer: field("referer"),
            destination: field("sec-fetch-dest"),
            anchor,
            order_anchor,
        }
    }
    fn rank(&self, variant: &ResourceVariant) -> PreferenceRank {
        let context_matches = [&self.origin, &self.referer]
            .into_iter()
            .zip([&variant.origin, &variant.referer])
            .filter(|(requested, recorded)| requested.is_some() && *requested == *recorded)
            .count();
        let sec_matches = variant
            .sec_headers
            .iter()
            .filter(|(name, value)| {
                self.headers
                    .and_then(|headers| header_value(headers, name))
                    .as_ref()
                    == Some(value)
            })
            .count();
        PreferenceRank {
            body: MatchQuality::from_match(
                self.body
                    .is_some_and(|body| variant.body_sha256.as_deref() == Some(body)),
            ),
            destination: MatchQuality::from_match(self.destination.as_ref().is_some_and(|dest| {
                variant
                    .sec_headers
                    .iter()
                    .any(|(name, value)| name == "sec-fetch-dest" && value == dest)
            })),
            user_agent: MatchQuality::from_match(
                self.user_agent
                    .as_ref()
                    .is_some_and(|ua| variant.user_agent.as_ref() == Some(ua)),
            ),
            process: MatchQuality::from_match(variant.same_process),
            context_matches: std::cmp::Reverse(context_matches),
            context_mismatches: usize::from(self.origin != variant.origin)
                + usize::from(self.referer != variant.referer),
            sec_matches: std::cmp::Reverse(sec_matches),
            vary: MatchQuality::from_match(variant.varies_with(self.headers)),
            before_anchor: variant.time < self.anchor,
            time_distance: variant.time.abs_diff(self.anchor),
            capture_order_distance: self
                .order_anchor
                .map_or(variant.order, |order| variant.order.abs_diff(order)),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct ProcessContext {
    trace_id: Option<String>,
    pid: u32,
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
    /// Selects frozen responses using client, navigation and body evidence as hints.
    /// Only method/URL and the selected source scope are required; no origin is contacted.
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
        // Reload resets captured sequence even when an overlay supplies the document.
        if document && method == "GET" && canonical_url(url).as_deref() == Some(&self.url) {
            self.positions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        let overlay_reason =
            match self
                .overlay
                .resolve(method, url, headers, document, &self.diagnostics)
            {
                crate::captured_page_overlay::Resolution::Served(resource) => {
                    return Some(resource);
                }
                crate::captured_page_overlay::Resolution::Fallback(reason) => reason,
            };
        let key = canonical_url(url).map(|url| (method.to_owned(), url));
        let variants = key.as_ref().and_then(|key| self.files.get(key));
        let mut positions = self
            .positions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_position = key.as_ref().and_then(|key| positions.get(key)).copied();
        let selection_reason = "Captured response selected by sequence, body, client, Origin/Referer, Sec- headers and time preferences";
        let selected = variants.and_then(|variants| {
            if document
                && method == "GET"
                && canonical_url(url).as_deref() == Some(&self.url)
                && let Some(primary) = variants.iter().find(|variant| variant.primary)
            {
                positions.clear();
                return Some((primary, "Selected captured HTML document"));
            }
            let last = key.as_ref().and_then(|key| positions.get(key)).copied();
            let advance =
                last.is_some_and(|last| variants.iter().any(|variant| variant.position() > last));
            let preferences = RequestPreferences::new(
                headers,
                body_sha256,
                self.user_agent.clone(),
                last.map_or(self.navigation_time, |last| last.0),
                last.map(|last| last.1),
            );
            let variant = variants
                .iter()
                .filter(|variant| !advance || last.is_some_and(|last| variant.position() > last))
                .min_by_key(|variant| preferences.rank(variant))?;
            if let Some(key) = &key {
                positions.insert(key.clone(), variant.position());
            }
            Some((variant, selection_reason))
        });
        let reason = selected.map_or_else(
            || {
                if variants.is_none() {
                    "No captured response for this method and URL"
                } else {
                    "No available captured response for this method and URL"
                }
            },
            |(_, reason)| reason,
        );
        let mut reason = reason.to_owned();
        if let Some((variant, _)) = selected {
            reason.push_str(&variant.relaxed_reason(
                headers,
                body_sha256,
                self.user_agent.clone(),
                self.navigation_time,
            ));
            if previous_position == Some(variant.position()) && !variant.primary {
                reason.push_str("; reused last captured variant");
            }
        }
        reason.push_str(&overlay_reason);
        if let Some((variant, _)) = selected {
            self.diagnostics.supplied(
                method,
                url,
                Some(variant.entry_id.clone()),
                None,
                &variant.resource,
                &reason,
            );
        } else {
            self.diagnostics.request(method, url, None, &reason);
        }
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
    user_agent: Option<String>,
    process: Option<ProcessContext>,
    origin: Option<String>,
    referer: Option<String>,
    sec_headers: Vec<(String, String)>,
}
pub(crate) async fn prepare(
    application: Application,
    id: String,
    options: CapturedPageOptions,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<CapturedPage, AppError> {
    if options.case_preview.is_some() {
        return Err(error("Open a case preview through its owning package"));
    }
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(build(&application, &id, &options, &canceled, None))
    })
    .await
    .map_err(|_| error("Captured page preparation failed"))?
}
pub(crate) async fn build(
    application: &Application,
    id: &str,
    options: &CapturedPageOptions,
    canceled: &std::sync::atomic::AtomicBool,
    allowed: Option<&std::collections::HashSet<String>>,
) -> Result<CapturedPage, AppError> {
    if let Some(reference) = &options.page_reference
        && (reference.entry_id != id || !application.experiment_page_available(reference))
    {
        return Err(error(
            "The chosen captured page is unavailable. Choose a replacement in Response sets.",
        ));
    }
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
    if let Some(allowed) = allowed {
        candidates.retain(|candidate| allowed.contains(&format!("{:032x}", candidate.id.0)));
    }
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
    let mut scene = preparation::SceneBuilder::new(numeric, root, anchor);
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
    let page = scene.finish(options.scope, source_name, source)?;
    crate::captured_page_overlay::attach(application, page, options, canceled).await
}
#[path = "captured_page_build.rs"]
mod preparation;

/// Opens a new run from retained immutable capture bytes with independently frozen rules.
pub(crate) fn fork(page: &CapturedPage) -> CapturedPage {
    let mut report = page.diagnostics.snapshot();
    report.bytes -= report.rules.iter().map(|r| r.bytes).sum::<u64>();
    report.rules.clear();
    report.variant = None;
    report.case_origin = None;
    report.hits = 0;
    report.misses = 0;
    report.failures = 0;
    report.requests.clear();
    report.scripts_enabled = None;
    CapturedPage {
        url: page.url.clone(),
        user_agent: page.user_agent.clone(),
        resources: page.resources,
        skipped: page.skipped,
        files: page.files.clone(),
        overlay: crate::captured_page_overlay::Overlay::default(),
        diagnostics: CapturedPageDiagnostics::new(report),
        navigation_time: page.navigation_time,
        positions: Mutex::new(HashMap::new()),
        root: page.root.clone(),
    }
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

fn recorded_header(headers: &transmog_core::HeaderBlock, name: &str) -> Option<String> {
    headers
        .values(name)
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .map(|value| value.trim_matches([' ', '\t']))
        .filter(|value| !value.is_empty() && value.len() <= 8192)
        .map(str::to_owned)
}
fn recorded_sec_headers(headers: &transmog_core::HeaderBlock) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|field| !field.is_redacted())
        .filter_map(|field| std::str::from_utf8(field.name()).ok())
        .map(str::to_ascii_lowercase)
        .filter(|name| name.starts_with("sec-"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(32)
        .filter_map(|name| recorded_header(headers, &name).map(|value| (name, value)))
        .collect()
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
            let trace_id = application
                .traces
                .source_id(&format!("{:032x}", snapshot.exchange_id.0));
            Some(Candidate {
                process: match snapshot.metadata.client_identity {
                    transmog_core::ClientIdentity::LocalProcess { pid, .. } if pid != 0 => {
                        Some(ProcessContext {
                            trace_id: trace_id.clone(),
                            pid,
                        })
                    }
                    _ => None,
                },
                origin: recorded_header(&request.head.headers, "origin"),
                referer: recorded_header(&request.head.headers, "referer"),
                sec_headers: recorded_sec_headers(&request.head.headers),
                user_agent: recorded_header(&request.head.headers, "user-agent"),
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
                time: snapshot
                    .performance
                    .points
                    .iter()
                    .find(|point| {
                        point.milestone
                            == transmog_core::performance::Milestone::ClientResponseBegin
                    })
                    .map(|point| point.unix_millis)
                    .or_else(|| snapshot.terminal_at.map(millis))
                    .unwrap_or_else(|| millis(snapshot.metadata.started_at)),
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
pub(super) fn browser_headers(head: &ResponseHead, length: u64) -> String {
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
    if !matches!(head.status, 204 | 304) {
        write!(out, "Content-Length: {length}\r\n").expect("string write");
    }
    out.push_str("Content-Security-Policy: worker-src 'none'; frame-src http: https: data: blob:; connect-src http: https:; form-action http: https:; object-src 'none'; base-uri http: https:; sandbox allow-scripts allow-forms allow-same-origin\r\n");
    out
}
pub(crate) fn millis(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
pub(super) fn error(message: &str) -> AppError {
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

    #[tokio::test]
    async fn remembered_page_requires_its_exact_capture_after_expiry_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("page.saz");
        fixture(&capture, "chosen");
        let config = || AppConfig {
            automation_path: Some(root.path().join("automation")),
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        };
        let app = Application::new(config()).unwrap();
        app.change_experiment(
            0,
            crate::ExperimentChange::Create {
                id: "set".into(),
                name: "Saved POST".into(),
                variant_id: "edited".into(),
                variant_name: "Edited response".into(),
                rules: Vec::new(),
            },
        )
        .unwrap();
        let import = |operation: &str| TraceImportRequest {
            password: None,
            path: capture.clone(),
            operation_id: operation.into(),
            max_file_bytes: 1024 * 1024,
        };
        app.import_trace(import("one"), Arc::new(|_| {}))
            .await
            .unwrap();
        let choices = app.experiment_preview_pages("set", None, true).unwrap();
        assert!(choices.page.is_none());
        assert_eq!(choices.pages.len(), 1);
        let chosen = choices.pages[0].clone();
        let before = app.automation_status();
        app.set_experiment_preview_page("set", 0, chosen.clone())
            .unwrap();
        assert_eq!(app.automation_status().generation, before.generation);
        app.remove_traffic_entries(std::slice::from_ref(&chosen.entry_id), false)
            .unwrap();
        assert!(!app.experiment_page_available(&chosen));
        app.import_trace(import("two"), Arc::new(|_| {}))
            .await
            .unwrap();
        let again = app.experiment_preview_pages("set", None, true).unwrap();
        assert_eq!(again.page, Some(chosen.clone()));
        assert!(!again.available);
        assert_eq!(again.pages[0].url, chosen.url);
        assert_ne!(again.pages[0].entry_id, chosen.entry_id);
        drop(app);
        let restarted = Application::new(config()).unwrap();
        assert!(!restarted.experiment_page_capture_current(&chosen));
        let remembered = restarted
            .experiment_preview_pages("set", None, false)
            .unwrap();
        assert_eq!(remembered.page, Some(chosen.clone()));
        assert!(!remembered.available);
        assert!(
            restarted
                .set_experiment_preview_page("set", 1, chosen.clone())
                .is_err()
        );
        let result = restarted
            .prepare_captured_page_with_options(
                chosen.entry_id.clone(),
                CapturedPageOptions {
                    page_reference: Some(chosen),
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await;
        assert!(result.is_err());
    }

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
    async fn experiment_document_choices_keep_source_identity_and_do_not_move_selection() {
        let root = tempfile::tempdir().unwrap();
        let app = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        })
        .unwrap();
        for name in ["original", "other"] {
            let path = root.path().join(format!("{name}.saz"));
            fixture(&path, name);
            app.import_trace(
                TraceImportRequest {
                    path,
                    password: None,
                    operation_id: name.into(),
                    max_file_bytes: 1024 * 1024,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        }
        let rows = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        let resource = rows.iter().find(|row| row.path == "/style.css").unwrap();
        let choices = app.captured_page_choices(&resource.id).unwrap();
        assert_eq!(choices.pages.len(), 1);
        assert!(!choices.more);
        let page = rows
            .iter()
            .find(|row| row.id == choices.pages[0].id)
            .unwrap();
        assert_eq!(page.trace_id, resource.trace_id);
        assert_eq!(page.path, "/page");
        assert_eq!(
            app.query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions
                .len(),
            rows.len()
        );
        app.remove_traffic_entries(std::slice::from_ref(&resource.id), false)
            .unwrap();
        assert!(app.captured_page_choices(&resource.id).is_err());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One scene qualifies variant matching, source isolation and bounded diagnostics together.
    async fn preview_hints_fall_back_without_crossing_source_scope() {
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
                    password: None,
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
                password: None,
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
        let response = scene
            .resolve(
                "POST",
                "https://variants.invalid/api",
                Some(&headers),
                Some(&alpha),
                false,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(response.body_path).unwrap(),
            "alpha response"
        );
        assert!(
            scene
                .resolve("POST", "https://variants.invalid/api", None, None, false)
                .is_some()
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
                .is_some()
        );
        assert!(
            scene
                .resolve("GET", "https://variants.invalid/page", None, None, true)
                .is_some()
        );
        let report = scene.diagnostics().snapshot();
        assert_eq!((report.hits, report.misses), (5, 1));
        assert_eq!(report.skipped, 0);
        assert_eq!(report.requests.len(), 6);
        let all = app
            .prepare_captured_page_with_options(
                id,
                CapturedPageOptions {
                    scope: CapturedPageScope::AllLoaded,
                    ..Default::default()
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
                        password: None,
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
        assert!(page.headers.contains("Cache-Control: max-age=3600\r\n"));
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

    #[test]
    fn browser_cache_policy_is_preserved_without_synthesizing_a_bypass() {
        use transmog_core::{HeaderBlock, HeaderField};
        let head = ResponseHead {
            status: 200,
            source_version: transmog_core::HttpLegVersion::Http1,
            headers: HeaderBlock::from_fields(
                [
                    ("Cache-Control", "private, max-age=60"),
                    ("cache-control", "must-revalidate"),
                    ("Expires", "Thu, 15 Oct 2026 23:00:00 GMT"),
                    ("Age", "7"),
                    ("Content-Security-Policy", "default-src 'self'"),
                    ("Content-Encoding", "gzip"),
                    ("Content-Length", "123"),
                ]
                .into_iter()
                .map(|(name, value)| HeaderField::try_new(name, value).unwrap())
                .collect(),
            ),
        };
        let headers = browser_headers(&head, 10);
        assert!(headers.contains("Cache-Control: private, max-age=60\r\ncache-control: must-revalidate\r\nExpires: Thu, 15 Oct 2026 23:00:00 GMT\r\nAge: 7\r\n"));
        assert!(headers.contains("Content-Security-Policy: default-src 'self'\r\n"));
        assert!(headers.contains("Content-Security-Policy: worker-src 'none'"));
        assert!(headers.contains("Content-Length: 10\r\n"));
        assert!(!headers.contains("Content-Encoding"));
        assert!(!headers.contains("no-store"));
        let absent = browser_headers(
            &ResponseHead {
                status: 200,
                headers: HeaderBlock::new(),
                source_version: transmog_core::HttpLegVersion::Http1,
            },
            0,
        );
        assert!(!absent.to_ascii_lowercase().contains("cache-control"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Full frozen case export/recipient/preview lifetime.
    async fn case_preview_uses_package_assets_and_only_case_evidence_without_activation() {
        use crate::{
            AuthoredResponseAsset, CaseExportRequest, ExperimentChange, ExperimentSelection,
            RuleExportRequest,
        };
        use transmog_automation::{RequestActions, Rule, RuleMatcher, UrlCondition};
        use transmog_core::HeaderBlock;
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        let app = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
            response_asset_root: Some(root.path().join("assets")),
            ..Default::default()
        })
        .unwrap();
        let path = root.path().join("page.saz");
        fixture(&path, "Portable baseline");
        app.import_trace(
            TraceImportRequest {
                path,
                password: None,
                operation_id: "case-site".into(),
                max_file_bytes: 1024 * 1024,
            },
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        let asset = app
            .create_response_asset(AuthoredResponseAsset {
                id: "case-css".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::new(),
                body: b"h1 { color: blue; }".to_vec(),
                media_type: Some("text/css".into()),
            })
            .unwrap();
        let rule = Rule {
            id: "case-rule".into(),
            display_name: Some("Frozen CSS".into()),
            enabled: true,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher {
                url: Some(UrlCondition::Exact(
                    "https://example.invalid/style.css".into(),
                )),
                ..Default::default()
            },
            request: RequestActions {
                response_asset: Some(asset.asset_ref()),
                ..Default::default()
            },
            response: transmog_automation::ResponseActions::default(),
        };
        app.change_experiment(
            app.automation_status().generation,
            ExperimentChange::Create {
                id: "portable-exp".into(),
                name: "Frozen preview".into(),
                variant_id: "blue".into(),
                variant_name: "Blue CSS".into(),
                rules: vec![rule],
            },
        )
        .unwrap();
        let prepared = app
            .prepare_investigation(
                &CaseExportRequest {
                    rules: RuleExportRequest {
                        id: "case-preview".into(),
                        revision: "1".into(),
                        title: "Portable page".into(),
                        rule_ids: vec![],
                        selections: vec![ExperimentSelection {
                            experiment_id: "portable-exp".into(),
                            variant_id: "blue".into(),
                            revision: 1,
                        }],
                    },
                    evidence_review_id: Some(app.review_traffic_export(None).unwrap().id),
                    derived_plaintext: true,
                    notes: vec![],
                    comparison_ids: vec![],
                },
                stage.path(),
                &AtomicBool::new(false),
            )
            .unwrap();
        let package_path = stage.path().join("portable.tmcase.tmcap");
        let password = transmog_capture::CapturePassword::new("fixture password".into());
        transmog_portable::write_case_package(
            &prepared,
            &package_path,
            Some(password.clone()),
            false,
            &AtomicBool::new(false),
        )
        .unwrap();
        let package = Arc::new(
            transmog_portable::Package::open(
                &package_path,
                Some(&password),
                &AtomicBool::new(false),
            )
            .unwrap(),
        );
        let wire = prepared.manifest();
        let experiment: transmog_portable::Experiment =
            serde_json::from_value(wire.experiments[0].clone()).unwrap();
        let selection = transmog_portable::FrozenExperiment {
            experiment_id: experiment.id.clone(),
            variant_id: experiment
                .variants
                .iter()
                .find(|v| v.revisions.iter().any(|r| !r.rules.is_empty()))
                .unwrap()
                .id
                .clone(),
            revision: experiment.variants[0].revisions[0].revision.clone(),
        };
        let recipient_root = tempfile::tempdir().unwrap();
        let recipient = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(
                recipient_root.path().join("bodies"),
            )),
            response_asset_root: Some(recipient_root.path().join("assets")),
            ..Default::default()
        })
        .unwrap();
        let outside = recipient_root.path().join("outside.saz");
        variant_fixture(&outside, true);
        recipient
            .import_trace(
                TraceImportRequest {
                    path: outside,
                    password: None,
                    operation_id: "outside".into(),
                    max_file_bytes: 1024 * 1024,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        let opened = recipient
            .open_package_evidence(
                &package,
                &wire.sources[0].id,
                None,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        let id = recipient
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions
            .into_iter()
            .find(|r| r.url == "https://example.invalid/page")
            .unwrap()
            .id;
        let before = recipient.automation_status();
        let assets = recipient.response_assets();
        let page = recipient
            .prepare_case_preview(
                package.clone(),
                opened.links.clone(),
                id.clone(),
                crate::CasePreviewOptions {
                    selection: Some(selection),
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert!(
            page.diagnostics()
                .snapshot()
                .case_origin
                .unwrap()
                .contains("case-preview@1")
        );
        let css = page
            .resolve(
                "GET",
                "https://example.invalid/style.css",
                Some(&[]),
                None,
                false,
            )
            .unwrap();
        assert_eq!(
            std::fs::read(&css.body_path).unwrap(),
            b"h1 { color: blue; }"
        );
        assert!(
            page.resolve(
                "GET",
                "https://variants.invalid/extra.js",
                Some(&[]),
                None,
                false
            )
            .is_none()
        );
        let baseline = recipient
            .prepare_case_preview(
                package.clone(),
                opened.links.clone(),
                id.clone(),
                crate::CasePreviewOptions::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(
                baseline
                    .resolve(
                        "GET",
                        "https://example.invalid/style.css",
                        Some(&[]),
                        None,
                        false
                    )
                    .unwrap()
                    .body_path
            )
            .unwrap()
            .contains("green")
        );
        assert!(
            recipient
                .prepare_case_preview(
                    package.clone(),
                    opened.links.clone(),
                    "0".repeat(32),
                    crate::CasePreviewOptions::default(),
                    Arc::new(AtomicBool::new(false))
                )
                .await
                .is_err()
        );
        assert!(
            recipient
                .prepare_case_preview(
                    package.clone(),
                    opened.links,
                    id,
                    crate::CasePreviewOptions::default(),
                    Arc::new(AtomicBool::new(true))
                )
                .await
                .is_err()
        );
        assert_eq!(recipient.automation_status(), before);
        assert_eq!(recipient.response_assets(), assets);
        drop(package);
        drop(prepared);
        drop(app);
        std::fs::remove_file(package_path).unwrap();
        assert_eq!(
            std::fs::read(css.body_path).unwrap(),
            b"h1 { color: blue; }"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Complete baseline/modified/updated user flow.
    async fn overlay_freezes_disabled_rules_assets_order_and_related_scenes() {
        use crate::{AuthoredResponseAsset, AutomationRuleSet};
        use transmog_automation::{RequestActions, Rule, RuleMatcher, UrlCondition};
        use transmog_core::{HeaderBlock, HeaderField};
        let root = tempfile::tempdir().unwrap();
        let app = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
            response_asset_root: Some(root.path().join("assets")),
            ..Default::default()
        })
        .unwrap();
        let path = root.path().join("page.saz");
        fixture(&path, "Baseline");
        app.import_trace(
            TraceImportRequest {
                path,
                password: None,
                operation_id: "overlay".into(),
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
            .find(|r| r.path == "/page")
            .unwrap()
            .id;
        let baseline = Arc::new(
            app.prepare_captured_page(id, Arc::new(AtomicBool::new(false)))
                .await
                .unwrap(),
        );
        let captured = baseline
            .resolve("GET", &baseline.url, Some(&[]), None, true)
            .unwrap();
        assert!(captured.headers.contains("Cache-Control: max-age=3600\r\n"));
        assert!(!captured.headers.contains("no-store"));
        let asset = app
            .create_response_asset(AuthoredResponseAsset {
                id: "preview".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("cache-control", "max-age=99").unwrap(),
                    HeaderField::try_new("content-security-policy", "default-src 'none'").unwrap(),
                    HeaderField::try_new("x-duplicate", "one").unwrap(),
                    HeaderField::try_new("x-duplicate", "two").unwrap(),
                ]),
                body: b"Modified".to_vec(),
                media_type: Some("text/html".into()),
            })
            .unwrap();
        let rule = Rule {
            id: "first".into(),
            display_name: Some("Disabled experiment".into()),
            enabled: false,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher {
                method: Some("GET".into()),
                url: Some(UrlCondition::Exact(baseline.url.clone())),
                ..Default::default()
            },
            request: RequestActions {
                response_asset: Some(asset.asset_ref()),
                ..Default::default()
            },
            response: transmog_automation::ResponseActions::default(),
        };
        let mut second = rule.clone();
        second.id = "second".into();
        second.priority = 1;
        let candidate = app
            .validate_automation(AutomationRuleSet {
                rules: vec![second, rule.clone()],
                autoresponses_enabled: false,
                ..Default::default()
            })
            .unwrap();
        let status = app.activate_automation(&candidate.candidate_id).unwrap();
        let options = CapturedPageOptions {
            rule_ids: Some(vec!["second".into(), "first".into()]),
            generation: Some(status.generation),
            ..Default::default()
        };
        let modified = app
            .prepare_related_captured_page(
                baseline.clone(),
                options.clone(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        for _ in 0..2 {
            let r = modified
                .resolve("GET", &baseline.url, Some(&[]), None, true)
                .unwrap();
            assert_eq!(std::fs::read_to_string(r.body_path).unwrap(), "Modified");
            assert!(r.headers.contains("max-age=99"));
            assert!(r.headers.contains("default-src 'none'"));
            assert!(r.headers.contains("worker-src 'none'"));
            assert!(r.headers.contains("x-duplicate: one\r\nx-duplicate: two"));
        }
        let report = modified.diagnostics().snapshot();
        assert_eq!((report.hits, report.misses), (2, 0));
        assert_eq!(report.requests[0].outcome, "auto-response");
        assert_eq!(report.requests[0].rule_id.as_deref(), Some("first"));
        assert_eq!(report.rules[1].shadowed_by.as_deref(), Some("first"));
        assert!(!app.automation_status().autoresponses_enabled);
        assert!(app.automation_status().rules.iter().all(|r| !r.enabled));
        let again = app
            .prepare_related_captured_page(
                Arc::new(modified),
                CapturedPageOptions::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(
                again
                    .resolve("GET", &baseline.url, None, None, true)
                    .unwrap()
                    .body_path
            )
            .unwrap()
            .contains("Baseline")
        );
        let mut missing = rule.clone();
        missing.id = "missing".into();
        missing.matcher.method = None;
        missing.matcher.url = Some(UrlCondition::Exact("https://absent.invalid/".into()));
        let candidate = app
            .validate_automation(AutomationRuleSet {
                generation: status.generation,
                rules: vec![missing],
                ..Default::default()
            })
            .unwrap();
        app.activate_automation(&candidate.candidate_id).unwrap();
        assert!(
            app.prepare_related_captured_page(
                baseline.clone(),
                options,
                Arc::new(AtomicBool::new(false))
            )
            .await
            .err()
            .unwrap()
            .message
            .contains("changed")
        );
        let missing = app
            .prepare_related_captured_page(
                baseline.clone(),
                CapturedPageOptions {
                    rule_ids: Some(vec!["missing".into()]),
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert!(
            missing
                .resolve("GET", "https://absent.invalid/", Some(&[]), None, false)
                .is_some()
        );
        let head = missing
            .resolve("HEAD", "https://absent.invalid/", Some(&[]), None, false)
            .unwrap();
        assert!(std::fs::read(head.body_path).unwrap().is_empty());
        assert!(
            missing
                .resolve("GET", "https://unknown.invalid/", None, None, false)
                .is_none()
        );
        for status in [204, 205, 304, 404] {
            let asset = app
                .create_response_asset(AuthoredResponseAsset {
                    id: format!("status-{status}"),
                    revision: 1,
                    status,
                    headers: HeaderBlock::new(),
                    body: b"authored body".to_vec(),
                    media_type: Some("text/plain".into()),
                })
                .unwrap();
            let mut rule = rule.clone();
            rule.request.response_asset = Some(asset.asset_ref());
            let current = app.automation_status();
            let candidate = app
                .validate_automation(AutomationRuleSet {
                    generation: current.generation,
                    rules: vec![rule],
                    ..Default::default()
                })
                .unwrap();
            app.activate_automation(&candidate.candidate_id).unwrap();
            let prepared = app
                .prepare_related_captured_page(
                    baseline.clone(),
                    CapturedPageOptions {
                        rule_ids: Some(vec!["first".into()]),
                        ..Default::default()
                    },
                    Arc::new(AtomicBool::new(false)),
                )
                .await
                .unwrap();
            let resource = prepared
                .resolve("GET", &baseline.url, Some(&[]), None, false)
                .unwrap();
            assert_eq!(resource.status, status);
            assert_eq!(
                std::fs::read(&resource.body_path).unwrap().is_empty(),
                status != 404
            );
            if matches!(status, 204 | 304) {
                assert!(!resource.headers.contains("Content-Length:"));
            }
            prepared
                .resolve("GET", &baseline.url, Some(&[]), None, true)
                .unwrap();
            let report = prepared.diagnostics().snapshot();
            assert_eq!(
                (report.hits, report.failures),
                if status == 404 { (2, 0) } else { (1, 1) }
            );
            assert_eq!(report.requests[1].id, 2);
        }
        assert!(
            app.prepare_related_captured_page(
                baseline.clone(),
                CapturedPageOptions {
                    rule_ids: Some(vec!["first".into()]),
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(true))
            )
            .await
            .err()
            .unwrap()
            .message
            .contains("canceled")
        );
        let linked_path = root.path().join("unavailable.css");
        std::fs::File::create(&linked_path)
            .unwrap()
            .set_len(25_000_001)
            .unwrap();
        let linked = app
            .import_response_asset(crate::ImportResponseAsset {
                id: "unavailable".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::new(),
                body_path: linked_path.clone(),
                body_file_mode: crate::ResponseBodyFileMode::Linked,
                media_type: Some("text/css".into()),
            })
            .unwrap();
        std::fs::remove_file(linked_path).unwrap();
        let mut unavailable = rule;
        unavailable.request.response_asset = Some(linked.asset_ref());
        let candidate = app
            .validate_automation(AutomationRuleSet {
                generation: app.automation_status().generation,
                rules: vec![unavailable],
                ..Default::default()
            })
            .unwrap();
        app.activate_automation(&candidate.candidate_id).unwrap();
        let failure = app
            .prepare_related_captured_page(
                baseline,
                CapturedPageOptions {
                    rule_ids: Some(vec!["first".into()]),
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .err()
            .unwrap();
        assert!(
            failure.message.contains("Disabled experiment")
                && failure.message.contains("unavailable")
        );
    }
}

#[cfg(test)]
#[path = "captured_page_matching_tests.rs"]
mod matching_tests;
