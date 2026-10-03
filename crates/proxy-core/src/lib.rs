//! Protocol-neutral data model, breakpoint engine, and translation policy.
//!
//! Stable APIs in this crate deliberately do not expose Hyper or quiche types.

mod body;
mod breakpoint;
mod header;
mod message;
mod policy;
mod protocol;
mod session;
mod translation;

pub use body::{
    BodyChannelClosed, BodyFrame, BodyLimitError, BodyStream, BodyStreamError, BodyStreamSender,
    BoundedBodyBuffer,
};
pub use breakpoint::{
    AbortReason, BodyTransform, BoxBreakpointFuture, BreakpointDecision, BreakpointEvent,
    BreakpointHandler, BreakpointLimits, BreakpointPhase, BreakpointRunner, BreakpointRunnerError,
    BreakpointState, BreakpointTransitionError, ContinueHandler,
};
pub use header::{HeaderBlock, HeaderError, HeaderField};
pub use message::{
    CanonicalRequest, CanonicalResponse, RequestHead, ResponseHead, StreamingRequest,
    StreamingResponse, Target,
};
pub use policy::{FallbackDecision, Replayability, RoutePolicy};
pub use protocol::{HttpLegVersion, TlsSummary, VerificationResult};
pub use session::{ConnectionId, SessionId, SessionMetadata, StreamId};
pub use translation::{
    BodySemantics, MessageKind, TranslationError, TranslationOptions, body_semantics,
    prepare_headers,
};
