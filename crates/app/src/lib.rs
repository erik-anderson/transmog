#![deny(missing_docs)]

//! UI-neutral product operations and bounded presentation read models.
//!
//! This crate is the application boundary shared by the desktop shell and
//! future command-line frontends. It deliberately has no Tauri, `WebUI`,
//! `WebView`, or operating-system dependency.

mod artifacts;
mod automation;
mod autoresponse_batch;
mod body_store;
mod breakpoints;
mod captured_page;
mod captured_page_report;
mod composer;
mod diagnostics;
mod export_privacy;
mod inspector;
mod lifecycle;
mod preview;
mod product_state;
mod request_actions;
mod response_assets;
mod response_file;
mod response_filename;
mod scripts;
mod search;
mod sessions;
mod trace_body;
mod trace_save;
mod traces;
mod version;
mod workspace;

use std::{path::PathBuf, sync::Arc};

pub use artifacts::{
    CaptureReadModel, CaptureStartRequest, CaptureSummaryView, ExportFormat, ExportRequest,
    ExportResult, ImportRequest,
};
pub use automation::{
    AutoResponseTestInput, AutoResponseTestResult, AutomationCandidate, AutomationRuleSet,
    AutomationStatus, ExampleResult, RuleDiagnostic, RuleUsage,
};
pub use autoresponse_batch::{AutoResponseBatchInput, AutoResponseBatchResult};
pub use body_store::{
    BodyAvailability, BodyRange, BodyReadLease, BodyStore, BodyStoreConfig, BodyStoreCounters,
    BodyStoreError, BufferLimit, BufferStatus, BufferStorage, DEFAULT_BODY_READ_BYTES,
    RetentionMode, StoredBodyMetadata,
};
pub use breakpoints::{
    BreakpointDecision, BreakpointPhaseInput, BreakpointSettings, BreakpointStatus, PausedExchange,
};
pub use captured_page::{CapturedPage, CapturedResource};
pub use captured_page_report::{
    CapturedPageDiagnostics, CapturedPageOptions, CapturedPageReport, CapturedPageScope,
    CapturedPreviewRequest, CapturedResourceDecision,
};
pub use composer::{
    ComposerBodySource, ComposerHeader, ComposerOrigin, ComposerRequest, ComposerResult,
    ComposerSnapshot, SystemReplayExecutor,
};
pub use diagnostics::{
    DiagnosticEvent, DiagnosticLevel, DiagnosticsReport, RuntimeDiagnostics, SupportBundleRequest,
    SupportBundleResult,
};
pub use inspector::{
    AutoResponseMatchView, BodyInspection, BodyInspectionRequest, BodyRepresentation, BodyView,
    HeaderPage, HeaderSummary, HeaderView, SessionDetail,
};
pub use lifecycle::{CaCreateRequest, CaIdentity, ProxyRoute, ProxyStartRequest};
pub use product_state::{
    ArtifactKind, PrivacySettings, ProductPreferences, ProductState, RecentArtifact,
    ThemePreference, WindowState,
};
pub use request_actions::{ComposerSource, RequestCommand, RequestCommandFormat, RequestFile};
pub use response_assets::{
    AuthoredResponseAsset, ImportResponseAsset, ResponseAsset, ResponseAssetEdit,
    ResponseAssetInspection, ResponseAssetProvenance, SessionResponseAsset,
};
pub use response_file::{ResponseFile, ResponseFileResult};
pub use scripts::{ScriptCandidate, ScriptDraft, ScriptRevision, ScriptStatus};
pub use search::{
    TrafficSearchEntry, TrafficSearchMatch, TrafficSearchMode, TrafficSearchProgress,
    TrafficSearchRequest, TrafficSearchResult,
};
use serde::Serialize;
pub use sessions::{
    ClientIdentityView, FilterOperator, SessionColumnFilter, SessionHint, SessionPage,
    SessionQueryInput, SessionSort, SessionSummary, SessionUpdateSubscription, SortDirection,
};
use thiserror::Error;
pub use trace_save::{TraceSaveOptions, TraceSaveResult};
pub use traces::{TraceImportProgress, TraceImportRequest, TraceImportResult, TraceMetadataView};
pub use transmog_script::{ScriptAction, ScriptInvocation};
use transmog_session::{
    ApplicationSessionService, HostIntegration, ReplayExecutor, ServiceConfig, ServiceError,
    ServiceStatus,
};
pub use version::application_version;
pub use workspace::{ColumnPreference, TrafficColumn, TrafficLayout, WorkspacePreferences};

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
    /// Host proxy settings are restored while admitted work finishes.
    Draining,
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
    /// Optional crash-safe script workspace prefix.
    pub script_workspace_path: Option<PathBuf>,
    /// Exact packaged isolated script-host executable.
    pub script_host_executable: Option<PathBuf>,
    /// Exact packaged isolated raster preview worker executable.
    pub preview_worker_executable: Option<PathBuf>,
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
            .field("script_workspace_path", &self.script_workspace_path)
            .field("script_host_executable", &self.script_host_executable)
            .field("preview_worker_executable", &self.preview_worker_executable)
            .finish()
    }
}

/// Cloneable owner of authoritative application state.
#[derive(Clone)]
pub struct Application {
    service: ApplicationSessionService,
    runtime_ids: Arc<transmog_runtime::AtomicRuntimeIdGenerator>,
    cursors: Arc<std::sync::Mutex<sessions::CursorRegistry>>,
    breakpoints: breakpoints::BreakpointManager,
    composer: composer::ComposerManager,
    product_state: product_state::ProductStateManager,
    diagnostics: diagnostics::DiagnosticLog,
    body_store: Option<BodyStore>,
    automation: automation::AutomationRegistry,
    response_assets: response_assets::ResponseAssetStore,
    scripts: scripts::ScriptRegistry,
    previews: preview::PreviewService,
    traces: traces::TraceRegistry,
    searches: search::SearchRegistry,
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
        let (product_state, warning) =
            product_state::ProductStateManager::load(config.product_state_path);
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
        let scripts = scripts::ScriptRegistry::load(
            config.script_workspace_path,
            config.script_host_executable,
            Arc::new(response_assets.clone()),
        )?;
        let previews = preview::PreviewService::new(config.preview_worker_executable);
        let build_id = Arc::clone(&config.service.control_build_id);
        let service = ApplicationSessionService::new(config.service).map_err(AppError::from)?;
        let diagnostics = diagnostics::DiagnosticLog::new(config.diagnostics_log_path);
        diagnostics.record(
            diagnostics::DiagnosticLevel::Info,
            "application",
            "startup",
            "Transmog application initialized",
        );
        service.set_redact_sensitive_headers(
            product_state.snapshot().privacy.redact_sensitive_headers,
        );
        if let Some(store) = &body_store {
            let privacy = product_state.snapshot().privacy;
            store
                .set_buffer_limit(privacy.buffer_limit)
                .map_err(|error| {
                    AppError::new(ErrorCategory::Unavailable, error.to_string(), true)
                })?;
            store.set_request_body_limit(privacy.request_body_limit);
            store.set_privacy(
                privacy.retain_request_bodies,
                privacy.retain_response_bodies,
                privacy.redact_sensitive_headers,
            );
            store.set_mode(
                if privacy.retain_response_bodies || privacy.retain_request_bodies {
                    RetentionMode::Circular
                } else {
                    RetentionMode::Off
                },
            );
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
            runtime_ids: Arc::new(transmog_runtime::AtomicRuntimeIdGenerator::new()),
            cursors: Arc::new(std::sync::Mutex::new(sessions::CursorRegistry::default())),
            breakpoints: breakpoints::BreakpointManager::new(service, build_id),
            composer: composer::ComposerManager::new(config.replay_executor),
            product_state,
            diagnostics,
            body_store,
            automation,
            response_assets,
            scripts,
            previews,
            traces: traces::TraceRegistry::default(),
            searches: search::SearchRegistry::default(),
        })
    }

    /// Returns validated product preferences and window/artifact state.
    pub fn product_state(&self) -> ProductState {
        self.product_state.snapshot()
    }

    /// Saves non-sensitive workspace presentation without overwriting other settings.
    ///
    /// # Errors
    /// Returns a bounded validation or persistence failure.
    pub fn save_workspace_preferences(
        &self,
        preferences: WorkspacePreferences,
    ) -> Result<WorkspacePreferences, AppError> {
        self.product_state.save_workspace(preferences)
    }

    /// Validates and crash-safely persists product state when configured.
    ///
    /// # Errors
    /// Returns a bounded validation or persistence error. A persistence error
    /// never changes proxy lifecycle state or prevents shutdown.
    pub fn save_product_state(&self, state: ProductState) -> Result<ProductState, AppError> {
        let state = product_state::validate(state)?;
        if let Some(store) = &self.body_store {
            store
                .prepare_buffer_limit(state.privacy.buffer_limit)
                .map_err(|error| {
                    AppError::new(ErrorCategory::Unavailable, error.to_string(), true)
                })?;
        }
        let result = self.product_state.save(state);
        if let Ok(state) = &result {
            self.service
                .set_redact_sensitive_headers(state.privacy.redact_sensitive_headers);
        }
        if let (Ok(state), Some(store)) = (&result, &self.body_store) {
            store
                .set_buffer_limit(state.privacy.buffer_limit)
                .map_err(|error| {
                    AppError::new(ErrorCategory::Unavailable, error.to_string(), true)
                })?;
            store.set_request_body_limit(state.privacy.request_body_limit);
            store.set_privacy(
                state.privacy.retain_request_bodies,
                state.privacy.retain_response_bodies,
                state.privacy.redact_sensitive_headers,
            );
            store.set_mode(
                if state.privacy.retain_response_bodies || state.privacy.retain_request_bodies {
                    RetentionMode::Circular
                } else {
                    RetentionMode::Off
                },
            );
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

    /// Records one bounded, redacted operational event from a presentation
    /// adapter.
    ///
    /// Presentation layers use this for events that occur outside the
    /// UI-neutral application operations, such as `WebView` failures or native
    /// command dispatch. Messages pass through the same bounds and redaction
    /// policy as application-owned diagnostics.
    pub fn record_diagnostic(
        &self,
        level: DiagnosticLevel,
        component: &str,
        code: &str,
        message: &str,
    ) {
        self.diagnostics.record(level, component, code, message);
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
        let mut status = self.automation.status();
        let assets = self
            .response_assets
            .list()
            .into_iter()
            .map(|asset| (asset.asset_ref(), asset))
            .collect::<std::collections::HashMap<_, _>>();
        for diagnostic in &mut status.diagnostics {
            let response = |id: &str| {
                status
                    .rules
                    .iter()
                    .find(|rule| rule.id == id)
                    .and_then(|rule| rule.request.response_asset.as_ref())
                    .and_then(|reference| assets.get(reference))
            };
            if let (Some(left), Some(right)) = (
                response(&diagnostic.rule_id),
                response(&diagnostic.superseded_by),
            ) {
                diagnostic.duplicate_response = left.status == right.status
                    && left.headers == right.headers
                    && left.sha256 == right.sha256;
            }
        }
        let mut usage = std::collections::BTreeMap::<String, RuleUsage>::new();
        for (id, time) in self
            .service
            .catalog()
            .project_retained(|snapshot| {
                inspector::auto_response_match(snapshot)
                    .map(|matched| (matched.rule_id, snapshot.metadata.started_at))
            })
            .into_iter()
            .flatten()
        {
            let last = u64::try_from(
                time.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            let item = usage.entry(id.clone()).or_insert(RuleUsage {
                rule_id: id,
                matches: 0,
                last_matched_at: 0,
            });
            item.matches += 1;
            item.last_matched_at = item.last_matched_at.max(last);
        }
        status.usage = usage.into_values().collect();
        status
    }

    /// Changes the global autoresponse hook gate, preserving all rule properties.
    ///
    /// # Errors
    /// Rejects stale generations or failed persistence without changing active traffic.
    pub fn set_autoresponses_enabled(
        &self,
        enabled: bool,
        generation: u64,
    ) -> Result<AutomationStatus, AppError> {
        self.automation
            .set_autoresponses_enabled(enabled, generation)?;
        Ok(self.automation_status())
    }

    /// Tests a draft rule against a synthetic request without network traffic.
    ///
    /// # Errors
    /// Rejects invalid URLs, expressions, headers or finite resource bounds.
    pub fn test_autoresponse_match(
        &self,
        input: &AutoResponseTestInput,
    ) -> Result<AutoResponseTestResult, AppError> {
        self.automation.test_match(input)
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

    /// Returns active script revisions and candidate/history counts.
    pub fn script_status(&self) -> ScriptStatus {
        self.scripts.status()
    }

    /// Authoritatively compiles a script draft without changing traffic.
    ///
    /// # Errors
    /// Returns bounded manifest, TypeScript, capability, or resource errors.
    pub fn validate_script(&self, draft: ScriptDraft) -> Result<ScriptCandidate, AppError> {
        self.scripts.validate(draft)
    }

    /// Saves a bounded script draft without compiling or activating it.
    ///
    /// # Errors
    /// Returns a shape, resource, or crash-safe persistence error.
    pub fn save_script(&self, draft: ScriptDraft) -> Result<ScriptStatus, AppError> {
        self.scripts.save(draft)
    }

    /// Executes one synthetic invocation in a fresh production sandbox.
    ///
    /// # Errors
    /// Returns candidate, sandbox, runtime, or action errors without activation.
    pub async fn test_script(
        &self,
        candidate_id: &str,
        invocation: ScriptInvocation,
    ) -> Result<ScriptAction, AppError> {
        self.scripts.test(candidate_id, invocation).await
    }

    /// Returns the exact versioned TypeScript API declarations bundled in Rust.
    pub fn script_declarations(&self) -> &'static str {
        transmog_script::typescript_declarations()
    }

    /// Starts a sandboxed host and atomically activates a validated revision.
    ///
    /// # Errors
    /// Returns a stale candidate, sandbox startup, or persistence failure.
    pub fn activate_script(&self, candidate_id: &str) -> Result<ScriptStatus, AppError> {
        self.scripts.activate(candidate_id)
    }

    /// Disables one active script for newly admitted exchanges.
    ///
    /// # Errors
    /// Returns a persistence error without changing the active snapshot.
    pub fn disable_script(&self, script_id: &str) -> Result<ScriptStatus, AppError> {
        self.scripts.disable(script_id)
    }

    /// Lists durable immutable response assets.
    pub fn response_assets(&self) -> Vec<ResponseAsset> {
        self.response_assets.list()
    }

    /// Inspects saved response text independently of its original traffic source.
    ///
    /// # Errors
    /// Rejects unavailable revisions, bytes or content decoding failures.
    pub async fn inspect_response_asset(
        &self,
        reference: &str,
    ) -> Result<ResponseAssetInspection, AppError> {
        self.response_assets.inspect(reference).await
    }

    /// Creates a new immutable saved response revision.
    ///
    /// # Errors
    /// Rejects invalid headers, bodies, revisions or persistence failures.
    pub async fn edit_response_asset(
        &self,
        input: ResponseAssetEdit,
    ) -> Result<ResponseAsset, AppError> {
        self.response_assets.edit(input).await
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
    pub async fn create_response_asset_from_session(
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
        let exchange_id = response_assets::parse_exchange_id(&input.exchange_id)?;
        let boundary = response_assets::parse_response_boundary(&input.boundary)?;
        let metadata = body_store
            .metadata(exchange_id)
            .into_iter()
            .find(|candidate| candidate.boundary == input.boundary)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "client-visible response body metadata is unavailable",
                    false,
                )
            })?;
        let mut head = body_store
            .response_head(exchange_id, boundary)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "client-visible response headers are unavailable",
                    false,
                )
            })?;
        let encoded_body_override = if let Some(decoded) = input.decoded_body.clone() {
            if decoded.len() > body_store::MAX_BODY_READ_BYTES {
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "edited decoded response exceeds the sixteen MiB limit",
                    false,
                ));
            }
            if input.preserve_content_encoding {
                let encoded = inspector::encode_content(&metadata.content_codings, decoded).await?;
                response_assets::set_content_encoding(
                    &mut head.headers,
                    &metadata.content_codings,
                )?;
                Some(encoded)
            } else {
                head.headers.remove_all("content-encoding");
                Some(decoded)
            }
        } else {
            None
        };
        self.response_assets.create_from_session(
            body_store,
            input,
            head.status,
            &head.headers,
            metadata.media_type,
            encoded_body_override,
        )
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
            ServiceStatus::Draining { local_addr } => {
                let active = self.service.proxy_control().map_or(0, |control| {
                    let activity = control.activity();
                    activity.requests + activity.transports
                });
                AppStatus {
                    lifecycle: AppLifecycle::Draining,
                    listener: Some(local_addr.to_string()),
                    summary: format!(
                        "Finishing {active} active request/connection{}",
                        if active == 1 { "" } else { "s" }
                    ),
                    host_restore_pending: pending,
                }
            }
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

    /// Turns proxy routing off before draining existing work without a deadline.
    ///
    /// # Errors
    /// Returns host restoration failures while keeping admitted work usable.
    pub async fn stop_proxy(&self) -> Result<AppStatus, AppError> {
        self.service.begin_drain().await.map_err(AppError::from)?;
        Ok(self.status())
    }

    /// Resumes an existing pending drain without creating a second listener.
    ///
    /// # Errors
    /// Returns exact host configuration recovery/application failures.
    pub async fn resume_proxy(&self) -> Result<AppStatus, AppError> {
        self.service.resume_drain().await.map_err(AppError::from)?;
        Ok(self.status())
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
        if self.service.resume_drain().await.map_err(AppError::from)? {
            return Ok(self.status());
        }
        let result = lifecycle::start_proxy(
            &self.service,
            Arc::clone(&self.runtime_ids),
            self.body_store.as_ref(),
            self.automation.clone(),
            self.scripts.clone(),
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
        let matching = self.searches.resolve(query.search_result_id.as_deref())?;
        sessions::query_sessions_with_traces(
            &self.service,
            &self.cursors,
            query,
            &self.traces,
            matching.as_deref(),
        )
    }

    /// Returns every entry matching the current filters and search snapshot.
    ///
    /// # Errors
    /// Returns invalid filters or an expired search handle.
    pub fn matching_traffic_ids(&self, query: &SessionQueryInput) -> Result<Vec<String>, AppError> {
        let matching = self.searches.resolve(query.search_result_id.as_deref())?;
        sessions::matching_ids(&self.service, query, &self.traces, matching.as_deref())
    }

    /// Searches captured evidence without changing traffic or selections.
    ///
    /// # Errors
    /// Returns invalid expressions, bounded search failures or cancellation.
    pub async fn search_traffic(
        &self,
        request: TrafficSearchRequest,
        progress: Arc<dyn Fn(TrafficSearchProgress) + Send + Sync>,
    ) -> Result<TrafficSearchResult, AppError> {
        self.searches
            .search(
                request,
                self.service.clone(),
                self.body_store.clone(),
                progress,
            )
            .await
    }

    /// Returns bounded highlighted original-text locations for one saved result.
    ///
    /// # Errors
    /// Returns expired search, missing entry, invalid identity or decoder failures.
    pub async fn traffic_search_entry(
        &self,
        search_id: &str,
        id: &str,
    ) -> Result<TrafficSearchEntry, AppError> {
        self.searches
            .entry(search_id, id, self.service.clone(), self.body_store.clone())
            .await
    }

    /// Cancels one window's current search without publishing partial results.
    pub fn cancel_traffic_search(&self, operation_id: &str) {
        self.searches.cancel(operation_id);
    }

    /// Removes selected rows from Traffic, retaining bounded Undo evidence.
    ///
    /// # Errors
    /// Rejects oversized selections or malformed identifiers before changing the view.
    pub fn remove_traffic_entries(
        &self,
        ids: &[String],
        restore: bool,
    ) -> Result<Vec<String>, AppError> {
        if ids.len() > 200_000 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Select at most 200000 traffic entries.",
                false,
            ));
        }
        let ids = ids
            .iter()
            .map(|id| response_assets::parse_exchange_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let changed = if restore {
            self.service.catalog().restore_dismissed(&ids)
        } else {
            self.service.catalog().dismiss(&ids)
        };
        Ok(changed.iter().map(|id| format!("{:032x}", id.0)).collect())
    }

    /// Removes all unselected entries across the whole workspace, with Undo.
    ///
    /// # Errors
    /// Rejects oversized or malformed selections before changing the catalog.
    pub fn remove_unselected_traffic_entries(
        &self,
        ids: &[String],
    ) -> Result<Vec<String>, AppError> {
        if ids.len() > 200_000 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Selection exceeds the entry limit",
                false,
            ));
        }
        let selected = ids
            .iter()
            .map(|id| response_assets::parse_exchange_id(id))
            .collect::<Result<std::collections::HashSet<_>, _>>()?;
        Ok(self
            .service
            .catalog()
            .dismiss_unselected(&selected)
            .iter()
            .map(|id| format!("{:032x}", id.0))
            .collect())
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
    /// Returns an invalid identifier, unavailable/evicted session, or body
    /// metadata synchronization error.
    pub fn session_detail(&self, id: &str) -> Result<SessionDetail, AppError> {
        let mut detail = inspector::session_detail(&self.service, self.body_store.as_ref(), id)?;
        if let Some(entry) = self.traces.entry(id) {
            let native = entry.raw_headers.is_none();
            detail.trace_id = Some(entry.trace_id);
            detail.original_id = Some(entry.original_id);
            detail.saved_evidence = entry.timings;
            detail.diagnostics.extend(entry.diagnostics);
            if !entry.protocol_known || native {
                for head in detail
                    .requests
                    .iter_mut()
                    .chain(detail.responses.iter_mut())
                {
                    if !detail
                        .performance
                        .protocols
                        .iter()
                        .any(|item| item.boundary == head.boundary)
                    {
                        head.protocol = "Unavailable".into();
                    }
                }
            }
        }
        Ok(detail)
    }

    /// Produces clipboard text without starting a process or sending traffic.
    ///
    /// # Errors
    /// Returns invalid selection, non-text header or unavailable body errors.
    pub fn request_command(
        &self,
        id: &str,
        format: RequestCommandFormat,
    ) -> Result<RequestCommand, AppError> {
        let mut command =
            request_actions::command(&self.service, self.body_store.as_ref(), id, format)?;
        if self
            .traces
            .entry(id)
            .is_some_and(|entry| !entry.protocol_known)
        {
            command.notices.push(
                "The HTTP version was not recorded in this trace; the command uses HTTP/1.1."
                    .into(),
            );
        }
        Ok(command)
    }

    /// Reads a bounded page with measurements from the complete header block.
    ///
    /// # Errors
    /// Returns an invalid selection or unavailable message stage.
    pub fn inspect_headers(
        &self,
        id: &str,
        boundary: &str,
        offset: usize,
        largest_first: bool,
    ) -> Result<inspector::HeaderPage, AppError> {
        inspector::header_page(&self.service, id, boundary, offset, largest_first)
    }

    /// Copies complete original headers for the selected message stage.
    ///
    /// # Errors
    /// Returns unavailable evidence or non-text header values.
    pub fn copy_message_headers(&self, id: &str, boundary: &str) -> Result<String, AppError> {
        inspector::copy_message_headers(&self.service, id, boundary)
    }

    /// Copies full request and response heads separated by two blank lines.
    ///
    /// # Errors
    /// Returns invalid selection or non-text header errors.
    pub fn copy_all_headers(&self, id: &str) -> Result<String, AppError> {
        if let Some(entry) = self.traces.entry(id)
            && let Some(headers) = entry.raw_headers
        {
            return Ok(headers);
        }
        request_actions::copy_all_headers(&self.service, id)
    }

    /// Imports saved evidence into this window's Traffic catalog atomically.
    ///
    /// # Errors
    /// Returns invalid files, bounded import failures, collisions or cancellation.
    pub async fn import_trace(
        &self,
        request: TraceImportRequest,
        progress: Arc<dyn Fn(TraceImportProgress) + Send + Sync>,
    ) -> Result<TraceImportResult, AppError> {
        let store = self.body_store.clone().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "Saved trace body storage is not configured",
                false,
            )
        })?;
        self.traces
            .import(request, self.service.clone(), store, progress)
            .await
    }

    /// Cancels an in-progress saved-file import before catalog publication.
    pub fn cancel_trace_import(&self, operation_id: &str) {
        self.traces.cancel(operation_id);
    }

    /// Freezes retained responses for an isolated captured HTML preview.
    ///
    /// # Errors
    /// Returns unavailable HTML, bounded decoding or storage failures.
    pub async fn prepare_captured_page(
        &self,
        id: String,
        canceled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<CapturedPage, AppError> {
        captured_page::prepare(self.clone(), id, CapturedPageOptions::default(), canceled).await
    }

    /// Freezes response variants within an explicit captured resource scope.
    ///
    /// # Errors
    /// Returns unavailable HTML, request signatures, decoding or storage failures.
    pub async fn prepare_captured_page_with_options(
        &self,
        id: String,
        options: CapturedPageOptions,
        canceled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<CapturedPage, AppError> {
        captured_page::prepare(self.clone(), id, options, canceled).await
    }

    /// Saves every retained Traffic entry with its original trace association.
    ///
    /// # Errors
    /// Returns bounded storage, metadata collection, or output failures.
    pub async fn save_traffic_trace(
        &self,
        destination: PathBuf,
        options: TraceSaveOptions,
    ) -> Result<TraceSaveResult, AppError> {
        trace_save::save(self.clone(), destination, options).await
    }

    /// Lists source metadata for every trace in this window's saved workspace.
    pub fn trace_metadata_list(&self) -> Vec<TraceMetadataView> {
        self.traces.list()
    }

    /// Resolves one saved source's context, including its original network data.
    ///
    /// # Errors
    /// Returns an unavailable error for a stale or unknown trace identity.
    pub fn trace_metadata(&self, id: &str) -> Result<TraceMetadataView, AppError> {
        self.traces.metadata(id).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "Trace metadata is unavailable",
                false,
            )
        })
    }

    /// Protects complete encoded request bytes while a native save dialog opens.
    ///
    /// # Errors
    /// Returns invalid selection or missing/incomplete body errors.
    pub fn prepare_request_file(&self, id: &str) -> Result<RequestFile, AppError> {
        request_actions::prepare_file(&self.service, self.body_store.as_ref(), id)
    }

    /// Loads original headers and complete encoded bytes into an editable draft.
    ///
    /// # Errors
    /// Returns invalid selection or non-text header errors.
    pub fn composer_source(&self, id: &str) -> Result<ComposerSource, AppError> {
        request_actions::composer_source(&self.service, self.body_store.as_ref(), id)
    }

    /// Prepares a complete original response for an explicitly requested save.
    /// Holds an eviction lease until the caller saves or cancels the operation.
    ///
    /// # Errors
    /// Returns invalid selection, unavailable body, or incomplete capture errors.
    pub fn prepare_response_file(
        &self,
        session_id: &str,
        boundary: &str,
    ) -> Result<ResponseFile, AppError> {
        response_file::prepare(
            &self.service,
            self.body_store.as_ref(),
            session_id,
            boundary,
        )
    }

    /// Returns one bounded safe representation of a retained body boundary.
    ///
    /// # Errors
    /// Returns typed identifier, boundary, availability, decoding, or limit
    /// failures without exposing filesystem paths.
    pub async fn inspect_body(
        &self,
        mut request: BodyInspectionRequest,
    ) -> Result<BodyInspection, AppError> {
        if request.representation == BodyRepresentation::Auto
            && let Some(store) = &self.body_store
            && let Ok(id) = inspector::parse_session_id(&request.session_id)
            && store
                .metadata(transmog_core::intercept::ExchangeId(id))
                .iter()
                .any(|body| {
                    body.boundary == request.boundary
                        && body.retained_bytes > 0
                        && body
                            .media_type
                            .as_deref()
                            .is_some_and(|mime| mime.to_ascii_lowercase().starts_with("image/"))
                })
        {
            request.representation = BodyRepresentation::Image;
        }
        if request.representation == BodyRepresentation::Image {
            let (metadata, source, decoded) =
                inspector::image_source(self.body_store.as_ref(), &request).await?;
            let display_bytes = source.len();
            let media_type = metadata.media_type.clone();
            let (preview_handle, preview_mime_type) =
                self.previews.create(source, media_type.as_deref()).await?;
            return Ok(BodyInspection {
                metadata,
                representation: "image",
                decoded,
                text_encoding: None,
                display: String::new(),
                bytes_base64: None,
                byte_offset: 0,
                display_bytes,
                truncated: false,
                next_offset: None,
                warning: None,
                preview_handle: Some(preview_handle),
                preview_mime_type: Some(preview_mime_type),
            });
        }
        inspector::inspect_body(self.body_store.as_ref(), request).await
    }

    /// Resolves one opaque, short-lived safe image preview handle.
    pub fn image_preview(&self, handle: &str) -> Option<(Arc<Vec<u8>>, &'static str)> {
        self.previews
            .get(handle)
            .map(|asset| (asset.bytes, asset.mime_type))
    }

    /// Attaches the exclusive same-build breakpoint controller.
    ///
    /// Must be called within a Tokio runtime. The proxy may be stopped.
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
        if request.body_source.is_some() {
            composer::validate_stream_input(&request)?;
        }
        if let Some(id) = &request.source_entry_id {
            inspector::parse_session_id(id)?;
        }
        let source = request.source_entry_id.as_ref().map(|id| {
            let trace = self.traces.entry(id);
            ComposerOrigin {
                trace_name: trace
                    .as_ref()
                    .and_then(|entry| self.traces.metadata(&entry.trace_id))
                    .map(|metadata| metadata.name),
                entry_id: id.clone(),
                trace_id: trace.as_ref().map(|entry| entry.trace_id.clone()),
                original_id: trace.map(|entry| entry.original_id),
            }
        });
        let file = match &request.body_source {
            Some(ComposerBodySource::Captured { entry_id }) => Some(
                self.prepare_request_file(entry_id)?
                    .into_replay_file()
                    .await?,
            ),
            Some(ComposerBodySource::File { path }) => {
                let path = std::path::PathBuf::from(path);
                Some(
                    tokio::task::spawn_blocking(move || {
                        if !path.is_absolute() {
                            return Err(AppError::new(
                                ErrorCategory::InvalidInput,
                                "Choose an absolute body file path",
                                false,
                            ));
                        }
                        let file = std::fs::File::open(path).map_err(|_| {
                            AppError::new(
                                ErrorCategory::Unavailable,
                                "Replay body file could not be opened",
                                true,
                            )
                        })?;
                        let metadata = file.metadata().map_err(|_| {
                            AppError::new(
                                ErrorCategory::Unavailable,
                                "Replay body file metadata is unavailable",
                                true,
                            )
                        })?;
                        if !metadata.is_file() {
                            return Err(AppError::new(
                                ErrorCategory::InvalidInput,
                                "Replay body must be a regular file",
                                false,
                            ));
                        }
                        Ok((file, metadata.len()))
                    })
                    .await
                    .map_err(|_| {
                        AppError::new(
                            ErrorCategory::Unavailable,
                            "Replay body file worker failed",
                            true,
                        )
                    })??,
                )
            }
            None => None,
        };
        self.composer
            .execute(&self.service, request, file, source)
            .await
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
        artifacts::start_capture(
            &self.service,
            request,
            self.product_state().privacy.request_body_limit,
        )
        .await
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

    #[test]
    fn configured_diagnostic_sink_exists_at_startup_and_records_adapter_events() {
        let path = std::env::temp_dir().join(format!(
            "transmog-application-startup-diagnostics-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let application = Application::new(AppConfig {
            diagnostics_log_path: Some(path.clone()),
            ..AppConfig::default()
        })
        .unwrap();

        let startup = std::fs::read_to_string(&path).unwrap();
        assert!(startup.contains("\"code\":\"startup\""));
        application.record_diagnostic(
            DiagnosticLevel::Error,
            "desktop-webview",
            "form-handler-failed",
            "safe frontend failure",
        );
        let events = std::fs::read_to_string(&path).unwrap();
        assert!(events.contains("\"code\":\"form-handler-failed\""));
        std::fs::remove_file(path).unwrap();
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
