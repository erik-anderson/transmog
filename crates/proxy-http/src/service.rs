//! Protocol-neutral upstream-service adapter for Hyper.

use std::{num::NonZeroUsize, time::Duration};

use rustymiddle_core::{
    StreamingRequest,
    intercept::ExchangeCancellation,
    route::{UpstreamDestination, UpstreamPlan},
    upstream::{BoxUpstreamFuture, UpstreamError, UpstreamErrorKind, UpstreamService},
};

use crate::{HyperEgressMode, HyperOriginClient, HyperOriginError};

/// A pooled HTTP/1.1 and HTTP/2 client exposed through the canonical upstream
/// service boundary.
///
/// The selected plan destination must exactly match the canonical request
/// target. This prevents a caller from authorizing one socket destination and
/// then causing the transport adapter to connect to another.
#[derive(Clone)]
pub struct HyperUpstreamService {
    client: HyperOriginClient,
    max_response_bytes: usize,
    body_channel_capacity: NonZeroUsize,
    body_idle_timeout: Duration,
}

impl std::fmt::Debug for HyperUpstreamService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HyperUpstreamService")
            .field("max_response_bytes", &self.max_response_bytes)
            .field("body_channel_capacity", &self.body_channel_capacity)
            .field("body_idle_timeout", &self.body_idle_timeout)
            .finish_non_exhaustive()
    }
}

impl HyperUpstreamService {
    /// Creates a bounded service adapter around an immutable client pool.
    pub fn new(
        client: HyperOriginClient,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
        body_idle_timeout: Duration,
    ) -> Self {
        Self {
            client,
            max_response_bytes,
            body_channel_capacity,
            body_idle_timeout,
        }
    }
}

impl UpstreamService for HyperUpstreamService {
    fn execute(
        &self,
        request: StreamingRequest,
        plan: UpstreamPlan,
        cancellation: ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        let mode = match plan.pool_key.version_policy {
            rustymiddle_core::RoutePolicy::Http1Only => HyperEgressMode::Http1Only,
            rustymiddle_core::RoutePolicy::Http2Only => HyperEgressMode::Http2Only,
            rustymiddle_core::RoutePolicy::Auto => HyperEgressMode::Auto,
            rustymiddle_core::RoutePolicy::Http3Only => {
                return Box::pin(async {
                    Err(contract_error(
                        "HTTP/3-only plans cannot execute through the Hyper adapter",
                    ))
                });
            }
        };
        if !destination_matches(&request, &plan.pool_key.destination) {
            return Box::pin(async {
                Err(contract_error(
                    "authorized destination does not match the canonical request target",
                ))
            });
        }

        let client = self.client.clone();
        let max_response_bytes = self.max_response_bytes;
        let body_channel_capacity = self.body_channel_capacity;
        let body_idle_timeout = self.body_idle_timeout;
        Box::pin(async move {
            tokio::select! {
                () = cancellation.cancelled() => Err(cancelled_error()),
                result = client.execute_streaming(
                    request,
                    mode,
                    max_response_bytes,
                    body_channel_capacity,
                    body_idle_timeout,
                ) => result.map_err(|error| map_hyper_error(&error)),
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

fn map_hyper_error(error: &HyperOriginError) -> UpstreamError {
    let kind = match error {
        HyperOriginError::Trust(_) => UpstreamErrorKind::Tls,
        HyperOriginError::Client(_) => UpstreamErrorKind::Connect,
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
        BodyStream, HeaderBlock, HttpLegVersion, Replayability, RequestHead, RoutePolicy, Target,
        intercept::ExchangeCancellation,
        route::{UpstreamDestination, UpstreamPlan, UpstreamPoolKey},
        upstream::{UpstreamErrorKind, UpstreamService},
    };
    use rustymiddle_tls::{
        SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };

    use super::*;

    #[tokio::test]
    async fn rejects_mismatched_destination_before_network_io() {
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap());
        let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
        let service = HyperUpstreamService::new(
            HyperOriginClient::new(&tls).unwrap(),
            1024,
            NonZeroUsize::new(1).unwrap(),
            Duration::from_secs(1),
        );
        let (_sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let error = service
            .execute(
                StreamingRequest {
                    head: RequestHead {
                        method: "GET".to_owned(),
                        target: Target {
                            scheme: "http".to_owned(),
                            authority: "other.test".to_owned(),
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
                        version_policy: RoutePolicy::Http1Only,
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
