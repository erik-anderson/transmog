use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use http::Uri;
use serde::{Deserialize, Serialize};
use transmog_core::{
    BodyFrame, CanonicalRequest, HeaderBlock, HeaderField, HttpLegVersion, RequestHead, Target,
    intercept::ExchangeCancellation,
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_session::{
    ApplicationSessionService, BoxReplayFuture, ReplayCredentialPolicy, ReplayExecutionError,
    ReplayExecutor, ReplayLimits, ReplayRequest, ReplayResponse, ReplayRisk,
    ValidatedReplayRequest,
};
use transmog_tls::{
    SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

use crate::{AppError, ErrorCategory, ProxyRoute};

const MAX_HISTORY: usize = 100;
const MAX_BODY_INPUT_CHARS: usize = 8 * 1024 * 1024;

/// One duplicate-preserving composer header.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerHeader {
    /// HTTP field name.
    pub name: String,
    /// Text field value.
    pub value: String,
}

/// Structured request composer input.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerRequest {
    /// HTTP method token.
    pub method: String,
    /// Absolute HTTP or HTTPS URL.
    pub url: String,
    /// Ordered duplicate-preserving headers.
    #[serde(default)]
    pub headers: Vec<ComposerHeader>,
    /// UTF-8 request body or whitespace-tolerant hexadecimal bytes.
    #[serde(default)]
    pub body: String,
    /// Whether `body` is hexadecimal rather than UTF-8.
    #[serde(default)]
    pub body_is_hex: bool,
    /// Explicit side-effect acknowledgement for non-idempotent methods.
    #[serde(default)]
    pub acknowledge_non_idempotent: bool,
    /// Explicit acknowledgement for supplied credentials.
    #[serde(default)]
    pub acknowledge_credentials: bool,
}

/// Redaction-safe history entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerSnapshot {
    /// Monotonic local execution identifier.
    pub id: u64,
    /// Request method.
    pub method: String,
    /// Normalized target without user information.
    pub target: String,
    /// Terminal HTTP status, if execution completed.
    pub status: Option<u16>,
    /// Stable attribution for audit/display.
    pub attribution: &'static str,
}

/// Bounded composer response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerResult {
    /// History identifier.
    pub id: u64,
    /// Response status.
    pub status: u16,
    /// Ordered response headers rendered safely.
    pub headers: Vec<ComposerHeader>,
    /// Response body as text or hexadecimal.
    pub body: String,
    /// Whether `body` is hexadecimal.
    pub body_is_hex: bool,
    /// Whether the body hit the response bound.
    pub truncated: bool,
    /// Stable replay attribution.
    pub attribution: &'static str,
}

struct History {
    next_id: u64,
    items: VecDeque<ComposerSnapshot>,
}

#[derive(Clone)]
pub(crate) struct ComposerManager {
    executor: Option<Arc<dyn ReplayExecutor>>,
    history: Arc<Mutex<History>>,
}

impl ComposerManager {
    pub(crate) fn new(executor: Option<Arc<dyn ReplayExecutor>>) -> Self {
        Self {
            executor,
            history: Arc::new(Mutex::new(History {
                next_id: 0,
                items: VecDeque::new(),
            })),
        }
    }

    pub(crate) async fn execute(
        &self,
        service: &ApplicationSessionService,
        input: ComposerRequest,
    ) -> Result<ComposerResult, AppError> {
        let executor = self.executor.clone().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "no route-aware replay executor is configured",
                false,
            )
        })?;
        let (request, target) = build_request(&input)?;
        let method = request.method.clone();
        let cancellation = ExchangeCancellation::new();
        let response = service
            .replay(executor, request, ReplayLimits::default(), &cancellation)
            .await
            .map_err(|error| {
                let category = match error {
                    transmog_session::ReplayError::NonIdempotentNotAcknowledged
                    | transmog_session::ReplayError::CredentialsNotAcknowledged
                    | transmog_session::ReplayError::InvalidMethod
                    | transmog_session::ReplayError::InvalidTarget
                    | transmog_session::ReplayError::InvalidTimeout => ErrorCategory::InvalidInput,
                    transmog_session::ReplayError::RequestBodyExceeded { .. }
                    | transmog_session::ReplayError::ResponseBodyExceeded { .. }
                    | transmog_session::ReplayError::HeaderBytesExceeded { .. }
                    | transmog_session::ReplayError::HeaderCountExceeded { .. } => {
                        ErrorCategory::Limit
                    }
                    _ => ErrorCategory::Unavailable,
                };
                AppError::new(
                    category,
                    error.to_string(),
                    category == ErrorCategory::Unavailable,
                )
            })?;
        let id = self.record(method, target, Some(response.status));
        let headers = response
            .headers
            .iter()
            .take(128)
            .map(|field| ComposerHeader {
                name: safe_bytes(field.name()).0,
                value: safe_bytes(field.value()).0,
            })
            .collect();
        let (body, body_is_hex) = safe_bytes(&response.body);
        Ok(ComposerResult {
            id,
            status: response.status,
            headers,
            body,
            body_is_hex,
            truncated: false,
            attribution: "transmog.app.composer-replay",
        })
    }

    pub(crate) fn history(&self) -> Vec<ComposerSnapshot> {
        self.history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .iter()
            .cloned()
            .collect()
    }

    fn record(&self, method: String, target: String, status: Option<u16>) -> u64 {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        history.next_id = history.next_id.saturating_add(1);
        let id = history.next_id;
        history.items.push_front(ComposerSnapshot {
            id,
            method,
            target,
            status,
            attribution: "transmog.app.composer-replay",
        });
        history.items.truncate(MAX_HISTORY);
        id
    }
}

/// Canonical TLS-verifying HTTP/1.1 and HTTP/2 replay executor.
#[derive(Clone)]
pub struct SystemReplayExecutor {
    client: HyperOriginClient,
    mode: HyperEgressMode,
}

impl SystemReplayExecutor {
    /// Builds a replay executor from a snapshot of operating-system trust.
    ///
    /// # Errors
    /// Returns a bounded trust or TLS client construction failure.
    pub fn new(route: ProxyRoute) -> Result<Self, AppError> {
        let mode = match route {
            ProxyRoute::Auto => HyperEgressMode::Auto,
            ProxyRoute::Http1 => HyperEgressMode::Http1Only,
            ProxyRoute::Http2 => HyperEgressMode::Http2Only,
            ProxyRoute::Http3 => {
                return Err(AppError::new(
                    ErrorCategory::InvalidInput,
                    "the canonical Hyper replay executor does not support HTTP/3-only routing",
                    false,
                ));
            }
        };
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "operating-system trust roots could not be loaded for replay",
                true,
            )
        })?);
        let factory = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let client = HyperOriginClient::new(&factory).map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "canonical replay TLS client could not be created",
                true,
            )
        })?;
        Ok(Self { client, mode })
    }
}

impl ReplayExecutor for SystemReplayExecutor {
    fn execute(
        &self,
        request: ValidatedReplayRequest,
        cancellation: ExchangeCancellation,
    ) -> BoxReplayFuture<'_> {
        let client = self.client.clone();
        let mode = self.mode;
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(ReplayExecutionError::new("replay cancelled"));
            }
            let body = if request.body().is_empty() {
                Vec::new()
            } else {
                vec![BodyFrame::Data(request.body().clone())]
            };
            let response = client
                .execute(
                    CanonicalRequest {
                        head: RequestHead {
                            method: request.method().to_owned(),
                            target: request.target().clone(),
                            headers: request.headers().clone(),
                            source_version: HttpLegVersion::Http1,
                        },
                        body,
                    },
                    mode,
                    ReplayLimits::default().max_response_body_bytes,
                    Duration::from_secs(10),
                )
                .await
                .map_err(|_| ReplayExecutionError::new("canonical upstream replay failed"))?;
            let mut bytes = Vec::new();
            for frame in response.body {
                if let BodyFrame::Data(data) = frame {
                    bytes.extend_from_slice(&data);
                }
            }
            Ok(ReplayResponse {
                status: response.head.status,
                headers: response.head.headers,
                body: Bytes::from(bytes),
            })
        })
    }
}

fn build_request(input: &ComposerRequest) -> Result<(ReplayRequest, String), AppError> {
    if input.url.chars().count() > 8 * 1024 || input.body.chars().count() > MAX_BODY_INPUT_CHARS {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "composer URL or body exceeds its input limit",
            false,
        ));
    }
    let uri: Uri = input.url.parse().map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "composer URL must be an absolute HTTP or HTTPS URL",
            false,
        )
    })?;
    let scheme = uri.scheme_str().unwrap_or_default();
    if !matches!(scheme, "http" | "https") {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "composer URL must use HTTP or HTTPS",
            false,
        ));
    }
    let authority = uri.authority().ok_or_else(|| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "composer URL must include an authority",
            false,
        )
    })?;
    if authority.as_str().contains('@') {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "composer URL must not contain user information",
            false,
        ));
    }
    let port = authority
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let path_and_query = uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    let target = Target {
        scheme: scheme.to_owned(),
        authority: authority.as_str().to_owned(),
        host: authority.host().to_owned(),
        port,
        path: path.to_owned(),
        query: query.map(str::to_owned),
    };
    let mut headers = HeaderBlock::new();
    for header in &input.headers {
        headers.push(
            HeaderField::try_new(header.name.as_bytes(), header.value.as_bytes()).map_err(
                |_| {
                    AppError::new(
                        ErrorCategory::InvalidInput,
                        "composer contains an invalid header",
                        false,
                    )
                },
            )?,
        );
    }
    let body = if input.body_is_hex {
        parse_hex(&input.body)?
    } else {
        input.body.as_bytes().to_vec()
    };
    Ok((
        ReplayRequest {
            method: input.method.clone(),
            target,
            headers,
            body: Bytes::from(body),
            risk: if input.acknowledge_non_idempotent {
                ReplayRisk::AcknowledgeNonIdempotent
            } else {
                ReplayRisk::RejectNonIdempotent
            },
            credentials: if input.acknowledge_credentials {
                ReplayCredentialPolicy::ExplicitlyProvided
            } else {
                ReplayCredentialPolicy::Reject
            },
        },
        format!("{scheme}://{authority}{path_and_query}"),
    ))
}

fn parse_hex(value: &str) -> Result<Vec<u8>, AppError> {
    let compact = value
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.len() % 2 != 0 || !compact.iter().all(u8::is_ascii_hexdigit) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "hexadecimal body is invalid",
            false,
        ));
    }
    compact
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|value| u8::from_str_radix(value, 16).ok())
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::InvalidInput,
                        "hexadecimal body is invalid",
                        false,
                    )
                })
        })
        .collect()
}

fn safe_bytes(bytes: &[u8]) -> (String, bool) {
    match std::str::from_utf8(bytes) {
        Ok(text)
            if text.chars().all(|character| {
                !character.is_control() || matches!(character, '\r' | '\n' | '\t')
            }) =>
        {
            (text.chars().take(64 * 1024).collect(), false)
        }
        _ => (
            bytes
                .iter()
                .take(64 * 1024)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoExecutor;

    impl ReplayExecutor for EchoExecutor {
        fn execute(
            &self,
            request: ValidatedReplayRequest,
            _cancellation: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            let body = request.body().clone();
            Box::pin(async move {
                Ok(ReplayResponse {
                    status: 201,
                    headers: HeaderBlock::new(),
                    body,
                })
            })
        }
    }

    #[tokio::test]
    async fn composer_requires_explicit_risk_and_credentials_acknowledgements() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let manager = ComposerManager::new(Some(Arc::new(EchoExecutor)));
        let mut request = ComposerRequest {
            method: "POST".to_owned(),
            url: "https://example.test/path".to_owned(),
            headers: vec![ComposerHeader {
                name: "authorization".to_owned(),
                value: "secret".to_owned(),
            }],
            body: "hello".to_owned(),
            body_is_hex: false,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        assert!(manager.execute(&service, request.clone()).await.is_err());
        request.acknowledge_non_idempotent = true;
        assert!(manager.execute(&service, request.clone()).await.is_err());
        request.acknowledge_credentials = true;
        let result = manager.execute(&service, request).await.unwrap();
        assert_eq!(result.status, 201);
        assert_eq!(result.body, "hello");
        assert_eq!(manager.history().len(), 1);
    }

    #[test]
    fn target_and_hex_validation_are_deterministic() {
        assert_eq!(parse_hex("00 ff 3c").unwrap(), [0, 255, 60]);
        assert!(parse_hex("0").is_err());
        let input = ComposerRequest {
            method: "GET".to_owned(),
            url: "relative".to_owned(),
            headers: vec![],
            body: String::new(),
            body_is_hex: false,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        assert!(build_request(&input).is_err());
    }
}
