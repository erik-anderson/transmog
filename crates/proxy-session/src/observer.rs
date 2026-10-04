use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use rustymiddle_core::observe::{
    BodyObservation, BoxObserverFuture, ObservationInterest, Observer, ObserverConfig,
    ObserverDeliveryPolicy, ObserverEvent,
};

use crate::{CaptureManager, SessionCatalog};

/// Observer that feeds the authoritative catalog and optional dynamic capture.
#[derive(Clone, Debug)]
pub struct SessionObserver {
    catalog: SessionCatalog,
    capture: CaptureManager,
}

impl SessionObserver {
    /// Creates a service observer over shared catalog and capture controllers.
    pub fn new(catalog: SessionCatalog, capture: CaptureManager) -> Self {
        Self { catalog, capture }
    }

    /// Returns the authoritative catalog fed by this observer.
    pub fn catalog(&self) -> &SessionCatalog {
        &self.catalog
    }

    /// Returns the dynamic-capture controller fed by this observer.
    pub fn capture(&self) -> &CaptureManager {
        &self.capture
    }
}

impl Observer for SessionObserver {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        self.capture.record(event.clone());
        self.catalog.apply(event);
        Box::pin(async { Ok(()) })
    }
}

/// Recommended bounded runtime registration for [`SessionObserver`].
///
/// Full samples are requested because both catalog and capture apply their own
/// explicit retention policy. Queue saturation drops the newest event and is
/// observable through both runtime observer statistics and catalog sequence
/// gaps.
pub fn session_observer_config(queue_capacity: NonZeroUsize) -> ObserverConfig {
    ObserverConfig {
        interest: ObservationInterest {
            lifecycle: true,
            request_body: BodyObservation::Full,
            response_body: BodyObservation::Full,
        },
        queue_capacity,
        delivery: ObserverDeliveryPolicy::DropNewest,
        callback_timeout: Duration::from_secs(2),
    }
}

/// Converts a session observer into the trait object expected by runtime builders.
impl From<SessionObserver> for Arc<dyn Observer> {
    fn from(observer: SessionObserver) -> Self {
        Arc::new(observer)
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, num::NonZeroUsize, sync::Arc, time::SystemTime};

    use rustymiddle_core::{
        ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeId, ExchangeMetadata},
        observe::{ObserverEvent, ObserverEventKind},
    };

    use super::*;
    use crate::{CaptureStatus, SessionLimits};

    #[tokio::test]
    async fn observer_updates_catalog_while_capture_is_idle() {
        let catalog = SessionCatalog::new(SessionLimits::default());
        let capture = CaptureManager::new(NonZeroUsize::new(2).unwrap()).unwrap();
        let observer = SessionObserver::new(catalog.clone(), capture.clone());
        let metadata = Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
                ingress_version: HttpLegVersion::Http1,
                egress_version: None,
            },
            Target {
                scheme: "http".into(),
                authority: "example.test".into(),
                host: "example.test".into(),
                port: 80,
                path: "/".into(),
                query: None,
            },
            SystemTime::UNIX_EPOCH,
        ));
        observer
            .on_event(ObserverEvent {
                exchange_id: ExchangeId(1),
                sequence: 1,
                kind: ObserverEventKind::ExchangeStarted { metadata },
            })
            .await
            .unwrap();
        assert!(catalog.get(ExchangeId(1)).is_some());
        assert_eq!(capture.status(), CaptureStatus::Idle);
        capture.shutdown().await;
    }
}
