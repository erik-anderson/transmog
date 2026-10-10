use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use transmog_core::observe::{
    BodyObservation, BoxObserverFuture, ObservationInterest, Observer, ObserverConfig,
    ObserverDeliveryPolicy, ObserverEvent,
};

use crate::{CaptureManager, ControlConnector, SessionCatalog};

/// Observer that feeds the authoritative catalog and optional dynamic capture.
#[derive(Clone, Debug)]
pub struct SessionObserver {
    catalog: SessionCatalog,
    capture: CaptureManager,
    control: Option<ControlConnector>,
    redact_sensitive: Arc<AtomicBool>,
    retain_traffic: bool,
}

impl SessionObserver {
    /// Creates a service observer over shared catalog and capture controllers.
    pub fn new(catalog: SessionCatalog, capture: CaptureManager) -> Self {
        Self {
            catalog,
            capture,
            control: None,
            redact_sensitive: Arc::new(AtomicBool::new(true)),
            retain_traffic: true,
        }
    }

    /// Publishes supported lifecycle events to the attached control endpoint.
    #[must_use]
    pub fn with_control(mut self, control: ControlConnector) -> Self {
        self.control = Some(control);
        self
    }

    /// Shares the application's dynamically saved privacy choice.
    #[must_use]
    pub fn with_redaction_policy(mut self, policy: Arc<AtomicBool>) -> Self {
        self.redact_sensitive = policy;
        self
    }
    /// Selects whether this run publishes retained Traffic entries.
    #[must_use]
    pub fn with_traffic_retention(mut self, retain: bool) -> Self {
        self.retain_traffic = retain;
        self
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
        Box::pin(async move {
            let event = if self.redact_sensitive.load(Ordering::Acquire) {
                event.redacted()
            } else {
                event
            };
            if let Some(control) = &self.control {
                control.publish(&event);
            }
            self.capture.record(event.clone()).await;
            if self.retain_traffic {
                self.catalog.apply(event);
            }
            Ok(())
        })
    }
}

/// Recommended bounded runtime registration for [`SessionObserver`].
///
/// Full samples are requested because both catalog and capture apply their own
/// explicit retention policy. Saturation waits for capacity and slows requests
/// instead of dropping start, body or terminal evidence. No callback deadline
/// interrupts application-owned storage while it is catching up.
pub fn session_observer_config(queue_capacity: NonZeroUsize) -> ObserverConfig {
    ObserverConfig {
        interest: ObservationInterest {
            lifecycle: true,
            sensitive_headers: true,
            request_body: BodyObservation::Full,
            response_body: BodyObservation::Full,
        },
        queue_capacity,
        delivery: ObserverDeliveryPolicy::WaitForCapacity,
        callback_timeout: None,
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
    use std::{
        future::{Future, poll_fn},
        net::SocketAddr,
        num::NonZeroUsize,
        sync::Arc,
        task::Poll,
        time::{Duration, SystemTime},
    };

    use tokio::sync::Notify;

    use transmog_core::{
        ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{
            ExchangeFailure, ExchangeFailureKind, ExchangeId, ExchangeMetadata, ExchangeStage,
        },
        observe::{
            ExchangeBoundary, ObservedBodyChunk, ObserverDelivery, ObserverEvent,
            ObserverEventKind, ObserverHub,
        },
    };

    use super::*;
    use crate::{CaptureStatus, SessionLimits};

    fn metadata(id: u128) -> Arc<ExchangeMetadata> {
        Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(id),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
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
        ))
    }

    #[tokio::test]
    async fn observer_updates_catalog_while_capture_is_idle() {
        let catalog = SessionCatalog::new(SessionLimits::default());
        let capture = CaptureManager::new(NonZeroUsize::new(2).unwrap()).unwrap();
        let observer = SessionObserver::new(catalog.clone(), capture.clone());
        let metadata = metadata(1);
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

    struct GatedObserver {
        observer: SessionObserver,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl Observer for GatedObserver {
        fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
            Box::pin(async move {
                if event.exchange_id == ExchangeId(0) && event.sequence == 1 {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                self.observer.on_event(event).await
            })
        }
    }

    #[tokio::test]
    async fn saturation_preserves_admission_body_and_terminal_evidence() {
        let catalog = SessionCatalog::new(SessionLimits::default());
        let capture = CaptureManager::new(NonZeroUsize::new(2).unwrap()).unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let hub = ObserverHub::new(vec![(
            Arc::new(GatedObserver {
                observer: SessionObserver::new(catalog.clone(), capture.clone()),
                entered: entered.clone(),
                release: release.clone(),
            }),
            session_observer_config(NonZeroUsize::new(1).unwrap()),
        )]);
        let exchanges = (0..3)
            .map(|id| hub.start_exchange(metadata(id)))
            .collect::<Vec<_>>();
        exchanges[0]
            .emit(ObserverEventKind::ExchangeStarted {
                metadata: metadata(0),
            })
            .await;
        entered.notified().await;
        // The worker is paused on the first event and the one-slot queue is full.
        exchanges[1]
            .emit(ObserverEventKind::ExchangeStarted {
                metadata: metadata(1),
            })
            .await;
        let mut admission = Box::pin(exchanges[2].emit(ObserverEventKind::ExchangeStarted {
            metadata: metadata(2),
        }));
        let early = poll_fn(|cx| Poll::Ready(admission.as_mut().poll(cx))).await;
        // Hold pressure beyond both the old 25 ms admission deadline and the
        // two-second callback deadline. Healthy storage must retain the event.
        tokio::time::sleep(Duration::from_millis(2100)).await;
        let late = if early.is_pending() {
            poll_fn(|cx| Poll::Ready(admission.as_mut().poll(cx))).await
        } else {
            Poll::Pending
        };
        release.notify_one();
        let waited = early.is_pending() && late.is_pending();
        let admission = match early {
            Poll::Ready(result) => result,
            Poll::Pending => match late {
                Poll::Ready(result) => result,
                Poll::Pending => admission.await,
            },
        };
        for (id, exchange) in exchanges.iter().enumerate() {
            exchange
                .emit(ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: 4,
                    sample: Some(bytes::Bytes::from_static(b"data")),
                    truncated: false,
                }))
                .await;
            exchange
                .emit(ObserverEventKind::BodyCompleted {
                    boundary: ExchangeBoundary::ClientRequest,
                })
                .await;
            exchange
                .failed(ExchangeFailure {
                    metadata: metadata(id as u128),
                    stage: ExchangeStage::Upstream,
                    kind: ExchangeFailureKind::Upstream,
                    request_committed: true,
                    response_committed: false,
                    message: "synthetic terminal failure".into(),
                })
                .await;
        }
        hub.shutdown().await;
        capture.shutdown().await;
        assert!(waited, "retention abandoned an event during slow storage");
        assert_eq!(admission.deliveries(), [ObserverDelivery::Delivered]);
        assert_eq!(hub.stats()[0].dropped, 0);
        assert_eq!(catalog.counters().sequence_gaps, 0);
        assert_eq!(catalog.counters().unknown_exchange_events, 0);
        for id in 0..3 {
            let session = catalog.get(ExchangeId(id)).unwrap();
            assert_eq!(session.last_sequence, 4);
            assert!(session.terminal.is_some());
            assert_eq!(session.bodies[0].observed_bytes, 4);
        }
    }

    #[tokio::test]
    async fn file_only_observer_records_without_retaining_traffic() {
        let catalog = SessionCatalog::new(SessionLimits::default());
        let capture = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "transmog-file-only-{}-{}.tmcap",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        capture
            .start(crate::CaptureStart {
                path: path.clone(),
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                limits: transmog_capture::CaptureLimits::default(),
                policy: transmog_capture::CapturePolicy::default(),
            })
            .await
            .unwrap();
        let observer =
            SessionObserver::new(catalog.clone(), capture.clone()).with_traffic_retention(false);
        let metadata = Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
                ingress_version: HttpLegVersion::Http1,
                egress_version: None,
            },
            Target {
                scheme: "http".into(),
                authority: "placeholder.invalid".into(),
                host: "placeholder.invalid".into(),
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
        capture.stop().await.unwrap();
        assert!(catalog.get(ExchangeId(1)).is_none());
        let saved = transmog_capture::recover(
            std::fs::File::open(&path).unwrap(),
            transmog_capture::CaptureLimits::default(),
        )
        .unwrap();
        assert!(saved.sealed);
        assert!(saved.records.iter().any(|record| record.exchange_id == 1));
        capture.shutdown().await;
        std::fs::remove_file(path).unwrap();
    }
}
