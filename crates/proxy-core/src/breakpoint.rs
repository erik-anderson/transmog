use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{sync::Semaphore, time::timeout};

use crate::{BodyFrame, CanonicalResponse, RequestHead, ResponseHead, SessionMetadata};

/// Phase at which an interception callback runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BreakpointPhase {
    /// Request metadata is complete and no upstream bytes have been sent.
    BeforeRequestHeaders,
    /// One request body frame is available.
    RequestBody,
    /// Response metadata is complete and no downstream bytes have been released.
    BeforeResponseHeaders,
    /// One response body frame is available.
    ResponseBody,
    /// Exchange completed successfully.
    Completed,
    /// Exchange failed or was aborted.
    Failed,
}

/// Validated state of one exchange's breakpoint sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BreakpointState {
    /// No event has fired yet.
    Start,
    /// Request head was accepted.
    Request,
    /// Response head was accepted.
    Response,
    /// Terminal success.
    Completed,
    /// Terminal failure.
    Failed,
}

impl BreakpointState {
    /// Advances the state machine or rejects an impossible phase ordering.
    ///
    /// # Errors
    ///
    /// Returns [`BreakpointTransitionError`] when `phase` is not legal from
    /// the current state.
    pub fn advance(self, phase: BreakpointPhase) -> Result<Self, BreakpointTransitionError> {
        use BreakpointPhase as P;
        use BreakpointState as S;
        match (self, phase) {
            (S::Start, P::BeforeRequestHeaders) | (S::Request, P::RequestBody) => Ok(S::Request),
            (S::Request, P::BeforeResponseHeaders) | (S::Response, P::ResponseBody) => {
                Ok(S::Response)
            }
            (S::Request | S::Response, P::Completed) => Ok(S::Completed),
            (S::Start | S::Request | S::Response, P::Failed) => Ok(S::Failed),
            _ => Err(BreakpointTransitionError { state: self, phase }),
        }
    }
}

/// Invalid breakpoint phase transition.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("breakpoint phase {phase:?} is invalid from state {state:?}")]
pub struct BreakpointTransitionError {
    /// Current state.
    pub state: BreakpointState,
    /// Attempted phase.
    pub phase: BreakpointPhase,
}

/// Information delivered to a handler. Only the relevant payload is populated.
#[derive(Clone, Debug)]
pub struct BreakpointEvent {
    /// Session/transport metadata.
    pub session: SessionMetadata,
    /// Event phase.
    pub phase: BreakpointPhase,
    /// Request head when known.
    pub request_head: Option<RequestHead>,
    /// Response head when known.
    pub response_head: Option<ResponseHead>,
    /// Current body frame for a body event.
    pub body_frame: Option<BodyFrame>,
}

/// Reason an interceptor intentionally terminated one exchange.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AbortReason {
    /// Human-readable policy reason safe for operator display.
    Policy(String),
    /// Handler declined the exchange.
    Rejected,
}

/// A low-level streaming transformer. Implementations see one stream only.
pub trait BodyTransform: Send {
    /// Processes a body frame. Returning an empty vector drops that frame.
    ///
    /// # Errors
    ///
    /// Returns [`AbortReason`] when the transformation elects to terminate
    /// the current exchange.
    fn transform(&mut self, frame: BodyFrame) -> Result<Vec<BodyFrame>, AbortReason>;

    /// Flushes buffered output at end-of-stream. Implementations must remain bounded.
    ///
    /// # Errors
    ///
    /// Returns [`AbortReason`] when buffered output cannot be completed.
    fn finish(&mut self) -> Result<Vec<BodyFrame>, AbortReason> {
        Ok(Vec::new())
    }
}

/// Decision returned by a breakpoint handler.
pub enum BreakpointDecision {
    /// Stream the original event unchanged.
    Continue,
    /// Replace the request head.
    ReplaceRequestHead(RequestHead),
    /// Replace the response head.
    ReplaceResponseHead(ResponseHead),
    /// Replace the complete body. Adapters must repair framing.
    ReplaceBody {
        /// Replacement bytes.
        data: Bytes,
        /// Optional replacement trailers.
        trailers: Option<crate::HeaderBlock>,
    },
    /// Install a bounded streaming transformation for this body.
    TransformBodyStream(Box<dyn BodyTransform>),
    /// Complete the exchange without contacting the origin.
    RespondLocally(CanonicalResponse),
    /// Abort only this stream/exchange where the protocol permits.
    Abort(AbortReason),
}

/// Boxed asynchronous callback result.
pub type BoxBreakpointFuture<'a> = Pin<Box<dyn Future<Output = BreakpointDecision> + Send + 'a>>;

/// Asynchronous interception callback.
pub trait BreakpointHandler: Send + Sync {
    /// Handles one event for one session/stream.
    fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_>;
}

/// Pass-through handler useful for running the proxy without active edits.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContinueHandler;

impl BreakpointHandler for ContinueHandler {
    fn on_breakpoint(&self, _event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
        Box::pin(async { BreakpointDecision::Continue })
    }
}

/// Hard limits applied independently to paused streams.
#[derive(Clone, Copy, Debug)]
pub struct BreakpointLimits {
    /// Maximum time a callback may hold a stream.
    pub timeout: Duration,
    /// Maximum number of concurrently paused streams.
    pub max_paused_streams: usize,
}

impl Default for BreakpointLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_paused_streams: 256,
        }
    }
}

/// Applies deadlines and a global paused-stream bound around a handler.
pub struct BreakpointRunner<H: ?Sized> {
    handler: Arc<H>,
    permits: Arc<Semaphore>,
    timeout: Duration,
}

impl<H: ?Sized> Clone for BreakpointRunner<H> {
    fn clone(&self) -> Self {
        Self {
            handler: Arc::clone(&self.handler),
            permits: Arc::clone(&self.permits),
            timeout: self.timeout,
        }
    }
}

impl<H: BreakpointHandler + ?Sized> BreakpointRunner<H> {
    /// Wraps a handler in independently acquired stream permits and timeouts.
    pub fn new(handler: Arc<H>, limits: BreakpointLimits) -> Self {
        Self {
            handler,
            permits: Arc::new(Semaphore::new(limits.max_paused_streams)),
            timeout: limits.timeout,
        }
    }

    /// Runs one event. Cancellation drops the permit and handler future.
    ///
    /// # Errors
    ///
    /// Returns [`BreakpointRunnerError`] when shutdown closes the permit pool
    /// or the callback exceeds its deadline.
    pub async fn run(
        &self,
        event: BreakpointEvent,
    ) -> Result<BreakpointDecision, BreakpointRunnerError> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| BreakpointRunnerError::ShuttingDown)?;
        let outcome = timeout(self.timeout, self.handler.on_breakpoint(event))
            .await
            .map_err(|_| BreakpointRunnerError::TimedOut);
        drop(permit);
        outcome
    }
}

/// Failure around callback execution rather than an explicit callback decision.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BreakpointRunnerError {
    /// Handler exceeded its per-stream deadline.
    #[error("breakpoint handler timed out")]
    TimedOut,
    /// Runner semaphore was closed during shutdown.
    #[error("breakpoint runner is shutting down")]
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::{ConnectionId, HttpLegVersion, SessionId, StreamId};

    struct Never;

    #[derive(Default)]
    struct FirstNeverThenContinue(AtomicUsize);

    impl BreakpointHandler for Never {
        fn on_breakpoint(&self, _event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(pending())
        }
    }

    impl BreakpointHandler for FirstNeverThenContinue {
        fn on_breakpoint(&self, _event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
            Box::pin(async move {
                if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    pending().await
                } else {
                    BreakpointDecision::Continue
                }
            })
        }
    }

    fn event() -> BreakpointEvent {
        BreakpointEvent {
            session: SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1".parse().unwrap(),
                proxy_addr: "127.0.0.1:2".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            phase: BreakpointPhase::BeforeRequestHeaders,
            request_head: None,
            response_head: None,
            body_frame: None,
        }
    }

    #[test]
    fn exhaustive_state_transitions_are_terminal_safe() {
        let phases = [
            BreakpointPhase::BeforeRequestHeaders,
            BreakpointPhase::RequestBody,
            BreakpointPhase::BeforeResponseHeaders,
            BreakpointPhase::ResponseBody,
            BreakpointPhase::Completed,
            BreakpointPhase::Failed,
        ];
        for terminal in [BreakpointState::Completed, BreakpointState::Failed] {
            for phase in phases {
                assert!(terminal.advance(phase).is_err());
            }
        }
    }

    #[tokio::test]
    async fn handler_timeout_is_typed() {
        let runner = BreakpointRunner::new(
            Arc::new(Never),
            BreakpointLimits {
                timeout: Duration::from_millis(5),
                max_paused_streams: 1,
            },
        );
        assert!(matches!(
            runner.run(event()).await,
            Err(BreakpointRunnerError::TimedOut)
        ));
    }

    #[tokio::test]
    async fn cancelling_one_callback_releases_its_pause_permit() {
        let handler = Arc::new(FirstNeverThenContinue::default());
        let runner = BreakpointRunner::new(
            handler.clone(),
            BreakpointLimits {
                timeout: Duration::from_secs(10),
                max_paused_streams: 1,
            },
        );
        let first_runner = runner.clone();
        let first = tokio::spawn(async move { first_runner.run(event()).await });
        while handler.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        first.abort();
        let _ = first.await;

        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), runner.run(event())).await,
            Ok(Ok(BreakpointDecision::Continue))
        ));
    }
}
