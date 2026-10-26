#![allow(clippy::needless_pass_by_value)] // Tauri commands deserialize owned IPC arguments.

#[path = "auth.rs"]
mod auth;
#[path = "captured_preview.rs"]
mod captured_preview;
#[path = "comparison.rs"]
mod comparison;
#[path = "extension_management.rs"]
mod extension_management;
#[path = "format_converter.rs"]
mod format_converter;
#[path = "incidents.rs"]
mod incidents;
#[path = "portable_cases.rs"]
mod portable_cases;
#[path = "saved_demos.rs"]
mod saved_demos;
#[path = "visualizers.rs"]
mod visualizers;
#[path = "window_workspaces.rs"]
mod workspaces;

use std::{
    path::PathBuf,
    sync::{
        Arc,
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
    AppConfig, AppError, AppLifecycle, AppStatus, Application, ArtifactKind, AuthoredResponseAsset,
    AutoResponseBatchInput, AutoResponseBatchResult, AutoResponseTestInput, AutoResponseTestResult,
    AutomationCandidate, AutomationRuleSet, AutomationStatus, BodyInspection,
    BodyInspectionRequest, BodyStoreConfig, BreakpointDecision, BreakpointSettings,
    BreakpointStatus, CaCreateRequest, CaIdentity, CaptureReadModel, CaptureStartRequest,
    CaptureSummaryView, ComposerRequest, ComposerResult, ComposerSnapshot, ComposerSource,
    DiagnosticLevel, DiagnosticsReport, ExportFormat, ExportRequest, ExportResult, ImportRequest,
    ImportResponseAsset, ProductState, ProxyRoute, ProxyStartRequest, RequestCommand,
    RequestCommandFormat, ResponseAsset, ResponseAssetEdit, ResponseAssetInspection,
    ResponseFileResult, RuntimeDiagnostics, ScriptAction, ScriptCandidate, ScriptDraft,
    ScriptInvocation, ScriptStatus, SessionDetail, SessionHint, SessionPage, SessionQueryInput,
    SessionResponseAsset, SessionSummary, SupportBundleRequest, SupportBundleResult,
    SystemReplayExecutor, TraceSaveOptions, TraceSaveResult, TrafficTimelineRow, TrafficView,
    WindowState, WorkspacePreferences,
};
use transmog_app_webui::{AppRenderer, ShellView, UiError, UiResponse};
use transmog_host_windows::{
    CurrentUserCertificateStore, CurrentUserKeyProtection, OwnedCertificateRegistry,
    WindowsProxyIntegration,
};

const UI_HOST: &str = "transmog-ui.localhost";
static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// Large imports and searches must not enqueue one WebView message per entry.
// Always report the first update and completion; intermediate feedback is bounded.
fn progress_due(
    last: &std::sync::Mutex<Option<std::time::Instant>>,
    completed: u64,
    total: u64,
) -> bool {
    let now = std::time::Instant::now();
    let mut last = last
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if last.is_none_or(|previous| {
        now.duration_since(previous) >= std::time::Duration::from_millis(100)
    }) || completed >= total
    {
        *last = Some(now);
        true
    } else {
        false
    }
}

#[derive(Clone)]
struct DesktopState {
    #[cfg(feature = "desktop-test-fixtures")]
    certificate_test_fixture: Option<Arc<crate::certificate_test_fixture::Fixture>>,
    _client_certificate_controller:
        Arc<transmog_tls::windows_client_certificates::WindowsClientCertificateController>,
    workspaces: Arc<workspaces::WindowWorkspaces>,
    webview_data_path: PathBuf,
    application: Application,
    host: Arc<WindowsProxyIntegration>,
    owned_certificate: OwnedCertificateRegistry,
    certificate_operation: Arc<tokio::sync::Mutex<()>>,
    loopback_operation: Arc<tokio::sync::Mutex<()>>,
    ca_certificate_path: PathBuf,
    ca_private_key_path: PathBuf,
    diagnostics_path: PathBuf,
    export_root: PathBuf,
    closing: Arc<AtomicBool>,
}

#[tauri::command]
fn open_product_website() -> Result<(), String> {
    open_default_browser("https://transmog.net")
}

fn open_default_browser(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    let system =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    std::process::Command::new(system.join("System32/rundll32.exe"))
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|_| "Windows could not open the default browser.".to_owned())
}

#[tauri::command]
fn open_response_set_live_page(
    id: String,
    incarnation: String,
    url: String,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let status = state.application.automation_status();
    let set = status
        .response_sets
        .iter()
        .find(|set| set.id == id && set.incarnation == incarnation)
        .ok_or("This response set is no longer available.")?;
    let page = set
        .preview_page
        .as_ref()
        .ok_or("Choose a captured page first.")?;
    if page.url != url {
        return Err("The selected page changed. Review the current page before opening it.".into());
    }
    let target = tauri::Url::parse(&page.url).map_err(|_| "The saved page URL is invalid.")?;
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err("Only HTTP or HTTPS pages can open in the default browser.".into());
    }
    open_default_browser(target.as_str())
}

#[tauri::command]
async fn app_containers() -> Result<Vec<crate::loopback::AppContainer>, String> {
    tauri::async_runtime::spawn_blocking(crate::loopback::enumerate)
        .await
        .map_err(|_| "AppContainer enumeration could not complete.".to_owned())?
}

#[tauri::command]
async fn set_loopback_exemptions(
    change: crate::loopback::ExemptionChange,
    state: State<'_, DesktopState>,
) -> Result<Vec<crate::loopback::AppContainer>, String> {
    let _operation = state
        .loopback_operation
        .try_lock()
        .map_err(|_| "Another loopback exemption change is in progress.".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || crate::loopback::request_change(&change))
        .await
        .map_err(|_| {
            "The exemption change could not complete. Refresh to inspect the current state."
                .to_owned()
        })?
}

#[tauri::command]
fn update_status(
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> crate::updates::UpdateStatus {
    updates.status()
}

#[tauri::command]
fn set_automatic_update_checks(
    enabled: bool,
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> Result<crate::updates::UpdateStatus, String> {
    updates.set_automatic_checks(enabled)
}

#[tauri::command]
async fn check_updates(
    app: tauri::AppHandle,
    manual: bool,
    state: State<'_, DesktopState>,
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> Result<crate::updates::UpdateStatus, String> {
    updates
        .check(&app, &state.application, &state.ca_certificate_path, manual)
        .await
}

#[tauri::command]
async fn download_update(
    after_session: bool,
    progress: Channel<crate::updates::UpdateStatus>,
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> Result<crate::updates::UpdateStatus, String> {
    updates.inner().stage(after_session, progress).await
}

#[tauri::command]
fn cancel_update(
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> crate::updates::UpdateStatus {
    updates.cancel()
}

#[tauri::command]
fn remind_update(
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> Result<crate::updates::UpdateStatus, String> {
    updates.remind()
}

#[tauri::command]
fn watch_update_close(
    channel: Channel<()>,
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) {
    updates.watch_close(channel);
}

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    reopen: bool,
    state: State<'_, DesktopState>,
    updates: State<'_, Arc<crate::updates::UpdateCoordinator>>,
) -> Result<(), String> {
    if state.closing.swap(true, Ordering::AcqRel) {
        return Err("Transmog is already closing.".to_owned());
    }
    let result = async {
        updates.ensure_ready()?;
        let _lifecycle = state.certificate_operation.lock().await;
        if !state
            .workspaces
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return Err(
                "Close saved traffic viewer windows before installing the update.".to_owned(),
            );
        }
        if let Some(window) = app.get_webview_window("main") {
            persist_window_state(&state.application, &window.as_ref().window());
        }
        state
            .application
            .shutdown()
            .await
            .map_err(|error| error.to_string())?;
        state.host.recover_pending().map_err(
            |_| "Windows proxy settings could not be restored. The update has not started.",
        )?;
        if state.host.recovery_pending() {
            return Err(
                "Windows proxy restoration is still pending. The update has not started."
                    .to_owned(),
            );
        }
        updates.install(reopen)
    }
    .await;
    if let Err(message) = &result {
        state.closing.store(false, Ordering::Release);
        updates.fail(message);
    }
    result
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Independent file, trust, and recovery observations for IPC.
struct DesktopBootstrap {
    viewer_mode: bool,
    windows: bool,
    ca_certificate_path: PathBuf,
    ca_private_key_path: PathBuf,
    ca_files_present: bool,
    ca_files_exist: bool,
    ca_key_error: Option<String>,
    owned_ca_sha256: Option<String>,
    owned_ca_trusted: bool,
    host_restore_pending: bool,
    diagnostics_path: PathBuf,
}

impl DesktopState {
    fn window_application(&self, window: &tauri::WebviewWindow) -> Result<Application, AppError> {
        self.workspaces
            .application(&self.application, window.label())
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn app_status(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<AppStatus, AppError> {
    let application = state.window_application(&window)?;
    Ok(application.status())
}

#[tauri::command]
fn buffer_status(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<transmog_app::BufferStatus>, AppError> {
    Ok(state
        .window_application(&window)?
        .body_store()
        .map(transmog_app::BodyStore::buffer_status))
}

#[tauri::command]
fn product_state(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ProductState, AppError> {
    let application = state.window_application(&window)?;
    Ok(application.product_state())
}

#[tauri::command]
fn client_certificate_status(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ClientCertificateStatus, AppError> {
    state.window_application(&window)?;
    let caller = (window.label() != "main").then(|| format!("composer:{}", window.label()));
    Ok(state
        .application
        .client_certificates()
        .expect("desktop certificate controller")
        .status(caller.as_deref()))
}

#[tauri::command]
fn client_certificate_request(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ClientCertificateRequestDetail, AppError> {
    state
        .application
        .client_certificates()
        .expect("desktop certificate controller")
        .request(&id)
}

#[tauri::command]
async fn client_certificate_identities(
    state: State<'_, DesktopState>,
) -> Result<Vec<transmog_app::ClientCertificateIdentity>, AppError> {
    state
        .application
        .client_certificates()
        .expect("desktop certificate controller")
        .identities()
        .await
}

#[tauri::command]
fn decide_client_certificate(
    id: String,
    decision: transmog_app::ClientCertificateDecision,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ClientCertificateStatus, AppError> {
    let manager = state
        .application
        .client_certificates()
        .expect("desktop certificate controller");
    manager.answer(&id, decision)?;
    Ok(manager.status(None))
}

#[tauri::command]
fn save_client_certificate_policy(
    policy: transmog_app::ClientCertificatePolicy,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ClientCertificatePolicy, AppError> {
    state.application.save_client_certificate_policy(policy)
}

#[cfg(feature = "desktop-test-fixtures")]
#[tauri::command]
fn certificate_test_info(
    state: State<'_, DesktopState>,
) -> Result<crate::certificate_test_fixture::Info, String> {
    let mut info = state
        .certificate_test_fixture
        .as_ref()
        .ok_or("Certificate acceptance fixture is not enabled")?
        .info
        .clone();
    info.authorization_generation =
        transmog_tls::client_authentication::ClientAuthenticationResolver::generation(
            state
                .application
                .client_certificates()
                .expect("desktop certificate controller")
                .resolver()
                .as_ref(),
        )
        .to_string();
    Ok(info)
}
#[cfg(feature = "desktop-test-fixtures")]
#[tauri::command]
async fn delete_certificate_test_key(state: State<'_, DesktopState>) -> Result<(), String> {
    let fixture = state
        .certificate_test_fixture
        .clone()
        .ok_or("Certificate acceptance fixture is not enabled")?;
    tokio::task::spawn_blocking(move || fixture.delete_key())
        .await
        .map_err(|_| "Owned certificate fixture deletion failed".to_owned())
}

#[cfg(feature = "desktop-test-fixtures")]
#[tauri::command]
fn close_certificate_test_main(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    if state.certificate_test_fixture.is_none() {
        return Err("Certificate acceptance fixture is not enabled".into());
    }
    app.get_webview_window("main")
        .ok_or("Owned main window is unavailable")?
        .close()
        .map_err(|_| "Owned main window could not close".into())
}

#[cfg(feature = "desktop-test-fixtures")]
#[tauri::command]
fn close_certificate_test_viewer(
    label: String,
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    if state.certificate_test_fixture.is_none()
        || !state
            .workspaces
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&label)
    {
        return Err("Owned certificate acceptance viewer is unavailable".into());
    }
    app.get_webview_window(&label)
        .ok_or("Owned viewer is unavailable")?
        .destroy()
        .map_err(|_| "Owned viewer could not close".into())
}

#[tauri::command]
async fn save_network_profiles(
    expected_revision: u64,
    profiles: Vec<transmog_app::NetworkProfileSnapshot>,
    use_profile: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::NetworkProfiles, AppError> {
    let _operation = state.certificate_operation.lock().await;
    let next = state.application.preview_network_profiles(
        expected_revision,
        profiles.clone(),
        use_profile.as_deref(),
    )?;
    let viewer_updates = if next.active_profile
        == state
            .application
            .product_state()
            .preferences
            .network_profiles
            .active_profile
    {
        Vec::new()
    } else {
        state
            .workspaces
            .prepare_network_profile(next.active_profile)?
    };
    let saved = state
        .application
        .save_network_profiles(expected_revision, profiles, use_profile)
        .await?;
    for update in viewer_updates {
        update();
    }
    Ok(saved)
}

#[tauri::command]
async fn save_product_state(
    product_state: ProductState,
    state: State<'_, DesktopState>,
) -> Result<ProductState, AppError> {
    let _certificate_operation = state.certificate_operation.lock().await;
    let saved = state.application.save_product_state(product_state)?;
    Ok(saved)
}

#[tauri::command]
fn save_workspace_preferences(
    preferences: WorkspacePreferences,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<WorkspacePreferences, AppError> {
    let application = state.window_application(&window)?;
    application.save_workspace_preferences(preferences)
}

#[tauri::command]
fn update_response_set_guide(
    action: transmog_app::ResponseSetGuideAction,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ResponseSetGuideUpdate, AppError> {
    state.application.update_response_set_guide(action)
}

#[tauri::command]
fn desktop_bootstrap(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<DesktopBootstrap, String> {
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
        viewer_mode: window.label() != "main",
        windows: true,
        ca_certificate_path: state.ca_certificate_path.clone(),
        ca_private_key_path: state.ca_private_key_path.clone(),
        ca_files_present: state.ca_certificate_path.is_file()
            && state.ca_private_key_path.is_file(),
        ca_files_exist: state.ca_certificate_path.exists() || state.ca_private_key_path.exists(),
        ca_key_error: managed_ca_key_error(&state.ca_certificate_path, &state.ca_private_key_path),
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
fn change_response_transform(
    generation: u64,
    change: transmog_app::HeaderRuleChange,
    state: State<'_, DesktopState>,
) -> Result<AutomationStatus, AppError> {
    state
        .application
        .change_response_transform(generation, change)
}
#[tauri::command]
fn test_response_transform(
    input: transmog_app::TextTransformTestInput,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TextTransformTest, AppError> {
    state.application.test_response_transform(&input)
}

#[tauri::command]
fn change_header_rule(
    generation: u64,
    change: transmog_app::HeaderRuleChange,
    state: State<'_, DesktopState>,
) -> Result<AutomationStatus, AppError> {
    state.application.change_header_rule(generation, change)
}

#[tauri::command]
fn test_header_rule(
    input: transmog_app::HeaderRuleTestInput,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::HeaderRuleTest, AppError> {
    state.application.test_header_rule(&input)
}

#[tauri::command]
fn change_response_set(
    generation: u64,
    change: transmog_app::ResponseSetChange,
    state: State<'_, DesktopState>,
) -> Result<AutomationStatus, AppError> {
    state.application.change_response_set(generation, change)
}

#[tauri::command]
fn response_set_overlaps(
    id: String,
    revision: u64,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    state.application.response_set_overlaps(&id, revision)
}

#[tauri::command]
async fn clear_traffic(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TrafficClearResult, AppError> {
    state.window_application(&window)?.clear_traffic().await
}

#[tauri::command]
async fn remove_traffic_entries(
    ids: Vec<String>,
    restore: bool,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    let application = state.window_application(&window)?;
    application.remove_traffic_entries(&ids, restore).await
}

#[tauri::command]
async fn remove_unselected_traffic_entries(
    ids: Vec<String>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    state
        .window_application(&window)?
        .remove_unselected_traffic_entries(&ids)
        .await
}

#[tauri::command]
async fn cancel_requests(
    ids: Vec<String>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    state
        .window_application(&window)?
        .cancel_requests(&ids)
        .await
}

#[tauri::command]
async fn search_traffic(
    request: transmog_app::TrafficSearchRequest,
    on_progress: Channel<transmog_app::TrafficSearchProgress>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TrafficSearchResult, AppError> {
    let application = state.window_application(&window)?;
    let cancel = application.clone();
    let last_progress = std::sync::Mutex::new(None);
    application
        .search_traffic(
            request,
            Arc::new(move |progress| {
                if !progress_due(
                    &last_progress,
                    progress.completed as u64,
                    progress.total as u64,
                ) {
                    return;
                }
                let operation = progress.operation_id.clone();
                if on_progress.send(progress).is_err() {
                    cancel.cancel_traffic_search(&operation);
                }
            }),
        )
        .await
}

#[tauri::command]
async fn traffic_search_entry(
    search_id: String,
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TrafficSearchEntry, AppError> {
    state
        .window_application(&window)?
        .traffic_search_entry(&search_id, &id)
        .await
}

#[tauri::command]
fn matching_traffic_ids(
    query: SessionQueryInput,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    state
        .window_application(&window)?
        .matching_traffic_ids(&query)
}

#[tauri::command]
fn cancel_traffic_search(
    operation_id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .cancel_traffic_search(&operation_id);
    Ok(())
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
    expected_generation: Option<u64>,
    state: State<'_, DesktopState>,
) -> Result<ScriptStatus, AppError> {
    state
        .application
        .save_script_checked(draft, expected_generation)
}

#[tauri::command]
fn validate_script(
    draft: ScriptDraft,
    expected_generation: Option<u64>,
    state: State<'_, DesktopState>,
) -> Result<ScriptCandidate, AppError> {
    state
        .application
        .validate_script_checked(draft, expected_generation)
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
async fn inspect_response_asset(
    reference: String,
    state: State<'_, DesktopState>,
) -> Result<ResponseAssetInspection, AppError> {
    state.application.inspect_response_asset(&reference).await
}

#[tauri::command]
fn autoresponse_source_headers(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ResponseAssetHeaders, AppError> {
    state.application.autoresponse_source_headers(&id)
}

#[tauri::command]
async fn edit_response_asset(
    input: ResponseAssetEdit,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    state.application.edit_response_asset(input).await
}

#[tauri::command]
async fn create_autoresponse_batch(
    input: AutoResponseBatchInput,
    state: State<'_, DesktopState>,
) -> Result<AutoResponseBatchResult, AppError> {
    state.application.create_autoresponse_batch(input).await
}

#[tauri::command]
fn review_response_set_intake(
    input: transmog_app::ResponseSetIntakeInput,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ResponseSetIntakeReview, AppError> {
    state.application.review_response_set_intake(&input)
}

#[tauri::command]
async fn collect_response_set(
    input: transmog_app::ResponseSetIntakeInput,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ResponseSetIntakeResult, AppError> {
    state.application.collect_response_set(input).await
}

#[tauri::command]
async fn pick_response_body() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title("Serve response body from file")
        .pick_file()
        .await
        .map(|file| file.path().to_string_lossy().into_owned())
}

#[tauri::command]
async fn response_body_file_info(
    path: String,
    reference: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ResponseBodyFileInfo, AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || {
        application.response_body_file_info(std::path::Path::new(&path), reference.as_deref())
    })
    .await
    .map_err(|_| AppError {
        category: transmog_app::ErrorCategory::Unavailable,
        message: "Response file check could not complete".into(),
        retryable: true,
    })?
}

#[tauri::command]
fn create_response_asset(
    input: AuthoredResponseAsset,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    state.application.create_response_asset(input)
}

#[tauri::command]
async fn import_response_asset(
    input: ImportResponseAsset,
    state: State<'_, DesktopState>,
) -> Result<ResponseAsset, AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || application.import_response_asset(input))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Response file import could not complete".into(),
            retryable: true,
        })?
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
    state.application.stop_proxy().await
}

#[tauri::command]
async fn resume_application(state: State<'_, DesktopState>) -> Result<AppStatus, AppError> {
    state.application.resume_proxy().await
}

#[tauri::command]
async fn start_proxy(
    request: ProxyStartRequest,
    state: State<'_, DesktopState>,
) -> Result<AppStatus, String> {
    let _certificate_operation = state.certificate_operation.lock().await;
    start_proxy_locked(request, &state).await
}

async fn start_proxy_locked(
    mut request: ProxyStartRequest,
    state: &DesktopState,
) -> Result<AppStatus, String> {
    let preferences = state.application.product_state().preferences;
    request.listen = std::net::SocketAddr::from((
        if preferences.allow_remote_clients {
            [0, 0, 0, 0]
        } else {
            [127, 0, 0, 1]
        },
        preferences.proxy_port.unwrap_or(0),
    ));
    request.allow_remote_clients = preferences.allow_remote_clients;
    let configure_system_proxy = preferences.configure_system_proxy;
    validate_proxy_start(state, configure_system_proxy)?;
    if state.application.product_state().preferences.decrypt_https {
        transmog_app::protect_ca_private_key(
            &request.ca_certificate_path,
            &request.ca_private_key_path,
        )
        .map_err(|error| error.to_string())?;
    }
    state.application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "proxy-start-requested",
        if configure_system_proxy {
            "proxy start requested with current-user Windows proxy integration"
        } else {
            "proxy start requested with manual client routing"
        },
    );
    let host = configure_system_proxy
        .then(|| Arc::clone(&state.host) as Arc<dyn transmog_session::HostIntegration>);
    state
        .application
        .start_proxy(request, host)
        .await
        .map_err(|error| error.to_string())
}

fn validate_proxy_start(state: &DesktopState, configure_system_proxy: bool) -> Result<(), String> {
    if state.closing.load(Ordering::Acquire) {
        return Err("Transmog is closing for an update.".to_owned());
    }
    if state.host.recovery_pending() {
        return Err(
            "Restore the journaled Windows proxy settings before starting a new proxy run."
                .to_owned(),
        );
    }
    if !state.application.product_state().preferences.decrypt_https {
        return Ok(());
    }
    if !state.ca_certificate_path.is_file() || !state.ca_private_key_path.is_file() {
        return Err(
            "Set up the Transmog interception certificate before starting the proxy.".to_owned(),
        );
    }
    transmog_app::protect_ca_private_key(&state.ca_certificate_path, &state.ca_private_key_path)
        .map_err(|error| error.to_string())?;
    let sha256 = state
        .owned_certificate
        .owned_thumbprint()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "Set up the Transmog interception certificate before starting the proxy.".to_owned()
        })?;
    validate_proxy_trust(&sha256, configure_system_proxy)
}

fn validate_proxy_trust(sha256: &str, configure_system_proxy: bool) -> Result<(), String> {
    if configure_system_proxy
        && !CurrentUserCertificateStore
            .contains(sha256)
            .map_err(|error| error.to_string())?
    {
        return Err(
            "Trust the Transmog interception certificate before starting the proxy.".to_owned(),
        );
    }
    Ok(())
}

fn managed_ca_key_error(
    certificate_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Option<String> {
    if !certificate_path.is_file() || !key_path.is_file() {
        return None;
    }
    transmog_app::validate_ca_private_key(certificate_path, key_path)
        .err()
        .map(|error| error.to_string())
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
    let _certificate_operation = state
        .certificate_operation
        .try_lock()
        .map_err(|_| "Another certificate or proxy operation is in progress.".to_owned())?;
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
    let _certificate_operation = state
        .certificate_operation
        .try_lock()
        .map_err(|_| "Another certificate or proxy operation is in progress.".to_owned())?;
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
    let _certificate_operation = state.certificate_operation.lock().await;
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
async fn reset_ca(state: State<'_, DesktopState>) -> Result<(), String> {
    let _certificate_operation = state.certificate_operation.lock().await;
    if !matches!(
        state.application.status().lifecycle,
        AppLifecycle::Stopped | AppLifecycle::Failed
    ) {
        return Err("Stop the proxy before resetting its interception certificate.".to_owned());
    }
    let result = reset_managed_ca(
        &state.owned_certificate,
        &state.ca_certificate_path,
        &state.ca_private_key_path,
        |sha256| {
            CurrentUserCertificateStore
                .remove(sha256)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    );
    match &result {
        Ok(()) => state.application.record_diagnostic(
            DiagnosticLevel::Info,
            "certificate",
            "reset-succeeded",
            "app-managed interception CA files and recorded identity were removed",
        ),
        Err(message) => state.application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "reset-failed",
            message,
        ),
    }
    result
}

fn reset_managed_ca(
    ownership: &OwnedCertificateRegistry,
    certificate_path: &std::path::Path,
    private_key_path: &std::path::Path,
    remove_trusted: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let sha256 = ownership
        .owned_thumbprint()
        .map_err(|error| error.to_string())?;
    if let Some(sha256) = &sha256 {
        remove_trusted(sha256)?;
    }
    remove_managed_ca_files(certificate_path, private_key_path)?;
    if let Some(sha256) = sha256 {
        ownership
            .clear(&sha256)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn remove_managed_ca_files(
    certificate_path: &std::path::Path,
    private_key_path: &std::path::Path,
) -> Result<(), String> {
    // Validate both destinations before deleting either; never follow links or
    // remove a directory in place of an app-managed certificate file.
    for path in [private_key_path, certificate_path] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err("An interception CA path is not a regular file.".to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not inspect interception CA files: {error}")),
        }
    }
    // Retain the ownership record until both files are gone so cleanup can be
    // retried after a permission or I/O failure.
    for path in [private_key_path, certificate_path] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not remove interception CA files: {error}")),
        }
    }
    Ok(())
}

#[tauri::command]
fn query_sessions(
    query: SessionQueryInput,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<SessionPage, AppError> {
    let application = state.window_application(&window)?;
    application.query_sessions(query)
}

#[tauri::command]
async fn traffic_facets(
    request: transmog_app::TrafficFacetInput,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TrafficFacetPage, AppError> {
    let application = state.window_application(&window)?;
    tauri::async_runtime::spawn_blocking(move || application.traffic_facets(&request))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Traffic facet worker failed".into(),
            retryable: true,
        })?
}

#[tauri::command]
async fn query_traffic_view(
    query: SessionQueryInput,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<TrafficView, AppError> {
    let application = state.window_application(&window)?;
    tauri::async_runtime::spawn_blocking(move || application.query_traffic_view(&query))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Traffic query worker failed".into(),
            retryable: true,
        })?
}

#[tauri::command]
fn traffic_view_rows(
    view_id: String,
    offset: usize,
    limit: usize,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<SessionSummary>, AppError> {
    state
        .window_application(&window)?
        .traffic_view_rows(&view_id, offset, limit)
}

#[tauri::command]
async fn extension_traffic_facets(
    request: transmog_app::ClassificationFacetInput,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::ClassificationFacetPage, AppError> {
    let application = state.window_application(&window)?;
    tauri::async_runtime::spawn_blocking(move || application.extension_traffic_facets(&request))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Extension filter counts could not be read".into(),
            retryable: true,
        })?
}

#[tauri::command]
fn extension_traffic_columns(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<transmog_app::extensions::ClassificationChoice>, AppError> {
    Ok(state
        .window_application(&window)?
        .extension_traffic_columns())
}

#[tauri::command]
async fn classify_traffic_rows(
    ids: Vec<String>,
    columns: Vec<String>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<transmog_app::TrafficClassification>, AppError> {
    let application = state.window_application(&window)?;
    tauri::async_runtime::spawn_blocking(move || application.classify_traffic_rows(&ids, &columns))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Header classification worker failed".into(),
            retryable: true,
        })?
}

#[tauri::command]
fn release_traffic_view(
    view_id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .release_traffic_view(&view_id);
    Ok(())
}

#[tauri::command]
async fn traffic_timeline_rows(
    view_id: String,
    offset: usize,
    limit: usize,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<TrafficTimelineRow>, AppError> {
    let application = state.window_application(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        application.traffic_timeline_rows(&view_id, offset, limit)
    })
    .await
    .map_err(|_| AppError {
        category: transmog_app::ErrorCategory::Internal,
        message: "Timeline query worker failed".into(),
        retryable: true,
    })?
}

#[tauri::command]
fn session_detail(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<SessionDetail, AppError> {
    let application = state.window_application(&window)?;
    application.session_detail(&id)
}

#[tauri::command]
fn inspect_certificates(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::CertificateInspection, AppError> {
    state.window_application(&window)?.inspect_certificates(&id)
}

#[tauri::command]
async fn request_command(
    id: String,
    format: RequestCommandFormat,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<RequestCommand, AppError> {
    let application = state.window_application(&window)?;
    let app = application.clone();
    tokio::task::spawn_blocking(move || app.request_command(&id, format))
        .await
        .map_err(|_| request_action_error())?
}

#[tauri::command]
async fn inspect_headers(
    id: String,
    boundary: String,
    offset: usize,
    largest_first: bool,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::HeaderPage, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || {
        application.inspect_headers(&id, &boundary, offset, largest_first)
    })
    .await
    .map_err(|_| request_action_error())?
}

#[tauri::command]
async fn converter_header_input(
    id: String,
    boundary: String,
    index: usize,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::format_converter::ConverterText, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || application.converter_header_input(&id, &boundary, index))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Converter header input is unavailable".into(),
            retryable: false,
        })?
}

#[tauri::command]
async fn converter_header_value(
    id: String,
    boundary: String,
    index: usize,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || application.converter_header_value(&id, &boundary, index))
        .await
        .map_err(|_| request_action_error())?
}

#[tauri::command]
async fn copy_header_field(
    id: String,
    boundary: String,
    index: usize,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::HeaderCopy, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || application.copy_header_field(&id, &boundary, index))
        .await
        .map_err(|_| request_action_error())?
}

#[tauri::command]
fn copy_message_url(
    id: String,
    boundary: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    state
        .window_application(&window)?
        .copy_message_url(&id, &boundary)
}

#[tauri::command]
async fn copy_message_headers(
    id: String,
    boundary: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || application.copy_message_headers(&id, &boundary))
        .await
        .map_err(|_| request_action_error())?
}

#[tauri::command]
fn copy_all_headers(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    let application = state.window_application(&window)?;
    application.copy_all_headers(&id)
}

#[tauri::command]
async fn composer_source(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ComposerSource, AppError> {
    let application = state.window_application(&window)?;
    let app = application.clone();
    tokio::task::spawn_blocking(move || app.composer_source(&id))
        .await
        .map_err(|_| request_action_error())?
}

#[tauri::command]
async fn save_request_body(
    id: String,
    format: RequestCommandFormat,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<RequestCommand>, AppError> {
    let application = state.window_application(&window)?;
    let app = application.clone();
    let file = tokio::task::spawn_blocking(move || app.prepare_request_file(&id))
        .await
        .map_err(|_| request_action_error())??;
    let Some(destination) = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Save complete request body as")
        .set_file_name("request-body.bin")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path_text = destination.path().to_str().ok_or_else(|| AppError {
        category: transmog_app::ErrorCategory::InvalidInput,
        message: "Choose a request body path that can be represented in a command".into(),
        retryable: false,
    })?;
    let command = file.command(format, path_text)?;
    file.save_to(destination.path().to_owned()).await?;
    Ok(Some(command))
}

fn request_action_error() -> AppError {
    AppError {
        category: transmog_app::ErrorCategory::Unavailable,
        message: "Request action worker failed".into(),
        retryable: true,
    }
}

#[tauri::command]
async fn import_trace(
    request: transmog_app::TraceImportRequest,
    on_progress: Channel<transmog_app::TraceImportProgress>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TraceImportResult, AppError> {
    let application = state.window_application(&window)?;
    let cancel_application = application.clone();
    let last_progress = std::sync::Mutex::new(None);
    application
        .import_trace(
            request,
            Arc::new(move |progress| {
                if !progress_due(&last_progress, progress.completed, progress.total) {
                    return;
                }
                let operation = progress.operation_id.clone();
                if on_progress.send(progress).is_err() {
                    cancel_application.cancel_trace_import(&operation);
                }
            }),
        )
        .await
}

#[tauri::command]
fn cancel_trace_import(
    operation_id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .cancel_trace_import(&operation_id);
    Ok(())
}

#[tauri::command]
fn trace_metadata_list(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<transmog_app::TraceMetadataView>, AppError> {
    Ok(state.window_application(&window)?.trace_metadata_list())
}

#[tauri::command]
fn trace_metadata(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TraceMetadataView, AppError> {
    state.window_application(&window)?.trace_metadata(&id)
}

#[tauri::command]
async fn read_body_range(
    request: transmog_app::BodyRangeRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<tauri::ipc::Response, AppError> {
    let page = state
        .window_application(&window)?
        .read_body_range(request)
        .await?;
    Ok(tauri::ipc::Response::new(crate::body_page::encode(&page)?))
}

#[tauri::command]
async fn inspect_body(
    request: BodyInspectionRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<BodyInspection, AppError> {
    let application = state.window_application(&window)?;
    application.inspect_body(request).await
}

#[tauri::command]
async fn save_traffic_trace(
    options: TraceSaveOptions,
    on_progress: Channel<transmog_app::TraceSaveProgress>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<TraceSaveResult>, AppError> {
    let application = state.window_application(&window)?;
    let (extension, kind) = match options.format {
        transmog_app::TraceSaveFormat::Native => ("tmcap", ArtifactKind::NativeCapture),
        transmog_app::TraceSaveFormat::Har => ("har", ArtifactKind::Har),
        transmog_app::TraceSaveFormat::SazStrict | transmog_app::TraceSaveFormat::SazExtended => {
            ("saz", ArtifactKind::Saz)
        }
    };
    let picker = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Save traffic trace")
        .set_file_name(format!("Transmog-trace.{extension}"))
        .add_filter("Traffic traces", &[extension]);
    let Some(file) = picker.save_file().await else {
        return Ok(None);
    };
    let last_progress = std::sync::Mutex::new(None);
    let last_phase = std::sync::Mutex::new(String::new());
    let result = application
        .save_traffic_trace_with_progress(
            file.path().to_owned(),
            options,
            Arc::new(move |progress| {
                let mut phase = last_phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *phase != progress.phase
                    || progress_due(
                        &last_progress,
                        progress.completed as u64,
                        progress.total as u64,
                    )
                {
                    phase.clone_from(&progress.phase);
                    let _ = on_progress.send(progress);
                }
            }),
        )
        .await?;
    application.remember_artifact(result.destination.clone(), kind);
    Ok(Some(result))
}

#[tauri::command]
async fn save_response_body(
    session_id: String,
    boundary: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<ResponseFileResult>, AppError> {
    let application = state.window_application(&window)?;
    let application = application.clone();
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
async fn stream_page(
    query: transmog_app::StreamQuery,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::StreamPage, AppError> {
    let app = state.window_application(&window)?.clone();
    tokio::task::spawn_blocking(move || app.stream_page(&query))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Stream query failed".into(),
            retryable: true,
        })?
}
#[tauri::command]
async fn stream_message(
    id: String,
    sequence: u64,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::StreamMessage, AppError> {
    let app = state.window_application(&window)?.clone();
    tokio::task::spawn_blocking(move || app.stream_message(&id, sequence))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Stream read failed".into(),
            retryable: true,
        })?
}
#[tauri::command]
async fn save_stream_message(
    id: String,
    sequence: u64,
    raw: bool,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<bool, AppError> {
    let app = state.window_application(&window)?.clone();
    // Retain the exact selected bytes before the user opens a native dialog.
    let message = tokio::task::spawn_blocking(move || app.stream_message(&id, sequence))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Stream read failed".into(),
            retryable: true,
        })??;
    let bytes = if raw {
        message
            .raw
            .map_or_else(|| message.payload.to_vec(), String::into_bytes)
    } else {
        message.payload.to_vec()
    };
    let extension = if message.kind == "binary" {
        "bin"
    } else {
        "txt"
    };
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Save retained message as")
        .set_file_name(format!("message-{sequence}.{extension}"))
        .save_file()
        .await
    else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || transmog_app::save_stream_content(file.path(), &bytes))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Internal,
            message: "Message save failed".into(),
            retryable: true,
        })??;
    Ok(true)
}

#[tauri::command]
fn watch_sessions(
    on_event: Channel<SessionHint>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    let application = state.window_application(&window)?;
    application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "session-watch-requested",
        "live session refresh requested",
    );
    let mut updates = application.subscribe_session_updates()?;
    tauri::async_runtime::spawn(async move {
        loop {
            let update = tokio::select! {
                update = updates.recv() => update,
                () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    if window.is_visible().is_err() { break; }
                    continue;
                }
            };
            let Ok(mut hint) = update else {
                break;
            };
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
    application.record_diagnostic(
        DiagnosticLevel::Info,
        "desktop",
        "session-watch-started",
        "live session refresh channel started",
    );
    Ok(())
}

#[tauri::command]
#[allow(clippy::unused_async)] // Tauri must enter its Tokio runtime before spawning the worker.
async fn enable_breakpoints(
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
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ComposerResult, AppError> {
    let application = state.window_application(&window)?;
    application.execute_composer(request).await
}

#[tauri::command]
fn composer_history(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<ComposerSnapshot>, AppError> {
    let application = state.window_application(&window)?;
    Ok(application.composer_history())
}

#[tauri::command]
async fn start_capture(
    request: CaptureStartRequest,
    state: State<'_, DesktopState>,
) -> Result<CaptureReadModel, AppError> {
    let _certificate_operation = state.certificate_operation.lock().await;
    if !matches!(state.application.status().lifecycle, AppLifecycle::Stopped) {
        return Err(AppError {
            category: transmog_app::ErrorCategory::InvalidInput,
            message: "Stop the proxy before starting a file-only recording.".into(),
            retryable: false,
        });
    }
    validate_proxy_start(
        &state,
        state
            .application
            .product_state()
            .preferences
            .configure_system_proxy,
    )
    .map_err(|message| AppError {
        category: transmog_app::ErrorCategory::InvalidInput,
        message,
        retryable: false,
    })?;
    let path = request.path.clone();
    let result = state.application.start_capture(request).await?;
    if let Err(error) = start_proxy_locked(
        ProxyStartRequest {
            ca_certificate_path: state.ca_certificate_path.clone(),
            ca_private_key_path: state.ca_private_key_path.clone(),
            recording_only: true,
            ..ProxyStartRequest::default()
        },
        &state,
    )
    .await
    {
        let _ = state.application.stop_capture().await;
        return Err(AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: error,
            retryable: true,
        });
    }
    state
        .application
        .remember_artifact(path, ArtifactKind::NativeCapture);
    Ok(result)
}

#[tauri::command]
async fn stop_capture(state: State<'_, DesktopState>) -> Result<CaptureReadModel, AppError> {
    let _certificate_operation = state.certificate_operation.lock().await;
    state.application.shutdown().await?;
    Ok(state.application.capture_status())
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
async fn inspect_trace_metadata(
    request: ImportRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::TraceInspection, AppError> {
    state
        .window_application(&window)?
        .inspect_trace_metadata(request)
        .await
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
    std::fs::create_dir_all(&state.export_root)
        .map_err(|error| format!("capture export directory is unavailable: {error}"))?;
    let destination = unique_artifact_path(&state.export_root, "Transmog-capture", "tmcap");
    let result = state
        .application
        .save_traffic_trace(
            destination.clone(),
            transmog_app::TraceSaveOptions::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
    state
        .application
        .remember_artifact(destination.clone(), ArtifactKind::NativeCapture);
    Ok(ExportResult {
        destination,
        records: result.entries,
        bytes: result.bytes,
        source_sealed: true,
        source_truncated_tail: false,
        fidelity: format!(
            "Saved retained Traffic; {} incomplete body boundaries",
            result.incomplete_bodies
        ),
    })
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
    let state_root = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Transmog");
    #[cfg(feature = "desktop-test-fixtures")]
    let certificate_test_fixture = std::env::var_os("TRANSMOG_DESKTOP_CERTIFICATE_TEST")
        .filter(|value| value == "1")
        .map(|_| {
            Arc::new(tauri::async_runtime::block_on(
                crate::certificate_test_fixture::Fixture::create(&state_root),
            ))
        });
    let certificate_controller = Arc::new(
        tauri::async_runtime::block_on(
            transmog_tls::windows_client_certificates::WindowsClientCertificateController::new(
                transmog_app::ClientCertificatePolicy::default(),
            ),
        )
        .expect("default certificate policy must be valid"),
    );
    let certificates = transmog_app::ClientCertificateManager::new(
        Arc::new(transmog_tls::windows_client_certificates::WindowsCurrentUserCertificateSource),
        certificate_controller.resolver(),
        certificate_controller.broker(),
    );
    #[cfg(feature = "desktop-test-fixtures")]
    let replay = match &certificate_test_fixture {
        Some(fixture) => {
            SystemReplayExecutor::with_tls_factory(ProxyRoute::Auto, fixture.factory())
        }
        None => SystemReplayExecutor::new(ProxyRoute::Auto),
    };
    #[cfg(not(feature = "desktop-test-fixtures"))]
    let replay = SystemReplayExecutor::new(ProxyRoute::Auto);
    let replay = replay
        .expect("operating-system trust must initialize for composer replay")
        .with_client_certificate_resolver(certificates.resolver(), "composer:main")
        .expect("Composer certificate transport must initialize");
    let application = Application::new(AppConfig {
        host_build_info: Some(
            serde_json::from_str(include_str!(concat!(
                env!("OUT_DIR"),
                "/host-build-info.json"
            )))
            .expect("generated desktop build metadata"),
        ),
        #[cfg(feature = "desktop-test-fixtures")]
        upstream_trust: certificate_test_fixture
            .as_ref()
            .map(|fixture| fixture.trust.clone()),
        client_certificates: Some(certificates),
        replay_executor: Some(Arc::new(replay)),
        product_state_path: Some(state_root.join("preferences")),
        automation_path: Some(state_root.join("automation-v1")),
        script_workspace_path: Some(state_root.join("scripts-v1")),
        script_host_executable: packaged_script_host(),
        preview_worker_executable: packaged_preview_worker(),
        extensions: Some(transmog_app::extensions::Registry::load_with_bundled(
            state_root.join("extensions-v1"),
            packaged_script_host()
                .map(|executable| transmog_app::extensions::HostConfig { executable }),
            &crate::bundled_extensions::RESOURCES,
        )),
        response_asset_root: Some(state_root.join("response-assets-v1")),
        saved_demo_root: Some(state_root.join("saved-demos-v1")),
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
    if ca_private_key_path.is_file()
        && let Err(error) =
            transmog_app::protect_ca_private_key(&ca_certificate_path, &ca_private_key_path)
    {
        application.record_diagnostic(
            DiagnosticLevel::Error,
            "certificate",
            "startup-key-protection-failed",
            &error.to_string(),
        );
    }
    let diagnostics_path = state_root.join("diagnostics.jsonl");
    let webview_data_path = state_root.join("WebView2");
    let close_application = application.clone();
    let close_started = Arc::new(AtomicBool::new(false));
    let close_guard = Arc::clone(&close_started);
    let updates = Arc::new(crate::updates::UpdateCoordinator::new(state_root.clone()));
    let close_updates = Arc::clone(&updates);
    let close_host = Arc::clone(&host);

    let exit_host = Arc::clone(&host);
    let exit_application = application.clone();
    let arguments = std::env::args().collect::<Vec<_>>();
    let initial_files =
        workspaces::opened_files(&arguments, &std::env::current_dir().unwrap_or_default());
    let setup_profile = webview_data_path.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, arguments, cwd| {
            workspaces::activate_files(
                app,
                workspaces::opened_files(&arguments, std::path::Path::new(&cwd)),
            );
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates)
        .manage(format_converter::ConverterWindows::new())
        .manage(DesktopState {
            #[cfg(feature = "desktop-test-fixtures")]
            certificate_test_fixture,
            _client_certificate_controller: certificate_controller,
            workspaces: Arc::new(workspaces::WindowWorkspaces::default()),
            webview_data_path,
            application: application.clone(),
            host: Arc::clone(&host),
            owned_certificate,
            certificate_operation: Arc::new(tokio::sync::Mutex::new(())),
            loopback_operation: Arc::new(tokio::sync::Mutex::new(())),
            ca_certificate_path,
            ca_private_key_path,
            diagnostics_path,
            export_root: state_root.join("exports"),
            closing: close_started,
        })
        .register_uri_scheme_protocol("transmog-ui", move |context, request: Request<Vec<u8>>| {
            let state = context.app_handle().state::<DesktopState>();
            let converter_mode = context.app_handle().state::<format_converter::ConverterWindows>().contains(context.webview_label());
            let application = state
                .workspaces
                .application(&state.application, context.webview_label());
            let application = if converter_mode { Ok(state.application.clone()) } else { application };
            let Ok(application) = application else {
                return Response::builder()
                    .status(404)
                    .body(Vec::new())
                    .expect("fixed response");
            };
            let mut view = ShellView::from(&application.status());
            view.converter_mode = converter_mode;
            if converter_mode { view.page_title = "Format converter — Transmog"; }
            view.viewer_mode = context.webview_label() != "main";
            into_tauri_response(renderer.respond(
                request.method().as_str(),
                request.uri().path(),
                &view,
            ))
        })
        .register_uri_scheme_protocol(
            "transmog-preview",
            move |context, request: Request<Vec<u8>>| {
                let state = context.app_handle().state::<DesktopState>();
                let Ok(application) = state
                    .workspaces
                    .application(&state.application, context.webview_label())
                else {
                    return Response::builder()
                        .status(404)
                        .body(Vec::new())
                        .expect("fixed response");
                };
                preview_response(&application, &request)
            },
        )
        .invoke_handler({
            let dispatch: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                format_converter::convert_format,
                format_converter::auth_conversion_catalog,
                format_converter::cancel_format_conversion,
                format_converter::open_converter_window,
                format_converter::take_converter_draft,
                format_converter::acknowledge_converter_window,
                format_converter::close_converter_window,
                format_converter::save_converter_output,
                workspaces::open_trace_viewer,
                workspaces::open_main_window,
                captured_preview::open_captured_page,
                captured_preview::open_saved_demo,
                saved_demos::saved_demos,
                saved_demos::prepare_demo_save,
                saved_demos::commit_demo_save,
                saved_demos::cancel_demo_save,
                saved_demos::cancel_demo_operation,
                saved_demos::delete_saved_demo,
                captured_preview::captured_page_choices,
                captured_preview::experiment_preview_pages,
                captured_preview::set_experiment_preview_page,
                captured_preview::captured_page_report,
                captured_preview::cancel_captured_page,
                incidents::incident_intake,
                incidents::trace_marker_reviews,
                incidents::change_trace_marker,
                incidents::add_trace_marker,
                incidents::discard_marker_changes,
                incidents::set_marker_draft_dirty,
                incidents::close_trace_window,
                incidents::review_traffic_export,
                portable_cases::case_export_choices,
                portable_cases::prepare_case_export,
                portable_cases::save_case_export,
                portable_cases::save_open_case,
                portable_cases::choose_case,
                portable_cases::open_case,
                portable_cases::inspect_case_source,
                portable_cases::case_preview_choices,
                portable_cases::resolve_case_comparison,
                portable_cases::read_case_context,
                portable_cases::save_case_working_copy,
                portable_cases::review_case_rules,
                portable_cases::import_case_rules,
                portable_cases::cancel_case_operation,
                portable_cases::close_case,
                incidents::mark_incident,
                comparison::compare_evidence,
                comparison::close_comparison,
                comparison::cancel_comparison,
                comparison::comparison_page,
                comparison::comparison_stale,
                comparison::comparison_bytes,
                comparison::comparison_fields,
                comparison::compare_captures,
                comparison::reopen_case_capture_comparison,
                comparison::capture_comparison_page,
                comparison::capture_comparison_entries,
                comparison::close_capture_comparison,
                workspaces::take_opened_traces,
                workspaces::pick_trace_path,
                portable_cases::saved_file_is_case,
                import_trace,
                cancel_trace_import,
                trace_metadata_list,
                trace_metadata,
                app_status,
                buffer_status,
                product_state,
                client_certificate_status,
                client_certificate_request,
                client_certificate_identities,
                decide_client_certificate,
                save_client_certificate_policy,
                save_product_state,
                save_network_profiles,
                save_workspace_preferences,
                update_response_set_guide,
                desktop_bootstrap,
                record_frontend_diagnostic,
                automation_status,
                change_header_rule,
                change_response_transform,
                test_response_transform,
                test_header_rule,
                change_response_set,
                response_set_overlaps,
                remove_traffic_entries,
                clear_traffic,
                cancel_requests,
                remove_unselected_traffic_entries,
                search_traffic,
                traffic_search_entry,
                matching_traffic_ids,
                cancel_traffic_search,
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
                inspect_response_asset,
                autoresponse_source_headers,
                edit_response_asset,
                create_autoresponse_batch,
                review_response_set_intake,
                collect_response_set,
                pick_response_body,
                response_body_file_info,
                crate::file_dialogs::pick_capture_path,
                crate::file_dialogs::pick_support_path,
                create_response_asset,
                import_response_asset,
                create_response_asset_from_session,
                diagnostics_report,
                create_support_bundle,
                prepare_update_handoff,
                update_status,
                open_product_website,
                open_response_set_live_page,
                check_updates,
                set_automatic_update_checks,
                app_containers,
                set_loopback_exemptions,
                download_update,
                cancel_update,
                remind_update,
                watch_update_close,
                install_update,
                start_proxy,
                stop_application,
                resume_application,
                retry_host_restore,
                recover_windows_proxy,
                certificate_is_trusted,
                install_certificate,
                remove_certificate,
                create_ca,
                reset_ca,
                query_sessions,
                query_traffic_view,
                traffic_facets,
                traffic_timeline_rows,
                traffic_view_rows,
                extension_traffic_columns,
                extension_traffic_facets,
                classify_traffic_rows,
                release_traffic_view,
                session_detail,
                stream_page,
                stream_message,
                save_stream_message,
                inspect_certificates,
                inspect_headers,
                converter_header_value,
                converter_header_input,
                auth::auth_converter_input,
                copy_header_field,
                copy_message_headers,
                copy_message_url,
                request_command,
                copy_all_headers,
                composer_source,
                save_request_body,
                inspect_body,
                auth::inspect_auth,
                auth::auth_value,
                auth::auth_values,
                auth::auth_source,
                auth::related_auth,
                auth::cancel_auth,
                visualizers::visualizer_page,
                visualizers::visualizer_choices,
                visualizers::activate_visualizer,
                visualizers::reload_visualizer,
                visualizers::visualizer_reload_status,
                visualizers::expand_visualizer,
                visualizers::poll_visualizer_live,
                visualizers::stop_visualizer_live,
                visualizers::visualizer_source,
                visualizers::visualizer_active,
                visualizers::deactivate_visualizer,
                extension_management::extension_status,
                extension_management::install_extension,
                extension_management::load_unpacked_extension,
                extension_management::enable_extension,
                extension_management::extension_hot_reload,
                extension_management::reload_extension,
                extension_management::package_extension,
                extension_management::export_extension,
                extension_management::uninstall_extension,
                read_body_range,
                save_response_body,
                save_traffic_trace,
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
                inspect_trace_metadata,
                export_capture,
                export_live_capture
            ];
            move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
                let window = invoke.message.webview_ref();
                let state = window.state::<DesktopState>();
                let label = window.label();
                let converters = window.state::<format_converter::ConverterWindows>();
                let allowed = if converters.contains(label) { format_converter::command_allowed(invoke.message.command()) } else { state
                    .workspaces
                    .application(&state.application, label)
                    .is_ok()
                    && (label == "main"
                        || workspaces::viewer_command_allowed(invoke.message.command())
                        || format_converter::command_allowed(invoke.message.command())) };
                if allowed {
                    #[cfg(feature = "desktop-test-fixtures")]
                    if matches!(invoke.message.command(), "certificate_test_info" | "delete_certificate_test_key" | "close_certificate_test_viewer" | "close_certificate_test_main") {
                        let dispatch_test: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![certificate_test_info, delete_certificate_test_key, close_certificate_test_viewer, close_certificate_test_main];
                        return dispatch_test(invoke);
                    }
                    dispatch(invoke)
                } else {
                    invoke
                        .resolver
                        .reject("This action is available only in the main proxy window.");
                    true
                }
            }
        })
        .setup(move |app| {
            if initial_files.is_empty() {
                create_main_window(app.handle(), initial_window, setup_profile)?;
            } else {
                workspaces::create_viewer(app.handle(), initial_files)
                    .map_err(|error| std::io::Error::other(error.message))?;
            }
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started
                && let Some(state) = webview.try_state::<DesktopState>()
                && let Ok(application) = state.workspaces.application(&state.application, webview.label())
            {
                application.reset_visualizer_document();
                let window = webview.window();
                application.set_visualizers_visible(!matches!(window.is_minimized(), Ok(true)) && !matches!(window.is_visible(), Ok(false)));
            }
        })
        .on_window_event(move |window, event| {
            if matches!(event, tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)) {
                let state = window.state::<DesktopState>();
                if let Ok(application) = state.workspaces.application(&state.application, window.label()) {
                    let visible = !matches!(window.is_minimized(), Ok(true)) && !matches!(window.is_visible(), Ok(false));
                    application.set_visualizers_visible(visible);
                    if !visible { let _ = tauri::Emitter::emit_to(window, window.label(), "visualizers-suspended", ()); }
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state=window.state::<DesktopState>();
                if window.get_webview_window(window.label()).and_then(|webview| state.window_application(&webview).ok()).is_some_and(|application| application.has_marker_changes()) {
                    api.prevent_close();let _=tauri::Emitter::emit_to(window, window.label(), "trace-close-requested", ());return;
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window.state::<format_converter::ConverterWindows>().release(window.label());
                portable_cases::release(window.label());
                if window.label() == "main" {
                    let state = window.state::<DesktopState>();
                    state.application.reset_visualizer_document();
                    state.application.set_visualizers_visible(false);
                    close_guard.store(false, Ordering::Release);
                } else {
                    let removed = window
                        .state::<DesktopState>()
                        .workspaces
                        .viewers
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(window.label());
                    if let Some(viewer) = removed { viewer.application.deactivate_visualizers(); viewer.application.cancel_composer_requests(); }
                }
            }
            if window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                if close_guard.load(Ordering::Acquire) || close_updates.request_scheduled_close() {
                    return;
                }
                if close_guard.swap(true, Ordering::AcqRel) {
                    return;
                }
                close_updates.cancel();
                let application = close_application.clone();
                let host = Arc::clone(&close_host);
                let window = window.clone();
                let close_guard = Arc::clone(&close_guard);
                tauri::async_runtime::spawn(async move {
                    persist_window_state(&application, &window);
                    if application.shutdown().await.is_ok()
                        && host.recover_pending().is_ok()
                        && !host.recovery_pending()
                    {
                        if let Err(error) = application.discard_traffic() {
                            application.record_diagnostic(
                                DiagnosticLevel::Error,
                                "desktop",
                                "traffic-cache-cleanup-failed",
                                &error.message,
                            );
                        }
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
    if let Some(code) = crate::loopback::maintenance_exit_code() {
        return Some(code);
    }
    if let Some(code) = crate::update_policy::preference_maintenance_exit_code() {
        return Some(code);
    }
    if let Some(code) = crate::updates::artifact_verification_exit_code() {
        return Some(code);
    }
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
    }
    remove_owned_app_data(&state_root)
}

fn remove_owned_app_data(state_root: &std::path::Path) -> Result<(), ()> {
    if state_root.file_name().and_then(|name| name.to_str()) != Some("Transmog") {
        return Err(());
    }
    remove_managed_ca_files(
        &state_root.join("interception-ca.pem"),
        &state_root.join("interception-ca.key"),
    )
    .map_err(|_| ())?;
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
            || crate::update_policy::is_owned_update_file(&name)
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
    app: &tauri::AppHandle,
    initial: WindowState,
    webview_data_path: PathBuf,
) -> tauri::Result<()> {
    let url =
        tauri::Url::parse("transmog-ui://localhost/").expect("fixed application URL must parse");
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
        .title(format!("Transmog {}", transmog_app::application_version()))
        .min_inner_size(760.0, 520.0)
        .visible(false)
        .data_directory(webview_data_path)
        .on_navigation(is_allowed_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()?;
    // WindowState stores physical pixels; builder geometry uses logical pixels.
    // Move to the saved monitor before sizing so its DPI applies, and finish
    // restoring the normal bounds before maximizing or showing the window.
    if let (Some(x), Some(y)) = (initial.x, initial.y) {
        window.set_position(tauri::PhysicalPosition::new(x, y))?;
    }
    window.set_size(tauri::PhysicalSize::new(initial.width, initial.height))?;
    recover_offscreen_main_window(&window, &initial)?;
    if initial.maximized {
        window.maximize()?;
    }
    window.show()?;
    Ok(())
}

fn recover_offscreen_main_window(
    window: &tauri::WebviewWindow,
    initial: &WindowState,
) -> tauri::Result<()> {
    use crate::window_geometry::{PhysicalBounds, recovery_monitor};

    // Missing geometry is not evidence that the restored window is off screen.
    let (Ok(position), Ok(size), Ok(monitors)) = (
        window.outer_position(),
        window.outer_size(),
        window.available_monitors(),
    ) else {
        return Ok(());
    };
    let bounds: Vec<_> = monitors
        .iter()
        .map(|monitor| PhysicalBounds {
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
        })
        .collect();
    let primary = window.primary_monitor().ok().flatten().and_then(|primary| {
        monitors.iter().position(|monitor| {
            monitor.position() == primary.position() && monitor.size() == primary.size()
        })
    });
    let Some(index) = recovery_monitor(
        PhysicalBounds {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        },
        &bounds,
        primary,
    ) else {
        return Ok(());
    };
    let monitor = &monitors[index];
    let work_area = monitor.work_area();
    let position = if work_area.size.width > 0 && work_area.size.height > 0 {
        work_area.position
    } else {
        *monitor.position()
    };
    // Anchor the title bar inside the work area even if the saved size is larger
    // than this display. Moving can change DPI, so restore the physical inner
    // size again on the destination before maximizing or showing the window.
    window.set_position(position)?;
    window.set_size(tauri::PhysicalSize::new(initial.width, initial.height))?;
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
    use super::{
        OwnedCertificateRegistry, is_allowed_navigation, preview_handle, remove_owned_app_data,
        reset_managed_ca, validate_proxy_trust,
    };

    #[tokio::test]
    async fn manual_client_routing_does_not_require_installing_a_test_root() {
        // A fresh fixture identity is never installed by this test.
        let root = tempfile::tempdir().unwrap();
        let application =
            transmog_app::Application::new(transmog_app::AppConfig::default()).unwrap();
        let identity = application
            .create_ca(transmog_app::CaCreateRequest {
                certificate_path: root.path().join("ca.pem"),
                private_key_path: root.path().join("ca.key"),
                common_name: "Manual routing fixture".to_owned(),
                validity_days: 1,
            })
            .await
            .unwrap();
        assert!(validate_proxy_trust(&identity.sha256, false).is_ok());
        assert!(validate_proxy_trust(&identity.sha256, true).is_err());
    }

    #[test]
    fn certificate_reset_removes_the_exact_recorded_root_and_material() {
        let root = tempfile::tempdir().unwrap();
        let ownership = OwnedCertificateRegistry::new(root.path().join("ownership.json"));
        let sha256 = "AB".repeat(32);
        ownership.claim(&sha256).unwrap();
        let certificate = root.path().join("interception-ca.pem");
        let key = root.path().join("interception-ca.key");
        std::fs::write(&certificate, b"certificate").unwrap();
        std::fs::write(&key, b"key").unwrap();
        reset_managed_ca(&ownership, &certificate, &key, |expected| {
            assert_eq!(expected, sha256);
            assert!(certificate.is_file() && key.is_file());
            assert_eq!(ownership.owned_thumbprint().unwrap(), Some(sha256.clone()));
            Ok(())
        })
        .unwrap();
        assert!(!certificate.exists() && !key.exists());
        assert_eq!(ownership.owned_thumbprint().unwrap(), None);
    }

    #[test]
    fn certificate_reset_handles_legacy_and_partial_files_without_guessing_a_root() {
        for (has_certificate, has_key) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let root = tempfile::tempdir().unwrap();
            let ownership = OwnedCertificateRegistry::new(root.path().join("ownership.json"));
            let certificate = root.path().join("interception-ca.pem");
            let key = root.path().join("interception-ca.key");
            if has_certificate {
                std::fs::write(&certificate, b"legacy certificate").unwrap();
            }
            if has_key {
                std::fs::write(&key, b"legacy key").unwrap();
            }
            reset_managed_ca(&ownership, &certificate, &key, |_| {
                panic!("unrecorded roots must not be removed")
            })
            .unwrap();
            assert!(!certificate.exists() && !key.exists());
        }
    }

    #[test]
    fn certificate_reset_retains_material_and_identity_when_root_removal_fails() {
        let root = tempfile::tempdir().unwrap();
        let ownership = OwnedCertificateRegistry::new(root.path().join("ownership.json"));
        let sha256 = "CD".repeat(32);
        ownership.claim(&sha256).unwrap();
        let certificate = root.path().join("interception-ca.pem");
        let key = root.path().join("interception-ca.key");
        std::fs::write(&certificate, b"certificate").unwrap();
        std::fs::write(&key, b"key").unwrap();
        assert!(
            reset_managed_ca(&ownership, &certificate, &key, |_| {
                Err("store unavailable".to_owned())
            })
            .is_err()
        );
        assert!(certificate.is_file() && key.is_file());
        assert_eq!(ownership.owned_thumbprint().unwrap(), Some(sha256));
    }

    #[test]
    fn uninstall_retains_identity_and_key_when_a_ca_path_is_not_a_file() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("Transmog");
        std::fs::create_dir(&root).unwrap();
        let ownership = OwnedCertificateRegistry::new(root.join("certificate-ownership-v1.json"));
        let sha256 = "EF".repeat(32);
        ownership.claim(&sha256).unwrap();
        std::fs::create_dir(root.join("interception-ca.pem")).unwrap();
        std::fs::write(root.join("interception-ca.key"), b"key").unwrap();
        assert!(remove_owned_app_data(&root).is_err());
        assert!(root.join("interception-ca.key").is_file());
        assert_eq!(ownership.owned_thumbprint().unwrap(), Some(sha256));
    }

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
            "certificate-ownership-v1.json",
            "interception-ca.pem",
            "interception-ca.key",
            "preferences.00000000000000000001.json",
            "preferences.00000000000000000002.json.corrupt",
            ".transmog-state-00000000000000000003-1.tmp",
        ] {
            std::fs::write(root.join(name), b"owned").unwrap();
        }
        std::fs::write(root.join("preferences.user-notes.txt"), b"keep too").unwrap();
        std::fs::write(root.join("user-kept.txt"), b"keep").unwrap();
        std::fs::write(root.join("custom-ca.pem"), b"user certificate").unwrap();
        std::fs::write(root.join("custom-ca.key"), b"user key").unwrap();
        remove_owned_app_data(&root).unwrap();
        assert_eq!(std::fs::read(root.join("user-kept.txt")).unwrap(), b"keep");
        assert_eq!(
            std::fs::read(root.join("custom-ca.pem")).unwrap(),
            b"user certificate"
        );
        assert_eq!(
            std::fs::read(root.join("custom-ca.key")).unwrap(),
            b"user key"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 4);
        std::fs::remove_dir_all(parent).unwrap();
    }
}
