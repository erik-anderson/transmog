use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{BodyFrame, BodyStream, HeaderBlock, HttpLegVersion};

/// Normalized request target independent of H1 request-target and H2/H3 pseudo-headers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// URI scheme.
    pub scheme: String,
    /// Normalized authority, including a non-default port.
    pub authority: String,
    /// Host without brackets or port.
    pub host: String,
    /// Explicit destination port.
    pub port: u16,
    /// Origin-form path, beginning with `/`.
    pub path: String,
    /// Query without the leading `?`.
    pub query: Option<String>,
}

/// Protocol-neutral request metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestHead {
    /// Method token.
    pub method: String,
    /// Normalized target.
    pub target: Target,
    /// Ordered, duplicate-preserving regular fields.
    pub headers: HeaderBlock,
    /// Source HTTP version.
    pub source_version: HttpLegVersion,
}

/// Protocol-neutral response metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResponseHead {
    /// Three-digit HTTP status code.
    pub status: u16,
    /// Ordered, duplicate-preserving fields.
    pub headers: HeaderBlock,
    /// Source HTTP version.
    pub source_version: HttpLegVersion,
}

/// Fully buffered request used only by explicitly bounded helpers and synthetic tests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRequest {
    /// Request head.
    pub head: RequestHead,
    /// Body frames.
    pub body: Vec<BodyFrame>,
}

/// A locally generated or explicitly buffered response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResponse {
    /// Response head.
    pub head: ResponseHead,
    /// Body frames.
    pub body: Vec<BodyFrame>,
}

/// Protocol-neutral request whose body crosses a bounded asynchronous channel.
#[derive(Debug)]
pub struct StreamingRequest {
    /// Request head.
    pub head: RequestHead,
    /// Backpressured body frames.
    pub body: BodyStream,
}

/// Protocol-neutral response whose body crosses a bounded asynchronous channel.
#[derive(Debug)]
pub struct StreamingResponse {
    /// Response head.
    pub head: ResponseHead,
    /// Backpressured body frames.
    pub body: BodyStream,
}

impl CanonicalResponse {
    /// Creates a small local response without silently buffering caller data.
    pub fn local(status: u16, headers: HeaderBlock, body: Bytes) -> Self {
        Self {
            head: ResponseHead {
                status,
                headers,
                source_version: HttpLegVersion::Http1,
            },
            body: vec![BodyFrame::Data(body)],
        }
    }
}
