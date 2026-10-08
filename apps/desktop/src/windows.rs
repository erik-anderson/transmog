#![allow(clippy::needless_pass_by_value)] // Tauri commands deserialize owned IPC arguments.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use percent_encoding::percent_decode_str;
use serde::Serialize;
use tauri::{
    Manager, State, WebviewWindowBuilder,
    http::{Request, Response, StatusCode, header},
    ipc::Channel,
    utils::config::WebviewUrl,
};
use transmog_app::{
    AppConfig, AppError, AppStatus, Application, ArtifactKind, AuthoredResponseAsset,
    AutoResponseTestInput, AutoResponseTestResult, AutomationCandidate, AutomationRuleSet,
    AutomationStatus, BodyInspection, BodyInspectionRequest, BodyStoreConfig, BreakpointDecision,
    BreakpointSettings, BreakpointStatus, CaCreateRequest, CaIdentity, CaptureReadModel,
    CaptureStartRequest, CaptureSummaryView, ComposerRequest, ComposerResult, ComposerSnapshot,
    DiagnosticLevel, DiagnosticsReport, ExportFormat, ExportRequest, ExportResult, ImportRequest,
    ImportResponseAsset, ProductState, ProxyRoute, ProxyStartRequest, ResponseAsset,
    ResponseFileResult, RuntimeDiagnostics, ScriptAction, ScriptCandidate, ScriptDraft,
    ScriptInvocation, ScriptStatus, SessionDetail, SessionHint, SessionPage, SessionQueryInput,
    SessionResponseAsset, SupportBundleRequest, SupportBundleResult, SystemReplayExecutor,
    WindowState, WorkspacePreferences,
};
use transmog_app_webui::{AppRenderer, ShellView, UiError, UiResponse};
use transmog_host_windows::{
    CurrentUserCertificateStore, CurrentUserKeyProtection, OwnedCertificateRegistry,
    WindowsProxyIntegration,
};

const UI_HOST: &str = "transmog-ui.localhost";
const AUTOMATIC_CAPTURE_BYTES: u64 = 1024 * 1024 * 1024;
static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct DesktopState {
    application: Application,
    host: Arc<WindowsProxyIntegration>,
    owned_certificate: OwnedCertificateRegistry,
    ca_certificate_path: PathBuf,
    ca_private_key_path: PathBuf,
    diagnostics_path: PathBuf,
    capture_root: PathBuf,
    export_root: PathBuf,
    live_capture_path: Arc<Mutex<Option<PathBuf>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopBootstrap {
    ca_certificate_path: PathBuf,
    ca_private_key_path: PathBuf,
    ca_files_present: bool,
    owned_ca_sha256: Option<String>,
    owned_ca_trusted: bool,
    host_restore_pending: bool,
    diagnostics_path: PathBuf,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn app_status(state: State<'_, DesktopState>) -> AppStatus {
    state.application.status()
}

#[tauri::command]
fn product_state(state: State<'_, DesktopState>) -> ProductState {
    state.application.product_state()
}

#[tauri::command]
fn save_product_state(
    product_state: ProductState,
    state: State<'_, DesktopState>,
) -> Result<ProductState, AppError> {
    state.application.save_product_state(product_state)
}

#[tauri::command]
fn save_workspace_preferences(
    preferences: WorkspacePreferences,
    state: State<'_, DesktopState>,
) -> Result<WorkspacePreferences, AppError> {
    state.application.save_workspace_preferences(preferences)
}

#[tauri::command]
fn desktop_bootstrap(state: State<'_, DesktopState>) -> Result<DesktopBootstrap, String> {
    let owned_ca_sha256 = state
        .owned_certificate
        .owned_thumbprint()
        .map_err(|error| error.to_string())?;
    let owned_ca_trusted = owned_ca_sha256
        .as_deref()
        .map(|sha256| CurrentUserCertificateStore.contains(sha256))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    Ok(DesktopBootstrap {
        ca_certificate_path: state.ca_certificate_path.clone(),
        ca_private_key_path: state.ca_private_key_path.clone(),
        ca_files_present: state.ca_certificate_path.is_file()
            && state.ca_private_key_path.is_file(),
        owned_ca_sha256,
        owned_ca_trusted,
        host_restore_pending: state.host.recovery_pending(),
        diagnostics_path: state.diagnostics_path.clone(),
    })
}

#[tauri::command]
fn record_frontend_diagnostic(code: String, message: String, state: State<'_, DesktopState>) {
    let code = if code.len() <= 64
        && !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        code
    } else {
        "invalid-frontend-code".to_owned()
    };
    state
        .application
        .record_diagnostic(DiagnosticLevel::Error, "desktop-webview", &code, &message);
}

#[tauri::command]
fn automation_status(state: State<'_, DesktopState>) -> AutomationStatus {
    state.application.automation_status()
}

#[tauri::command]
fn set_autoresponses_enabled(
    enabled: bool,
    generation: u64,
    state: State<'_, DesktopState>,
) -> Result<AutomationStatus, AppError> {
    state
        .application
        .set_autoresponses_enabled(enabled, generation)
}

#[tauri::command]
fn test_autoresponse_match(
    input: AutoResponseTestInput,
    state: State<'_, DesktopState>,
) -> Result<AutoResponseTestResult, AppError> {
    state.application.test_autoresponse_match(&input)
}

#[tauri::command]
fn validate_automation(
    document: AutomationRuleSet,
    state: State<'_, DesktopState>,
) -> Result<AutomationCandidate, AppError> {
    state.application.validate_automation(document)
}

#[tauri::command]
fn activate_automation(
    candidate_id: String,
    state: State<'_, DesktopState>,
) -> Result<AutomationStatus, AppError> {
    state.application.activate_automation(&candidate_id)
}

#[tauri::command]
fn script_status(state: State<'_, DesktopState>) -> ScriptStatus {
    state.application.script_status()
}

#[tauri::command]
fn script_declarations(state: State<'_, DesktopState>) -> String {
    state.application.script_declarations().to_owned()
}

#[tauri::command]
fn save_script(
    draft: ScriptDraft,
    state: State<'_, DesktopState>,
) -> Result<ScriptStatus, AppError> {
    state.application.save_script(draft)
}

#[tauri::command]
fn validate_script(
    draft: ScriptDraft,
    state: State<'_, DesktopState>,
) -> Result<ScriptCandidate, AppError> {
    state.application.validate_script(draft)
}

#[tauri::command]
async fn test_script(
    candidate_id: String,
    invocation: ScriptInvocation,
    state: State<'_, DesktopState>,
) -> Result<ScriptAction, AppError> {
    state
        .application
        .test_script(&candidate_id, invocation)
        .await
}

#[tauri::command]
fn activate_script(
    candidate_id: String,
    state: State<'_, DesktopState>,
) -> Result<ScriptStatus, AppError> {
    state.application.activate_script(&candidate_id)
}

#[tauri::command]
fn disable_script(
    script_id: String,
    state: State<'_, DesktopState>,
) -> Result<ScriptStatus, AppError> {
    state.application.disable_script(&script_id)
}

#[tauri::command]
fn response_assets(state: State<'_, DesktopState>) -> Vec<ResponseAsset> {
    state.application.response_assets()
}

#[tauri::command]
fn create_response_asset(
    input: AuthoredResponseAsset,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    state.application.create_response_asset(input)
}

#[tauri::command]
fn import_response_asset(
    input: ImportResponseAsset,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    state.application.import_response_asset(input)
}

#[tauri::command]
async fn create_response_asset_from_session(
    input: SessionResponseAsset,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    state
        .application
        .create_response_asset_from_session(input)
        .await
}

#[tauri::command]
fn diagnostics_report(state: State<'_, DesktopState>) -> DiagnosticsReport {
    state.application.diagnostics(runtime_diagnostics())
}

#[tauri::command]
async fn create_support_bundle(
    destination: PathBuf,
    include_recent_paths: bool,
    state: State<'_, DesktopState>,
) -> Result<SupportBundleResult, AppError> {
    state
        .application
        .create_support_bundle(SupportBundleRequest {
            destination,
            runtime: runtime_diagnostics(),
            include_recent_paths,
        })
        .await
}

#[tauri::command]
async fn prepare_update_handoff(state: State<'_, DesktopState>) -> Result<AppStatus, String> {
    state
        .application
        .shutdown()
        .await
        .map_err(|error| error.to_string())?;
    if state.host.recovery_pending() {
        return Err("Windows proxy restoration is still pending".to_owned());
    }
    Ok(state.application.status())
}

#[tauri::command]
async fn stop_application(state: State<'_, DesktopState>) -> Result<AppStatus, AppError> {
    state.application.shutdown().await?;
    Ok(state.application.status())
}

#[tauri::command]
async fn start_proxy(
    request: ProxyStartRequest,
    state: State<'_, DesktopState>,
) -> Result<AppStatus, String> {
    if state.host.recovery_pending() {
        return Err(
            "Restore the journaled Windows proxy settings before starting a new proxy run."
                .to_owned(),
        );
    }
    if !state.ca_certificate_path.is_file() || !state.ca_private_key_path.is_file() {
        return Err(
            "Set up the Transmog interception certificate before starting the proxy.".to_owned(),
        );
    }
    let sha256 = state
        .owned_certificate
        .owned_thumbprint()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "Set up the Transmog interception certificate before starting the proxy.".to_owned()
        })?;
    if !CurrentUserCertificateStore
        .contains(&sha256)
        .map_err(|error| error.to_string())?
    {
        return Err(
            "Trust the Transmog interception certificate before starting the proxy.".to_owned(),
        );
    }
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "proxy-start-requested",
        "proxy start requested with automatic capture and current-user Windows proxy integration",
    );
    std::fs::create_dir_all(&state.capture_root)
        .map_err(|error| format!("automatic capture directory is unavailable: {error}"))?;
    let (capture_path, capture_started) = match state.application.capture_status() {
        CaptureReadModel::Active { path, .. } => (path, false),
        CaptureReadModel::Shutdown => {
            return Err("capture service is unavailable until Transmog restarts".to_owned());
        }
        CaptureReadModel::Idle
        | CaptureReadModel::Sealed { .. }
        | CaptureReadModel::Failed { .. } => {
            let path = unique_artifact_path(&state.capture_root, "live", "tmcap");
            state
                .application
                .start_capture(CaptureStartRequest {
                    path: path.clone(),
                    max_file_bytes: AUTOMATIC_CAPTURE_BYTES,
                    retain_body_samples: state
                        .application
                        .product_state()
                        .privacy
                        .retain_body_samples,
                })
                .await
                .map_err(|error| error.to_string())?;
            (path, true)
        }
    };
    *state
        .live_capture_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(capture_path);
    let host = Some(Arc::clone(&state.host) as Arc<dyn transmog_session::HostIntegration>);
    match state.application.start_proxy(request, host).await {
        Ok(status) => Ok(status),
        Err(error) => {
            if capture_started {
                let _ = state.application.stop_capture().await;
            }
            Err(error.to_string())
        }
    }
}

#[tauri::command]
async fn retry_host_restore(state: State<'_, DesktopState>) -> Result<AppStatus, AppError> {
    state.application.retry_host_restore().await
}

#[tauri::command]
fn recover_windows_proxy(state: State<'_, DesktopState>) -> Result<bool, String> {
    state
        .host
        .recover_pending()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn certificate_is_trusted(sha256: String) -> Result<bool, String> {
    CurrentUserCertificateStore
        .contains(&sha256)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn install_certificate(
    path: PathBuf,
    sha256: String,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "certificate",
        "trust-requested",
        "current-user interception CA trust requested",
    );
    let newly_claimed = match state.owned_certificate.claim(&sha256) {
        Ok(newly_claimed) => newly_claimed,
        Err(error) => {
            let message = error.to_string();
            state.application.record_diagnostic(
                DiagnosticLevel::Error,
                "certificate",
                "ownership-claim-failed",
                &message,
            );
            return Err(message);
        }
    };
    if let Err(error) = CurrentUserCertificateStore.install(&path, &sha256) {
        if newly_claimed {
            let _ = state.owned_certificate.clear(&sha256);
        }
        let message = error.to_string();
        state.application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "trust-failed",
            &message,
        );
        return Err(message);
    }
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "certificate",
        "trust-succeeded",
        "current-user interception CA trust succeeded",
    );
    Ok(())
}

#[tauri::command]
fn remove_certificate(sha256: String, state: State<'_, DesktopState>) -> Result<(), String> {
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "certificate",
        "remove-requested",
        "current-user interception CA removal requested",
    );
    let result = (|| {
        if state
            .owned_certificate
            .owned_thumbprint()
            .map_err(|error| error.to_string())?
            .is_none_or(|owned| !owned.eq_ignore_ascii_case(&sha256))
        {
            return Err(
                "certificate is not recorded as owned by this Transmog installation".to_owned(),
            );
        }
        CurrentUserCertificateStore
            .remove(&sha256)
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    match &result {
        Ok(()) => state.application.record_diagnostic(
            DiagnosticLevel::Info,
            "certificate",
            "remove-succeeded",
            "current-user interception CA removal succeeded",
        ),
        Err(error) => state.application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "remove-failed",
            error,
        ),
    }
    result
}

#[tauri::command]
async fn create_ca(
    request: CaCreateRequest,
    state: State<'_, DesktopState>,
) -> Result<CaIdentity, String> {
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "certificate",
        "create-requested",
        "durable interception CA creation requested",
    );
    let private_key_path = request.private_key_path.clone();
    let identity = match state.application.create_ca(request).await {
        Ok(identity) => identity,
        Err(error) => {
            let message = error.to_string();
            state.application.record_diagnostic(
                DiagnosticLevel::Error,
                "certificate",
                "create-failed",
                &message,
            );
            return Err(message);
        }
    };
    if let Err(error) = CurrentUserKeyProtection.protect(&private_key_path) {
        let message = format!("CA was created but private-key protection failed: {error}");
        state.application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "key-protection-failed",
            &message,
        );
        return Err(message);
    }
    if let Err(error) = state.owned_certificate.claim(&identity.sha256) {
        let message =
            format!("CA was created but its durable identity could not be recorded: {error}");
        state.application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "ownership-claim-failed",
            &message,
        );
        return Err(message);
    }
    state
        .application
        .remember_artifact(identity.certificate_path.clone(), ArtifactKind::Certificate);
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "certificate",
        "create-succeeded",
        "durable interception CA creation and private-key protection succeeded",
    );
    Ok(identity)
}

#[tauri::command]
fn query_sessions(
    query: SessionQueryInput,
    state: State<'_, DesktopState>,
) -> Result<SessionPage, AppError> {
    state.application.query_sessions(query)
}

#[tauri::command]
fn session_detail(id: String, state: State<'_, DesktopState>) -> Result<SessionDetail, AppError> {
    state.application.session_detail(&id)
}

#[tauri::command]
async fn inspect_body(
    request: BodyInspectionRequest,
    state: State<'_, DesktopState>,
) -> Result<BodyInspection, AppError> {
    state.application.inspect_body(request).await
}

#[tauri::command]
async fn save_response_body(
    session_id: String,
    boundary: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<ResponseFileResult>, AppError> {
    let application = state.application.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        application.prepare_response_file(&session_id, &boundary)
    })
    .await
    .map_err(|_| AppError {
        category: transmog_app::ErrorCategory::Internal,
        message: "Response save preparation failed".to_owned(),
        retryable: true,
    })??;
    let name = prepared.suggested_name().to_owned();
    let destination = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Save response as")
        .set_file_name(name)
        .save_file()
        .await;
    match destination {
        Some(file) => prepared.save_to(file.path().to_owned()).await.map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
fn watch_sessions(
    on_event: Channel<SessionHint>,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "session-watch-requested",
        "live session refresh requested",
    );
    let mut updates = state.application.subscribe_session_updates()?;
    tauri::async_runtime::spawn(async move {
        while let Ok(mut hint) = updates.recv().await {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(75);
            loop {
                tokio::select! {
                    update = updates.recv() => match update {
                        Ok(next) => {
                            hint.exchange_id = next.exchange_id.or(hint.exchange_id);
                            hint.sequence = hint.sequence.max(next.sequence);
                            hint.lagged |= next.lagged;
                        }
                        Err(_) => break,
                    },
                    () = tokio::time::sleep_until(deadline) => break,
                }
            }
            if on_event.send(hint).is_err() {
                break;
            }
        }
    });
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "session-watch-started",
        "live session refresh channel started",
    );
    Ok(())
}

#[tauri::command]
fn enable_breakpoints(
    settings: BreakpointSettings,
    state: State<'_, DesktopState>,
) -> Result<BreakpointStatus, AppError> {
    state.application.enable_breakpoints(&settings)
}

#[tauri::command]
fn breakpoint_status(state: State<'_, DesktopState>) -> BreakpointStatus {
    state.application.paused_exchanges()
}

#[tauri::command]
fn decide_breakpoint(
    decision: BreakpointDecision,
    state: State<'_, DesktopState>,
) -> Result<BreakpointStatus, AppError> {
    state.application.decide_breakpoint(decision)
}

#[tauri::command]
async fn disable_breakpoints(state: State<'_, DesktopState>) -> Result<BreakpointStatus, AppError> {
    Ok(state.application.disable_breakpoints().await)
}

#[tauri::command]
async fn execute_composer(
    request: ComposerRequest,
    state: State<'_, DesktopState>,
) -> Result<ComposerResult, AppError> {
    state.application.execute_composer(request).await
}

#[tauri::command]
fn composer_history(state: State<'_, DesktopState>) -> Vec<ComposerSnapshot> {
    state.application.composer_history()
}

#[tauri::command]
async fn start_capture(
    request: CaptureStartRequest,
    state: State<'_, DesktopState>,
) -> Result<CaptureReadModel, AppError> {
    let path = request.path.clone();
    let result = state.application.start_capture(request).await?;
    state
        .application
        .remember_artifact(path, ArtifactKind::NativeCapture);
    Ok(result)
}

#[tauri::command]
async fn stop_capture(state: State<'_, DesktopState>) -> Result<CaptureReadModel, AppError> {
    state.application.stop_capture().await
}

#[tauri::command]
fn capture_status(state: State<'_, DesktopState>) -> CaptureReadModel {
    state.application.capture_status()
}

#[tauri::command]
async fn import_capture(
    request: ImportRequest,
    state: State<'_, DesktopState>,
) -> Result<CaptureSummaryView, AppError> {
    let path = request.path.clone();
    let result = state.application.import_capture(request).await?;
    state
        .application
        .remember_artifact(path, ArtifactKind::NativeCapture);
    Ok(result)
}

#[tauri::command]
async fn export_capture(
    request: ExportRequest,
    state: State<'_, DesktopState>,
) -> Result<ExportResult, AppError> {
    let path = request.destination.clone();
    let kind = match request.format {
        ExportFormat::Native => ArtifactKind::NativeCapture,
        ExportFormat::JsonLines => ArtifactKind::JsonLines,
        ExportFormat::SazStrict | ExportFormat::SazExtended => ArtifactKind::Saz,
    };
    let result = state.application.export_capture(request).await?;
    state.application.remember_artifact(path, kind);
    Ok(result)
}

#[tauri::command]
async fn export_live_capture(state: State<'_, DesktopState>) -> Result<ExportResult, String> {
    let source = state
        .live_capture_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| "Start the proxy before exporting its traffic.".to_owned())?;
    std::fs::create_dir_all(&state.export_root)
        .map_err(|error| format!("capture export directory is unavailable: {error}"))?;
    let destination = unique_artifact_path(&state.export_root, "Transmog-capture", "tmcap");
    let result = state
        .application
        .export_capture(ExportRequest {
            source,
            destination: destination.clone(),
            format: ExportFormat::Native,
            max_source_bytes: 4 * 1024 * 1024 * 1024,
        })
        .await
        .map_err(|error| error.to_string())?;
    state
        .application
        .remember_artifact(destination, ArtifactKind::NativeCapture);
    Ok(result)
}

fn unique_artifact_path(root: &std::path::Path, prefix: &str, extension: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    root.join(format!(
        "{prefix}-{timestamp}-{}-{sequence}.{extension}",
        std::process::id()
    ))
}

/// Runs the Windows `WebView2` host with a fixed embedded origin.
///
/// # Panics
///
/// Panics if the embedded assets, application service, or Tauri host cannot initialize.
#[allow(clippy::too_many_lines)]
pub fn run() {
    exit_for_maintenance_if_requested();
    let replay = SystemReplayExecutor::new(ProxyRoute::Auto)
        .expect("operating-system trust must initialize for composer replay");
    let state_root = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Transmog");
    let application = Application::new(AppConfig {
        replay_executor: Some(Arc::new(replay)),
        product_state_path: Some(state_root.join("preferences")),
        automation_path: Some(state_root.join("automation-v1")),
        script_workspace_path: Some(state_root.join("scripts-v1")),
        script_host_executable: packaged_script_host(),
        preview_worker_executable: packaged_preview_worker(),
        response_asset_root: Some(state_root.join("response-assets-v1")),
        diagnostics_log_path: Some(state_root.join("diagnostics.jsonl")),
        body_store: Some(BodyStoreConfig::product_default(
            state_root.join("body-cache-v1"),
        )),
        ..AppConfig::default()
    })
    .expect("application must initialize");
    let renderer = AppRenderer::new().expect("embedded WebUI assets must be valid");
    let initial_window = application.product_state().window;
    let host = Arc::new(WindowsProxyIntegration::system(
        state_root.join("proxy-recovery-v1.json"),
    ));
    if let Err(error) = host.recover_pending() {
        application.record_diagnostic(
            DiagnosticLevel::Error,
            "desktop",
            "startup-proxy-recovery-failed",
            &error.to_string(),
        );
    }
    let owned_certificate =
        OwnedCertificateRegistry::new(state_root.join("certificate-ownership-v1.json"));
    let ca_certificate_path = state_root.join("interception-ca.pem");
    let ca_private_key_path = state_root.join("interception-ca.key");
    let diagnostics_path = state_root.join("diagnostics.jsonl");
    let webview_data_path = state_root.join("WebView2");
    let protocol_application = application.clone();
    let preview_application = application.clone();
    let close_application = application.clone();
    let close_started = Arc::new(AtomicBool::new(false));
    let close_guard = Arc::clone(&close_started);

    let exit_host = Arc::clone(&host);
    let exit_application = application.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            },
        ))
        .manage(DesktopState {
            application: application.clone(),
            host: Arc::clone(&host),
            owned_certificate,
            ca_certificate_path,
            ca_private_key_path,
            diagnostics_path,
            capture_root: state_root.join("captures"),
            export_root: state_root.join("exports"),
            live_capture_path: Arc::new(Mutex::new(None)),
        })
        .register_uri_scheme_protocol("transmog-ui", move |_context, request: Request<Vec<u8>>| {
            let view = ShellView::from(&protocol_application.status());
            into_tauri_response(renderer.respond(
                request.method().as_str(),
                request.uri().path(),
                &view,
            ))
        })
        .register_uri_scheme_protocol(
            "transmog-preview",
            move |_context, request: Request<Vec<u8>>| {
                preview_response(&preview_application, &request)
            },
        )
        .invoke_handler(tauri::generate_handler![
            app_status,
            product_state,
            save_product_state,
            save_workspace_preferences,
            desktop_bootstrap,
            record_frontend_diagnostic,
            automation_status,
            set_autoresponses_enabled,
            test_autoresponse_match,
            validate_automation,
            activate_automation,
            script_status,
            script_declarations,
            save_script,
            validate_script,
            test_script,
            activate_script,
            disable_script,
            response_assets,
            create_response_asset,
            import_response_asset,
            create_response_asset_from_session,
            diagnostics_report,
            create_support_bundle,
            prepare_update_handoff,
            start_proxy,
            stop_application,
            retry_host_restore,
            recover_windows_proxy,
            certificate_is_trusted,
            install_certificate,
            remove_certificate,
            create_ca,
            query_sessions,
            session_detail,
            inspect_body,
            save_response_body,
            watch_sessions,
            enable_breakpoints,
            breakpoint_status,
            decide_breakpoint,
            disable_breakpoints,
            execute_composer,
            composer_history,
            start_capture,
            stop_capture,
            capture_status,
            import_capture,
            export_capture,
            export_live_capture
        ])
        .setup(move |app| Ok(create_main_window(app, initial_window, webview_data_path)?))
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && !close_guard.swap(true, Ordering::AcqRel)
            {
                api.prevent_close();
                let application = close_application.clone();
                let window = window.clone();
                let close_guard = Arc::clone(&close_guard);
                tauri::async_runtime::spawn(async move {
                    persist_window_state(&application, &window);
                    if application.shutdown().await.is_ok() {
                        let _ = window.destroy();
                    } else {
                        close_guard.store(false, Ordering::Release);
                        let _ = window.set_title("Transmog — shutdown needs attention");
                    }
                });
            }
        })
        .build(tauri::generate_context!())
        .expect("Tauri desktop host failed to build");
    app.run(move |_app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) && let Err(error) = exit_host.recover_pending()
        {
            exit_application.record_diagnostic(
                DiagnosticLevel::Error,
                "desktop",
                "exit-proxy-recovery-failed",
                &error.to_string(),
            );
        }
    });
}

fn packaged_script_host() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let host = executable.with_file_name("transmog-script-host.exe");
    host.is_file().then_some(host)
}

fn packaged_preview_worker() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let worker = executable.with_file_name("transmog-preview-worker.exe");
    worker.is_file().then_some(worker)
}

fn preview_response(application: &Application, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let handle = preview_handle(request.uri().path());
    let preview = (request.method() == tauri::http::Method::GET)
        .then(|| handle.and_then(|handle| application.image_preview(&handle)))
        .flatten();
    let (status, content_type, body) = preview.map_or_else(
        || {
            (
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                b"preview unavailable".to_vec(),
            )
        },
        |(bytes, mime_type)| (StatusCode::OK, mime_type, bytes.as_ref().clone()),
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_DISPOSITION, "inline")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::REFERRER_POLICY, "no-referrer")
        .header("cross-origin-resource-policy", "cross-origin")
        .body(body)
        .expect("fixed preview response headers must be valid")
}

fn preview_handle(path: &str) -> Option<String> {
    let decoded = percent_decode_str(path.strip_prefix('/')?)
        .decode_utf8()
        .ok()?;
    decoded
        .strip_prefix("preview/")
        .filter(|handle| !handle.is_empty() && !handle.contains('/'))
        .map(str::to_owned)
}

fn exit_for_maintenance_if_requested() {
    if let Some(exit_code) = maintenance_exit_code() {
        std::process::exit(exit_code);
    }
}

fn maintenance_exit_code() -> Option<i32> {
    let uninstall = std::env::args_os().skip(1).find_map(|argument| {
        let argument = argument.to_string_lossy();
        match argument.as_ref() {
            "--prepare-update" => Some(false),
            "--uninstall-cleanup" => Some(true),
            _ => None,
        }
    })?;
    Some(if run_maintenance(uninstall).is_ok() {
        0
    } else {
        2
    })
}

fn run_maintenance(uninstall: bool) -> Result<(), ()> {
    let local_app_data = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or(())?);
    if !local_app_data.is_absolute() {
        return Err(());
    }
    let state_root = local_app_data.join("Transmog");
    WindowsProxyIntegration::system(state_root.join("proxy-recovery-v1.json"))
        .recover_pending()
        .map_err(|_| ())?;
    if !uninstall {
        return Ok(());
    }
    let ownership = OwnedCertificateRegistry::new(state_root.join("certificate-ownership-v1.json"));
    if let Some(sha256) = ownership.owned_thumbprint().map_err(|_| ())? {
        CurrentUserCertificateStore
            .remove(&sha256)
            .map_err(|_| ())?;
        ownership.clear(&sha256).map_err(|_| ())?;
    }
    remove_owned_app_data(&state_root)
}

fn remove_owned_app_data(state_root: &std::path::Path) -> Result<(), ()> {
    if state_root.file_name().and_then(|name| name.to_str()) != Some("Transmog") {
        return Err(());
    }
    let entries = match std::fs::read_dir(state_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && matches!(
                entry.file_name().to_string_lossy().as_ref(),
                "body-cache-v1" | "response-assets-v1"
            )
        {
            std::fs::remove_dir_all(path).map_err(|_| ())?;
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let owned = matches!(
            name.as_ref(),
            "diagnostics.jsonl"
                | "diagnostics.jsonl.1"
                | "certificate-ownership-v1.json"
                | "proxy-recovery-v1.json"
        ) || is_owned_preference_generation(&name)
            || matches!(
                name.as_ref(),
                "automation-v1.0.json"
                    | "automation-v1.1.json"
                    | "scripts-v1.0.json"
                    | "scripts-v1.1.json"
            )
            || name.starts_with("certificate-ownership-v1.tmp-")
            || (name.starts_with(".transmog-state-") && name.ends_with(".tmp"));
        if owned {
            std::fs::remove_file(path).map_err(|_| ())?;
        }
    }
    match std::fs::remove_dir(state_root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn is_owned_preference_generation(name: &str) -> bool {
    let Some(remainder) = name.strip_prefix("preferences.") else {
        return false;
    };
    let generation = remainder
        .strip_suffix(".json")
        .or_else(|| remainder.strip_suffix(".json.corrupt"));
    generation
        .is_some_and(|value| value.len() == 20 && value.bytes().all(|byte| byte.is_ascii_digit()))
}

fn create_main_window(
    app: &tauri::App,
    initial: WindowState,
    webview_data_path: PathBuf,
) -> tauri::Result<()> {
    let url =
        tauri::Url::parse("transmog-ui://localhost/").expect("fixed application URL must parse");
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
        .title("Transmog")
        .inner_size(f64::from(initial.width), f64::from(initial.height))
        .min_inner_size(760.0, 520.0)
        .data_directory(webview_data_path)
        .on_navigation(is_allowed_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    if let (Some(x), Some(y)) = (initial.x, initial.y) {
        builder = builder.position(f64::from(x), f64::from(y));
    }
    let window = builder.build()?;
    if initial.maximized {
        window.maximize()?;
    }
    Ok(())
}

fn runtime_diagnostics() -> RuntimeDiagnostics {
    RuntimeDiagnostics {
        operating_system: format!("{} {}", std::env::consts::OS, std::env::consts::FAMILY),
        architecture: std::env::consts::ARCH.to_owned(),
        webview_version: tauri::webview_version().ok(),
    }
}

fn persist_window_state(application: &Application, window: &tauri::Window) {
    let mut state = application.product_state();
    let maximized = window.is_maximized().unwrap_or(false);
    state.window.maximized = maximized;
    if !maximized {
        if let Ok(size) = window.inner_size() {
            state.window.width = size.width;
            state.window.height = size.height;
        }
        if let Ok(position) = window.outer_position() {
            state.window.x = Some(position.x);
            state.window.y = Some(position.y);
        }
    }
    let _ = application.save_product_state(state);
}

fn is_allowed_navigation(url: &tauri::Url) -> bool {
    (url.scheme() == "transmog-ui" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some(UI_HOST))
}

fn into_tauri_response(result: Result<UiResponse, UiError>) -> Response<Vec<u8>> {
    let response = match result {
        Ok(response) => response,
        Err(_) => UiResponse {
            status: 500,
            content_type: "text/plain; charset=utf-8",
            cache_control: "no-store",
            content_security_policy: None,
            body: b"embedded UI unavailable".to_vec(),
        },
    };
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, response.content_type)
        .header(header::CACHE_CONTROL, response.cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header("cross-origin-opener-policy", "same-origin");
    if let Some(csp) = response.content_security_policy {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, csp);
    }
    builder
        .body(response.body)
        .expect("fixed response headers must be valid")
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_navigation, preview_handle, remove_owned_app_data};

    #[test]
    fn navigation_is_limited_to_the_embedded_origin() {
        for allowed in [
            "transmog-ui://localhost/",
            "transmog-ui://localhost/app.js",
            "http://transmog-ui.localhost/",
        ] {
            assert!(is_allowed_navigation(&tauri::Url::parse(allowed).unwrap()));
        }
        for denied in [
            "https://transmog-ui.localhost/",
            "http://localhost/",
            "https://example.com/",
            "transmog-ui://attacker.invalid/",
        ] {
            assert!(!is_allowed_navigation(&tauri::Url::parse(denied).unwrap()));
        }
    }

    #[test]
    fn preview_handles_accept_direct_and_tauri_encoded_paths() {
        assert_eq!(
            preview_handle("/preview/opaque-handle_123").as_deref(),
            Some("opaque-handle_123")
        );
        assert_eq!(
            preview_handle("/preview%2Fopaque-handle_123").as_deref(),
            Some("opaque-handle_123")
        );
        assert_eq!(preview_handle("/preview%2Fnested%2Fhandle"), None);
        assert_eq!(preview_handle("/not-preview/opaque-handle_123"), None);
    }

    #[test]
    fn uninstall_cleanup_removes_only_declared_app_data() {
        let parent =
            std::env::temp_dir().join(format!("transmog-uninstall-test-{}", std::process::id()));
        let root = parent.join("Transmog");
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&root).unwrap();
        for name in [
            "diagnostics.jsonl",
            "diagnostics.jsonl.1",
            "preferences.00000000000000000001.json",
            "preferences.00000000000000000002.json.corrupt",
            ".transmog-state-00000000000000000003-1.tmp",
        ] {
            std::fs::write(root.join(name), b"owned").unwrap();
        }
        std::fs::write(root.join("preferences.user-notes.txt"), b"keep too").unwrap();
        std::fs::write(root.join("user-kept.txt"), b"keep").unwrap();
        remove_owned_app_data(&root).unwrap();
        assert_eq!(std::fs::read(root.join("user-kept.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(parent).unwrap();
    }
}
