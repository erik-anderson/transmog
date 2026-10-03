//! Protocol-neutral upstream-service adapter for HTTP/3.

use std::num::NonZeroUsize;

use rustymiddle_core::{
    RoutePolicy, StreamingRequest,
    intercept::ExchangeCancellation,
    route::{UpstreamDestination, UpstreamPlan},
    upstream::{BoxUpstreamFuture, UpstreamError, UpstreamErrorKind, UpstreamService},
};

use crate::{H3OriginClient, H3OriginError};

/// A pooled HTTP/3 client exposed through the canonical upstream service
/// boundary.
///
/// The selected plan destination must exactly match the canonical request
/// target. The adapter accepts `Http3Only` and `Auto` plans; retry and fallback
/// policy remains the caller's responsibility.
#[derive(Clone)]
pub struct H3UpstreamService {
    client: H3OriginClient,
    max_response_bytes: usize,
    body_channel_capacity: NonZeroUsize,
}

impl std::fmt::Debug for H3UpstreamService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("H3UpstreamService")
            .field("max_response_bytes", &self.max_response_bytes)
            .field("body_channel_capacity", &self.body_channel_capacity)
            .finish_non_exhaustive()
    }
}

impl H3UpstreamService {
    /// Creates a bounded service adapter around an immutable HTTP/3 client
    /// pool.
    pub fn new(
        client: H3OriginClient,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
    ) -> Self {
        Self {
            client,
            max_response_bytes,
            body_channel_capacity,
        }
    }
}

impl UpstreamService for H3UpstreamService {
    fn execute(
        &self,
        request: StreamingRequest,
        plan: UpstreamPlan,
        cancellation: ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        if !matches!(
            plan.pool_key.version_policy,
            RoutePolicy::Http3Only | RoutePolicy::Auto
        ) {
            return Box::pin(async {
                Err(contract_error(
                    "HTTP/1.1 and HTTP/2-only plans cannot execute through the HTTP/3 adapter",
                ))
            });
        }
        if !request.head.target.scheme.eq_ignore_ascii_case("https") {
            return Box::pin(async {
                Err(contract_error(
                    "HTTP/3 requires an HTTPS canonical request target",
                ))
            });
        }
        if !destination_matches(&request, &plan.pool_key.destination) {
            return Box::pin(async {
                Err(contract_error(
                    "authorized destination does not match the canonical request target",
                ))
            });
        }

        let client = self.client.clone();
        let peer_port = plan.pool_key.destination.port;
        let max_response_bytes = self.max_response_bytes;
        let body_channel_capacity = self.body_channel_capacity;
        Box::pin(async move {
            tokio::select! {
                () = cancellation.cancelled() => Err(cancelled_error()),
                result = client.execute_duplex_streaming(
                    request,
                    peer_port,
                    max_response_bytes,
                    body_channel_capacity,
                ) => result
                    .map(|response| response.response)
                    .map_err(|error| map_h3_error(&error)),
            }
        })
    }
}

fn destination_matches(request: &StreamingRequest, destination: &UpstreamDestination) -> bool {
    destination.matches_target(&request.head.target)
}

fn contract_error(message: &str) -> UpstreamError {
    UpstreamError {
        kind: UpstreamErrorKind::Http,
        request_committed: false,
        response_started: false,
        message: message.to_owned(),
    }
}

fn cancelled_error() -> UpstreamError {
    UpstreamError {
        kind: UpstreamErrorKind::Cancelled,
        request_committed: false,
        response_started: false,
        message: "exchange was cancelled".to_owned(),
    }
}

fn map_h3_error(error: &H3OriginError) -> UpstreamError {
    let kind = match error {
        H3OriginError::DnsNoAddresses(_) => UpstreamErrorKind::Dns,
        H3OriginError::Boring(_) | H3OriginError::Config(_) => UpstreamErrorKind::Tls,
        H3OriginError::Io(_)
        | H3OriginError::Quiche(_)
        | H3OriginError::PoolCapacityExceeded { .. }
        | H3OriginError::PoolDriverStopped
        | H3OriginError::ConnectionFailed(_)
        | H3OriginError::ClosedBeforeResponse => UpstreamErrorKind::Connect,
        _ => UpstreamErrorKind::Http,
    };
    UpstreamError {
        kind,
        request_committed: true,
        response_started: false,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, sync::Arc};

    use rustymiddle_core::{
        BodyStream, HeaderBlock, HttpLegVersion, Replayability, RequestHead, Target,
        intercept::ExchangeCancellation,
        route::{UpstreamDestination, UpstreamPlan, UpstreamPoolKey},
        upstream::{UpstreamErrorKind, UpstreamService},
    };
    use rustymiddle_tls::{
        SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };

    use super::*;
    use crate::H3TransportLimits;

    #[tokio::test]
    async fn rejects_non_https_target_before_network_io() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap());
        let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let service = H3UpstreamService::new(
            H3OriginClient::new(tls, H3TransportLimits::default()),
            1024,
            NonZeroUsize::new(1).unwrap(),
        );
        let (_sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let error = service
            .execute(
                StreamingRequest {
                    head: RequestHead {
                        method: "GET".to_owned(),
                        target: Target {
                            scheme: "http".to_owned(),
                            authority: "example.test".to_owned(),
                            host: "example.test".to_owned(),
                            port: 80,
                            path: "/".to_owned(),
                            query: None,
                        },
                        headers: HeaderBlock::new(),
                        source_version: HttpLegVersion::Http1,
                    },
                    body,
                },
                UpstreamPlan {
                    pool_key: UpstreamPoolKey {
                        destination: UpstreamDestination {
                            scheme: "http".to_owned(),
                            host: "example.test".to_owned(),
                            port: 80,
                        },
                        version_policy: RoutePolicy::Http3Only,
                        trust_generation: 1,
                        tls_policy_id: "test".into(),
                        connector_policy_id: "direct".into(),
                    },
                    route_id: "test".into(),
                    reason: "test".into(),
                    replayability: Replayability::SafeMethod,
                },
                ExchangeCancellation::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, UpstreamErrorKind::Http);
        assert!(!error.request_committed);
    }
}
