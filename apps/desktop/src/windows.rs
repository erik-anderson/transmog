#![allow(clippy::needless_pass_by_value)] // Tauri commands deserialize owned IPC arguments.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tauri::{
    Manager, State, WebviewWindowBuilder,
    http::{Request, Response, StatusCode, header},
    ipc::Channel,
    utils::config::WebviewUrl,
};
use transmog_app::{
    AppConfig, AppError, AppStatus, Application, ArtifactKind, BodyInspection,
    BodyInspectionRequest, BodyStoreConfig, BreakpointDecision, BreakpointSettings,
    BreakpointStatus, CaCreateRequest, CaIdentity, CaptureReadModel, CaptureStartRequest,
    CaptureSummaryView, ComposerRequest, ComposerResult, ComposerSnapshot, DiagnosticsReport,
    ExportFormat, ExportRequest, ExportResult, ImportRequest, ProductState, ProxyRoute,
    ProxyStartRequest, RuntimeDiagnostics, SessionDetail, SessionHint, SessionPage,
    SessionQueryInput, SupportBundleRequest, SupportBundleResult, SystemReplayExecutor,
    WindowState,
};
use transmog_app_webui::{AppRenderer, ShellView, UiError, UiResponse};
use transmog_host_windows::{
    CurrentUserCertificateStore, CurrentUserKeyProtection, OwnedCertificateRegistry,
    WindowsProxyIntegration,
};

const UI_HOST: &str = "transmog-ui.localhost";

#[derive(Clone)]
struct DesktopState {
    application: Application,
    host: Arc<WindowsProxyIntegration>,
    owned_certificate: OwnedCertificateRegistry,
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
    configure_system_proxy: bool,
    state: State<'_, DesktopState>,
) -> Result<AppStatus, AppError> {
    let host = configure_system_proxy
        .then(|| Arc::clone(&state.host) as Arc<dyn transmog_session::HostIntegration>);
    state.application.start_proxy(request, host).await
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
    let newly_claimed = state
        .owned_certificate
        .claim(&sha256)
        .map_err(|error| error.to_string())?;
    if let Err(error) = CurrentUserCertificateStore.install(&path, &sha256) {
        if newly_claimed {
            let _ = state.owned_certificate.clear(&sha256);
        }
        return Err(error.to_string());
    }
    Ok(())
}

#[tauri::command]
fn remove_certificate(sha256: String, state: State<'_, DesktopState>) -> Result<(), String> {
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
    state
        .owned_certificate
        .clear(&sha256)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_ca(
    request: CaCreateRequest,
    state: State<'_, DesktopState>,
) -> Result<CaIdentity, String> {
    let private_key_path = request.private_key_path.clone();
    let identity = state
        .application
        .create_ca(request)
        .await
        .map_err(|error| error.to_string())?;
    CurrentUserKeyProtection
        .protect(&private_key_path)
        .map_err(|error| format!("CA was created but private-key protection failed: {error}"))?;
    state
        .application
        .remember_artifact(identity.certificate_path.clone(), ArtifactKind::Certificate);
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
fn watch_sessions(
    on_event: Channel<SessionHint>,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
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
        ExportFormat::JsonLines => ArtifactKind::JsonLines,
        ExportFormat::SazStrict | ExportFormat::SazExtended => ArtifactKind::Saz,
    };
    let result = state.application.export_capture(request).await?;
    state.application.remember_artifact(path, kind);
    Ok(result)
}

/// Runs the Windows `WebView2` host with a fixed embedded origin.
///
/// # Panics
///
/// Panics if the embedded assets, application service, or Tauri host cannot initialize.
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
    let owned_certificate =
        OwnedCertificateRegistry::new(state_root.join("certificate-ownership-v1.json"));
    let protocol_application = application.clone();
    let close_application = application.clone();
    let close_started = Arc::new(AtomicBool::new(false));
    let close_guard = Arc::clone(&close_started);

    tauri::Builder::default()
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
            host,
            owned_certificate,
        })
        .register_uri_scheme_protocol("transmog-ui", move |_context, request: Request<Vec<u8>>| {
            let view = ShellView::from(&protocol_application.status());
            into_tauri_response(renderer.respond(
                request.method().as_str(),
                request.uri().path(),
                &view,
            ))
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            product_state,
            save_product_state,
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
            export_capture
        ])
        .setup(move |app| Ok(create_main_window(app, initial_window)?))
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
        .run(tauri::generate_context!())
        .expect("Tauri desktop host failed");
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
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && entry.file_name() == "body-cache-v1"
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

fn create_main_window(app: &tauri::App, initial: WindowState) -> tauri::Result<()> {
    let url =
        tauri::Url::parse("transmog-ui://localhost/").expect("fixed application URL must parse");
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
        .title("Transmog")
        .inner_size(f64::from(initial.width), f64::from(initial.height))
        .min_inner_size(760.0, 520.0)
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
    use super::{is_allowed_navigation, remove_owned_app_data};

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
