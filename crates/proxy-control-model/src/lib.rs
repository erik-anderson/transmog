#![deny(missing_docs)]

//! Owned DTOs for the experimental, same-build rustymiddle control protocol.
//!
//! These types deliberately do not serialize core structs. Version zero is
//! breakable until a human maintainer explicitly decides to stabilize it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Experimental protocol revision supported by this build.
pub const EXPERIMENTAL_CONTROL_REVISION: u32 = 0;

/// Stable exchange identifier in control messages.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ControlExchangeId(pub u128);

/// Monotonic decision identifier local to one control connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct DecisionId(pub u64);

/// Features understood by one control peer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Request-head pause and modification.
    RequestHead,
    /// Request-body replacement within negotiated bounds.
    RequestBody,
    /// Response-head pause and modification.
    ResponseHead,
    /// Response-body replacement within negotiated bounds.
    ResponseBody,
    /// Attributed hook-effect events.
    HookEffects,
    /// Full body observation events.
    FullBodyObservation,
}

/// First message exchanged by same-build peers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    /// Experimental protocol revision.
    pub revision: u32,
    /// Opaque build identity; v0 peers require an exact match.
    pub build_id: String,
    /// Supported optional features.
    pub capabilities: BTreeSet<Capability>,
    /// Largest body edit accepted in one command.
    pub max_body_edit_bytes: usize,
}

impl Handshake {
    /// Creates a same-build v0 handshake.
    pub fn v0(build_id: impl Into<String>, max_body_edit_bytes: usize) -> Self {
        Self {
            revision: EXPERIMENTAL_CONTROL_REVISION,
            build_id: build_id.into(),
            capabilities: BTreeSet::new(),
            max_body_edit_bytes,
        }
    }

    /// Validates fields before transport negotiation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty build identity or zero body limit.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.build_id.trim().is_empty() {
            return Err(ModelError::EmptyBuildId);
        }
        if self.max_body_edit_bytes == 0 {
            return Err(ModelError::ZeroBodyEditLimit);
        }
        Ok(())
    }
}

/// Protocol-neutral request/response boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Boundary {
    /// Request received from the client.
    ClientRequest,
    /// Request committed toward the upstream.
    UpstreamRequest,
    /// Response received from the upstream.
    UpstreamResponse,
    /// Response committed toward the client.
    ClientResponse,
}

/// Breakpoint phase at which a controller decision is requested.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreakpointPhase {
    /// Request head before routing.
    RequestHead,
    /// Complete bounded request body.
    RequestBody,
    /// Response head before downstream commitment.
    ResponseHead,
    /// Complete bounded response body.
    ResponseBody,
}

/// One duplicate-preserving header field owned by the control model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderField {
    /// Lowercase or original header name as supplied by the producer.
    pub name: String,
    /// Opaque header value bytes.
    pub value: Vec<u8>,
}

/// Editable request head detached from core types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestHead {
    /// HTTP method token.
    pub method: String,
    /// Normalized scheme.
    pub scheme: String,
    /// Normalized authority.
    pub authority: String,
    /// Path and optional query.
    pub path_and_query: String,
    /// Ordered, duplicate-preserving fields.
    pub headers: Vec<HeaderField>,
}

/// Editable response head detached from core types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseHead {
    /// Numeric HTTP status.
    pub status: u16,
    /// Ordered, duplicate-preserving fields.
    pub headers: Vec<HeaderField>,
}

/// Controller-facing event payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EventKind {
    /// Exchange lifecycle began.
    ExchangeStarted,
    /// A head crossed a logical boundary.
    HeadObserved {
        /// Boundary where the head was observed.
        boundary: Boundary,
    },
    /// A body segment crossed a logical boundary.
    BodyObserved {
        /// Boundary where bytes were observed.
        boundary: Boundary,
        /// Segment byte count; bytes travel through a separately bounded path.
        byte_count: usize,
    },
    /// A hook produced an attributed traffic effect.
    HookEffect {
        /// Stable hook identity.
        hook_id: String,
        /// Redacted action category.
        action: String,
    },
    /// Exchange completed successfully.
    Completed,
    /// Exchange failed.
    Failed {
        /// Stable, redacted failure category.
        category: String,
    },
}

/// Sequenced event delivered independently of breakpoint decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEvent {
    /// Monotonic connection-local event sequence.
    pub sequence: u64,
    /// Associated exchange.
    pub exchange_id: ControlExchangeId,
    /// Typed event payload.
    pub kind: EventKind,
}

/// Uncorrelated input used to request a bounded breakpoint decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BreakpointInput {
    /// Exchange to pause.
    pub exchange_id: ControlExchangeId,
    /// Hook phase being paused.
    pub phase: BreakpointPhase,
    /// Optional editable request head.
    pub request_head: Option<RequestHead>,
    /// Optional editable response head.
    pub response_head: Option<ResponseHead>,
    /// Optional bounded complete body.
    pub body: Option<Vec<u8>>,
}

/// One bounded breakpoint request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointRequest {
    /// Correlation identity assigned by the transport.
    pub decision_id: DecisionId,
    /// Exchange waiting on the decision.
    pub exchange_id: ControlExchangeId,
    /// Hook phase being paused.
    pub phase: BreakpointPhase,
    /// Optional editable request head.
    pub request_head: Option<RequestHead>,
    /// Optional editable response head.
    pub response_head: Option<ResponseHead>,
    /// Optional bounded complete body.
    pub body: Option<Vec<u8>>,
}

/// Decision returned by the controller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum DecisionAction {
    /// Continue without changing traffic.
    Continue,
    /// Replace the request head.
    ReplaceRequestHead {
        /// Complete replacement head.
        head: RequestHead,
    },
    /// Replace the response head.
    ReplaceResponseHead {
        /// Complete replacement head.
        head: ResponseHead,
    },
    /// Replace the complete body within the negotiated bound.
    ReplaceBody {
        /// Complete replacement body.
        body: Vec<u8>,
    },
    /// Abort the exchange with an operator-safe reason.
    Abort {
        /// Operator-safe reason.
        reason: String,
    },
}

/// Correlated controller reply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionCommand {
    /// Decision being answered.
    pub decision_id: DecisionId,
    /// Exchange being answered.
    pub exchange_id: ControlExchangeId,
    /// Selected action.
    pub action: DecisionAction,
}

impl DecisionCommand {
    /// Validates negotiated finite limits.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a reserved ID or oversized body edit.
    pub fn validate(&self, max_body_edit_bytes: usize) -> Result<(), ModelError> {
        if self.decision_id.0 == 0 {
            return Err(ModelError::ZeroDecisionId);
        }
        if let DecisionAction::ReplaceBody { body } = &self.action
            && body.len() > max_body_edit_bytes
        {
            return Err(ModelError::BodyEditTooLarge {
                actual: body.len(),
                limit: max_body_edit_bytes,
            });
        }
        Ok(())
    }
}

/// Invalid control-model value.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ModelError {
    /// Build identity was blank.
    #[error("control build identity must not be empty")]
    EmptyBuildId,
    /// Body-edit limit was zero.
    #[error("control body-edit limit must be nonzero")]
    ZeroBodyEditLimit,
    /// Decision ID zero is reserved.
    #[error("control decision id must be nonzero")]
    ZeroDecisionId,
    /// Body replacement exceeded the negotiated bound.
    #[error("body edit contains {actual} bytes, exceeding limit {limit}")]
    BodyEditTooLarge {
        /// Actual replacement size.
        actual: usize,
        /// Negotiated size limit.
        limit: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_and_body_edit_validation_are_strict() {
        assert_eq!(
            Handshake::v0(" ", 1).validate(),
            Err(ModelError::EmptyBuildId)
        );
        assert_eq!(
            Handshake::v0("build", 0).validate(),
            Err(ModelError::ZeroBodyEditLimit)
        );
        let command = DecisionCommand {
            decision_id: DecisionId(1),
            exchange_id: ControlExchangeId(2),
            action: DecisionAction::ReplaceBody { body: vec![0; 3] },
        };
        assert_eq!(
            command.validate(2),
            Err(ModelError::BodyEditTooLarge {
                actual: 3,
                limit: 2
            })
        );
    }

    #[test]
    fn serialized_model_rejects_unknown_fields() {
        let json = r#"{"revision":0,"build_id":"build","capabilities":[],"max_body_edit_bytes":4,"extra":true}"#;
        assert!(serde_json::from_str::<Handshake>(json).is_err());
    }
}
