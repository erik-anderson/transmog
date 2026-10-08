#![deny(missing_docs)]

//! Protocol-neutral data model, interception engine, and translation policy.
//!
//! Stable APIs in this crate deliberately do not expose Hyper or quiche types.

mod body;
mod extensions;
mod header;
pub mod intercept;
mod message;
/// Bounded, immutable, redacted lifecycle observation.
pub mod observe;
/// Measured local timings and physical transport facts.
pub mod performance;
mod policy;
mod protocol;
pub mod route;
mod session;
mod task;
mod translation;
pub mod upstream;

pub use body::{
    BodyChannelClosed, BodyFrame, BodyLimitError, BodyStream, BodyStreamError, BodyStreamSender,
    BoundedBodyBuffer,
};
pub use extensions::ExchangeExtensions;
pub use header::{HeaderBlock, HeaderError, HeaderField};
pub use message::{
    CanonicalRequest, CanonicalResponse, LocalStreamingResponse, RequestHead, ResponseHead,
    StreamingRequest, StreamingResponse, Target,
};
pub use policy::{FallbackDecision, Replayability, RoutePolicy};
pub use protocol::{HttpLegVersion, TlsSummary, VerificationResult};
pub use session::{ClientIdentity, ConnectionId, SessionId, SessionMetadata, StreamId};
pub use translation::{
    BodySemantics, MessageKind, TranslationError, TranslationOptions, body_semantics,
    prepare_headers,
};
