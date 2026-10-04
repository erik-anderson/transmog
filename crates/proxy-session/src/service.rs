use std::{
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
};

use rustymiddle_control_model::Handshake;
use rustymiddle_control_transport::TransportConfig;
use rustymiddle_core::{
    intercept::{ExchangeCancellation, InterceptorRegistration, InterceptorRequirement},
    observe::Observer,
};
use rustymiddle_runtime::{ProxyComponents, ProxyControl, ProxyServer};
use thiserror::Error;
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};

use crate::{
    AttachedController, CaptureManager, CaptureServiceError, CaptureStart, CaptureStatus,
    ControlConnectionError, ControlConnector, ControlPolicy, INTERACTIVE_CONTROL_HOOK_ID,
    SessionCatalog, SessionLimits, SessionObserver, session_observer_config,
};

/// Finite composition settings for one application/session service.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// Live catalog retention and fan-out limits.
    pub sessions: SessionLimits,
    /// Runtime-to-service observer queue capacity.
    pub observer_queue_capacity: NonZeroUsize,
    /// Service-to-capture-writer queue capacity.
    pub capture_queue_capacity: NonZeroUsize,
    /// Opaque identity required to match experimental same-build controllers.
    pub control_build_id: Arc<str>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            sessions: SessionLimits::default(),
            observer_queue_capacity: NonZeroUsize::new(2_048).expect("constant is nonzero"),
            capture_queue_capacity: NonZeroUsize::new(2_048).expect("constant is nonzero"),
            control_build_id: Arc::from(concat!("rustymiddle-session/", env!("CARGO_PKG_VERSION"))),
        }
    }
}

/// Public lifecycle state for one owned proxy run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceStatus {
    /// No runtime task is active.
    Stopped,
    /// The listener is accepting traffic.
    Running {
        /// Bound listener address.
        local_addr: SocketAddr,
    },
    /// Graceful shutdown and bounded drain are in progress.
    Stopping {
        /// Bound listener address.
        local_addr: SocketAddr,
    },
    /// The runtime or an owned shutdown operation failed.
    Failed {
        /// Listener address, when binding had completed.
        local_addr: Option<SocketAddr>,
        /// Redaction-safe operator description.
        message: String,
    },
}

/// Application/session lifecycle operation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ServiceError {
    /// Start was requested while a run was active or stopping.
    #[error("application/session service is already running")]
    AlreadyRunning,
    /// The bound listener address could not be read.
    #[error("proxy listener address is unavailable: {0}")]
    ListenerAddress(String),
    /// The runtime task failed.
    #[error("proxy runtime failed: {0}")]
    Runtime(String),
    /// Dynamic capture failed during a service operation.
    #[error(transparent)]
    Capture(#[from] CaptureServiceError),
}

struct RunTask {
    generation: u64,
    local_addr: SocketAddr,
    shutdown: ExchangeCancellation,
    control: Option<ProxyControl>,
    handle: JoinHandle<Result<(), String>>,
}

struct Lifecycle {
    status: ServiceStatus,
    next_generation: u64,
    task: Option<RunTask>,
}

struct ServiceInner {
    catalog: SessionCatalog,
    capture: CaptureManager,
    controller: ControlConnector,
    observer_queue_capacity: NonZeroUsize,
    lifecycle: Mutex<Lifecycle>,
    operation: AsyncMutex<()>,
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        let lifecycle = self
            .lifecycle
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(task) = lifecycle.task.take() {
            task.shutdown.cancel();
            task.handle.abort();
        }
    }
}

/// UI-independent owner of one proxy run and its bounded live state.
#[derive(Clone)]
pub struct ApplicationSessionService {
    inner: Arc<ServiceInner>,
}

impl std::fmt::Debug for ApplicationSessionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApplicationSessionService")
            .field("status", &self.status())
            .field("capture", &self.inner.capture.status())
            .finish_non_exhaustive()
    }
}

impl ApplicationSessionService {
    /// Creates an idle service with finite state, observer, and capture limits.
    ///
    /// # Errors
    ///
    /// Returns a typed capture-worker or control-build-identity error.
    pub fn new(config: ServiceConfig) -> Result<Self, ServiceError> {
        let capture = CaptureManager::new(config.capture_queue_capacity)?;
        let controller = ControlConnector::new(config.control_build_id)
            .map_err(|error| ServiceError::Runtime(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(ServiceInner {
                catalog: SessionCatalog::new(config.sessions),
                capture,
                controller,
                observer_queue_capacity: config.observer_queue_capacity,
                lifecycle: Mutex::new(Lifecycle {
                    status: ServiceStatus::Stopped,
                    next_generation: 0,
                    task: None,
                }),
                operation: AsyncMutex::new(()),
            }),
        })
    }

    /// Adds the service observer and identified interactive hook without
    /// replacing application-supplied observers or hooks.
    ///
    /// Call exactly once before binding the returned components. This method
    /// must run within a Tokio runtime because observer registration starts a
    /// bounded worker.
    #[must_use]
    pub fn prepare_components(&self, components: ProxyComponents) -> ProxyComponents {
        let observer = SessionObserver::new(self.inner.catalog.clone(), self.inner.capture.clone())
            .with_control(self.inner.controller.clone());
        components
            .with_interceptor(InterceptorRegistration::named(
                INTERACTIVE_CONTROL_HOOK_ID,
                "Interactive controller",
                self.inner.controller.interceptor_factory(),
                InterceptorRequirement::Required,
            ))
            .with_observer(
                Arc::<dyn Observer>::from(observer),
                session_observer_config(self.inner.observer_queue_capacity),
            )
    }

    /// Returns the authoritative bounded live catalog.
    pub fn catalog(&self) -> &SessionCatalog {
        &self.inner.catalog
    }

    /// Returns the dynamic native-capture controller.
    pub fn capture(&self) -> &CaptureManager {
        &self.inner.capture
    }

    /// Returns the same-process experimental controller connector.
    pub fn controller(&self) -> &ControlConnector {
        &self.inner.controller
    }

    /// Negotiates one exclusive same-build interactive controller.
    ///
    /// # Errors
    ///
    /// Returns a typed exclusivity, handshake, or capability failure.
    pub fn connect_controller(
        &self,
        peer: &Handshake,
        transport: TransportConfig,
        policy: ControlPolicy,
    ) -> Result<AttachedController, ControlConnectionError> {
        self.inner.controller.connect(peer, transport, policy)
    }

    /// Returns the current lifecycle state.
    pub fn status(&self) -> ServiceStatus {
        self.lock_lifecycle().status.clone()
    }

    /// Returns a runtime control handle while a bound run remains owned.
    pub fn proxy_control(&self) -> Option<ProxyControl> {
        self.lock_lifecycle()
            .task
            .as_ref()
            .and_then(|task| task.control.clone())
    }

    /// Starts a bound runtime task and begins collecting WebSocket evidence.
    ///
    /// # Errors
    ///
    /// Returns a listener-address error or [`ServiceError::AlreadyRunning`].
    pub async fn start(&self, server: ProxyServer) -> Result<SocketAddr, ServiceError> {
        let local_addr = server
            .local_addr()
            .map_err(|error| ServiceError::ListenerAddress(error.to_string()))?;
        let control = server.control();
        let websocket = server.subscribe_websocket_evidence();
        self.start_runner(Box::new(ProxyRunner {
            server,
            local_addr,
            control,
            websocket,
            catalog: self.inner.catalog.clone(),
        }))
        .await
    }

    /// Starts a create-new native capture while the service remains live.
    ///
    /// # Errors
    ///
    /// Returns typed capture state, destination, queue, or writer failures.
    pub async fn start_capture(&self, request: CaptureStart) -> Result<(), ServiceError> {
        self.inner.capture.start(request).await.map_err(Into::into)
    }

    /// Seals the active native capture.
    ///
    /// # Errors
    ///
    /// Returns typed capture state, queue, or writer failures.
    pub async fn stop_capture(&self) -> Result<crate::SealedCapture, ServiceError> {
        self.inner.capture.stop().await.map_err(Into::into)
    }

    /// Requests graceful runtime shutdown, waits for bounded drain, and seals
    /// an active capture. Concurrent callers serialize and the stopped result
    /// is idempotent.
    ///
    /// # Errors
    ///
    /// Returns a runtime or capture failure after owned cleanup is attempted.
    pub async fn stop(&self) -> Result<(), ServiceError> {
        let _operation = self.inner.operation.lock().await;
        let (task, existing_failure) = {
            let mut lifecycle = self.lock_lifecycle();
            let existing_failure = match &lifecycle.status {
                ServiceStatus::Failed { message, .. } => Some(message.clone()),
                _ => None,
            };
            if let Some(task) = lifecycle.task.take() {
                task.shutdown.cancel();
                lifecycle.status = ServiceStatus::Stopping {
                    local_addr: task.local_addr,
                };
                (Some(task), existing_failure)
            } else {
                (None, existing_failure)
            }
        };

        let runtime_result = if let Some(task) = task {
            match task.handle.await {
                Ok(result) => result,
                Err(error) => Err(format!("runtime task join failed: {error}")),
            }
        } else if let Some(message) = existing_failure {
            Err(message)
        } else {
            Ok(())
        };

        let capture_result = if matches!(
            self.inner.capture.status(),
            CaptureStatus::Active { .. } | CaptureStatus::Failed(_)
        ) {
            self.inner.capture.stop().await.map(|_| ())
        } else {
            Ok(())
        };

        let result = runtime_result
            .map_err(ServiceError::Runtime)
            .and_then(|()| capture_result.map_err(ServiceError::Capture));
        let mut lifecycle = self.lock_lifecycle();
        lifecycle.status = match &result {
            Ok(()) => ServiceStatus::Stopped,
            Err(error) => ServiceStatus::Failed {
                local_addr: match &lifecycle.status {
                    ServiceStatus::Stopping { local_addr } => Some(*local_addr),
                    ServiceStatus::Failed { local_addr, .. } => *local_addr,
                    _ => None,
                },
                message: error.to_string(),
            },
        };
        result
    }

    async fn start_runner(
        &self,
        runner: Box<dyn SessionRunner>,
    ) -> Result<SocketAddr, ServiceError> {
        let _operation = self.inner.operation.lock().await;
        let old_task = {
            let mut lifecycle = self.lock_lifecycle();
            if matches!(
                lifecycle.status,
                ServiceStatus::Running { .. } | ServiceStatus::Stopping { .. }
            ) {
                return Err(ServiceError::AlreadyRunning);
            }
            lifecycle.task.take()
        };
        if let Some(task) = old_task {
            task.shutdown.cancel();
            let _ = task.handle.await;
        }

        let local_addr = runner.local_addr();
        let control = runner.control();
        let shutdown = ExchangeCancellation::new();
        let task_shutdown = shutdown.clone();
        let inner = Arc::clone(&self.inner);
        let generation = {
            let mut lifecycle = self.lock_lifecycle();
            lifecycle.next_generation = lifecycle.next_generation.saturating_add(1);
            lifecycle.next_generation
        };
        let (published, wait_until_published) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = wait_until_published.await;
            let result = runner.run(task_shutdown).await;
            let mut lifecycle = inner
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lifecycle
                .task
                .as_ref()
                .is_none_or(|task| task.generation == generation)
            {
                match &result {
                    Err(message) => {
                        lifecycle.status = ServiceStatus::Failed {
                            local_addr: Some(local_addr),
                            message: message.clone(),
                        };
                    }
                    Ok(()) if matches!(lifecycle.status, ServiceStatus::Running { .. }) => {
                        lifecycle.status = ServiceStatus::Stopped;
                    }
                    Ok(()) => {}
                }
            }
            result
        });
        let mut lifecycle = self.lock_lifecycle();
        lifecycle.status = ServiceStatus::Running { local_addr };
        lifecycle.task = Some(RunTask {
            generation,
            local_addr,
            shutdown,
            control,
            handle,
        });
        drop(lifecycle);
        let _ = published.send(());
        Ok(local_addr)
    }

    fn lock_lifecycle(&self) -> std::sync::MutexGuard<'_, Lifecycle> {
        self.inner
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

type RunnerFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

trait SessionRunner: Send {
    fn local_addr(&self) -> SocketAddr;
    fn control(&self) -> Option<ProxyControl>;
    fn run(self: Box<Self>, shutdown: ExchangeCancellation) -> RunnerFuture;
}

struct ProxyRunner {
    server: ProxyServer,
    local_addr: SocketAddr,
    control: ProxyControl,
    websocket: tokio::sync::broadcast::Receiver<rustymiddle_runtime::WebSocketSessionEvidence>,
    catalog: SessionCatalog,
}

impl SessionRunner for ProxyRunner {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn control(&self) -> Option<ProxyControl> {
        Some(self.control.clone())
    }

    fn run(self: Box<Self>, shutdown: ExchangeCancellation) -> RunnerFuture {
        Box::pin(async move {
            let ProxyRunner {
                server,
                mut websocket,
                catalog,
                ..
            } = *self;
            let serve = server.serve(shutdown.cancelled());
            tokio::pin!(serve);
            let mut websocket_open = true;
            loop {
                tokio::select! {
                    result = &mut serve => return result.map_err(|error| error.to_string()),
                    evidence = websocket.recv(), if websocket_open => match evidence {
                        Ok(evidence) => { catalog.apply_websocket(evidence); }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            catalog.record_websocket_lag(count);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            websocket_open = false;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    struct TestRunner {
        address: SocketAddr,
        immediate_failure: Option<String>,
    }

    impl SessionRunner for TestRunner {
        fn local_addr(&self) -> SocketAddr {
            self.address
        }

        fn control(&self) -> Option<ProxyControl> {
            None
        }

        fn run(self: Box<Self>, shutdown: ExchangeCancellation) -> RunnerFuture {
            Box::pin(async move {
                if let Some(failure) = self.immediate_failure {
                    return Err(failure);
                }
                shutdown.cancelled().await;
                Ok(())
            })
        }
    }

    fn runner(failure: Option<&str>) -> Box<dyn SessionRunner> {
        Box::new(TestRunner {
            address: "127.0.0.1:43210".parse().unwrap(),
            immediate_failure: failure.map(str::to_owned),
        })
    }

    #[tokio::test]
    async fn start_twice_is_typed_and_stop_is_idempotent() {
        let service = ApplicationSessionService::new(ServiceConfig::default()).unwrap();
        let address = service.start_runner(runner(None)).await.unwrap();
        assert_eq!(
            service.status(),
            ServiceStatus::Running {
                local_addr: address
            }
        );
        assert_eq!(
            service.start_runner(runner(None)).await,
            Err(ServiceError::AlreadyRunning)
        );
        service.stop().await.unwrap();
        service.stop().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn unexpected_runner_failure_becomes_visible_and_restartable() {
        let service = ApplicationSessionService::new(ServiceConfig::default()).unwrap();
        service
            .start_runner(runner(Some("accept failed")))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !matches!(service.status(), ServiceStatus::Failed { .. }) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        service.start_runner(runner(None)).await.unwrap();
        assert!(matches!(service.status(), ServiceStatus::Running { .. }));
        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_stop_waits_for_one_owned_run() {
        let service = ApplicationSessionService::new(ServiceConfig::default()).unwrap();
        service.start_runner(runner(None)).await.unwrap();
        let first = {
            let service = service.clone();
            tokio::spawn(async move { service.stop().await })
        };
        let second = {
            let service = service.clone();
            tokio::spawn(async move { service.stop().await })
        };
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }
}
