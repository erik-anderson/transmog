#![deny(missing_docs)]

//! UI-neutral product operations and bounded presentation read models.
//!
//! This crate is the application boundary shared by the desktop shell and
//! future command-line frontends. It deliberately has no Tauri, `WebUI`,
//! `WebView`, or operating-system dependency.

mod artifacts;
mod automation;
mod body_store;
mod breakpoints;
mod composer;
mod diagnostics;
mod inspector;
mod lifecycle;
mod product_state;
mod response_assets;
mod sessions;

use std::{path::PathBuf, sync::Arc};

pub use artifacts::{
    CaptureReadModel, CaptureStartRequest, CaptureSummaryView, ExportFormat, ExportRequest,
    ExportResult, ImportRequest,
};
pub use automation::{AutomationCandidate, AutomationRuleSet, AutomationStatus};
pub use body_store::{
    BodyAvailability, BodyRange, BodyReadLease, BodyStore, BodyStoreConfig, BodyStoreCounters,
    BodyStoreError, DEFAULT_BODY_READ_BYTES, RetentionMode, StoredBodyMetadata,
};
pub use breakpoints::{
    BreakpointDecision, BreakpointPhaseInput, BreakpointSettings, BreakpointStatus, PausedExchange,
};
pub use composer::{
    ComposerHeader, ComposerRequest, ComposerResult, ComposerSnapshot, SystemReplayExecutor,
};
pub use diagnostics::{
    DiagnosticEvent, DiagnosticLevel, DiagnosticsReport, RuntimeDiagnostics, SupportBundleRequest,
    SupportBundleResult,
};
pub use inspector::{
    BodyInspection, BodyInspectionRequest, BodyRepresentation, BodyView, HeaderView, SessionDetail,
};
pub use lifecycle::{CaCreateRequest, CaIdentity, ProxyRoute, ProxyStartRequest};
pub use product_state::{
    ArtifactKind, PrivacySettings, ProductPreferences, ProductState, RecentArtifact,
    ThemePreference, WindowState,
};
pub use response_assets::{
    AuthoredResponseAsset, ImportResponseAsset, ResponseAsset, ResponseAssetProvenance,
    SessionResponseAsset,
};
use serde::Serialize;
pub use sessions::{
    SessionHint, SessionPage, SessionQueryInput, SessionSummary, SessionUpdateSubscription,
};
use thiserror::Error;
use transmog_session::{
    ApplicationSessionService, HostIntegration, ReplayExecutor, ServiceConfig, ServiceError,
    ServiceStatus,
};

/// Stable application failure category suitable for presentation boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCategory {
    /// Caller input failed validation.
    InvalidInput,
    /// Requested operation conflicts with current application state.
    Conflict,
    /// A finite resource or result limit was reached.
    Limit,
    /// An external resource was unavailable.
    Unavailable,
    /// An operation failed but may be retried without changing its meaning.
    Retryable,
    /// An internal operation failed with a redaction-safe description.
    Internal,
}

/// Bounded, redaction-safe application error.
#[derive(Clone, Debug, Eq, Error, PartialEq, Serialize)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    /// Stable machine-readable category.
    pub category: ErrorCategory,
    /// Operator-safe message.
    pub message: String,
    /// Whether retrying the same action can be useful.
    pub retryable: bool,
}

impl AppError {
    pub(crate) fn new(
        category: ErrorCategory,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        let message = message.into().chars().take(512).collect();
        Self {
            category,
            message,
            retryable,
        }
    }
}

impl From<ServiceError> for AppError {
    fn from(error: ServiceError) -> Self {
        let (category, retryable) = match error {
            ServiceError::AlreadyRunning | ServiceError::FailedRunNeedsStop => {
                (ErrorCategory::Conflict, false)
            }
            ServiceError::HostRestore(_) | ServiceError::HostRestorePending => {
                (ErrorCategory::Retryable, true)
            }
            ServiceError::ListenerAddress(_)
            | ServiceError::Runtime(_)
            | ServiceError::Capture(_)
            | ServiceError::HostApply(_) => (ErrorCategory::Unavailable, true),
        };
        Self::new(category, error.to_string(), retryable)
    }
}

/// Public application lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppLifecycle {
    /// No proxy run is active.
    Stopped,
    /// The proxy listener is active.
    Running,
    /// Graceful shutdown is in progress.
    Stopping,
    /// The last proxy run failed.
    Failed,
}

/// Bounded status read model returned to every presentation layer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// Current lifecycle category.
    pub lifecycle: AppLifecycle,
    /// Bound listener endpoint, when one exists.
    pub listener: Option<String>,
    /// Operator-safe status summary.
    pub summary: String,
    /// Whether exact host restoration remains pending.
    pub host_restore_pending: bool,
}

/// Construction settings for the application facade.
#[derive(Clone, Default)]
pub struct AppConfig {
    /// Session-service limits and same-build identity.
    pub service: ServiceConfig,
    /// Route-aware executor shared by composer/replay operations.
    pub replay_executor: Option<Arc<dyn ReplayExecutor>>,
    /// Optional prefix for crash-safe product-state generations.
    pub product_state_path: Option<PathBuf>,
    /// Optional bounded JSON-lines operational log.
    pub diagnostics_log_path: Option<PathBuf>,
    /// Optional product-layer on-disk response-body cache.
    pub body_store: Option<BodyStoreConfig>,
    /// Optional crash-recoverable built-in automation workspace prefix.
    pub automation_path: Option<PathBuf>,
    /// Optional durable content-addressed response asset directory.
    pub response_asset_root: Option<PathBuf>,
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("service", &self.service)
            .field("replay_executor", &self.replay_executor.is_some())
            .field("product_state_path", &self.product_state_path)
            .field("diagnostics_log_path", &self.diagnostics_log_path)
            .field("body_store", &self.body_store)
            .field("automation_path", &self.automation_path)
            .field("response_asset_root", &self.response_asset_root)
            .finish()
    }
}

/// Cloneable owner of authoritative application state.
#[derive(Clone)]
pub struct Application {
    service: ApplicationSessionService,
    cursors: Arc<std::sync::Mutex<sessions::CursorRegistry>>,
    breakpoints: breakpoints::BreakpointManager,
    composer: composer::ComposerManager,
    product_state: product_state::ProductStateManager,
    diagnostics: diagnostics::DiagnosticLog,
    body_store: Option<BodyStore>,
    automation: automation::AutomationRegistry,
    response_assets: response_assets::ResponseAssetStore,
}

impl std::fmt::Debug for Application {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Application")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Application {
    /// Creates an idle application with finite service state.
    ///
    /// # Errors
    ///
    /// Returns a bounded application error if the owned service cannot start.
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        let body_store = config
            .body_store
            .map(BodyStore::new)
            .transpose()
            .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), true))?;
        let response_assets =
            response_assets::ResponseAssetStore::load(config.response_asset_root)?;
        let automation = automation::AutomationRegistry::load(
            config.automation_path,
            Arc::new(response_assets.clone()),
        )?;
        let build_id = Arc::clone(&config.service.control_build_id);
        let service = ApplicationSessionService::new(config.service).map_err(AppError::from)?;
        let diagnostics = diagnostics::DiagnosticLog::new(config.diagnostics_log_path);
        let (product_state, warning) =
            product_state::ProductStateManager::load(config.product_state_path);
        if let Some(store) = &body_store {
            store.set_mode(if product_state.snapshot().privacy.retain_response_bodies {
                RetentionMode::Circular
            } else {
                RetentionMode::Off
            });
        }
        if let Some(warning) = warning {
            diagnostics.record(
                diagnostics::DiagnosticLevel::Warning,
                "product-state",
                "state-recovered",
                &warning,
            );
        }
        Ok(Self {
            service: service.clone(),
            cursors: Arc::new(std::sync::Mutex::new(sessions::CursorRegistry::default())),
            breakpoints: breakpoints::BreakpointManager::new(service, build_id),
            composer: composer::ComposerManager::new(config.replay_executor),
            product_state,
            diagnostics,
            body_store,
            automation,
            response_assets,
        })
    }

    /// Returns validated product preferences and window/artifact state.
    pub fn product_state(&self) -> ProductState {
        self.product_state.snapshot()
    }

    /// Validates and crash-safely persists product state when configured.
    ///
    /// # Errors
    /// Returns a bounded validation or persistence error. A persistence error
    /// never changes proxy lifecycle state or prevents shutdown.
    pub fn save_product_state(&self, state: ProductState) -> Result<ProductState, AppError> {
        let result = self.product_state.save(state);
        if let (Ok(state), Some(store)) = (&result, &self.body_store) {
            store.set_mode(if state.privacy.retain_response_bodies {
                RetentionMode::Circular
            } else {
                RetentionMode::Off
            });
        }
        match &result {
            Ok(_) => self.diagnostics.record(
                DiagnosticLevel::Info,
                "product-state",
                "state-saved",
                "product state saved",
            ),
            Err(error) => self.diagnostics.record(
                DiagnosticLevel::Warning,
                "product-state",
                "state-save-failed",
                &error.message,
            ),
        }
        result
    }

    /// Adds one bounded recent artifact reference if privacy settings allow it.
    pub fn remember_artifact(&self, path: PathBuf, kind: ArtifactKind) {
        self.product_state.remember(path, kind);
    }

    /// Returns a bounded, redacted operational report.
    pub fn diagnostics(&self, runtime: RuntimeDiagnostics) -> DiagnosticsReport {
        self.diagnostics
            .report(runtime, &self.product_state.snapshot())
    }

    /// Creates a privacy-safe, create-new support bundle.
    ///
    /// # Errors
    /// Returns validation, destination, serialization, or finalization errors.
    pub async fn create_support_bundle(
        &self,
        request: SupportBundleRequest,
    ) -> Result<SupportBundleResult, AppError> {
        let report = self.diagnostics(request.runtime.clone());
        let state = self.product_state.snapshot();
        diagnostics::create_support_bundle(request, report, state).await
    }

    /// Returns the authoritative session service for product use-case modules.
    pub fn session_service(&self) -> &ApplicationSessionService {
        &self.service
    }

    /// Returns the optional product-layer response-body cache.
    pub fn body_store(&self) -> Option<&BodyStore> {
        self.body_store.as_ref()
    }

    /// Returns active native automation and candidate/history counts.
    pub fn automation_status(&self) -> AutomationStatus {
        self.automation.status()
    }

    /// Validates and compiles native automation without changing traffic.
    ///
    /// # Errors
    /// Returns a bounded schema, syntax, conflict, or limit failure.
    pub fn validate_automation(
        &self,
        document: AutomationRuleSet,
    ) -> Result<AutomationCandidate, AppError> {
        self.automation.validate(document)
    }

    /// Atomically activates a previously validated candidate for new exchanges.
    ///
    /// # Errors
    /// Returns a stale-token or durable persistence failure.
    pub fn activate_automation(&self, candidate_id: &str) -> Result<AutomationStatus, AppError> {
        self.automation.activate(candidate_id)
    }

    /// Lists durable immutable response assets.
    pub fn response_assets(&self) -> Vec<ResponseAsset> {
        self.response_assets.list()
    }

    /// Creates a bounded authored response asset.
    ///
    /// # Errors
    /// Returns validation, collision, quota, or persistence errors.
    pub fn create_response_asset(
        &self,
        input: AuthoredResponseAsset,
    ) -> Result<ResponseAsset, AppError> {
        self.response_assets.create_authored(input)
    }

    /// Imports a potentially large response asset without buffering it fully.
    ///
    /// # Errors
    /// Returns validation, source, quota, collision, or persistence errors.
    pub fn import_response_asset(
        &self,
        input: ImportResponseAsset,
    ) -> Result<ResponseAsset, AppError> {
        self.response_assets.import_file(input)
    }

    /// Copies a complete retained response boundary into durable asset storage.
    ///
    /// # Errors
    /// Incomplete, truncated, lossy, evicted, disabled, invalid, or unavailable
    /// bodies are rejected.
    pub fn create_response_asset_from_session(
        &self,
        input: SessionResponseAsset,
    ) -> Result<ResponseAsset, AppError> {
        let body_store = self.body_store.as_ref().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "response body retention is not configured",
                false,
            )
        })?;
        self.response_assets.create_from_session(body_store, input)
    }

    /// Returns a bounded point-in-time lifecycle read model.
    pub fn status(&self) -> AppStatus {
        let pending = self.service.has_pending_host_restore();
        match self.service.status() {
            ServiceStatus::Stopped => AppStatus {
                lifecycle: AppLifecycle::Stopped,
                listener: None,
                summary: "Proxy stopped".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Running { local_addr } => AppStatus {
                lifecycle: AppLifecycle::Running,
                listener: Some(local_addr.to_string()),
                summary: "Proxy listening".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Stopping { local_addr } => AppStatus {
                lifecycle: AppLifecycle::Stopping,
                listener: Some(local_addr.to_string()),
                summary: "Proxy stopping".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Failed {
                local_addr,
                message,
            } => AppStatus {
                lifecycle: AppLifecycle::Failed,
                listener: local_addr.map(|address| address.to_string()),
                summary: message.chars().take(256).collect(),
                host_restore_pending: pending,
            },
        }
    }

    /// Gracefully stops the service and restores any active host transaction.
    ///
    /// # Errors
    ///
    /// Returns a retryable safe error when drain or restoration fails.
    pub async fn shutdown(&self) -> Result<(), AppError> {
        self.breakpoints.disable().await;
        let result = self.service.stop().await.map_err(AppError::from);
        self.diagnostics.record(
            if result.is_ok() {
                DiagnosticLevel::Info
            } else {
                DiagnosticLevel::Error
            },
            "lifecycle",
            if result.is_ok() {
                "shutdown-complete"
            } else {
                "shutdown-failed"
            },
            result
                .as_ref()
                .err()
                .map_or("proxy shutdown complete", |error| error.message.as_str()),
        );
        result
    }

    /// Binds and starts the product proxy using validated local CA material.
    ///
    /// The optional host adapter is caller-owned so this crate remains
    /// operating-system independent.
    ///
    /// # Errors
    /// Returns a bounded validation, bind, lifecycle, or host failure.
    pub async fn start_proxy(
        &self,
        request: ProxyStartRequest,
        host: Option<Arc<dyn HostIntegration>>,
    ) -> Result<AppStatus, AppError> {
        let result = lifecycle::start_proxy(
            &self.service,
            self.body_store.as_ref(),
            self.automation.clone(),
            request,
            host,
        )
        .await;
        self.diagnostics.record(
            if result.is_ok() {
                DiagnosticLevel::Info
            } else {
                DiagnosticLevel::Error
            },
            "lifecycle",
            if result.is_ok() {
                "proxy-started"
            } else {
                "proxy-start-failed"
            },
            result
                .as_ref()
                .err()
                .map_or("proxy listener started", |error| error.message.as_str()),
        );
        result?;
        Ok(self.status())
    }

    /// Retries an exact host restore retained after a failed stop.
    ///
    /// # Errors
    /// Returns a retryable bounded restoration failure.
    pub async fn retry_host_restore(&self) -> Result<AppStatus, AppError> {
        self.service
            .retry_host_restore()
            .await
            .map_err(AppError::from)?;
        Ok(self.status())
    }

    /// Creates a new application-owned interception CA without overwriting files.
    ///
    /// # Errors
    /// Returns a validation, destination, generation, or durable-write failure.
    pub async fn create_ca(&self, request: CaCreateRequest) -> Result<CaIdentity, AppError> {
        lifecycle::create_ca(request).await
    }

    /// Queries one bounded authoritative page of live sessions.
    ///
    /// # Errors
    /// Returns invalid filters, page sizes, cursors, or token-generation failures.
    pub fn query_sessions(&self, query: SessionQueryInput) -> Result<SessionPage, AppError> {
        sessions::query_sessions(&self.service, &self.cursors, query)
    }

    /// Opens a bounded hint-only subscription for presentation refreshes.
    ///
    /// # Errors
    /// Returns a finite subscriber-limit failure.
    pub fn subscribe_session_updates(&self) -> Result<SessionUpdateSubscription, AppError> {
        sessions::subscribe(&self.service)
    }

    /// Returns one bounded, display-safe session inspector read model.
    ///
    /// # Errors
    /// Returns an invalid identifier or unavailable/evicted session error.
    pub fn session_detail(&self, id: &str) -> Result<SessionDetail, AppError> {
        inspector::session_detail(&self.service, self.body_store.as_ref(), id)
    }

    /// Returns one bounded safe representation of a retained body boundary.
    ///
    /// # Errors
    /// Returns typed identifier, boundary, availability, decoding, or limit
    /// failures without exposing filesystem paths.
    pub async fn inspect_body(
        &self,
        request: BodyInspectionRequest,
    ) -> Result<BodyInspection, AppError> {
        inspector::inspect_body(self.body_store.as_ref(), request).await
    }

    /// Attaches the exclusive same-build breakpoint controller.
    ///
    /// # Errors
    /// Returns a configuration, exclusivity, negotiation, or runtime failure.
    pub fn enable_breakpoints(
        &self,
        settings: &BreakpointSettings,
    ) -> Result<BreakpointStatus, AppError> {
        self.breakpoints.enable(settings)
    }

    /// Detaches control and fails all unresolved decisions closed.
    pub async fn disable_breakpoints(&self) -> BreakpointStatus {
        self.breakpoints.disable().await
    }

    /// Returns bounded paused exchanges awaiting an operator decision.
    pub fn paused_exchanges(&self) -> BreakpointStatus {
        self.breakpoints.status()
    }

    /// Submits one correlated, phase-valid breakpoint action.
    ///
    /// # Errors
    /// Returns a stale, invalid, oversized, or failed reply.
    pub fn decide_breakpoint(
        &self,
        decision: BreakpointDecision,
    ) -> Result<BreakpointStatus, AppError> {
        self.breakpoints.decide(decision)
    }

    /// Validates and executes a composer request through the configured
    /// canonical route-aware replay executor.
    ///
    /// # Errors
    /// Returns a validation, acknowledgement, timeout, bounds, or execution failure.
    pub async fn execute_composer(
        &self,
        request: ComposerRequest,
    ) -> Result<ComposerResult, AppError> {
        self.composer.execute(&self.service, request).await
    }

    /// Returns a bounded newest-first replay history without credential values.
    pub fn composer_history(&self) -> Vec<ComposerSnapshot> {
        self.composer.history()
    }

    /// Starts a create-new streaming native capture.
    ///
    /// # Errors
    /// Returns invalid quota, destination, state, queue, or writer failures.
    pub async fn start_capture(
        &self,
        request: CaptureStartRequest,
    ) -> Result<CaptureReadModel, AppError> {
        artifacts::start_capture(&self.service, request).await
    }

    /// Seals and stops the active native capture.
    ///
    /// # Errors
    /// Returns capture state, queue, or durable finalization failures.
    pub async fn stop_capture(&self) -> Result<CaptureReadModel, AppError> {
        artifacts::stop_capture(&self.service).await
    }

    /// Returns current native capture state.
    pub fn capture_status(&self) -> CaptureReadModel {
        artifacts::capture_status(&self.service)
    }

    /// Recovers and summarizes a bounded native capture, including a valid
    /// prefix after an interrupted final frame.
    ///
    /// # Errors
    /// Returns path, format, checksum, or configured bound failures.
    pub async fn import_capture(
        &self,
        request: ImportRequest,
    ) -> Result<CaptureSummaryView, AppError> {
        artifacts::import_capture(request).await
    }

    /// Exports a recovered native capture to streaming JSONL or finalized SAZ.
    ///
    /// # Errors
    /// Returns path, create-new, recovery, quota, or format failures.
    pub async fn export_capture(&self, request: ExportRequest) -> Result<ExportResult, AppError> {
        artifacts::export_capture(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_application_is_stopped_and_ui_neutral() {
        let application = Application::new(AppConfig::default()).unwrap();
        assert_eq!(
            application.status(),
            AppStatus {
                lifecycle: AppLifecycle::Stopped,
                listener: None,
                summary: "Proxy stopped".to_owned(),
                host_restore_pending: false,
            }
        );
        let json = serde_json::to_value(application.status()).unwrap();
        assert_eq!(json["lifecycle"], "stopped");
        assert!(json.get("tauri").is_none());
    }

    #[tokio::test]
    async fn stopped_shutdown_is_idempotent() {
        let application = Application::new(AppConfig::default()).unwrap();
        application.shutdown().await.unwrap();
        application.shutdown().await.unwrap();
        assert_eq!(application.status().lifecycle, AppLifecycle::Stopped);
    }

    #[tokio::test]
    async fn persistence_failure_cannot_prevent_proxy_shutdown() {
        let root = std::env::temp_dir().join(format!(
            "transmog-unwritable-state-parent-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        std::fs::write(&root, b"not a directory").unwrap();
        let application = Application::new(AppConfig {
            product_state_path: Some(root.join("preferences")),
            ..AppConfig::default()
        })
        .unwrap();
        assert!(
            application
                .save_product_state(application.product_state())
                .is_err()
        );
        application.shutdown().await.unwrap();
        assert_eq!(application.status().lifecycle, AppLifecycle::Stopped);
        std::fs::remove_file(root).unwrap();
    }
}
