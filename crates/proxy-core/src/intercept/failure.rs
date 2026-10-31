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
    /// Canonical request translation or framing repair failed.
    RequestTranslation,
    /// Canonical response translation or framing repair failed.
    ResponseTranslation,
    /// Routing or destination authorization failed.
    Route,
    /// DNS, transport, TLS, or upstream HTTP failed.
    Upstream,
    /// The runtime is shutting down.
    Shutdown,
}

/// Stable failure names shared by script events and native capture evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExchangeFailureCategory {
    /// An interceptor could not be created.
    HookInitialization,
    /// A hook exceeded its deadline.
    HookTimedOut,
    /// A hook panicked.
    HookPanicked,
    /// A hook aborted with a policy reason.
    #[serde(rename = "HookAborted(Policy)")]
    HookAbortedPolicy,
    /// A hook rejected the exchange.
    #[serde(rename = "HookAborted(Rejected)")]
    HookAbortedRejected,
    /// The client or application cancelled the exchange.
    Cancelled,
    /// Body processing failed.
    Body,
    /// Request translation failed.
    RequestTranslation,
    /// Response translation failed.
    ResponseTranslation,
    /// Routing or destination authorization failed.
    Route,
    /// The upstream connection or protocol failed.
    Upstream,
    /// The runtime is shutting down.
    Shutdown,
}

impl ExchangeFailureCategory {
    /// All supported failure categories.
    pub const ALL: &[Self] = &[
        Self::HookInitialization,
        Self::HookTimedOut,
        Self::HookPanicked,
        Self::HookAbortedPolicy,
        Self::HookAbortedRejected,
        Self::Cancelled,
        Self::Body,
        Self::RequestTranslation,
        Self::ResponseTranslation,
        Self::Route,
        Self::Upstream,
        Self::Shutdown,
    ];
    /// Stable case-sensitive wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HookInitialization => "HookInitialization",
            Self::HookTimedOut => "HookTimedOut",
            Self::HookPanicked => "HookPanicked",
            Self::HookAbortedPolicy => "HookAborted(Policy)",
            Self::HookAbortedRejected => "HookAborted(Rejected)",
            Self::Cancelled => "Cancelled",
            Self::Body => "Body",
            Self::RequestTranslation => "RequestTranslation",
            Self::ResponseTranslation => "ResponseTranslation",
            Self::Route => "Route",
            Self::Upstream => "Upstream",
            Self::Shutdown => "Shutdown",
        }
    }
}

impl ExchangeFailureKind {
    /// Stable capture category, independent of operator-facing explanations.
    #[must_use]
    pub const fn category(&self) -> &'static str {
        self.category_kind().as_str()
    }

    /// Typed category without the failure's diagnostic text.
    #[must_use]
    pub const fn category_kind(&self) -> ExchangeFailureCategory {
        match self {
            Self::HookInitialization => ExchangeFailureCategory::HookInitialization,
            Self::HookTimedOut => ExchangeFailureCategory::HookTimedOut,
            Self::HookPanicked => ExchangeFailureCategory::HookPanicked,
            Self::HookAborted(HookAbort::Policy(_)) => ExchangeFailureCategory::HookAbortedPolicy,
            Self::HookAborted(HookAbort::Rejected) => ExchangeFailureCategory::HookAbortedRejected,
            Self::Cancelled => ExchangeFailureCategory::Cancelled,
            Self::Body => ExchangeFailureCategory::Body,
            Self::RequestTranslation => ExchangeFailureCategory::RequestTranslation,
            Self::ResponseTranslation => ExchangeFailureCategory::ResponseTranslation,
            Self::Route => ExchangeFailureCategory::Route,
            Self::Upstream => ExchangeFailureCategory::Upstream,
            Self::Shutdown => ExchangeFailureCategory::Shutdown,
        }
    }
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
