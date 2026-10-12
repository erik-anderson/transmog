#![deny(missing_docs)]

//! Protocol-neutral data model, interception engine, and translation policy.
//!
//! Stable APIs in this crate deliberately do not expose Hyper or quiche types.

mod body;
/// Explicit authorization policies for server-requested TLS client certificates.
pub mod client_authentication;
/// Outbound connection partition and protocol admission policies.
pub mod connection_policy;
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
pub mod stream;
mod task;
mod tls_evidence;
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
pub use tls_evidence::{PublicCertificate, TlsEvidence};
pub use translation::{
    BodySemantics, MessageKind, TranslationError, TranslationOptions, body_semantics,
    prepare_headers,
};
