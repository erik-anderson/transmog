use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use http::Uri;
use serde::{Deserialize, Serialize};
use transmog_core::{
    BodyFrame, BodyStream, BodyStreamError, CanonicalRequest, HeaderBlock, HeaderField,
    HttpLegVersion, RequestHead, StreamingRequest, Target, intercept::ExchangeCancellation,
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

/// Source association preserved even when the replay draft is edited.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerOrigin {
    /// Workspace traffic entry from which the draft originated.
    pub entry_id: String,
    /// Original loaded trace, when known.
    pub trace_id: Option<String>,
    /// Friendly source trace filename, without its parent path.
    pub trace_name: Option<String>,
    /// Original trace-local entry identifier, when known.
    pub original_id: Option<String>,
}

/// A body that streams without entering the `WebView` text editor.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ComposerBodySource {
    /// Complete encoded bytes from the selected retained traffic entry.
    Captured {
        /// Workspace entry identifier.
        #[serde(rename = "entryId")]
        entry_id: String,
    },
    /// User-selected replacement file, read from one opened handle.
    File {
        /// Absolute local path explicitly supplied by the user.
        path: String,
    },
}

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
    /// Optional streamed body; mutually exclusive with inline body bytes.
    #[serde(default)]
    pub body_source: Option<ComposerBodySource>,
    /// Traffic entry that originally supplied the draft; retained after edits.
    #[serde(default)]
    pub source_entry_id: Option<String>,
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
    /// Original captured request association, if this draft came from traffic.
    pub source: Option<ComposerOrigin>,
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
    /// Original captured request association, retained after edits.
    pub source: Option<ComposerOrigin>,
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
        file: Option<(std::fs::File, u64)>,
        source: Option<ComposerOrigin>,
    ) -> Result<ComposerResult, AppError> {
        if input.body_source.is_some() != file.is_some() {
            return Err(AppError::new(
                ErrorCategory::Unavailable,
                "A complete streamed request body is required",
                true,
            ));
        }
        let executor = self.executor.clone().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "no route-aware replay executor is configured",
                false,
            )
        })?;
        let (mut request, target) = build_request(&input)?;
        let executor: Arc<dyn ReplayExecutor> = if let Some((file, length)) = file {
            request.headers.remove_all("content-length");
            request.headers.push(
                HeaderField::try_new("Content-Length", length.to_string()).expect("decimal length"),
            );
            Arc::new(FileReplayExecutor {
                inner: executor,
                file: Mutex::new(Some((file, length))),
            })
        } else {
            executor
        };
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
        let id = self.record(method, target, Some(response.status), source.clone());
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
            truncated: if body_is_hex {
                response.body.len() > 64 * 1024
            } else {
                std::str::from_utf8(&response.body)
                    .is_ok_and(|text| text.chars().count() > 64 * 1024)
            },
            attribution: "transmog.app.composer-replay",
            source,
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

    fn record(
        &self,
        method: String,
        target: String,
        status: Option<u16>,
        source: Option<ComposerOrigin>,
    ) -> u64 {
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
            source,
            attribution: "transmog.app.composer-replay",
        });
        history.items.truncate(MAX_HISTORY);
        id
    }
}

struct ReplayProducer(tokio::task::JoinHandle<()>, ExchangeCancellation);
impl Drop for ReplayProducer {
    fn drop(&mut self) {
        self.1.cancel();
        self.0.abort();
    }
}

struct FileReplayExecutor {
    inner: Arc<dyn ReplayExecutor>,
    file: Mutex<Option<(std::fs::File, u64)>>,
}
impl ReplayExecutor for FileReplayExecutor {
    fn execute_file(
        &self,
        request: ValidatedReplayRequest,
        file: std::fs::File,
        length: u64,
        cancellation: ExchangeCancellation,
    ) -> BoxReplayFuture<'_> {
        self.inner.execute_file(request, file, length, cancellation)
    }
    fn execute(
        &self,
        request: ValidatedReplayRequest,
        cancellation: ExchangeCancellation,
    ) -> BoxReplayFuture<'_> {
        let file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match file {
            Some((file, length)) => self.inner.execute_file(request, file, length, cancellation),
            None => Box::pin(async {
                Err(ReplayExecutionError::new(
                    "replay file was already consumed",
                ))
            }),
        }
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
    fn execute_file(
        &self,
        request: ValidatedReplayRequest,
        file: std::fs::File,
        length: u64,
        cancellation: ExchangeCancellation,
    ) -> BoxReplayFuture<'_> {
        let client = self.client.clone();
        let mode = self.mode;
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(ReplayExecutionError::new("replay cancelled"));
            }
            if file
                .metadata()
                .map_err(|_| ReplayExecutionError::new("replay body metadata unavailable"))?
                .len()
                != length
            {
                return Err(ReplayExecutionError::new(
                    "replay body changed before sending",
                ));
            }
            let head = RequestHead {
                method: request.method().into(),
                target: request.target().clone(),
                headers: request.headers().clone(),
                source_version: HttpLegVersion::Http1,
            };
            let (sender, body) = BodyStream::channel(NonZeroUsize::new(2).expect("nonzero"));
            let producer_cancel = cancellation.clone();
            let producer = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut file = tokio::fs::File::from_std(file);
                let mut sent = 0_u64;
                loop {
                    let mut bytes = vec![0; 64 * 1024];
                    let read = tokio::select! {()=producer_cancel.cancelled()=>return,read=file.read(&mut bytes)=>read};
                    let frame = match read {
                        Ok(0) if sent == length => return,
                        Ok(count) if count > 0 && sent.saturating_add(count as u64) <= length => {
                            sent += count as u64;
                            bytes.truncate(count);
                            Ok(BodyFrame::Data(Bytes::from(bytes)))
                        }
                        _ => Err(BodyStreamError::Failed(
                            "Replay body could not be read completely or changed during sending"
                                .into(),
                        )),
                    };
                    let failed = frame.is_err();
                    if tokio::select! {()=producer_cancel.cancelled()=>true,result=sender.send(frame)=>result.is_err()}
                        || failed
                    {
                        return;
                    }
                }
            });
            let _producer = ReplayProducer(producer, cancellation);
            let mut response = client
                .execute_streaming(
                    StreamingRequest { head, body },
                    mode,
                    ReplayLimits::default().max_response_body_bytes,
                    NonZeroUsize::new(2).expect("nonzero"),
                    Duration::from_secs(10),
                )
                .await
                .map_err(|_| ReplayExecutionError::new("canonical upstream file replay failed"))?;
            let mut bytes = Vec::new();
            while let Some(frame) = response.body.recv().await {
                if let BodyFrame::Data(data) =
                    frame.map_err(|_| ReplayExecutionError::new("replay response body failed"))?
                {
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

pub(crate) fn validate_stream_input(input: &ComposerRequest) -> Result<(), AppError> {
    build_request(input)?
        .0
        .validate(ReplayLimits::default())
        .map(|_| ())
        .map_err(|error| AppError::new(ErrorCategory::InvalidInput, error.to_string(), false))
}

fn build_request(input: &ComposerRequest) -> Result<(ReplayRequest, String), AppError> {
    if input.body_source.is_some() && !input.body.is_empty() {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Choose either a streamed body or an inline body",
            false,
        ));
    }
    if input.url.chars().count() > 8 * 1024 || input.body.chars().count() > MAX_BODY_INPUT_CHARS {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "composer URL or body exceeds its input limit",
            false,
        ));
    }
    let (target, label) = composer_target(&input.url)?;
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
    // The draft owns complete bytes; never reuse captured chunk framing or a
    // Content-Length that predates the user's edits. Preserve content coding.
    headers.remove_all("content-length");
    headers.remove_all("transfer-encoding");
    headers.push(
        HeaderField::try_new("Content-Length", body.len().to_string()).expect("decimal length"),
    );
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
        label,
    ))
}

fn composer_target(value: &str) -> Result<(Target, String), AppError> {
    let uri: Uri = value.parse().map_err(|_| {
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
    let host = authority.host().trim_matches(['[', ']']).to_ascii_lowercase();
    let normalized_host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let normalized_authority = if (scheme == "https" && port == 443) || (scheme == "http" && port == 80) {
        normalized_host
    } else {
        format!("{normalized_host}:{port}")
    };
    let target = Target {
        scheme: scheme.to_owned(),
        authority: normalized_authority.clone(),
        host,
        port,
        path: path.to_owned(),
        query: query.map(str::to_owned),
    };
    Ok((target, format!("{scheme}://{normalized_authority}{path_and_query}")))
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
        fn execute_file(
            &self,
            _request: ValidatedReplayRequest,
            _file: std::fs::File,
            _length: u64,
            _cancellation: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            Box::pin(async {
                Err(ReplayExecutionError::new(
                    "this test executor does not support file bodies",
                ))
            })
        }
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
            body_source: None,
            source_entry_id: None,
            body_is_hex: false,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        assert!(
            manager
                .execute(&service, request.clone(), None, None)
                .await
                .is_err()
        );
        request.acknowledge_non_idempotent = true;
        assert!(
            manager
                .execute(&service, request.clone(), None, None)
                .await
                .is_err()
        );
        request.acknowledge_credentials = true;
        let result = manager
            .execute(&service, request, None, None)
            .await
            .unwrap();
        assert_eq!(result.status, 201);
        assert_eq!(result.body, "hello");
        assert_eq!(manager.history().len(), 1);
    }

    #[tokio::test]
    async fn file_replay_streams_large_binary_body_and_preserves_source_after_edits() {
        use std::io::{Seek, Write};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let length = 8 * 1024 * 1024 + 193;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(socket.read_u8().await.unwrap());
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("POST /edited HTTP/1.1"));
            let declared = head
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            assert_eq!(declared, length);
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer explicit")
            );
            let mut read = 0;
            let mut chunk = vec![0; 32768];
            while read < length {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                for (index, byte) in chunk[..count].iter().enumerate() {
                    assert_eq!(*byte, u8::try_from((read + index) % 251).unwrap());
                }
                read += count;
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let mut file = tempfile::tempfile().unwrap();
        let chunk = (0..65536)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect::<Vec<_>>();
        // Write the position-dependent pattern without retaining the whole body.
        for offset in (0..length).step_by(chunk.len()) {
            let chunk = (offset..(offset + 65536).min(length))
                .map(|index| u8::try_from(index % 251).unwrap())
                .collect::<Vec<_>>();
            file.write_all(&chunk).unwrap();
        }
        file.rewind().unwrap();
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let manager = ComposerManager::new(Some(Arc::new(
            SystemReplayExecutor::new(ProxyRoute::Http1).unwrap(),
        )));
        let origin = ComposerOrigin {
            entry_id: "00000000000000000000000000000001".into(),
            trace_id: Some("trace-original".into()),
            trace_name: Some("original.tmcap".into()),
            original_id: Some("17".into()),
        };
        let input = ComposerRequest {
            method: "POST".into(),
            url: format!("http://{address}/edited"),
            headers: vec![ComposerHeader {
                name: "Authorization".into(),
                value: "Bearer explicit".into(),
            }],
            body: String::new(),
            body_is_hex: false,
            body_source: Some(ComposerBodySource::Captured {
                entry_id: origin.entry_id.clone(),
            }),
            source_entry_id: Some(origin.entry_id.clone()),
            acknowledge_non_idempotent: true,
            acknowledge_credentials: true,
        };
        let result = manager
            .execute(
                &service,
                input,
                Some((file, u64::try_from(length).unwrap())),
                Some(origin.clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.body, "ok");
        assert_eq!(result.source, Some(origin.clone()));
        assert_eq!(manager.history()[0].source, Some(origin));
        assert!(manager.history()[0].target.ends_with("/edited"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unsupported_executor_rejects_file_body_instead_of_sending_empty_bytes() {
        use std::io::{Seek, Write};
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"body").unwrap();
        file.rewind().unwrap();
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let manager = ComposerManager::new(Some(Arc::new(EchoExecutor)));
        let input = ComposerRequest {
            method: "PUT".into(),
            url: "https://example.invalid/".into(),
            headers: vec![],
            body: String::new(),
            body_is_hex: false,
            body_source: Some(ComposerBodySource::File {
                path: "unused".into(),
            }),
            source_entry_id: None,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        let error = manager
            .execute(&service, input, Some((file, 4)), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not support file bodies"));
        assert!(manager.history().is_empty());
    }

    #[test]
    fn explicit_default_ports_are_normalized_before_replay_validation() {
        for (url, authority, host) in [
            ("https://EXAMPLE.invalid:443/resource", "example.invalid", "example.invalid"),
            ("http://example.invalid:80/resource", "example.invalid", "example.invalid"),
            ("https://[::1]:443/resource", "[::1]", "::1"),
            ("https://example.invalid:8443/resource", "example.invalid:8443", "example.invalid"),
        ] {
            let (target, _) = composer_target(url).unwrap();
            assert_eq!(target.authority, authority);
            assert_eq!(target.host, host);
            assert!(transmog_core::route::UpstreamDestination::from_target(&target).matches_target(&target));
            ReplayRequest { method: "GET".into(), target, headers: HeaderBlock::default(), body: Bytes::new(), risk: ReplayRisk::RejectNonIdempotent, credentials: ReplayCredentialPolicy::Reject }.validate(ReplayLimits::default()).unwrap();
        }
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
            body_source: None,
            source_entry_id: None,
            body_is_hex: false,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        assert!(build_request(&input).is_err());
    }

    #[test]
    fn edited_encoded_request_repairs_framing_and_preserves_content_coding() {
        let input = ComposerRequest {
            method: "POST".into(),
            url: "https://example.invalid/".into(),
            headers: vec![
                ComposerHeader {
                    name: "Content-Length".into(),
                    value: "999".into(),
                },
                ComposerHeader {
                    name: "Transfer-Encoding".into(),
                    value: "chunked".into(),
                },
                ComposerHeader {
                    name: "Content-Encoding".into(),
                    value: "gzip".into(),
                },
            ],
            body: "00 ff 3c".into(),
            body_source: None,
            source_entry_id: None,
            body_is_hex: true,
            acknowledge_non_idempotent: true,
            acknowledge_credentials: false,
        };
        let (request, _) = build_request(&input).unwrap();
        assert_eq!(request.body.as_ref(), [0, 255, 60]);
        assert_eq!(
            request.headers.values("content-length").collect::<Vec<_>>(),
            [b"3".as_slice()]
        );
        assert!(request.headers.values("transfer-encoding").next().is_none());
        assert_eq!(
            request.headers.values("content-encoding").next(),
            Some(b"gzip".as_slice())
        );
    }
}
