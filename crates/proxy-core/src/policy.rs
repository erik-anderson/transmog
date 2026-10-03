use serde::{Deserialize, Serialize};

use crate::HttpLegVersion;

/// Per-route egress selection policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RoutePolicy {
    /// Prefer a validated HTTP/3 alternative and safely fall back as allowed.
    #[default]
    Auto,
    /// Require HTTP/1.1.
    Http1Only,
    /// Require HTTP/2.
    Http2Only,
    /// Require HTTP/3 and never fall back.
    Http3Only,
}

/// Whether a request can be sent again without duplicating side effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Replayability {
    /// GET or HEAD with no streaming side effects.
    SafeMethod,
    /// Caller opted in and retained a bounded copy of the complete body.
    ExplicitlyBuffered,
    /// Streaming or otherwise unsafe to repeat.
    NotReplayable,
}

/// Outcome of evaluating a protocol failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FallbackDecision {
    /// Try the selected lower protocol.
    Retry(HttpLegVersion),
    /// Surface the original error.
    Fail,
}

impl RoutePolicy {
    /// Determines whether a failed attempt may fall back before response bytes are exposed.
    pub fn fallback_after(
        self,
        failed: HttpLegVersion,
        replayability: Replayability,
        response_started: bool,
    ) -> FallbackDecision {
        if self != Self::Auto || response_started || replayability == Replayability::NotReplayable {
            return FallbackDecision::Fail;
        }
        match failed {
            HttpLegVersion::Http3 => FallbackDecision::Retry(HttpLegVersion::Http2),
            HttpLegVersion::Http2 => FallbackDecision::Retry(HttpLegVersion::Http1),
            HttpLegVersion::Http1 => FallbackDecision::Fail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FallbackDecision, Replayability, RoutePolicy};
    use crate::HttpLegVersion;

    #[test]
    fn only_modes_never_fall_back() {
        for policy in [
            RoutePolicy::Http1Only,
            RoutePolicy::Http2Only,
            RoutePolicy::Http3Only,
        ] {
            assert_eq!(
                policy.fallback_after(HttpLegVersion::Http3, Replayability::SafeMethod, false),
                FallbackDecision::Fail
            );
        }
    }

    #[test]
    fn auto_never_replays_after_response_started() {
        assert_eq!(
            RoutePolicy::Auto.fallback_after(
                HttpLegVersion::Http3,
                Replayability::SafeMethod,
                true
            ),
            FallbackDecision::Fail
        );
    }

    #[test]
    fn auto_retries_only_replayable_requests_before_a_response() {
        for replayability in [Replayability::SafeMethod, Replayability::ExplicitlyBuffered] {
            assert_eq!(
                RoutePolicy::Auto.fallback_after(HttpLegVersion::Http3, replayability, false),
                FallbackDecision::Retry(HttpLegVersion::Http2)
            );
        }
        assert_eq!(
            RoutePolicy::Auto.fallback_after(
                HttpLegVersion::Http3,
                Replayability::NotReplayable,
                false
            ),
            FallbackDecision::Fail
        );
    }
}
