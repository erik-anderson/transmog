use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use transmog_core::ClientIdentity;
use transmog_session::{
    ApplicationSessionService, CatalogCursor, CatalogQuery, CatalogSubscription, SessionFilter,
    SessionSnapshot, SubscriptionEvent,
};

use crate::workspace::TrafficColumn;
use crate::{AppError, AutoResponseMatchView, ErrorCategory, inspector::auto_response_match};

/// Direction of a retained-traffic metadata sort.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortDirection {
    /// Lower values first.
    Ascending,
    /// Higher values first.
    Descending,
}

/// Sort applied across retained metadata before paging.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSort {
    /// Metadata field.
    pub column: TrafficColumn,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Supported deterministic column filter operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterOperator {
    /// Case-insensitive substring.
    Contains,
    /// Exact value.
    Equals,
    /// Inclusive numeric lower bound.
    Minimum,
    /// Inclusive numeric upper bound.
    Maximum,
}

/// One bounded column filter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionColumnFilter {
    /// Metadata field.
    pub column: TrafficColumn,
    /// Comparison operation.
    pub operator: FilterOperator,
    /// Text or base-ten integer, bounded to 255 characters.
    pub value: String,
}

const MAX_CURSOR_TOKENS: usize = 256;
const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_FILTER_CHARS: usize = 255;
const FETCH_DESTINATIONS: [&str; 24] = [
    "audio",
    "audioworklet",
    "document",
    "embed",
    "empty",
    "font",
    "frame",
    "iframe",
    "image",
    "json",
    "manifest",
    "object",
    "paintworklet",
    "report",
    "script",
    "serviceworker",
    "sharedworker",
    "style",
    "text",
    "track",
    "video",
    "webidentity",
    "worker",
    "xslt",
];

#[derive(Default)]
pub(crate) struct CursorRegistry {
    tokens: VecDeque<(String, CatalogCursor)>,
}

impl CursorRegistry {
    fn insert(&mut self, cursor: CatalogCursor) -> Result<String, AppError> {
        let mut bytes = [0_u8; 18];
        getrandom::fill(&mut bytes).map_err(|_| {
            AppError::new(ErrorCategory::Internal, "cursor generation failed", true)
        })?;
        let token = URL_SAFE_NO_PAD.encode(bytes);
        if self.tokens.len() == MAX_CURSOR_TOKENS {
            self.tokens.pop_front();
        }
        self.tokens.push_back((token.clone(), cursor));
        Ok(token)
    }

    fn resolve(&self, token: &str) -> Option<CatalogCursor> {
        self.tokens
            .iter()
            .find_map(|(candidate, cursor)| (candidate == token).then_some(*cursor))
    }
}

/// Bounded session query accepted from presentation layers.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueryInput {
    /// Optional original-source incident time/host restriction, applied before paging.
    #[serde(default)]
    pub incident_window: Option<crate::incidents::IncidentWindow>,
    /// Opaque completed content-search snapshot handle.
    #[serde(default)]
    pub search_result_id: Option<String>,
    /// Opaque continuation token returned by the prior page.
    pub cursor: Option<String>,
    /// Return the newest matching bounded window without pagination.
    #[serde(default)]
    pub latest: bool,
    /// Requested page length; the session layer applies its configured cap.
    pub limit: Option<usize>,
    /// Optional terminal-state filter.
    pub terminal: Option<bool>,
    /// Optional ASCII-case-insensitive method filter.
    pub method: Option<String>,
    /// Optional ASCII-case-insensitive host filter.
    pub host: Option<String>,
    /// Global metadata substring search.
    #[serde(default)]
    pub search: Option<String>,
    /// Column filters, combined with AND.
    #[serde(default)]
    pub filters: Vec<SessionColumnFilter>,
    /// Original client fetch destinations, combined with OR within this group.
    #[serde(default)]
    pub fetch_destinations: Vec<String>,
    /// Sort all retained matching traffic before returning a bounded page.
    #[serde(default)]
    pub sort: Option<SessionSort>,
    /// Offset into the sorted matching read model.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Reveal the bounded page containing this retained exchange after sorting.
    #[serde(default)]
    pub focus_id: Option<String>,
}

/// One bounded row in the live-session browser.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Exact workspace admission number, transported as decimal text.
    pub request_number: String,
    /// Opaque display identifier for subsequent detail queries.
    pub id: String,
    /// Best-effort caller identity captured when the connection opened.
    pub caller: ClientIdentityView,
    /// Last observed request method.
    pub method: String,
    /// Original target host.
    pub host: String,
    /// Original path and query.
    pub path: String,
    /// Client-facing HTTP protocol, unavailable when upstream setup failed before HTTP.
    pub protocol: String,
    /// Observed application stream protocol, independent of the HTTP handshake.
    pub stream_protocol: Option<transmog_core::stream::StreamProtocol>,
    /// Last response status, if observed.
    pub status: Option<u16>,
    /// Milliseconds from start to terminal observation or the page snapshot.
    pub duration_ms: Option<u64>,
    /// Original saved trace namespace, when this row was imported.
    pub trace_id: Option<String>,
    /// Request body bytes observed at all explicit boundaries.
    pub request_bytes: u64,
    /// Response body bytes observed at all explicit boundaries.
    pub response_bytes: u64,
    /// Stable terminal category.
    pub terminal: &'static str,
    /// Whether this exchange has visible sequence loss.
    pub loss: bool,
    /// Whether native capture is active for this page.
    pub capturing: bool,
    /// Winning local autoresponse, when the origin was bypassed.
    pub auto_response: Option<AutoResponseMatchView>,
    /// Complete original request target.
    pub url: String,
    /// Start timestamp in Unix milliseconds.
    pub started_at: u64,
    /// Client-facing response content type, without parameters.
    pub content_type: Option<String>,
    /// Original client fetch metadata identifies a top-level document navigation.
    pub top_level_navigation: bool,
    /// Original client Sec-Fetch-Dest value, when unambiguous and recorded.
    pub fetch_destination: Option<String>,
}

/// Presentation-safe caller identity for one downstream connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdentityView {
    /// `local-process`, `local-unknown`, or `remote`.
    pub kind: &'static str,
    /// Executable file name when available.
    pub process_name: Option<String>,
    /// Process identifier when the socket owner was resolved.
    pub process_id: Option<u32>,
}

pub(crate) fn client_identity_view(identity: &ClientIdentity) -> ClientIdentityView {
    match identity {
        ClientIdentity::LocalProcess { pid, name } => ClientIdentityView {
            kind: "local-process",
            process_name: name.clone(),
            process_id: Some(*pid),
        },
        ClientIdentity::LocalUnknown => ClientIdentityView {
            kind: "local-unknown",
            process_name: None,
            process_id: None,
        },
        ClientIdentity::Remote => ClientIdentityView {
            kind: "remote",
            process_name: None,
            process_id: None,
        },
    }
}

/// Authoritative page plus visible global pressure counters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    /// Bounded rows.
    pub sessions: Vec<SessionSummary>,
    /// Opaque token for the next page.
    pub next_cursor: Option<String>,
    /// Total catalog evictions observed.
    pub evicted: u64,
    /// Total observer sequence gaps observed.
    pub sequence_gaps: u64,
    /// Total refresh-hint lag observed.
    pub subscriber_lag: u64,
    /// Matching retained rows before pagination.
    pub total_matched: usize,
    /// Total retained metadata rows.
    pub retained_count: usize,
    /// Effective offset when revealing a particular exchange.
    pub focus_offset: Option<usize>,
}

/// Ordered metadata snapshot for a continuous virtual traffic list.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficView {
    /// Window-local handle used to read bounded row windows.
    pub id: String,
    /// Every matching entry in display order, independent of rendered rows.
    pub ids: Vec<String>,
    /// Initial bounded metadata window and catalog counters.
    pub page: SessionPage,
}

/// Compact request interval for a shared traffic timeline; contains no body data.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficTimelineRow {
    /// Stable workspace exchange identity.
    pub id: String,
    /// Exact workspace admission number.
    pub request_number: String,
    /// Recorded request method.
    pub method: String,
    /// Display target, shortened to at most 2048 characters plus an ellipsis.
    pub url: String,
    /// Original source namespace, or absent for live traffic.
    pub trace_id: Option<String>,
    /// Recorded request-header anchor, or original source start, in Unix milliseconds.
    pub start_millis: Option<u64>,
    /// Measured or source-recorded duration; active native traffic uses elapsed time so far.
    pub duration_micros: Option<u64>,
    /// Upstream response-header offset from the same request anchor, when measured.
    pub response_micros: Option<u64>,
    /// Human-readable measurement boundary or reason timing is unavailable.
    pub basis: &'static str,
    /// Completion state, independent of timing availability.
    pub terminal: &'static str,
}

#[derive(Default)]
pub(crate) struct TrafficViews {
    next: u64,
    views: std::collections::HashMap<String, Arc<Vec<String>>>,
}

impl TrafficViews {
    pub(crate) fn insert(&mut self, ids: Vec<String>) -> Result<String, AppError> {
        // The desktop owns a displayed view, a pending update and transient searches.
        // Explicit release keeps old snapshots alive only while they are in use.
        if self.views.len() >= 8 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Too many traffic views are open",
                true,
            ));
        }
        self.next = self.next.wrapping_add(1);
        let id = format!("traffic-{:x}", self.next);
        self.views.insert(id.clone(), Arc::new(ids));
        Ok(id)
    }

    pub(crate) fn release(&mut self, id: &str) {
        self.views.remove(id);
    }

    pub(crate) fn ids(&self, id: &str) -> Result<Arc<Vec<String>>, AppError> {
        self.views.get(id).cloned().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "Traffic view is no longer available",
                true,
            )
        })
    }

    pub(crate) fn clear(&mut self) {
        self.views.clear();
    }
}

/// Small lossy signal that tells presentation layers to query again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHint {
    /// Last exchange identity seen in a coalesced batch, if any.
    pub exchange_id: Option<String>,
    /// Last observer sequence seen in a coalesced batch.
    pub sequence: u64,
    /// Whether the receiver fell behind and must perform a complete refresh.
    pub lagged: bool,
}

/// Counted bounded session update subscription.
pub struct SessionUpdateSubscription {
    inner: CatalogSubscription,
}

impl SessionUpdateSubscription {
    /// Waits for one delta hint or visible lag marker.
    ///
    /// # Errors
    /// Returns a bounded unavailable error when the catalog closes.
    pub async fn recv(&mut self) -> Result<SessionHint, AppError> {
        match self
            .inner
            .recv()
            .await
            .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), true))?
        {
            SubscriptionEvent::Delta(delta) => Ok(SessionHint {
                exchange_id: Some(format!("{:032x}", delta.exchange_id.0)),
                sequence: delta.sequence,
                lagged: false,
            }),
            SubscriptionEvent::Lagged(count) => Ok(SessionHint {
                exchange_id: None,
                sequence: count,
                lagged: true,
            }),
        }
    }
}

#[cfg(test)]
pub(crate) fn query_sessions(
    service: &ApplicationSessionService,
    registry: &Arc<Mutex<CursorRegistry>>,
    input: SessionQueryInput,
) -> Result<SessionPage, AppError> {
    query_sessions_with_traces(
        service,
        registry,
        input,
        &crate::traces::TraceRegistry::default(),
        None,
    )
}

pub(crate) fn query_sessions_with_traces(
    service: &ApplicationSessionService,
    registry: &Arc<Mutex<CursorRegistry>>,
    input: SessionQueryInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&std::collections::HashSet<String>>,
) -> Result<SessionPage, AppError> {
    validate_filter(input.method.as_deref())?;
    validate_filter(input.host.as_deref())?;
    validate_fetch_destinations(&input.fetch_destinations)?;
    if input.sort.is_some()
        || input.search.is_some()
        || !input.filters.is_empty()
        || !input.fetch_destinations.is_empty()
        || input.offset.is_some()
        || input.focus_id.is_some()
        || matching.is_some()
    {
        return query_sorted(service, &input, traces, matching);
    }
    if input.latest && input.cursor.is_some() {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "latest session queries do not accept a cursor",
            false,
        ));
    }
    let after = if let Some(token) = input.cursor.as_deref() {
        Some(
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .resolve(token)
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::InvalidInput,
                        "session cursor is invalid or expired",
                        false,
                    )
                })?,
        )
    } else {
        None
    };
    let limit = input.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    let limit = NonZeroUsize::new(limit).ok_or_else(|| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "page size must be nonzero",
            false,
        )
    })?;
    let query = CatalogQuery {
        after,
        limit: Some(limit),
        filter: SessionFilter {
            terminal: input.terminal,
            method: input.method,
            host: input.host,
        },
    };
    let page = if input.latest {
        service.catalog().query_latest(&query)
    } else {
        service.catalog().query(&query)
    };
    let capturing = matches!(
        service.capture().status(),
        transmog_session::CaptureStatus::Active { .. }
    );
    let now = SystemTime::now();
    let sessions: Vec<_> = page
        .sessions
        .iter()
        .map(|snapshot| summarize_with_trace(snapshot, now, capturing, traces))
        .collect();
    let next_cursor = page
        .next
        .map(|cursor| {
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(cursor)
        })
        .transpose()?;
    let counters = service.catalog().counters();
    Ok(SessionPage {
        total_matched: sessions.len(),
        retained_count: sessions.len(),
        focus_offset: None,
        sessions,
        next_cursor,
        evicted: counters.evicted,
        sequence_gaps: counters.sequence_gaps,
        subscriber_lag: counters.subscriber_lag,
    })
}

pub(crate) fn subscribe(
    service: &ApplicationSessionService,
) -> Result<SessionUpdateSubscription, AppError> {
    Ok(SessionUpdateSubscription {
        inner: service
            .catalog()
            .subscribe()
            .map_err(|error| AppError::new(ErrorCategory::Limit, error.to_string(), true))?,
    })
}

fn summarize(snapshot: &SessionSnapshot, now: SystemTime, capturing: bool) -> SessionSummary {
    let (fetch_destination, top_level_navigation) = fetch_context(snapshot);
    let target = snapshot.metadata.original_target.as_target();
    let request = snapshot.request_heads.last().map(|head| &head.head);
    let end = snapshot
        .terminal_at
        .or_else(|| (!snapshot.imported && snapshot.terminal.is_none()).then_some(now));
    let duration_ms = end
        .filter(|_| snapshot.metadata.started_at != SystemTime::UNIX_EPOCH)
        .and_then(|end| end.duration_since(snapshot.metadata.started_at).ok())
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
    let duration_ms = snapshot
        .performance
        .elapsed_micros()
        .map(|micros| micros / 1_000)
        .or(duration_ms);
    let (request_bytes, response_bytes) =
        snapshot
            .bodies
            .iter()
            .fold(
                (0_u64, 0_u64),
                |(request_total, response_total), body| match body.boundary {
                    transmog_core::observe::ExchangeBoundary::ClientRequest => (
                        request_total.saturating_add(body.observed_bytes),
                        response_total,
                    ),
                    transmog_core::observe::ExchangeBoundary::ClientResponse => (
                        request_total,
                        response_total.saturating_add(body.observed_bytes),
                    ),
                    transmog_core::observe::ExchangeBoundary::UpstreamRequest
                    | transmog_core::observe::ExchangeBoundary::UpstreamResponse => {
                        (request_total, response_total)
                    }
                },
            );
    let terminal = snapshot.state();
    SessionSummary {
        request_number: snapshot.request_number.to_string(),
        top_level_navigation,
        fetch_destination,
        trace_id: None,
        url: format!(
            "{}://{}{}{}",
            target.scheme,
            target.authority,
            target.path,
            target
                .query
                .as_ref()
                .map_or_else(String::new, |query| format!("?{query}"))
        ),
        started_at: u64::try_from(
            snapshot
                .metadata
                .started_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX),
        content_type: response_content_type(snapshot),
        stream_protocol: snapshot.stream.as_ref().map(|stream| stream.protocol),
        id: format!("{:032x}", snapshot.exchange_id.0),
        caller: client_identity_view(&snapshot.metadata.client_identity),
        method: request.map_or_else(|| "—".to_owned(), |head| head.method.clone()),
        host: target.host.clone(),
        path: target.query.as_ref().map_or_else(
            || target.path.clone(),
            |query| format!("{}?{query}", target.path),
        ),
        protocol: traffic_protocol(snapshot),
        status: snapshot.response_heads.last().map(|head| head.head.status),
        duration_ms,
        request_bytes,
        response_bytes,
        terminal,
        loss: snapshot.sequence_loss > 0,
        capturing: capturing && !snapshot.imported,
        auto_response: auto_response_match(snapshot),
    }
}

fn traffic_protocol(snapshot: &SessionSnapshot) -> String {
    use transmog_core::performance::TransportOutcome;

    let upstream_http_observed = snapshot.performance.protocols.iter().any(|item| {
        matches!(
            item.boundary.as_str(),
            "upstream-request" | "upstream-response"
        )
    }) || snapshot
        .response_heads
        .iter()
        .any(|head| head.boundary == transmog_core::observe::ExchangeBoundary::UpstreamResponse);
    let setup_failed = snapshot.performance.transports.iter().any(|transport| {
        transport.leg == "upstream"
            && matches!(
                transport.outcome,
                TransportOutcome::DnsFailed
                    | TransportOutcome::TcpFailed
                    | TransportOutcome::TlsFailed
                    | TransportOutcome::PolicyRejected
            )
    });
    // A client's HTTP version does not establish an upstream HTTP exchange.
    // Retain actual HTTP evidence if another connection attempt succeeded.
    if setup_failed && !upstream_http_observed {
        return "Unavailable".into();
    }
    crate::inspector::message_protocol(
        &snapshot.performance,
        "client-request",
        snapshot.metadata.ingress_version,
    )
}

fn response_content_type(snapshot: &SessionSnapshot) -> Option<String> {
    snapshot
        .response_heads
        .iter()
        .rev()
        .find(|head| head.boundary == transmog_core::observe::ExchangeBoundary::ClientResponse)
        .or_else(|| snapshot.response_heads.last())
        .and_then(|head| head.head.headers.values("content-type").next())
        .and_then(|value| std::str::from_utf8(value).ok())
        .map(|mime| {
            mime.split(';')
                .next()
                .unwrap_or(mime)
                .trim()
                .to_ascii_lowercase()
        })
}

fn fetch_context(snapshot: &SessionSnapshot) -> (Option<String>, bool) {
    let Some(request) = snapshot
        .request_heads
        .iter()
        .find(|head| head.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest)
    else {
        return (None, false);
    };
    let headers = &request.head.headers;
    let destination = single_header(headers, "sec-fetch-dest")
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_owned);
    let navigation = destination.as_deref() == Some("document")
        && (!headers.iter().any(|field| field.name_eq("sec-fetch-mode"))
            || single_header(headers, "sec-fetch-mode") == Some(b"navigate"));
    (destination, navigation)
}

fn single_header<'a>(headers: &'a transmog_core::HeaderBlock, name: &str) -> Option<&'a [u8]> {
    let mut fields = headers.iter().filter(|field| field.name_eq(name));
    let field = fields.next()?;
    (!field.is_redacted() && fields.next().is_none()).then(|| field.value().trim_ascii())
}

fn validate_fetch_destinations(values: &[String]) -> Result<(), AppError> {
    if values.len() > FETCH_DESTINATIONS.len()
        || values
            .iter()
            .any(|value| !FETCH_DESTINATIONS.contains(&value.as_str()))
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Choose supported fetch destinations, with at most 24 selections",
            false,
        ));
    }
    Ok(())
}

fn numeric_value(row: &SessionSummary, column: TrafficColumn) -> Option<u64> {
    match column {
        TrafficColumn::RequestNumber => row.request_number.parse().ok(),
        TrafficColumn::Status => row.status.map(u64::from),
        TrafficColumn::Pid => row.caller.process_id.map(u64::from),
        TrafficColumn::Duration => row.duration_ms,
        TrafficColumn::ResponseBytes => Some(row.response_bytes),
        TrafficColumn::RequestBytes => Some(row.request_bytes),
        TrafficColumn::StartedAt => Some(row.started_at),
        _ => None,
    }
}

fn numeric_column(column: TrafficColumn) -> bool {
    matches!(
        column,
        TrafficColumn::RequestNumber
            | TrafficColumn::Status
            | TrafficColumn::Pid
            | TrafficColumn::Duration
            | TrafficColumn::ResponseBytes
            | TrafficColumn::RequestBytes
            | TrafficColumn::StartedAt
    )
}

fn text_value(row: &SessionSummary, column: TrafficColumn) -> String {
    match column {
        TrafficColumn::Method => row.method.clone(),
        TrafficColumn::Process => row
            .caller
            .process_name
            .clone()
            .unwrap_or_else(|| row.caller.kind.to_owned()),
        TrafficColumn::Host => row.host.clone(),
        TrafficColumn::Path => row.path.clone(),
        TrafficColumn::Url => row.url.clone(),
        TrafficColumn::Protocol => match row.protocol.as_str() {
            "Http1" => "HTTP/1.1".to_owned(),
            "Http2" => "HTTP/2".to_owned(),
            other => other.to_owned(),
        },
        TrafficColumn::State => row.terminal.to_owned(),
        TrafficColumn::ContentType => row.content_type.clone().unwrap_or_default(),
        numeric => numeric_value(row, numeric).map_or_else(String::new, |value| value.to_string()),
    }
}

fn summarize_with_trace(
    snapshot: &SessionSnapshot,
    now: SystemTime,
    capturing: bool,
    traces: &crate::traces::TraceRegistry,
) -> SessionSummary {
    let mut row = summarize(snapshot, now, capturing);
    if let Some(entry) = traces.entry(&row.id) {
        row.trace_id = Some(entry.trace_id);
        if !entry.protocol_known {
            row.protocol = "Unavailable".into();
        }
        if !entry.target_known {
            row.url = "Unavailable".into();
        }
    }
    row
}

fn query_sorted(
    service: &ApplicationSessionService,
    input: &SessionQueryInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&std::collections::HashSet<String>>,
) -> Result<SessionPage, AppError> {
    let (rows, retained_count) = ordered_rows(service, input, traces, matching)?;
    let limit = input
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(service.catalog().page_size_limit());
    page_from_rows(service, input, rows, retained_count, limit)
}

fn ordered_rows(
    service: &ApplicationSessionService,
    input: &SessionQueryInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&std::collections::HashSet<String>>,
) -> Result<(Vec<SessionSummary>, usize), AppError> {
    validate_filter(input.method.as_deref())?;
    validate_filter(input.host.as_deref())?;
    validate_fetch_destinations(&input.fetch_destinations)?;
    if input.cursor.is_some() || input.filters.len() > 14 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "sorted traffic queries require a bounded offset and at most 14 filters",
            false,
        ));
    }
    validate_filter(input.search.as_deref())?;
    let filters = input
        .filters
        .iter()
        .map(PreparedFilter::new)
        .collect::<Result<Vec<_>, _>>()?;
    let search = input.search.as_ref().map(|value| value.to_lowercase());
    let capturing = matches!(
        service.capture().status(),
        transmog_session::CaptureStatus::Active { .. }
    );
    let now = SystemTime::now();
    let rows = service
        .catalog()
        .project_retained(|snapshot| summarize_with_trace(snapshot, now, capturing, traces));
    let retained_count = rows.len();
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|row| matches_query_row(row, input, matching, search.as_deref(), &filters))
        .collect();
    if let Some(sort) = &input.sort {
        sort_rows(&mut rows, sort);
    }
    Ok((rows, retained_count))
}

fn page_from_rows(
    service: &ApplicationSessionService,
    input: &SessionQueryInput,
    rows: Vec<SessionSummary>,
    retained_count: usize,
    limit: usize,
) -> Result<SessionPage, AppError> {
    if limit == 0 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "page size must be nonzero",
            false,
        ));
    }
    let total_matched = rows.len();
    let focus_offset = input
        .focus_id
        .as_ref()
        .map(|id| {
            rows.iter()
                .position(|row| &row.id == id)
                .map(|index| index / limit * limit)
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::Unavailable,
                        "source is no longer in the matching retained traffic",
                        false,
                    )
                })
        })
        .transpose()?;
    let counters = service.catalog().counters();
    Ok(SessionPage {
        sessions: rows
            .into_iter()
            .skip(focus_offset.unwrap_or_else(|| input.offset.unwrap_or(0)))
            .take(limit)
            .collect(),
        next_cursor: None,
        evicted: counters.evicted,
        sequence_gaps: counters.sequence_gaps,
        subscriber_lag: counters.subscriber_lag,
        total_matched,
        retained_count,
        focus_offset,
    })
}

pub(crate) fn traffic_view(
    service: &ApplicationSessionService,
    views: &Mutex<TrafficViews>,
    input: &SessionQueryInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&std::collections::HashSet<String>>,
) -> Result<TrafficView, AppError> {
    let (rows, retained) = ordered_rows(service, input, traces, matching)?;
    let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let page = page_from_rows(service, input, rows, retained, 100)?;
    let id = views
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(ids.clone())?;
    Ok(TrafficView { id, ids, page })
}

pub(crate) fn traffic_rows(
    service: &ApplicationSessionService,
    views: &Mutex<TrafficViews>,
    id: &str,
    offset: usize,
    limit: usize,
    traces: &crate::traces::TraceRegistry,
) -> Result<Vec<SessionSummary>, AppError> {
    if limit == 0 || limit > 512 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Request between 1 and 512 traffic rows",
            false,
        ));
    }
    let ids = views
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .ids(id)?;
    let now = SystemTime::now();
    let capturing = matches!(
        service.capture().status(),
        transmog_session::CaptureStatus::Active { .. }
    );
    Ok(ids
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|id| {
            let exchange = crate::response_assets::parse_exchange_id(id).ok()?;
            service
                .catalog()
                .get(exchange)
                .map(|snapshot| summarize_with_trace(&snapshot, now, capturing, traces))
        })
        .collect())
}

pub(crate) fn traffic_timeline_rows(
    service: &ApplicationSessionService,
    views: &Mutex<TrafficViews>,
    id: &str,
    offset: usize,
    limit: usize,
    traces: &crate::traces::TraceRegistry,
) -> Result<Vec<TrafficTimelineRow>, AppError> {
    if limit == 0 || limit > 2048 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Request between 1 and 2048 timeline rows",
            false,
        ));
    }
    let ids = views
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .ids(id)?;
    let now = SystemTime::now();
    Ok(ids
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|id| {
            let exchange = crate::response_assets::parse_exchange_id(id).ok()?;
            service
                .catalog()
                .get(exchange)
                .map(|snapshot| timeline_row(&snapshot, now, traces))
        })
        .collect())
}

pub(crate) fn timeline_row(
    snapshot: &SessionSnapshot,
    now: SystemTime,
    traces: &crate::traces::TraceRegistry,
) -> TrafficTimelineRow {
    use transmog_core::performance::Milestone;
    let summary = summarize_with_trace(snapshot, now, false, traces);
    let headers = snapshot
        .performance
        .points
        .iter()
        .find(|point| point.milestone == Milestone::RequestHeaders);
    let start_millis = headers.map_or_else(
        || (summary.started_at > 0).then_some(summary.started_at),
        |point| (point.unix_millis > 0).then_some(point.unix_millis),
    );
    let measured = snapshot.performance.elapsed_micros();
    let end = snapshot.terminal_at.or_else(|| {
        (!snapshot.imported && snapshot.terminal.is_none() && summary.terminal == "active")
            .then_some(now)
    });
    let source_duration = end
        .filter(|_| snapshot.metadata.started_at != SystemTime::UNIX_EPOCH)
        .and_then(|end| end.duration_since(snapshot.metadata.started_at).ok())
        .and_then(|duration| u64::try_from(duration.as_micros()).ok());
    // An active request with a header anchor uses that anchor, not admission time.
    let elapsed = (!snapshot.imported && measured.is_none())
        .then(|| {
            let start = SystemTime::UNIX_EPOCH
                .checked_add(std::time::Duration::from_millis(start_millis?))?;
            u64::try_from(end?.duration_since(start).ok()?.as_micros()).ok()
        })
        .flatten();
    let duration_micros = if headers.is_some() {
        measured.or(elapsed)
    } else {
        source_duration
    };
    let response_micros = headers
        .and_then(|start| {
            snapshot
                .performance
                .points
                .iter()
                .find(|point| point.milestone == Milestone::ResponseHeaders)
                .and_then(|end| end.offset_micros.checked_sub(start.offset_micros))
                .and_then(|offset| u64::try_from(offset).ok())
        })
        .filter(|offset| duration_micros.is_some_and(|duration| *offset <= duration));
    let basis = if measured.is_some() {
        "Request headers to proxy completion"
    } else if snapshot.imported {
        "Recorded source interval; source clock"
    } else if summary.terminal == "active" {
        "Elapsed so far at the proxy"
    } else {
        "Recorded start to local completion"
    };
    let mut url: String = summary.url.chars().take(2048).collect();
    if url.len() < summary.url.len() {
        url.push('…');
    }
    TrafficTimelineRow {
        id: summary.id,
        request_number: summary.request_number,
        method: summary.method,
        url,
        trace_id: summary.trace_id,
        start_millis,
        duration_micros,
        response_micros,
        basis,
        terminal: summary.terminal,
    }
}

fn matches_query_row(
    row: &SessionSummary,
    input: &SessionQueryInput,
    matching: Option<&std::collections::HashSet<String>>,
    search: Option<&str>,
    filters: &[PreparedFilter],
) -> bool {
    matching.is_none_or(|matching| matching.contains(&row.id))
        && input
            .method
            .as_ref()
            .is_none_or(|method| row.method.eq_ignore_ascii_case(method))
        && input
            .host
            .as_ref()
            .is_none_or(|host| row.host.eq_ignore_ascii_case(host))
        && input
            .terminal
            .is_none_or(|terminal| (row.terminal != "active") == terminal)
        && search.is_none_or(|search| matches_search(row, search))
        && (input.fetch_destinations.is_empty()
            || row
                .fetch_destination
                .as_ref()
                .is_some_and(|destination| input.fetch_destinations.contains(destination)))
        && filters.iter().all(|filter| filter.matches(row))
}

pub(crate) fn matching_ids(
    service: &ApplicationSessionService,
    input: &SessionQueryInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&std::collections::HashSet<String>>,
) -> Result<Vec<String>, AppError> {
    let (rows, _) = ordered_rows(service, input, traces, matching)?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

struct PreparedFilter {
    column: TrafficColumn,
    operator: FilterOperator,
    text: String,
    numeric: Option<u64>,
}

impl PreparedFilter {
    fn new(filter: &SessionColumnFilter) -> Result<Self, AppError> {
        validate_filter(Some(&filter.value))?;
        let numeric = filter.value.parse::<u64>().ok();
        if filter.column == TrafficColumn::RequestNumber
            && (filter.operator == FilterOperator::Contains
                || filter.value.is_empty()
                || !filter.value.bytes().all(|byte| byte.is_ascii_digit())
                || numeric.is_none_or(|value| value == 0))
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "Request order requires a positive decimal integer from 1 to 18446744073709551615 and Equals, At least, or At most.",
                false,
            ));
        }
        if matches!(
            filter.operator,
            FilterOperator::Minimum | FilterOperator::Maximum
        ) && !numeric_column(filter.column)
            || numeric_column(filter.column)
                && filter.operator != FilterOperator::Contains
                && numeric.is_none()
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "numeric filters require a base-ten integer and a numeric column",
                false,
            ));
        }
        Ok(Self {
            column: filter.column,
            operator: filter.operator,
            text: filter.value.to_lowercase(),
            numeric,
        })
    }

    fn matches(&self, row: &SessionSummary) -> bool {
        match self.operator {
            FilterOperator::Contains => text_value(row, self.column)
                .to_lowercase()
                .contains(&self.text),
            FilterOperator::Equals if numeric_column(self.column) => {
                numeric_value(row, self.column).is_some_and(|value| Some(value) == self.numeric)
            }
            FilterOperator::Equals => text_value(row, self.column).to_lowercase() == self.text,
            FilterOperator::Minimum => numeric_value(row, self.column)
                .is_some_and(|value| self.numeric.is_some_and(|minimum| value >= minimum)),
            FilterOperator::Maximum => numeric_value(row, self.column)
                .is_some_and(|value| self.numeric.is_some_and(|maximum| value <= maximum)),
        }
    }
}

fn matches_search(row: &SessionSummary, search: &str) -> bool {
    [
        row.url.as_str(),
        row.method.as_str(),
        row.host.as_str(),
        row.caller.process_name.as_deref().unwrap_or_default(),
        row.terminal,
        row.content_type.as_deref().unwrap_or_default(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(search))
        || url_without_default_port(&row.url).is_some_and(|url| url.to_lowercase().contains(search))
        || row
            .caller
            .process_id
            .is_some_and(|pid| pid.to_string().contains(search))
}

/// Search alias for targets retaining an explicit HTTP(S) default port.
/// Keep the captured URL intact and preserve non-default ports and IPv6 brackets.
pub(crate) fn url_without_default_port(url: &str) -> Option<String> {
    let (scheme, target) = url.split_once("://")?;
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        ":443"
    } else if scheme.eq_ignore_ascii_case("http") {
        ":80"
    } else {
        return None;
    };
    let boundary = target.find(['/', '?', '#']).unwrap_or(target.len());
    let authority = target[..boundary].strip_suffix(default_port)?;
    Some(format!("{scheme}://{authority}{}", &target[boundary..]))
}

fn sort_rows(rows: &mut [SessionSummary], sort: &SessionSort) {
    if numeric_column(sort.column) {
        rows.sort_by(|left, right| {
            let comparison = match (
                numeric_value(left, sort.column),
                numeric_value(right, sort.column),
            ) {
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (Some(_), None) => return std::cmp::Ordering::Less,
                (left, right) => left.cmp(&right),
            };
            let comparison = if sort.direction == SortDirection::Descending {
                comparison.reverse()
            } else {
                comparison
            };
            comparison.then_with(|| left.id.cmp(&right.id))
        });
    } else if sort.direction == SortDirection::Descending {
        rows.sort_by_cached_key(|row| {
            (
                std::cmp::Reverse(text_value(row, sort.column).to_lowercase()),
                row.id.clone(),
            )
        });
    } else {
        rows.sort_by_cached_key(|row| {
            (text_value(row, sort.column).to_lowercase(), row.id.clone())
        });
    }
}

fn validate_filter(value: Option<&str>) -> Result<(), AppError> {
    if value.is_some_and(|value| {
        value.chars().count() > MAX_FILTER_CHARS || value.chars().any(char::is_control)
    }) {
        Err(AppError::new(
            ErrorCategory::InvalidInput,
            "session filter is too long or contains control characters",
            false,
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_core::{
        ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeId, ExchangeMetadata},
        observe::{ObserverEvent, ObserverEventKind},
    };

    fn started(id: u128) -> ObserverEvent {
        let target = Target {
            scheme: "https".to_owned(),
            authority: "example.test".to_owned(),
            host: "example.test".to_owned(),
            port: 443,
            path: format!("/{id}"),
            query: None,
        };
        let session = SessionMetadata {
            session_id: SessionId(id),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(id),
            client_addr: "127.0.0.1:1".parse().unwrap(),
            client_identity: ClientIdentity::default(),
            proxy_addr: "127.0.0.1:2".parse().unwrap(),
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
        };
        ObserverEvent {
            exchange_id: ExchangeId(id),
            sequence: 1,
            kind: ObserverEventKind::ExchangeStarted {
                metadata: Arc::new(ExchangeMetadata::from_session(&session, target)),
            },
        }
    }

    #[test]
    fn virtual_view_keeps_full_order_and_reads_current_offscreen_metadata() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=50_000 {
            service.catalog().apply(started(id));
        }
        let traces = crate::traces::TraceRegistry::default();
        let views = Mutex::new(TrafficViews::default());
        let query = SessionQueryInput {
            sort: Some(SessionSort {
                column: TrafficColumn::Host,
                direction: SortDirection::Ascending,
            }),
            ..Default::default()
        };
        let view = traffic_view(&service, &views, &query, &traces, None).unwrap();
        assert_eq!(view.ids.len(), 50_000);
        assert_eq!(view.page.sessions.len(), 100);
        assert_eq!(
            matching_ids(&service, &query, &traces, None).unwrap(),
            view.ids
        );
        service.catalog().apply(started(50_001));
        service.catalog().dismiss(&[ExchangeId(5_002)]);
        let rows = traffic_rows(&service, &views, &view.id, 5_000, 3, &traces).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![view.ids[5_000].as_str(), view.ids[5_002].as_str()]
        );
        assert_eq!(views.lock().unwrap().ids(&view.id).unwrap().len(), 50_000);
        assert!(traffic_rows(&service, &views, &view.id, 0, 513, &traces).is_err());
        views.lock().unwrap().release(&view.id);
        assert!(traffic_rows(&service, &views, &view.id, 0, 100, &traces).is_err());
    }

    #[test]
    fn timeline_uses_the_header_anchor_and_monotonic_duration_without_losing_zero() {
        use transmog_core::performance::{Milestone, TimingPoint};
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        service.catalog().apply(started(1));
        let mut snapshot = service.catalog().get(ExchangeId(1)).unwrap();
        let traces = crate::traces::TraceRegistry::default();
        snapshot.performance.points = vec![
            TimingPoint {
                milestone: Milestone::RequestHeaders,
                unix_millis: 1_800_000_000_000,
                offset_micros: 0,
            },
            TimingPoint {
                milestone: Milestone::ResponseHeaders,
                unix_millis: 1_800_000_000_003,
                offset_micros: 3_250,
            },
            TimingPoint {
                milestone: Milestone::ExchangeDone,
                unix_millis: 1_800_000_000_005,
                offset_micros: 5_750,
            },
        ];
        let row = timeline_row(&snapshot, SystemTime::now(), &traces);
        assert_eq!(row.start_millis, Some(1_800_000_000_000));
        assert_eq!(row.duration_micros, Some(5_750));
        assert_eq!(row.response_micros, Some(3_250));
        snapshot.performance.points[2].offset_micros = 0;
        let row = timeline_row(&snapshot, SystemTime::now(), &traces);
        assert_eq!(row.duration_micros, Some(0));
        assert_eq!(row.response_micros, None);
        snapshot.performance.points.clear();
        snapshot.imported = true;
        snapshot.metadata = Arc::new(transmog_core::intercept::ExchangeMetadata {
            started_at: SystemTime::UNIX_EPOCH,
            ..(*snapshot.metadata).clone()
        });
        let row = timeline_row(&snapshot, SystemTime::now(), &traces);
        assert_eq!(row.start_millis, None);
        assert_eq!(row.duration_micros, None);
        assert_eq!(row.response_micros, None);
    }

    #[tokio::test]
    async fn stopped_proxy_freezes_traffic_and_timeline_durations() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let mut start = started(1);
        if let ObserverEventKind::ExchangeStarted { metadata } = &mut start.kind {
            Arc::make_mut(metadata).started_at =
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        }
        service.catalog().apply(start);
        service.catalog().apply(ObserverEvent {
            exchange_id: ExchangeId(1),
            sequence: 2,
            kind: ObserverEventKind::Performance(transmog_core::performance::PerformanceEvidence {
                points: vec![transmog_core::performance::TimingPoint {
                    milestone: transmog_core::performance::Milestone::RequestHeaders,
                    unix_millis: 1_700_000_000_000,
                    offset_micros: 0,
                }],
                ..Default::default()
            }),
        });
        service.stop().await.unwrap();
        let snapshot = service.catalog().get(ExchangeId(1)).unwrap();
        let now = SystemTime::now();
        let later = now + std::time::Duration::from_secs(3600);
        let traces = crate::traces::TraceRegistry::default();
        assert_eq!(summarize(&snapshot, now, false).terminal, "failed");
        assert!(summarize(&snapshot, now, false).duration_ms.is_some());
        assert!(
            timeline_row(&snapshot, now, &traces)
                .duration_micros
                .is_some()
        );
        assert_eq!(
            summarize(&snapshot, now, false).duration_ms,
            summarize(&snapshot, later, false).duration_ms
        );
        assert_eq!(
            timeline_row(&snapshot, now, &traces).duration_micros,
            timeline_row(&snapshot, later, &traces).duration_micros
        );
        assert_eq!(timeline_row(&snapshot, later, &traces).terminal, "failed");
        // A terminal entry with a missing timestamp must remain unknown.
        let mut untimed = snapshot;
        untimed.terminal_at = None;
        assert_eq!(summarize(&untimed, later, false).duration_ms, None);
        assert_eq!(timeline_row(&untimed, later, &traces).duration_micros, None);
    }

    #[test]
    fn timeline_pages_cover_offscreen_matches_and_reject_expired_views() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=3000 {
            service.catalog().apply(started(id));
        }
        let traces = crate::traces::TraceRegistry::default();
        let views = Mutex::new(TrafficViews::default());
        let view = traffic_view(
            &service,
            &views,
            &SessionQueryInput::default(),
            &traces,
            None,
        )
        .unwrap();
        let rows = traffic_timeline_rows(&service, &views, &view.id, 2000, 1000, &traces).unwrap();
        assert_eq!(rows.len(), 1000);
        assert_eq!(rows[0].id, view.ids[2000]);
        assert!(traffic_timeline_rows(&service, &views, &view.id, 0, 2049, &traces).is_err());
        assert!(traffic_timeline_rows(&service, &views, &view.id, 0, 0, &traces).is_err());
        views.lock().unwrap().release(&view.id);
        assert!(traffic_timeline_rows(&service, &views, &view.id, 0, 100, &traces).is_err());
    }

    #[test]
    fn traffic_views_are_bounded_and_release_does_not_expire_other_views() {
        let mut views = TrafficViews::default();
        let first = views.insert(vec!["first".into()]).unwrap();
        let second = views.insert(vec!["second".into()]).unwrap();
        for _ in 0..6 {
            views.insert(Vec::new()).unwrap();
        }
        assert!(views.insert(Vec::new()).is_err());
        views.release(&second);
        assert!(views.insert(Vec::new()).is_ok());
        assert_eq!(&*views.ids(&first).unwrap(), &["first".to_owned()]);
    }

    #[test]
    fn ten_thousand_rows_remain_paged_and_cursors_are_opaque() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=10_000 {
            assert_eq!(
                service.catalog().apply(started(id)),
                transmog_session::CatalogApply::Applied
            );
        }
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let first = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                limit: Some(200),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.sessions.len(), 200);
        let cursor = first.next_cursor.unwrap();
        assert_eq!(cursor.len(), 24);
        let second = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                cursor: Some(cursor),
                limit: Some(200),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second.sessions.first().unwrap().path, "/201");
    }

    #[test]
    fn invalid_or_expired_cursor_fails_closed() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let error = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                cursor: Some("invented".to_owned()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.category, ErrorCategory::InvalidInput);
    }

    #[test]
    fn latest_window_is_bounded_and_remains_in_admission_order() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=12 {
            assert_eq!(
                service.catalog().apply(started(id)),
                transmog_session::CatalogApply::Applied
            );
        }
        let page = query_sessions(
            &service,
            &Arc::new(Mutex::new(CursorRegistry::default())),
            SessionQueryInput {
                latest: true,
                limit: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            page.sessions
                .iter()
                .map(|session| session.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/9", "/10", "/11", "/12"]
        );
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn timestamp_conversion_is_bounded() {
        assert_eq!(
            std::time::UNIX_EPOCH
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            0
        );
    }

    #[test]
    fn traffic_protocol_does_not_label_setup_failures_with_the_client_version() {
        use transmog_core::{
            HeaderBlock, RequestHead,
            intercept::{ExchangeFailure, ExchangeFailureKind, ExchangeStage},
            performance::{ProtocolObservation, TransportObservation, TransportOutcome},
        };
        use transmog_session::{ObservedRequestHead, ObservedResponseHead, SessionTerminal};

        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        service.catalog().apply(started(1));
        let mut snapshot = service.catalog().get(ExchangeId(1)).unwrap();
        Arc::make_mut(&mut snapshot.metadata).ingress_version = HttpLegVersion::Http2;
        snapshot.performance.protocols.push(ProtocolObservation {
            boundary: "client-request".into(),
            version: "HTTP/2".into(),
            reason: None,
        });
        // A finalized upstream head is intent, before the transport connects.
        for boundary in [
            transmog_core::observe::ExchangeBoundary::ClientRequest,
            transmog_core::observe::ExchangeBoundary::UpstreamRequest,
        ] {
            snapshot.request_heads.push(ObservedRequestHead {
                boundary,
                head: RequestHead {
                    method: "GET".into(),
                    target: snapshot.metadata.original_target.as_target().clone(),
                    headers: HeaderBlock::default(),
                    source_version: HttpLegVersion::Http2,
                },
            });
        }
        snapshot.terminal = Some(SessionTerminal::Failed(ExchangeFailure {
            metadata: snapshot.metadata.clone(),
            stage: ExchangeStage::Upstream,
            kind: ExchangeFailureKind::Upstream,
            request_committed: false,
            response_committed: false,
            message: "Connection setup failed".into(),
        }));
        for outcome in [
            TransportOutcome::DnsFailed,
            TransportOutcome::TcpFailed,
            TransportOutcome::TlsFailed,
            TransportOutcome::PolicyRejected,
        ] {
            snapshot.performance.transports = vec![TransportObservation {
                leg: "upstream".into(),
                outcome,
                ..Default::default()
            }];
            let row = summarize(&snapshot, SystemTime::now(), false);
            assert_eq!(row.terminal, "failed");
            assert_eq!(text_value(&row, TrafficColumn::Protocol), "Unavailable");
            assert_eq!(
                crate::inspector::detail_from_snapshot(&snapshot, None)
                    .unwrap()
                    .requests[0]
                    .protocol,
                "HTTP/2"
            );
        }
        // A failed alternative must not hide HTTP evidence from another attempt,
        // including older captures which only retained the upstream response.
        snapshot.response_heads.push(ObservedResponseHead {
            boundary: transmog_core::observe::ExchangeBoundary::UpstreamResponse,
            head: transmog_core::ResponseHead {
                status: 200,
                headers: HeaderBlock::default(),
                source_version: HttpLegVersion::Http2,
            },
        });
        assert_eq!(
            summarize(&snapshot, SystemTime::now(), false).protocol,
            "HTTP/2"
        );
        snapshot.response_heads.clear();
        snapshot.performance.protocols.push(ProtocolObservation {
            boundary: "upstream-request".into(),
            version: "HTTP/2".into(),
            reason: None,
        });
        assert_eq!(
            summarize(&snapshot, SystemTime::now(), false).protocol,
            "HTTP/2"
        );
        snapshot.performance.transports.clear();
        snapshot.performance.protocols.pop();
        assert_eq!(
            summarize(&snapshot, SystemTime::now(), false).protocol,
            "HTTP/2"
        );
    }

    #[test]
    fn traffic_rows_preserve_stream_protocol_and_effective_lifetime() {
        use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        service.catalog().apply(started(1));
        let mut snapshot = service.catalog().get(ExchangeId(1)).unwrap();
        let now = SystemTime::now();
        assert_eq!(summarize(&snapshot, now, false).stream_protocol, None);
        for protocol in [StreamProtocol::WebSocket, StreamProtocol::Sse] {
            for (status, terminal) in [
                (StreamStatus::Connecting, "active"),
                (StreamStatus::Open, "active"),
                (StreamStatus::Closing, "active"),
                (StreamStatus::Closed, "completed"),
                (StreamStatus::Failed, "failed"),
                (StreamStatus::Interrupted, "interrupted"),
            ] {
                snapshot.stream = Some(StreamState::now(protocol, status, None));
                snapshot.imported = false;
                let row = summarize(&snapshot, now, false);
                assert_eq!(row.stream_protocol, Some(protocol));
                assert_eq!(row.terminal, terminal);
                let json = serde_json::to_value(&row).unwrap();
                assert_eq!(
                    json["streamProtocol"],
                    serde_json::to_value(protocol).unwrap()
                );
                snapshot.imported = true;
                let saved = summarize(&snapshot, now, false);
                assert_eq!(saved.stream_protocol, Some(protocol));
                assert_eq!(
                    saved.terminal,
                    if status.is_active() {
                        "open-at-capture"
                    } else {
                        terminal
                    }
                );
            }
        }
    }

    type NavigationCase = (&'static str, Vec<(&'static str, &'static str)>, bool);

    fn navigation_cases() -> Vec<NavigationCase> {
        let mut cases = vec![
            (
                "GET",
                vec![
                    ("Sec-Fetch-Dest", "document"),
                    ("Sec-Fetch-Mode", "navigate"),
                ],
                true,
            ),
            ("POST", vec![("Sec-Fetch-Dest", "document")], true),
            ("GET", vec![("sEc-FeTcH-dEsT", " document \t")], true),
            (
                "GET",
                vec![("Sec-Fetch-Dest", "iframe"), ("Sec-Fetch-Mode", "navigate")],
                false,
            ),
            ("GET", vec![("Sec-Fetch-Dest", "frame")], false),
            ("GET", vec![("Sec-Fetch-Dest", "image")], false),
            (
                "GET",
                vec![("Sec-Fetch-Mode", "navigate"), ("Accept", "text/html")],
                false,
            ),
            (
                "GET",
                vec![("Sec-Fetch-Dest", "document"), ("Sec-Fetch-Mode", "cors")],
                false,
            ),
            (
                "GET",
                vec![("Sec-Fetch-Dest", "document"), ("Sec-Fetch-Dest", "iframe")],
                false,
            ),
            (
                "GET",
                vec![
                    ("Sec-Fetch-Dest", "document"),
                    ("Sec-Fetch-Mode", "navigate"),
                    ("Sec-Fetch-Mode", "cors"),
                ],
                false,
            ),
            ("GET", vec![], false),
        ];
        // Upstream automation must not make a subresource appear to be a navigation.
        cases.push(("GET", vec![("Sec-Fetch-Dest", "script")], false));
        cases
    }

    fn observe_request(
        service: &ApplicationSessionService,
        id: u128,
        sequence: u64,
        boundary: transmog_core::observe::ExchangeBoundary,
        method: &str,
        headers: transmog_core::HeaderBlock,
    ) {
        let target = service
            .catalog()
            .get(ExchangeId(id))
            .unwrap()
            .metadata
            .original_target
            .as_target()
            .clone();
        service.catalog().apply(ObserverEvent {
            exchange_id: ExchangeId(id),
            sequence,
            kind: ObserverEventKind::RequestHeadObserved {
                boundary,
                head: transmog_core::RequestHead {
                    method: method.into(),
                    target,
                    source_version: HttpLegVersion::Http1,
                    headers,
                },
            },
        });
    }

    #[test]
    fn navigation_markers_require_original_unambiguous_client_fetch_metadata() {
        use transmog_core::{HeaderBlock, HeaderField, observe::ExchangeBoundary};
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let cases = navigation_cases();
        for (index, (method, headers, expected)) in cases.into_iter().enumerate() {
            let id = index as u128 + 1;
            service.catalog().apply(started(id));
            for (sequence, boundary, fields) in [
                (2, ExchangeBoundary::ClientRequest, headers),
                (
                    3,
                    ExchangeBoundary::UpstreamRequest,
                    vec![("Sec-Fetch-Dest", "document")],
                ),
            ] {
                observe_request(
                    &service,
                    id,
                    sequence,
                    boundary,
                    method,
                    HeaderBlock::from_fields(
                        fields
                            .into_iter()
                            .map(|(name, value)| HeaderField::try_new(name, value).unwrap())
                            .collect(),
                    ),
                );
            }
            let snapshot = service.catalog().get(ExchangeId(id)).unwrap();
            let row = summarize(&snapshot, SystemTime::now(), false);
            assert_eq!(row.top_level_navigation, expected, "case {index}");
            assert_eq!(
                serde_json::to_value(row).unwrap()["topLevelNavigation"],
                expected
            );
        }
    }

    #[test]
    fn destination_multiselect_filters_before_paging_and_matches_search_selection() {
        use transmog_core::{HeaderBlock, HeaderField, observe::ExchangeBoundary};
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=50 {
            service.catalog().apply(started(id));
            let destination = match id {
                49 => Some("empty"),
                50 => None,
                id if id % 2 == 0 => Some("script"),
                _ => Some("image"),
            };
            let fields = destination
                .into_iter()
                .map(|value| HeaderField::try_new("Sec-Fetch-Dest", value).unwrap())
                .collect();
            observe_request(
                &service,
                id,
                2,
                ExchangeBoundary::ClientRequest,
                if id % 2 == 0 { "GET" } else { "POST" },
                HeaderBlock::from_fields(fields),
            );
        }
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let query = SessionQueryInput {
            fetch_destinations: vec!["image".into(), "script".into()],
            limit: Some(2),
            offset: Some(46),
            ..Default::default()
        };
        let page = query_sessions(&service, &registry, query.clone()).unwrap();
        assert_eq!(
            (page.total_matched, page.retained_count, page.sessions.len()),
            (48, 50, 2)
        );
        assert!(page.sessions.iter().all(|row| {
            query
                .fetch_destinations
                .contains(row.fetch_destination.as_ref().unwrap())
        }));
        let traces = crate::traces::TraceRegistry::default();
        let matching = [
            format!("{:032x}", 1),
            format!("{:032x}", 2),
            format!("{:032x}", 49),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            matching_ids(&service, &query, &traces, Some(&matching))
                .unwrap()
                .len(),
            2
        );
        let query = SessionQueryInput {
            fetch_destinations: vec!["image".into()],
            method: Some("POST".into()),
            ..Default::default()
        };
        assert_eq!(
            query_sessions(&service, &registry, query.clone())
                .unwrap()
                .total_matched,
            24
        );
        assert_eq!(
            matching_ids(&service, &query, &traces, None).unwrap().len(),
            24
        );
        let empty = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                fetch_destinations: vec!["empty".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(empty.total_matched, 1);
        assert_eq!(empty.sessions[0].id, format!("{:032x}", 49));
        for destinations in [vec!["unknown".into()], vec!["image".into(); 25]] {
            let query = SessionQueryInput {
                fetch_destinations: destinations,
                ..Default::default()
            };
            assert!(query_sessions(&service, &registry, query.clone()).is_err());
            assert!(matching_ids(&service, &query, &traces, None).is_err());
        }
    }

    #[test]
    fn sorting_and_filters_apply_before_bounded_pagination() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for (id, status) in [(1, 500), (2, 304), (3, 200), (4, 200)] {
            service.catalog().apply(started(id));
            service.catalog().apply(ObserverEvent {
                exchange_id: ExchangeId(id),
                sequence: 2,
                kind: ObserverEventKind::ResponseHeadObserved {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                    head: transmog_core::ResponseHead {
                        status,
                        headers: transmog_core::HeaderBlock::default(),
                        source_version: HttpLegVersion::Http1,
                    },
                },
            });
        }
        service.catalog().apply(started(5)); // Pending values sort last in both directions.
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let query = SessionQueryInput {
            limit: Some(2),
            sort: Some(SessionSort {
                column: TrafficColumn::Status,
                direction: SortDirection::Ascending,
            }),
            ..Default::default()
        };
        let first = query_sessions(&service, &registry, query.clone()).unwrap();
        assert_eq!(
            first
                .sessions
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            ["/3", "/4"]
        );
        assert_eq!((first.total_matched, first.retained_count), (5, 5));
        let second = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                offset: Some(2),
                ..query.clone()
            },
        )
        .unwrap();
        assert_eq!(
            second
                .sessions
                .iter()
                .map(|row| row.status)
                .collect::<Vec<_>>(),
            [Some(304), Some(500)]
        );
        let descending = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                limit: Some(200),
                sort: Some(SessionSort {
                    column: TrafficColumn::Status,
                    direction: SortDirection::Descending,
                }),
                ..query.clone()
            },
        )
        .unwrap();
        assert_eq!(descending.sessions.last().unwrap().status, None);
        let filtered = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                filters: vec![SessionColumnFilter {
                    column: TrafficColumn::Status,
                    operator: FilterOperator::Minimum,
                    value: "300".into(),
                }],
                ..query
            },
        )
        .unwrap();
        assert_eq!(filtered.total_matched, 2);
        assert_eq!(filtered.sessions[0].status, Some(304));
    }

    #[test]
    fn sorted_queries_enforce_page_and_numeric_filter_bounds() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=300 {
            service.catalog().apply(started(id));
        }
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let capped = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                limit: Some(usize::MAX),
                offset: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(capped.sessions.len(), service.catalog().page_size_limit());
        let invalid = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                filters: vec![SessionColumnFilter {
                    column: TrafficColumn::Status,
                    operator: FilterOperator::Minimum,
                    value: "three hundred".into(),
                }],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(invalid.category, ErrorCategory::InvalidInput);
    }

    #[test]
    fn request_order_sorts_and_filters_before_windowing_and_transports_exactly() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=100 {
            service.catalog().apply(started(id));
        }
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let query = SessionQueryInput {
            limit: Some(2),
            sort: Some(SessionSort {
                column: TrafficColumn::RequestNumber,
                direction: SortDirection::Ascending,
            }),
            filters: vec![SessionColumnFilter {
                column: TrafficColumn::RequestNumber,
                operator: FilterOperator::Minimum,
                value: "9".into(),
            }],
            ..Default::default()
        };
        let page = query_sessions(&service, &registry, query.clone()).unwrap();
        assert_eq!(page.total_matched, 92);
        assert_eq!(
            page.sessions
                .iter()
                .map(|row| row.request_number.as_str())
                .collect::<Vec<_>>(),
            ["9", "10"]
        );
        let descending = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                sort: Some(SessionSort {
                    column: TrafficColumn::RequestNumber,
                    direction: SortDirection::Descending,
                }),
                ..query
            },
        )
        .unwrap();
        assert_eq!(descending.sessions[0].request_number, "100");
        let mut snapshot = service
            .catalog()
            .get(transmog_core::intercept::ExchangeId(1))
            .unwrap();
        snapshot.request_number = u64::MAX;
        let row = summarize(&snapshot, SystemTime::now(), false);
        assert_eq!(
            serde_json::to_value(&row).unwrap()["requestNumber"],
            "18446744073709551615"
        );
        let exact = SessionColumnFilter {
            column: TrafficColumn::RequestNumber,
            operator: FilterOperator::Equals,
            value: u64::MAX.to_string(),
        };
        assert!(PreparedFilter::new(&exact).unwrap().matches(&row));
        for value in ["0", "-1", "1.5", "+1", "18446744073709551616", "abc"] {
            assert!(
                PreparedFilter::new(&SessionColumnFilter {
                    value: value.into(),
                    ..exact.clone()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn searches_all_retained_rows_instead_of_only_the_current_page() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=300 {
            service.catalog().apply(started(id));
        }
        let page = query_sessions(
            &service,
            &Arc::new(Mutex::new(CursorRegistry::default())),
            SessionQueryInput {
                limit: Some(2),
                search: Some("EXAMPLE.TEST/299".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.total_matched, 1);
        assert_eq!(page.retained_count, 300);
        assert_eq!(page.sessions[0].path, "/299");
    }

    #[test]
    fn focuses_a_retained_exchange_in_the_correct_sorted_page() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        for id in 1..=30 {
            service.catalog().apply(started(id));
        }
        let registry = Arc::new(Mutex::new(CursorRegistry::default()));
        let query = SessionQueryInput {
            limit: Some(10),
            focus_id: Some(format!("{:032x}", 3)),
            sort: Some(SessionSort {
                column: TrafficColumn::Url,
                direction: SortDirection::Ascending,
            }),
            ..Default::default()
        };
        let page = query_sessions(&service, &registry, query.clone()).unwrap();
        assert!(page.sessions.iter().any(|row| row.path == "/3"));
        assert_eq!(page.focus_offset, Some(20));
        assert_eq!(page.sessions.len(), 10);
        let missing = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                focus_id: Some(format!("{:032x}", 99)),
                ..query.clone()
            },
        )
        .unwrap_err();
        assert_eq!(missing.category, ErrorCategory::Unavailable);
        let filtered = query_sessions(
            &service,
            &registry,
            SessionQueryInput {
                search: Some("/29".into()),
                ..query
            },
        )
        .unwrap_err();
        assert_eq!(filtered.category, ErrorCategory::Unavailable);
    }

    #[test]
    fn caller_identity_is_structured_for_process_and_remote_rows() {
        assert_eq!(
            client_identity_view(&ClientIdentity::LocalProcess {
                pid: 42,
                name: Some("browser.exe".to_owned()),
            }),
            ClientIdentityView {
                kind: "local-process",
                process_name: Some("browser.exe".to_owned()),
                process_id: Some(42),
            }
        );
        assert_eq!(
            client_identity_view(&ClientIdentity::Remote),
            ClientIdentityView {
                kind: "remote",
                process_name: None,
                process_id: None,
            }
        );
    }
}
