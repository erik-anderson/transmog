//! Protocol-neutral route selection and destination authorization.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::time::timeout;

use crate::{
    Replayability, RequestHead, RoutePolicy, Target,
    intercept::{ExchangeCancellation, ExchangeMetadata, OriginalTarget},
};

/// Boxed asynchronous route-selection result.
pub type BoxRouteFuture<'a> =
    Pin<Box<dyn Future<Output = Result<UpstreamPlan, RouteError>> + Send + 'a>>;

/// Route selector input preserving immutable client intent separately from edits.
#[derive(Clone, Debug)]
pub struct RouteInput {
    /// Immutable exchange metadata.
    pub metadata: Arc<ExchangeMetadata>,
    /// Immutable target originally supplied by the client.
    pub original_target: OriginalTarget,
    /// Effective logical request after request hooks.
    pub effective_request: RequestHead,
    /// Typed destination change requested by a reroute action.
    pub explicit_reroute: Option<Target>,
    /// Whether retry policy may replay this request.
    pub replayability: Replayability,
}

/// Authorized socket destination independent of logical request headers.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UpstreamDestination {
    /// URI scheme used for connection and TLS policy.
    pub scheme: String,
    /// Destination host without brackets or port.
    pub host: String,
    /// Destination port.
    pub port: u16,
}

impl UpstreamDestination {
    /// Converts a normalized target into a socket destination.
    pub fn from_target(target: &Target) -> Self {
        Self {
            scheme: target.scheme.clone(),
            host: target.host.clone(),
            port: target.port,
        }
    }
}

/// Identities needed to segregate upstream connection pools safely.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UpstreamPoolKey {
    /// Authorized socket destination.
    pub destination: UpstreamDestination,
    /// HTTP version-selection policy.
    pub version_policy: RoutePolicy,
    /// Immutable trust snapshot generation.
    pub trust_generation: u64,
    /// Stable TLS-policy identity.
    pub tls_policy_id: Arc<str>,
    /// Direct or upstream-proxy policy identity.
    pub connector_policy_id: Arc<str>,
}

/// Auditable, immutable plan for one upstream exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamPlan {
    /// Pool segregation and connection settings.
    pub pool_key: UpstreamPoolKey,
    /// Stable route policy identity.
    pub route_id: Arc<str>,
    /// Redacted operator-facing selection reason.
    pub reason: Arc<str>,
    /// Retry eligibility captured at selection time.
    pub replayability: Replayability,
}

/// Structured route-selection failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RouteError {
    /// Destination target was malformed or unsupported.
    #[error("invalid upstream destination: {0}")]
    InvalidDestination(String),
    /// Destination authorization rejected a reroute.
    #[error("upstream destination is not authorized")]
    Unauthorized,
    /// Selector exceeded its configured deadline.
    #[error("route selection timed out")]
    TimedOut,
    /// Exchange was cancelled while selecting a route.
    #[error("route selection was cancelled")]
    Cancelled,
    /// Selector panicked and was contained.
    #[error("route selector panicked")]
    Panicked,
    /// Redacted selector-specific failure.
    #[error("route selection failed: {0}")]
    Failed(String),
}

/// Application policy that authorizes an explicit socket destination.
pub trait DestinationAuthorizer: Send + Sync {
    /// Checks the proposed destination using immutable original intent.
    ///
    /// # Errors
    ///
    /// Returns [`RouteError::Unauthorized`] or another typed route failure.
    fn authorize(
        &self,
        original: &OriginalTarget,
        proposed: &UpstreamDestination,
    ) -> Result<(), RouteError>;
}

/// Default authorizer that permits only the original scheme, host, and port.
#[derive(Clone, Copy, Debug, Default)]
pub struct OriginalDestinationOnly;

impl DestinationAuthorizer for OriginalDestinationOnly {
    fn authorize(
        &self,
        original: &OriginalTarget,
        proposed: &UpstreamDestination,
    ) -> Result<(), RouteError> {
        let original = UpstreamDestination::from_target(original.as_target());
        if original == *proposed {
            Ok(())
        } else {
            Err(RouteError::Unauthorized)
        }
    }
}

/// Asynchronous protocol-neutral route selector.
pub trait RouteSelector: Send + Sync {
    /// Selects and authorizes one immutable upstream plan.
    fn select(&self, input: RouteInput) -> BoxRouteFuture<'_>;
}

/// Basic selector adapting [`RoutePolicy`] to the Hooks v2 route boundary.
#[derive(Clone)]
pub struct PolicyRouteSelector {
    authorizer: Arc<dyn DestinationAuthorizer>,
    version_policy: RoutePolicy,
    trust_generation: u64,
    tls_policy_id: Arc<str>,
    connector_policy_id: Arc<str>,
    route_id: Arc<str>,
}

impl std::fmt::Debug for PolicyRouteSelector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PolicyRouteSelector")
            .field("version_policy", &self.version_policy)
            .field("trust_generation", &self.trust_generation)
            .field("tls_policy_id", &self.tls_policy_id)
            .field("connector_policy_id", &self.connector_policy_id)
            .field("route_id", &self.route_id)
            .finish_non_exhaustive()
    }
}

impl PolicyRouteSelector {
    /// Creates a selector with explicit pool and audit identities.
    pub fn new(
        authorizer: Arc<dyn DestinationAuthorizer>,
        version_policy: RoutePolicy,
        trust_generation: u64,
        tls_policy_id: impl Into<Arc<str>>,
        connector_policy_id: impl Into<Arc<str>>,
        route_id: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            authorizer,
            version_policy,
            trust_generation,
            tls_policy_id: tls_policy_id.into(),
            connector_policy_id: connector_policy_id.into(),
            route_id: route_id.into(),
        }
    }
}

impl RouteSelector for PolicyRouteSelector {
    fn select(&self, input: RouteInput) -> BoxRouteFuture<'_> {
        Box::pin(async move {
            let selected = input
                .explicit_reroute
                .as_ref()
                .unwrap_or_else(|| input.original_target.as_target());
            if selected.host.is_empty()
                || selected.port == 0
                || !matches!(selected.scheme.as_str(), "http" | "https")
            {
                return Err(RouteError::InvalidDestination(
                    "scheme, host, and port are required".to_owned(),
                ));
            }
            let destination = UpstreamDestination::from_target(selected);
            self.authorizer
                .authorize(&input.original_target, &destination)?;
            let reason: Arc<str> = if input.explicit_reroute.is_some() {
                "explicit authorized reroute".into()
            } else {
                "original client destination".into()
            };
            Ok(UpstreamPlan {
                pool_key: UpstreamPoolKey {
                    destination,
                    version_policy: self.version_policy,
                    trust_generation: self.trust_generation,
                    tls_policy_id: Arc::clone(&self.tls_policy_id),
                    connector_policy_id: Arc::clone(&self.connector_policy_id),
                },
                route_id: Arc::clone(&self.route_id),
                reason,
                replayability: input.replayability,
            })
        })
    }
}

/// Timeout, cancellation, and panic boundary around a route selector.
#[derive(Clone)]
pub struct RouteSelectionService {
    selector: Arc<dyn RouteSelector>,
    timeout: Duration,
}

impl std::fmt::Debug for RouteSelectionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RouteSelectionService")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl RouteSelectionService {
    /// Wraps a selector with a finite per-call deadline.
    pub fn new(selector: Arc<dyn RouteSelector>, timeout: Duration) -> Self {
        Self { selector, timeout }
    }

    /// Selects a route while containing timeout, cancellation, and panic.
    ///
    /// # Errors
    ///
    /// Returns a structured route failure.
    pub async fn select(
        &self,
        input: RouteInput,
        cancellation: &ExchangeCancellation,
    ) -> Result<UpstreamPlan, RouteError> {
        let selector = Arc::clone(&self.selector);
        let mut task = tokio::spawn(async move { selector.select(input).await });
        let result = tokio::select! {
            joined = timeout(self.timeout, &mut task) => match joined {
                Ok(Ok(result)) => result,
                Ok(Err(error)) if error.is_panic() => Err(RouteError::Panicked),
                Ok(Err(_)) => Err(RouteError::Cancelled),
                Err(_) => Err(RouteError::TimedOut),
            },
            () = cancellation.cancelled() => Err(RouteError::Cancelled),
        };
        if result.is_err() && !task.is_finished() {
            task.abort();
            let _ = task.await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, net::SocketAddr};

    use crate::{ConnectionId, HttpLegVersion, StreamId};

    use super::*;
    use crate::intercept::ExchangeId;

    struct AllowAll;

    impl DestinationAuthorizer for AllowAll {
        fn authorize(
            &self,
            _original: &OriginalTarget,
            _proposed: &UpstreamDestination,
        ) -> Result<(), RouteError> {
            Ok(())
        }
    }

    struct Never;

    impl RouteSelector for Never {
        fn select(&self, _input: RouteInput) -> BoxRouteFuture<'_> {
            Box::pin(pending())
        }
    }

    struct Panics;

    impl RouteSelector for Panics {
        fn select(&self, _input: RouteInput) -> BoxRouteFuture<'_> {
            Box::pin(async { panic!("selector panic") })
        }
    }

    fn target(host: &str, port: u16) -> Target {
        Target {
            scheme: "https".to_owned(),
            authority: format!("{host}:{port}"),
            host: host.to_owned(),
            port,
            path: "/".to_owned(),
            query: None,
        }
    }

    fn input(reroute: Option<Target>) -> RouteInput {
        let original = target("original.test", 443);
        RouteInput {
            metadata: Arc::new(ExchangeMetadata {
                exchange_id: ExchangeId(1),
                downstream_connection_id: ConnectionId(2),
                downstream_stream_id: StreamId(3),
                client_addr: SocketAddr::from(([127, 0, 0, 1], 1000)),
                listener_addr: SocketAddr::from(([127, 0, 0, 1], 2000)),
                ingress_version: HttpLegVersion::Http2,
                original_target: OriginalTarget::new(original.clone()),
                started_at: std::time::SystemTime::now(),
            }),
            original_target: OriginalTarget::new(original.clone()),
            effective_request: RequestHead {
                method: "GET".to_owned(),
                target: target("edited-logical.test", 9443),
                headers: crate::HeaderBlock::new(),
                source_version: HttpLegVersion::Http2,
            },
            explicit_reroute: reroute,
            replayability: Replayability::SafeMethod,
        }
    }

    fn selector(authorizer: Arc<dyn DestinationAuthorizer>) -> PolicyRouteSelector {
        PolicyRouteSelector::new(
            authorizer,
            RoutePolicy::Auto,
            7,
            "system-trust-v7",
            "direct",
            "default",
        )
    }

    #[tokio::test]
    async fn logical_target_or_host_edits_do_not_redirect_the_socket() {
        let plan = selector(Arc::new(OriginalDestinationOnly))
            .select(input(None))
            .await
            .unwrap();
        assert_eq!(plan.pool_key.destination.host, "original.test");
        assert_eq!(plan.pool_key.destination.port, 443);
    }

    #[tokio::test]
    async fn explicit_reroute_requires_authorization_and_is_auditable() {
        let reroute = target("other.test", 8443);
        assert_eq!(
            selector(Arc::new(OriginalDestinationOnly))
                .select(input(Some(reroute.clone())))
                .await
                .unwrap_err(),
            RouteError::Unauthorized
        );
        let plan = selector(Arc::new(AllowAll))
            .select(input(Some(reroute)))
            .await
            .unwrap();
        assert_eq!(plan.pool_key.destination.host, "other.test");
        assert_eq!(&*plan.reason, "explicit authorized reroute");
    }

    #[test]
    fn pool_key_segregates_destination_protocol_trust_tls_and_connector() {
        let base = UpstreamPoolKey {
            destination: UpstreamDestination::from_target(&target("example.test", 443)),
            version_policy: RoutePolicy::Auto,
            trust_generation: 1,
            tls_policy_id: "tls-a".into(),
            connector_policy_id: "direct".into(),
        };
        let mut keys = std::collections::HashSet::new();
        keys.insert(base.clone());
        let mut changed = base.clone();
        changed.trust_generation = 2;
        keys.insert(changed);
        let mut changed = base.clone();
        changed.version_policy = RoutePolicy::Http3Only;
        keys.insert(changed);
        let mut changed = base.clone();
        changed.tls_policy_id = "tls-b".into();
        keys.insert(changed);
        let mut changed = base;
        changed.connector_policy_id = "proxy-a".into();
        keys.insert(changed);
        assert_eq!(keys.len(), 5);
    }

    #[tokio::test]
    async fn selector_timeout_panic_and_cancellation_are_typed() {
        let cancellation = ExchangeCancellation::new();
        assert_eq!(
            RouteSelectionService::new(Arc::new(Never), Duration::from_millis(5))
                .select(input(None), &cancellation)
                .await
                .unwrap_err(),
            RouteError::TimedOut
        );
        assert_eq!(
            RouteSelectionService::new(Arc::new(Panics), Duration::from_secs(1))
                .select(input(None), &cancellation)
                .await
                .unwrap_err(),
            RouteError::Panicked
        );
        cancellation.cancel();
        assert_eq!(
            RouteSelectionService::new(Arc::new(Never), Duration::from_secs(1))
                .select(input(None), &cancellation)
                .await
                .unwrap_err(),
            RouteError::Cancelled
        );
    }
}
