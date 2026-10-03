//! Application-owned and network upstream service boundary.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::time::timeout;

use crate::{
    StreamingRequest, StreamingResponse, intercept::ExchangeCancellation, route::UpstreamPlan,
    task::AbortOnDrop,
};

/// Boxed asynchronous upstream execution result.
pub type BoxUpstreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<StreamingResponse, UpstreamError>> + Send + 'a>>;

/// Stable upstream failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamErrorKind {
    /// DNS resolution failed.
    Dns,
    /// TCP or UDP connection failed.
    Connect,
    /// TLS authentication or handshake failed.
    Tls,
    /// HTTP protocol processing failed.
    Http,
    /// Application-owned upstream failed.
    Application,
    /// Upstream execution exceeded its deadline.
    TimedOut,
    /// Exchange was cancelled.
    Cancelled,
    /// Upstream implementation panicked and was contained.
    Panicked,
}

/// Structured, redacted upstream failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("upstream {kind:?} failure: {message}")]
pub struct UpstreamError {
    /// Stable failure category.
    pub kind: UpstreamErrorKind,
    /// Whether upstream request bytes may have been committed.
    pub request_committed: bool,
    /// Whether response bytes may have been observed.
    pub response_started: bool,
    /// Redacted operator-facing reason.
    pub message: String,
}

impl UpstreamError {
    /// Creates a pre-commit application-owned upstream failure.
    pub fn application(message: impl Into<String>) -> Self {
        Self {
            kind: UpstreamErrorKind::Application,
            request_committed: false,
            response_started: false,
            message: message.into(),
        }
    }
}

/// Protocol-neutral network or application-owned upstream.
pub trait UpstreamService: Send + Sync {
    /// Executes one canonical streaming request under an immutable route plan.
    fn execute(
        &self,
        request: StreamingRequest,
        plan: UpstreamPlan,
        cancellation: ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_>;
}

/// Timeout, cancellation, and panic boundary around an upstream service.
#[derive(Clone)]
pub struct UpstreamExecutor {
    service: Arc<dyn UpstreamService>,
    timeout: Duration,
}

impl std::fmt::Debug for UpstreamExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UpstreamExecutor")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl UpstreamExecutor {
    /// Creates an executor with a finite attempt deadline.
    pub fn new(service: Arc<dyn UpstreamService>, timeout: Duration) -> Self {
        Self { service, timeout }
    }

    /// Executes one attempt while containing timeout, cancellation, and panic.
    ///
    /// # Errors
    ///
    /// Returns a structured upstream failure.
    pub async fn execute(
        &self,
        request: StreamingRequest,
        plan: UpstreamPlan,
        cancellation: &ExchangeCancellation,
    ) -> Result<StreamingResponse, UpstreamError> {
        let service = Arc::clone(&self.service);
        let owned_cancellation = cancellation.clone();
        let mut task = AbortOnDrop::new(tokio::spawn(async move {
            service.execute(request, plan, owned_cancellation).await
        }));
        let result = tokio::select! {
            joined = timeout(self.timeout, task.handle()) => match joined {
                Ok(Ok(result)) => result,
                Ok(Err(error)) if error.is_panic() => Err(boundary_error(UpstreamErrorKind::Panicked, "upstream service panicked")),
                Ok(Err(_)) => Err(boundary_error(UpstreamErrorKind::Cancelled, "upstream task was cancelled")),
                Err(_) => Err(boundary_error(UpstreamErrorKind::TimedOut, "upstream attempt timed out")),
            },
            () = cancellation.cancelled() => Err(boundary_error(UpstreamErrorKind::Cancelled, "exchange was cancelled")),
        };
        if result.is_err() && !task.is_finished() {
            task.abort_and_wait().await;
        }
        result
    }
}

fn boundary_error(kind: UpstreamErrorKind, message: &str) -> UpstreamError {
    UpstreamError {
        kind,
        request_committed: false,
        response_started: false,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, num::NonZeroUsize};

    use bytes::Bytes;

    use crate::{
        BodyFrame, BodyStream, HeaderBlock, HttpLegVersion, Replayability, RequestHead,
        ResponseHead, RoutePolicy, Target,
        route::{UpstreamDestination, UpstreamPoolKey},
    };

    use super::*;

    struct InProcess;

    impl UpstreamService for InProcess {
        fn execute(
            &self,
            mut request: StreamingRequest,
            _plan: UpstreamPlan,
            _cancellation: ExchangeCancellation,
        ) -> BoxUpstreamFuture<'_> {
            Box::pin(async move {
                let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
                tokio::spawn(async move {
                    while let Some(frame) = request.body.recv().await {
                        sender.send(frame).await.unwrap();
                    }
                });
                Ok(StreamingResponse {
                    head: ResponseHead {
                        status: 200,
                        headers: HeaderBlock::new(),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                })
            })
        }
    }

    enum Failing {
        Wait,
        Panic,
    }

    impl UpstreamService for Failing {
        fn execute(
            &self,
            _request: StreamingRequest,
            _plan: UpstreamPlan,
            _cancellation: ExchangeCancellation,
        ) -> BoxUpstreamFuture<'_> {
            Box::pin(async move {
                match self {
                    Self::Wait => pending().await,
                    Self::Panic => panic!("upstream panic"),
                }
            })
        }
    }

    fn request() -> (crate::BodyStreamSender, StreamingRequest) {
        let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        (
            sender,
            StreamingRequest {
                head: RequestHead {
                    method: "POST".to_owned(),
                    target: Target {
                        scheme: "https".to_owned(),
                        authority: "example.test".to_owned(),
                        host: "example.test".to_owned(),
                        port: 443,
                        path: "/".to_owned(),
                        query: None,
                    },
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http2,
                },
                body,
            },
        )
    }

    fn plan() -> UpstreamPlan {
        UpstreamPlan {
            pool_key: UpstreamPoolKey {
                destination: UpstreamDestination {
                    scheme: "https".to_owned(),
                    host: "example.test".to_owned(),
                    port: 443,
                },
                version_policy: RoutePolicy::Auto,
                trust_generation: 1,
                tls_policy_id: "default".into(),
                connector_policy_id: "direct".into(),
            },
            route_id: "default".into(),
            reason: "test".into(),
            replayability: Replayability::NotReplayable,
        }
    }

    #[tokio::test]
    async fn in_process_service_preserves_streaming_backpressure_contract() {
        let cancellation = ExchangeCancellation::new();
        let executor = UpstreamExecutor::new(Arc::new(InProcess), Duration::from_secs(1));
        let (sender, request) = request();
        let mut response = executor
            .execute(request, plan(), &cancellation)
            .await
            .unwrap();
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"hello"))))
            .await
            .unwrap();
        assert_eq!(
            response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"hello"))
        );
    }

    #[tokio::test]
    async fn timeout_panic_and_cancellation_are_typed() {
        let cancellation = ExchangeCancellation::new();
        let (_, timed_request) = request();
        let error = UpstreamExecutor::new(Arc::new(Failing::Wait), Duration::from_millis(5))
            .execute(timed_request, plan(), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind, UpstreamErrorKind::TimedOut);

        let (_, panic_request) = request();
        let error = UpstreamExecutor::new(Arc::new(Failing::Panic), Duration::from_secs(1))
            .execute(panic_request, plan(), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind, UpstreamErrorKind::Panicked);

        cancellation.cancel();
        let (_, cancelled_request) = request();
        let error = UpstreamExecutor::new(Arc::new(Failing::Wait), Duration::from_secs(1))
            .execute(cancelled_request, plan(), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind, UpstreamErrorKind::Cancelled);
    }
}
