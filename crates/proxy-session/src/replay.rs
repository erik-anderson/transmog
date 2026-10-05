use std::{future::Future, num::NonZeroUsize, pin::Pin, sync::Arc, time::Duration};

use bytes::Bytes;
use http::{Method, StatusCode, uri::PathAndQuery};
use thiserror::Error;
use tokio::time::timeout;
use transmog_core::{
    HeaderBlock, Target, intercept::ExchangeCancellation, route::UpstreamDestination,
};

/// Finite validation and execution limits for one replay request and response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayLimits {
    /// Maximum number of request or response header fields.
    pub max_header_fields: NonZeroUsize,
    /// Maximum aggregate request or response header name/value bytes.
    pub max_header_bytes: NonZeroUsize,
    /// Maximum complete request body bytes.
    pub max_request_body_bytes: usize,
    /// Maximum complete response body bytes returned by the executor.
    pub max_response_body_bytes: usize,
    /// Maximum executor duration.
    pub timeout: Duration,
}

impl Default for ReplayLimits {
    fn default() -> Self {
        Self {
            max_header_fields: NonZeroUsize::new(128).expect("constant is nonzero"),
            max_header_bytes: NonZeroUsize::new(64 * 1024).expect("constant is nonzero"),
            max_request_body_bytes: 4 * 1024 * 1024,
            max_response_body_bytes: 16 * 1024 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Explicit acknowledgement required for methods that may not be idempotent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplayRisk {
    /// Reject a method that is not known to be idempotent.
    #[default]
    RejectNonIdempotent,
    /// The caller explicitly accepts duplicate side-effect risk.
    AcknowledgeNonIdempotent,
}

/// How credential-bearing headers in an owned request are handled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplayCredentialPolicy {
    /// Reject authorization and cookie headers by default.
    #[default]
    Reject,
    /// Caller explicitly confirms these owned header values were supplied for replay.
    ExplicitlyProvided,
}

/// Caller-owned request proposed for replay/composer execution.
#[derive(Clone, Debug)]
pub struct ReplayRequest {
    /// HTTP method token.
    pub method: String,
    /// Complete normalized target.
    pub target: Target,
    /// Ordered duplicate-preserving fields.
    pub headers: HeaderBlock,
    /// Explicitly bounded complete body.
    pub body: Bytes,
    /// Side-effect acknowledgement.
    pub risk: ReplayRisk,
    /// Credential acknowledgement.
    pub credentials: ReplayCredentialPolicy,
}

/// Validated replay request accepted by an injected executor.
#[derive(Clone, Debug)]
pub struct ValidatedReplayRequest {
    method: String,
    target: Target,
    headers: HeaderBlock,
    body: Bytes,
}

impl ValidatedReplayRequest {
    /// Validated HTTP method token.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Validated normalized target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Validated ordered fields.
    pub fn headers(&self) -> &HeaderBlock {
        &self.headers
    }

    /// Validated complete request body.
    pub fn body(&self) -> &Bytes {
        &self.body
    }
}

/// Owned bounded replay response returned by an injected executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayResponse {
    /// Numeric HTTP status.
    pub status: u16,
    /// Ordered duplicate-preserving fields.
    pub headers: HeaderBlock,
    /// Complete response body.
    pub body: Bytes,
}

/// Redaction-safe executor implementation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("replay executor failed: {message}")]
pub struct ReplayExecutionError {
    /// Operator-safe description.
    pub message: String,
}

impl ReplayExecutionError {
    /// Creates an operator-safe executor error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Boxed replay executor result.
pub type BoxReplayFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ReplayResponse, ReplayExecutionError>> + Send + 'a>>;

/// Caller-supplied replay path, normally backed by the application's existing
/// routing and network stack.
pub trait ReplayExecutor: Send + Sync {
    /// Executes one fully owned and validated request.
    fn execute(
        &self,
        request: ValidatedReplayRequest,
        cancellation: ExchangeCancellation,
    ) -> BoxReplayFuture<'_>;
}

/// Replay validation or contained execution failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ReplayError {
    /// Execution timeout was zero.
    #[error("replay timeout must be nonzero")]
    InvalidTimeout,
    /// Method token was not valid HTTP syntax.
    #[error("replay method is invalid")]
    InvalidMethod,
    /// Scheme, authority, host, port, path, or query was not normalized.
    #[error("replay target is invalid or not normalized")]
    InvalidTarget,
    /// Method may have side effects and lacked explicit acknowledgement.
    #[error("non-idempotent replay requires explicit acknowledgement")]
    NonIdempotentNotAcknowledged,
    /// Credential-bearing fields lacked explicit acknowledgement.
    #[error("credential-bearing replay headers require explicit acknowledgement")]
    CredentialsNotAcknowledged,
    /// Header field count exceeded the configured limit.
    #[error("replay contains {actual} header fields, exceeding limit {limit}")]
    HeaderCountExceeded {
        /// Actual field count.
        actual: usize,
        /// Configured field count.
        limit: usize,
    },
    /// Aggregate header bytes exceeded the configured limit.
    #[error("replay headers contain {actual} bytes, exceeding limit {limit}")]
    HeaderBytesExceeded {
        /// Actual aggregate bytes.
        actual: usize,
        /// Configured byte count.
        limit: usize,
    },
    /// Request body exceeded the configured limit.
    #[error("replay request body contains {actual} bytes, exceeding limit {limit}")]
    RequestBodyExceeded {
        /// Actual bytes.
        actual: usize,
        /// Configured bytes.
        limit: usize,
    },
    /// Response status was invalid.
    #[error("replay executor returned an invalid response status")]
    InvalidResponseStatus,
    /// Response body exceeded the configured limit.
    #[error("replay response body contains {actual} bytes, exceeding limit {limit}")]
    ResponseBodyExceeded {
        /// Actual bytes.
        actual: usize,
        /// Configured bytes.
        limit: usize,
    },
    /// The caller cancelled execution.
    #[error("replay execution was cancelled")]
    Cancelled,
    /// The finite execution deadline elapsed.
    #[error("replay execution timed out")]
    TimedOut,
    /// The executor panicked or its task was otherwise lost.
    #[error("replay executor task failed")]
    ExecutorTaskFailed,
    /// The injected executor returned a redaction-safe failure.
    #[error(transparent)]
    Executor(#[from] ReplayExecutionError),
}

impl ReplayRequest {
    /// Validates ownership, safety acknowledgements, normalization, and bounds.
    ///
    /// # Errors
    ///
    /// Returns a typed syntax, acknowledgement, or finite-limit error.
    pub fn validate(self, limits: ReplayLimits) -> Result<ValidatedReplayRequest, ReplayError> {
        if limits.timeout.is_zero() {
            return Err(ReplayError::InvalidTimeout);
        }
        let method =
            Method::from_bytes(self.method.as_bytes()).map_err(|_| ReplayError::InvalidMethod)?;
        if !is_idempotent(&method) && self.risk != ReplayRisk::AcknowledgeNonIdempotent {
            return Err(ReplayError::NonIdempotentNotAcknowledged);
        }
        if self.credentials == ReplayCredentialPolicy::Reject
            && self.headers.iter().any(|field| {
                field.name_eq("authorization")
                    || field.name_eq("proxy-authorization")
                    || field.name_eq("cookie")
            })
        {
            return Err(ReplayError::CredentialsNotAcknowledged);
        }
        validate_target(&self.target)?;
        validate_headers(&self.headers, limits)?;
        if self.body.len() > limits.max_request_body_bytes {
            return Err(ReplayError::RequestBodyExceeded {
                actual: self.body.len(),
                limit: limits.max_request_body_bytes,
            });
        }
        Ok(ValidatedReplayRequest {
            method: self.method,
            target: self.target,
            headers: self.headers,
            body: self.body,
        })
    }
}

/// Validates and executes one replay behind cancellation, panic, timeout, and
/// response-bound containment.
///
/// # Errors
///
/// Returns a typed request-validation, execution, cancellation, timeout, or
/// response-validation error.
pub async fn execute_replay(
    executor: Arc<dyn ReplayExecutor>,
    request: ReplayRequest,
    limits: ReplayLimits,
    cancellation: &ExchangeCancellation,
) -> Result<ReplayResponse, ReplayError> {
    let request = request.validate(limits)?;
    let attempt_cancellation = ExchangeCancellation::new();
    let executor_cancellation = attempt_cancellation.clone();
    let mut task =
        tokio::spawn(async move { executor.execute(request, executor_cancellation).await });
    let result = tokio::select! {
        joined = timeout(limits.timeout, &mut task) => match joined {
            Ok(Ok(result)) => result.map_err(ReplayError::Executor),
            Ok(Err(_)) => Err(ReplayError::ExecutorTaskFailed),
            Err(_) => Err(ReplayError::TimedOut),
        },
        () = cancellation.cancelled() => Err(ReplayError::Cancelled),
    };
    if result.is_err() && !task.is_finished() {
        attempt_cancellation.cancel();
        task.abort();
        let _ = task.await;
    }
    let response = result?;
    StatusCode::from_u16(response.status).map_err(|_| ReplayError::InvalidResponseStatus)?;
    validate_headers(&response.headers, limits)?;
    if response.body.len() > limits.max_response_body_bytes {
        return Err(ReplayError::ResponseBodyExceeded {
            actual: response.body.len(),
            limit: limits.max_response_body_bytes,
        });
    }
    Ok(response)
}

fn validate_target(target: &Target) -> Result<(), ReplayError> {
    if !(target.scheme.eq_ignore_ascii_case("http") || target.scheme.eq_ignore_ascii_case("https"))
        || target.host.is_empty()
        || target.port == 0
        || !UpstreamDestination::from_target(target).matches_target(target)
    {
        return Err(ReplayError::InvalidTarget);
    }
    let combined = target.query.as_ref().map_or_else(
        || target.path.clone(),
        |query| format!("{}?{query}", target.path),
    );
    combined
        .parse::<PathAndQuery>()
        .map_err(|_| ReplayError::InvalidTarget)?;
    Ok(())
}

fn validate_headers(headers: &HeaderBlock, limits: ReplayLimits) -> Result<(), ReplayError> {
    if headers.fields().len() > limits.max_header_fields.get() {
        return Err(ReplayError::HeaderCountExceeded {
            actual: headers.fields().len(),
            limit: limits.max_header_fields.get(),
        });
    }
    let bytes = headers.iter().try_fold(0_usize, |total, field| {
        total
            .checked_add(field.name().len())?
            .checked_add(field.value().len())
    });
    let actual = bytes.unwrap_or(usize::MAX);
    if actual > limits.max_header_bytes.get() {
        return Err(ReplayError::HeaderBytesExceeded {
            actual,
            limit: limits.max_header_bytes.get(),
        });
    }
    Ok(())
}

fn is_idempotent(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use transmog_core::HeaderField;

    use super::*;

    struct Echo;

    impl ReplayExecutor for Echo {
        fn execute(
            &self,
            request: ValidatedReplayRequest,
            _cancellation: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            Box::pin(async move {
                Ok(ReplayResponse {
                    status: 200,
                    headers: request.headers,
                    body: request.body,
                })
            })
        }
    }

    struct Wait;

    impl ReplayExecutor for Wait {
        fn execute(
            &self,
            _request: ValidatedReplayRequest,
            _cancellation: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            Box::pin(pending())
        }
    }

    fn request(method: &str) -> ReplayRequest {
        ReplayRequest {
            method: method.into(),
            target: Target {
                scheme: "https".into(),
                authority: "example.test".into(),
                host: "example.test".into(),
                port: 443,
                path: "/path".into(),
                query: Some("q=1".into()),
            },
            headers: HeaderBlock::new(),
            body: Bytes::from_static(b"body"),
            risk: ReplayRisk::RejectNonIdempotent,
            credentials: ReplayCredentialPolicy::Reject,
        }
    }

    #[test]
    fn non_idempotent_credentials_and_authority_need_explicit_safe_input() {
        assert_eq!(
            request("POST")
                .validate(ReplayLimits::default())
                .unwrap_err(),
            ReplayError::NonIdempotentNotAcknowledged
        );
        let mut credentialed = request("GET");
        credentialed
            .headers
            .push(HeaderField::try_new("authorization", b"Bearer explicit".to_vec()).unwrap());
        assert_eq!(
            credentialed
                .clone()
                .validate(ReplayLimits::default())
                .unwrap_err(),
            ReplayError::CredentialsNotAcknowledged
        );
        credentialed.credentials = ReplayCredentialPolicy::ExplicitlyProvided;
        assert!(credentialed.validate(ReplayLimits::default()).is_ok());
        let mut malformed = request("GET");
        malformed.target.authority = "other.test".into();
        assert_eq!(
            malformed.validate(ReplayLimits::default()).unwrap_err(),
            ReplayError::InvalidTarget
        );
    }

    #[tokio::test]
    async fn executor_is_used_and_response_is_bounded() {
        let cancellation = ExchangeCancellation::new();
        let response = execute_replay(
            Arc::new(Echo),
            request("GET"),
            ReplayLimits::default(),
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(response.body, Bytes::from_static(b"body"));

        let limits = ReplayLimits {
            max_response_body_bytes: 3,
            ..ReplayLimits::default()
        };
        assert_eq!(
            execute_replay(Arc::new(Echo), request("GET"), limits, &cancellation)
                .await
                .unwrap_err(),
            ReplayError::ResponseBodyExceeded {
                actual: 4,
                limit: 3
            }
        );
    }

    #[tokio::test]
    async fn timeout_and_cancellation_abort_executor_task() {
        let timeout_limits = ReplayLimits {
            timeout: Duration::from_millis(1),
            ..ReplayLimits::default()
        };
        assert_eq!(
            execute_replay(
                Arc::new(Wait),
                request("GET"),
                timeout_limits,
                &ExchangeCancellation::new(),
            )
            .await
            .unwrap_err(),
            ReplayError::TimedOut
        );
        let cancellation = ExchangeCancellation::new();
        cancellation.cancel();
        assert_eq!(
            execute_replay(
                Arc::new(Wait),
                request("GET"),
                ReplayLimits::default(),
                &cancellation,
            )
            .await
            .unwrap_err(),
            ReplayError::Cancelled
        );
    }
}
