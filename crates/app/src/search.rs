//! Explicit bounded searches over immutable snapshots of retained evidence.
use crate::{AppError, BodyAvailability, BodyStore, ErrorCategory, inspector};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};
use transmog_session::ApplicationSessionService;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_PATTERN: usize = 4096;
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESULTS: usize = 200_000;

/// String or bounded linear-time regular expression matching.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficSearchMode {
    /// Literal substring matching.
    Text,
    /// Rust regular expressions with a finite compilation budget.
    Regex,
}
/// One explicit content search; binary bodies are quietly inapplicable.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Independent search scopes and matching options.
pub struct TrafficSearchRequest {
    /// Caller operation identity for cancellation and stale responses.
    pub operation_id: String,
    /// Literal text or regular expression.
    pub pattern: String,
    /// Matching engine.
    pub mode: TrafficSearchMode,
    /// Preserve character case when comparing.
    pub case_sensitive: bool,
    /// Remove Unicode combining accents for literal string searches.
    pub ignore_diacritics: bool,
    /// Include URL, method, process, PID and status.
    pub metadata: bool,
    /// Include full retained request and response headers.
    pub headers: bool,
    /// Include decoded text client-response bodies.
    pub response_bodies: bool,
}
/// Progress without changing existing results or selections.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficSearchProgress {
    /// Caller operation identity.
    pub operation_id: String,
    /// Entries examined.
    pub completed: usize,
    /// Snapshot entry count.
    pub total: usize,
}
/// Complete snapshot result used for both paging and selecting all matches.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficSearchResult {
    /// Opaque bounded query handle; IDs need not be resent on every page.
    pub id: String,
    /// Caller operation identity.
    pub operation_id: String,
    /// Every matching entry, including off-page matches, in admission order.
    pub ids: Vec<String>,
    /// Number of entries in the searched snapshot.
    pub examined: usize,
    /// Inapplicable binary bodies; no user prompt is needed for these.
    pub binary_bodies: usize,
    /// Text bodies unavailable or beyond the finite decoding/read limit.
    pub unavailable_bodies: usize,
}

#[derive(Default)]
struct SearchState {
    operations: HashMap<String, Arc<AtomicBool>>,
    results: VecDeque<(String, Arc<HashSet<String>>)>,
}
#[derive(Clone, Default)]
pub(crate) struct SearchRegistry {
    state: Arc<Mutex<SearchState>>,
}
impl SearchRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, SearchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn cancel(&self, id: &str) {
        if let Some(operation) = self.lock().operations.get(id) {
            operation.store(true, Ordering::Release);
        }
    }
    pub(crate) fn resolve(
        &self,
        id: Option<&str>,
    ) -> Result<Option<Arc<HashSet<String>>>, AppError> {
        id.map(|id| {
            self.lock()
                .results
                .iter()
                .find(|(token, _)| token == id)
                .map(|(_, ids)| ids.clone())
                .ok_or_else(|| unavailable("Search results expired. Run the search again."))
        })
        .transpose()
    }
    pub(crate) async fn search(
        &self,
        request: TrafficSearchRequest,
        service: ApplicationSessionService,
        store: Option<BodyStore>,
        progress: Arc<dyn Fn(TrafficSearchProgress) + Send + Sync>,
    ) -> Result<TrafficSearchResult, AppError> {
        let matcher = Matcher::new(&request)?;
        if request.operation_id.is_empty()
            || request.operation_id.len() > 128
            || !(request.metadata || request.headers || request.response_bodies)
        {
            return Err(invalid("Choose search fields and a valid operation"));
        }
        let canceled = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.lock();
            if state.operations.len() >= 4
                || state.operations.contains_key(&request.operation_id)
                || state
                    .operations
                    .values()
                    .any(|operation| !operation.load(Ordering::Acquire))
            {
                return Err(unavailable(
                    "Cancel the current search before starting another",
                ));
            }
            state
                .operations
                .insert(request.operation_id.clone(), canceled.clone());
        }
        let registry = self.clone();
        let operation = request.operation_id.clone();
        let runtime = tokio::runtime::Handle::current();
        let result = tokio::task::spawn_blocking(move || {
            runtime.block_on(scan(
                &request,
                &matcher,
                &service,
                store.as_ref(),
                &canceled,
                progress.as_ref(),
            ))
        })
        .await;
        registry.lock().operations.remove(&operation);
        let mut result = result.map_err(|_| unavailable("Traffic search worker failed"))??;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| unavailable("Search result identity could not be created"))?;
        result.id = format!("search-{:032x}", u128::from_le_bytes(random));
        let mut state = registry.lock();
        if state.results.len() == 8 {
            state.results.pop_front();
        }
        state.results.push_back((
            result.id.clone(),
            Arc::new(result.ids.iter().cloned().collect()),
        ));
        Ok(result)
    }
}

enum Matcher {
    Text {
        pattern: String,
        case_sensitive: bool,
        ignore_diacritics: bool,
    },
    Regex(Regex),
}
impl Matcher {
    fn new(request: &TrafficSearchRequest) -> Result<Self, AppError> {
        if request.pattern.is_empty() || request.pattern.len() > MAX_PATTERN {
            return Err(invalid("Enter up to 4096 bytes of search text"));
        }
        match request.mode {
            TrafficSearchMode::Text => {
                let pattern = normalize(
                    &request.pattern,
                    request.case_sensitive,
                    request.ignore_diacritics,
                );
                if pattern.is_empty() {
                    return Err(invalid(
                        "Enter at least one character after ignoring accents",
                    ));
                }
                Ok(Self::Text {
                    pattern,
                    case_sensitive: request.case_sensitive,
                    ignore_diacritics: request.ignore_diacritics,
                })
            }
            TrafficSearchMode::Regex => {
                if request.ignore_diacritics {
                    return Err(invalid("Ignore accents is available for text matching"));
                }
                RegexBuilder::new(&request.pattern)
                    .case_insensitive(!request.case_sensitive)
                    .size_limit(8 * 1024 * 1024)
                    .dfa_size_limit(16 * 1024 * 1024)
                    .build()
                    .map(Self::Regex)
                    .map_err(|error| invalid(&format!("Invalid regular expression: {error}")))
            }
        }
    }
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(value),
            Self::Text {
                pattern,
                case_sensitive,
                ignore_diacritics,
            } => normalize(value, *case_sensitive, *ignore_diacritics).contains(pattern),
        }
    }
}
fn normalize(value: &str, case_sensitive: bool, ignore_diacritics: bool) -> String {
    let value = if ignore_diacritics {
        value
            .nfd()
            .filter(|character| !is_combining_mark(*character))
            .collect()
    } else {
        value.to_owned()
    };
    if case_sensitive {
        value
    } else {
        value.to_lowercase()
    }
}

async fn scan(
    request: &TrafficSearchRequest,
    matcher: &Matcher,
    service: &ApplicationSessionService,
    store: Option<&BodyStore>,
    canceled: &AtomicBool,
    progress: &(dyn Fn(TrafficSearchProgress) + Send + Sync),
) -> Result<TrafficSearchResult, AppError> {
    if let Some(store) = store {
        store
            .flush()
            .map_err(|_| unavailable("Recent body data could not be synchronized"))?;
    }
    let entries = service
        .catalog()
        .project_retained(|snapshot| snapshot.exchange_id);
    if entries.len() > MAX_RESULTS {
        return Err(invalid("The search snapshot exceeds the entry limit"));
    }
    let mut result = TrafficSearchResult {
        id: String::new(),
        operation_id: request.operation_id.clone(),
        ids: Vec::new(),
        examined: entries.len(),
        binary_bodies: 0,
        unavailable_bodies: 0,
    };
    for (position, id) in entries.iter().enumerate() {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Traffic search canceled"));
        }
        let Some(snapshot) = service.catalog().get(*id) else {
            continue;
        };
        let target = snapshot.metadata.original_target.as_target();
        let mut found = request.metadata
            && matcher.matches(&format!(
                "{}://{}{}{}\n{:?}\n{}\n{}",
                target.scheme,
                target.authority,
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}")),
                snapshot.metadata.client_identity,
                snapshot
                    .request_heads
                    .last()
                    .map_or("", |head| head.head.method.as_str()),
                snapshot
                    .response_heads
                    .last()
                    .map_or_else(String::new, |head| head.head.status.to_string())
            ));
        if !found && request.headers {
            found = snapshot
                .request_heads
                .iter()
                .map(|head| &head.head.headers)
                .chain(
                    snapshot
                        .response_heads
                        .iter()
                        .map(|head| &head.head.headers),
                )
                .any(|headers| {
                    headers.iter().any(|field| {
                        matcher.matches(&format!(
                            "{}: {}",
                            String::from_utf8_lossy(field.name()),
                            if field.is_redacted() {
                                String::new()
                            } else {
                                String::from_utf8_lossy(field.value()).into_owned()
                            }
                        ))
                    })
                });
        }
        if !found && request.response_bodies {
            match search_body(*id, store, canceled).await {
                BodyText::Text(text) => found = matcher.matches(&text),
                BodyText::Binary => result.binary_bodies += 1,
                BodyText::Unavailable => result.unavailable_bodies += 1,
            }
        }
        if found {
            result.ids.push(format!("{:032x}", id.0));
        }
        if position % 64 == 0 || position + 1 == entries.len() {
            progress(TrafficSearchProgress {
                operation_id: request.operation_id.clone(),
                completed: position + 1,
                total: entries.len(),
            });
        }
    }
    if canceled.load(Ordering::Acquire) {
        return Err(unavailable("Traffic search canceled"));
    }
    Ok(result)
}
enum BodyText {
    Text(String),
    Binary,
    Unavailable,
}
async fn search_body(id: ExchangeId, store: Option<&BodyStore>, canceled: &AtomicBool) -> BodyText {
    let Some(store) = store else {
        return BodyText::Unavailable;
    };
    let bodies = store.metadata(id);
    let Some(metadata) = bodies
        .iter()
        .find(|body| body.boundary == "client-response")
        .or_else(|| {
            bodies
                .iter()
                .find(|body| body.boundary == "upstream-response")
        })
    else {
        return BodyText::Unavailable;
    };
    let boundary = if metadata.boundary == "client-response" {
        ExchangeBoundary::ClientResponse
    } else {
        ExchangeBoundary::UpstreamResponse
    };
    if metadata
        .media_type
        .as_deref()
        .is_some_and(|mime| !inspector::is_text_media_type(mime))
    {
        return BodyText::Binary;
    }
    if metadata.availability != BodyAvailability::Complete
        || metadata.length_known && metadata.retained_bytes > MAX_TEXT_BYTES
    {
        return BodyText::Unavailable;
    }
    let Ok(mut lease) = store.open_complete(id, boundary) else {
        return BodyText::Unavailable;
    };
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        if canceled.load(Ordering::Acquire) {
            return BodyText::Unavailable;
        }
        let Ok(count) = lease.read(&mut buffer) else {
            return BodyText::Unavailable;
        };
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > MAX_TEXT_BYTES {
            return BodyText::Unavailable;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if !metadata.content_codings.is_empty() {
        let Ok(decoded) = inspector::decode_content(&metadata.content_codings, bytes).await else {
            return BodyText::Unavailable;
        };
        bytes = decoded;
    }
    let text = inspector::decode_unicode(&bytes, metadata.charset.as_deref())
        .filter(|text| !text.contains('\0'));
    text.map_or_else(
        || {
            if metadata
                .media_type
                .as_deref()
                .is_some_and(inspector::is_text_media_type)
            {
                BodyText::Unavailable
            } else {
                BodyText::Binary
            }
        },
        BodyText::Text,
    )
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, Application, BodyStoreConfig, SessionQueryInput, TraceImportRequest};
    use std::{fs::File, io::Write};

    fn request(pattern: &str) -> TrafficSearchRequest {
        TrafficSearchRequest {
            operation_id: "test-search".into(),
            pattern: pattern.into(),
            mode: TrafficSearchMode::Text,
            case_sensitive: false,
            ignore_diacritics: false,
            metadata: false,
            headers: false,
            response_bodies: true,
        }
    }
    async fn fixture(root: &std::path::Path) -> Application {
        let path = root.join("trace.saz");
        let encoded =
            inspector::encode_content(&["gzip".into()], "Café compressed body".as_bytes().to_vec())
                .await
                .unwrap();
        let utf16 = "CAFÉ Unicode body"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let payloads = [
            (
                "text/plain; charset=utf-8",
                encoded,
                "Content-Encoding: gzip\r\n",
            ),
            ("text/plain; charset=utf-16le", utf16, ""),
            ("image/png", vec![0, 255, 1, 2], ""),
        ];
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for (index, (mime, bytes, coding)) in payloads.into_iter().enumerate() {
            let source = index + 1;
            let request = format!(
                "GET https://example.invalid/{source} HTTP/1.1\r\nX-Long: {}END-NEEDLE\r\n\r\n",
                "x".repeat(7000)
            );
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n{coding}Content-Length: {}\r\n\r\n",
                bytes.len()
            )
            .into_bytes();
            response.extend(bytes);
            for (name, bytes) in [
                (format!("raw/{source}_c.txt"), request.into_bytes()),
                (format!("raw/{source}_s.txt"), response),
            ] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(&bytes).unwrap();
            }
        }
        zip.finish().unwrap();
        let application = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.join("bodies"))),
            ..AppConfig::default()
        })
        .unwrap();
        application
            .import_trace(
                TraceImportRequest {
                    path,
                    operation_id: "import".into(),
                    max_file_bytes: 4 * 1024 * 1024,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        application
    }
    #[test]
    fn string_options_and_bounded_regex_are_independent() {
        let mut input = request("cafe");
        assert!(!Matcher::new(&input).unwrap().matches("Café"));
        input.ignore_diacritics = true;
        assert!(Matcher::new(&input).unwrap().matches("CAFÉ"));
        assert!(Matcher::new(&input).unwrap().matches("Cafe\u{301}"));
        input.case_sensitive = true;
        assert!(!Matcher::new(&input).unwrap().matches("Café"));
        input.mode = TrafficSearchMode::Regex;
        input.ignore_diacritics = false;
        input.case_sensitive = false;
        input.pattern = "^caf[ée]$".into();
        assert!(Matcher::new(&input).unwrap().matches("CAFÉ"));
        input.pattern = "(".into();
        assert!(Matcher::new(&input).is_err());
    }
    #[tokio::test]
    async fn decoded_unicode_results_page_over_all_matches_and_skip_binary_quietly() {
        let root = tempfile::tempdir().unwrap();
        let app = fixture(root.path()).await;
        let mut input = request("cafe");
        input.ignore_diacritics = true;
        let result = app.search_traffic(input, Arc::new(|_| {})).await.unwrap();
        assert_eq!(result.ids.len(), 2);
        assert_eq!(result.binary_bodies, 1);
        assert_eq!(result.unavailable_bodies, 0);
        let page = app
            .query_sessions(SessionQueryInput {
                search_result_id: Some(result.id.clone()),
                offset: Some(0),
                limit: Some(1),
                ..SessionQueryInput::default()
            })
            .unwrap();
        assert_eq!(page.total_matched, 2);
        assert_eq!(page.sessions.len(), 1);
        let second = app
            .query_sessions(SessionQueryInput {
                search_result_id: Some(result.id.clone()),
                offset: Some(1),
                limit: Some(1),
                ..SessionQueryInput::default()
            })
            .unwrap();
        assert_ne!(second.sessions[0].id, page.sessions[0].id);
        let scoped = app
            .matching_traffic_ids(&SessionQueryInput {
                search_result_id: Some(result.id.clone()),
                filters: vec![crate::SessionColumnFilter {
                    column: crate::TrafficColumn::Path,
                    operator: crate::FilterOperator::Equals,
                    value: "/1".into(),
                }],
                ..SessionQueryInput::default()
            })
            .unwrap();
        assert_eq!(scoped.len(), 1);
        let mut headers = request("END-NEEDLE");
        headers.response_bodies = false;
        headers.headers = true;
        assert_eq!(
            app.search_traffic(headers, Arc::new(|_| {}))
                .await
                .unwrap()
                .ids
                .len(),
            3
        );
    }
    #[tokio::test]
    async fn cancellation_has_no_partial_result_and_unselected_removal_ignores_paging() {
        let root = tempfile::tempdir().unwrap();
        let app = fixture(root.path()).await;
        let registry = app.searches.clone();
        assert!(
            app.search_traffic(
                request("body"),
                Arc::new(move |_| registry.cancel("test-search"))
            )
            .await
            .is_err()
        );
        assert!(app.searches.lock().results.is_empty());
        let rows = app
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions;
        let removed = app
            .remove_unselected_traffic_entries(&[rows[0].id.clone()])
            .unwrap();
        assert_eq!(removed.len(), 2);
        assert_eq!(
            app.query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions
                .len(),
            1
        );
        assert_eq!(app.remove_traffic_entries(&removed, true).unwrap().len(), 2);
        assert_eq!(
            app.query_sessions(SessionQueryInput::default())
                .unwrap()
                .sessions
                .len(),
            3
        );
    }
}
