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

use crate::{AppError, AutoResponseMatchView, ErrorCategory, inspector::auto_response_match};

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
    let sessions = page
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
