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
    SessionSnapshot, SessionTerminal, SubscriptionEvent,
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
    /// Client-facing HTTP protocol.
    pub protocol: String,
    /// Last response status, if observed.
    pub status: Option<u16>,
    /// Milliseconds from start to terminal observation or the page snapshot.
    pub duration_ms: u64,
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

pub(crate) fn query_sessions(
    service: &ApplicationSessionService,
    registry: &Arc<Mutex<CursorRegistry>>,
    input: SessionQueryInput,
) -> Result<SessionPage, AppError> {
    validate_filter(input.method.as_deref())?;
    validate_filter(input.host.as_deref())?;
    if input.sort.is_some()
        || input.search.is_some()
        || !input.filters.is_empty()
        || input.offset.is_some()
        || input.focus_id.is_some()
    {
        return query_sorted(service, &input);
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
        .map(|snapshot| summarize(snapshot, now, capturing))
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
    let target = snapshot.metadata.original_target.as_target();
    let request = snapshot.request_heads.last().map(|head| &head.head);
    let end = snapshot.terminal_at.unwrap_or(now);
    let duration_ms = u64::try_from(
        end.duration_since(snapshot.metadata.started_at)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
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
    let terminal = match snapshot.terminal {
        Some(SessionTerminal::Completed(_)) => "completed",
        Some(SessionTerminal::Failed(_)) => "failed",
        None => "active",
    };
    SessionSummary {
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
        content_type: snapshot
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
            }),
        id: format!("{:032x}", snapshot.exchange_id.0),
        caller: client_identity_view(&snapshot.metadata.client_identity),
        method: request.map_or_else(|| "—".to_owned(), |head| head.method.clone()),
        host: target.host.clone(),
        path: target.query.as_ref().map_or_else(
            || target.path.clone(),
            |query| format!("{}?{query}", target.path),
        ),
        protocol: format!("{:?}", snapshot.metadata.ingress_version),
        status: snapshot.response_heads.last().map(|head| head.head.status),
        duration_ms,
        request_bytes,
        response_bytes,
        terminal,
        loss: snapshot.sequence_loss > 0,
        capturing,
        auto_response: auto_response_match(snapshot),
    }
}

fn numeric_value(row: &SessionSummary, column: TrafficColumn) -> Option<u64> {
    match column {
        TrafficColumn::Status => row.status.map(u64::from),
        TrafficColumn::Pid => row.caller.process_id.map(u64::from),
        TrafficColumn::Duration => Some(row.duration_ms),
        TrafficColumn::ResponseBytes => Some(row.response_bytes),
        TrafficColumn::RequestBytes => Some(row.request_bytes),
        TrafficColumn::StartedAt => Some(row.started_at),
        _ => None,
    }
}

fn numeric_column(column: TrafficColumn) -> bool {
    matches!(
        column,
        TrafficColumn::Status
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

fn query_sorted(
    service: &ApplicationSessionService,
    input: &SessionQueryInput,
) -> Result<SessionPage, AppError> {
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
    let limit = input
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(service.catalog().page_size_limit());
    if limit == 0 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "page size must be nonzero",
            false,
        ));
    }
    let capturing = matches!(
        service.capture().status(),
        transmog_session::CaptureStatus::Active { .. }
    );
    let now = SystemTime::now();
    let rows = service
        .catalog()
        .project_retained(|snapshot| summarize(snapshot, now, capturing));
    let retained_count = rows.len();
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            input
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
                && search
                    .as_deref()
                    .is_none_or(|search| matches_search(row, search))
                && filters.iter().all(|filter| filter.matches(row))
        })
        .collect();
    if let Some(sort) = &input.sort {
        sort_rows(&mut rows, sort);
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
        || row
            .caller
            .process_id
            .is_some_and(|pid| pid.to_string().contains(search))
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
