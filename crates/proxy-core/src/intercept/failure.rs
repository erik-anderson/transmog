use std::sync::Arc;

use thiserror::Error;

use crate::{RequestHead, ResponseHead};

use super::{ExchangeMetadata, HookAbort};

/// Hook or transport lifecycle stage associated with a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExchangeStage {
    /// Creating per-exchange interceptors.
    Initialize,
    /// Processing the request head.
    RequestHead,
    /// Selecting or processing the request body plan.
    RequestBody,
    /// Selecting a route or opening an upstream attempt.
    Upstream,
    /// Processing the response head.
    ResponseHead,
    /// Selecting or processing the response body plan.
    ResponseBody,
    /// Running terminal cleanup.
    Terminal,
}

/// Structured public category for one failed exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExchangeFailureKind {
    /// A required interceptor could not be created.
    HookInitialization,
    /// One hook exceeded its configured deadline.
    HookTimedOut,
    /// One hook panicked while unwinding was available.
    HookPanicked,
    /// A hook explicitly rejected or aborted the exchange.
    HookAborted(HookAbort),
    /// The client or embedding application cancelled the exchange.
    Cancelled,
    /// A body limit, framing rule, timeout, or transform failed.
    Body,
    /// Routing or destination authorization failed.
    Route,
    /// DNS, transport, TLS, or upstream HTTP failed.
    Upstream,
    /// The runtime is shutting down.
    Shutdown,
}

/// Terminal failure delivered to interceptors and observers.
#[derive(Clone, Debug)]
pub struct ExchangeFailure {
    /// Exchange metadata.
    pub metadata: Arc<ExchangeMetadata>,
    /// Stage that failed.
    pub stage: ExchangeStage,
    /// Stable failure category.
    pub kind: ExchangeFailureKind,
    /// Whether upstream request bytes may already have been committed.
    pub request_committed: bool,
    /// Whether downstream response bytes may already have been committed.
    pub response_committed: bool,
    /// Redacted operator-facing description.
    pub message: String,
}

/// Successful terminal outcome delivered after the response body completes.
#[derive(Clone, Debug)]
pub struct CompletedExchange {
    /// Exchange metadata.
    pub metadata: Arc<ExchangeMetadata>,
    /// Final effective request head.
    pub request_head: RequestHead,
    /// Final downstream response head.
    pub response_head: ResponseHead,
}

/// Failure creating a per-exchange interceptor.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("interceptor initialization failed: {message}")]
pub struct HookInitError {
    /// Redacted reason safe for operator diagnostics.
    pub message: String,
}

impl HookInitError {
    /// Creates a redacted initialization failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Failure around hook execution rather than an explicit hook action.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HookExecutionError {
    /// Callback exceeded its per-call deadline.
    #[error("hook callback timed out")]
    TimedOut,
    /// Exchange was cancelled before the callback completed.
    #[error("hook callback was cancelled")]
    Cancelled,
    /// Callback panicked and was contained.
    #[error("hook callback panicked")]
    Panicked,
    /// Global pause-permit pool is shutting down.
    #[error("hook runner is shutting down")]
    ShuttingDown,
}
