use crate::{CanonicalResponse, LocalStreamingResponse, RequestHead, ResponseHead, Target};

use super::{BodyPlan, BodyPlanSelection, BodyRepresentation, HookAbort, HookContext};

/// Typed request-head callback input.
#[derive(Clone, Debug)]
pub struct RequestHeadEvent {
    /// Exchange-local context.
    pub context: HookContext,
    /// Current effective request head.
    pub head: RequestHead,
}

/// Typed request-body planning input.
#[derive(Clone, Debug)]
pub struct RequestBodyEvent {
    /// Exchange-local context.
    pub context: HookContext,
    /// Final effective request head.
    pub head: RequestHead,
}

/// Typed response-head callback input.
#[derive(Clone, Debug)]
pub struct ResponseHeadEvent {
    /// Exchange-local context.
    pub context: HookContext,
    /// Final effective request head.
    pub request_head: RequestHead,
    /// Current effective response head.
    pub head: ResponseHead,
    /// Whether the response was generated locally.
    pub local_response: bool,
}

/// Typed response-body planning input.
#[derive(Clone, Debug)]
pub struct ResponseBodyEvent {
    /// Exchange-local context.
    pub context: HookContext,
    /// Final effective request head.
    pub request_head: RequestHead,
    /// Final effective response head.
    pub response_head: ResponseHead,
    /// Whether the response was generated locally.
    pub local_response: bool,
}

/// Legal decision at the request-head phase.
#[must_use = "request-head actions must be returned to the exchange engine"]
pub enum RequestHeadAction {
    /// Continue with the supplied head.
    Continue,
    /// Continue with a replacement logical head without changing the socket destination.
    Replace(RequestHead),
    /// Explicitly select a new logical target for later route authorization.
    Reroute {
        /// Replacement request head.
        head: RequestHead,
        /// Explicit new target.
        target: Target,
    },
    /// Complete without contacting an upstream.
    Respond(CanonicalResponse),
    /// Complete without contacting an upstream and stream the local body.
    RespondStreaming(LocalStreamingResponse),
    /// Abort the exchange.
    Abort(HookAbort),
}

impl std::fmt::Debug for RequestHeadAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Continue => formatter.write_str("Continue"),
            Self::Replace(head) => formatter.debug_tuple("Replace").field(head).finish(),
            Self::Reroute { head, target } => formatter
                .debug_struct("Reroute")
                .field("head", head)
                .field("target", target)
                .finish(),
            Self::Respond(response) => formatter.debug_tuple("Respond").field(response).finish(),
            Self::RespondStreaming(response) => formatter
                .debug_tuple("RespondStreaming")
                .field(response)
                .finish(),
            Self::Abort(reason) => formatter.debug_tuple("Abort").field(reason).finish(),
        }
    }
}

/// Legal decision at the response-head phase.
#[must_use = "response-head actions must be returned to the exchange engine"]
pub enum ResponseHeadAction {
    /// Continue with the supplied head.
    Continue,
    /// Continue with a replacement response head.
    Replace(ResponseHead),
    /// Replace the uncommitted response with a bounded local response.
    Respond(CanonicalResponse),
    /// Abort before downstream commitment.
    Abort(HookAbort),
}

impl std::fmt::Debug for ResponseHeadAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Continue => formatter.write_str("Continue"),
            Self::Replace(head) => formatter.debug_tuple("Replace").field(head).finish(),
            Self::Respond(response) => formatter.debug_tuple("Respond").field(response).finish(),
            Self::Abort(reason) => formatter.debug_tuple("Abort").field(reason).finish(),
        }
    }
}

/// Request-body plan selected before its pump starts.
#[derive(Debug)]
#[must_use = "request body plans must be returned to the exchange engine"]
pub struct RequestBodyAction(pub(crate) BodyPlanSelection);

impl RequestBodyAction {
    /// Selects a plan with an explicit byte representation.
    pub const fn new(plan: BodyPlan, representation: BodyRepresentation) -> Self {
        Self(BodyPlanSelection::new(plan, representation))
    }

    /// Selects exact pass-through without requesting content processing.
    pub const fn pass_through() -> Self {
        Self(BodyPlanSelection::pass_through())
    }

    /// Selects a plan that deliberately consumes and produces coded bytes.
    pub const fn raw(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::Raw)
    }

    /// Selects a plan that requires decoded identity bytes.
    pub const fn decoded(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::DecodedRequired)
    }

    /// Selects an optional decoded plan that is omitted for unsupported coding.
    pub const fn decoded_if_supported(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::DecodedIfSupported)
    }
}

/// Response-body plan selected before its pump starts.
#[derive(Debug)]
#[must_use = "response body plans must be returned to the exchange engine"]
pub struct ResponseBodyAction(pub(crate) BodyPlanSelection);

impl ResponseBodyAction {
    /// Selects a plan with an explicit byte representation.
    pub const fn new(plan: BodyPlan, representation: BodyRepresentation) -> Self {
        Self(BodyPlanSelection::new(plan, representation))
    }

    /// Selects exact pass-through without requesting content processing.
    pub const fn pass_through() -> Self {
        Self(BodyPlanSelection::pass_through())
    }

    /// Selects a plan that deliberately consumes and produces coded bytes.
    pub const fn raw(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::Raw)
    }

    /// Selects a plan that requires decoded identity bytes.
    pub const fn decoded(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::DecodedRequired)
    }

    /// Selects an optional decoded plan that is omitted for unsupported coding.
    pub const fn decoded_if_supported(plan: BodyPlan) -> Self {
        Self::new(plan, BodyRepresentation::DecodedIfSupported)
    }
}
