#![allow(clippy::needless_pass_by_value)] // Tauri commands deserialize owned IPC arguments.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tauri::{
    State, WebviewWindowBuilder,
    http::{Request, Response, StatusCode, header},
    ipc::Channel,
    utils::config::WebviewUrl,
};
use transmog_app::{
    AppConfig, AppError, AppStatus, Application, BreakpointDecision, BreakpointSettings,
    BreakpointStatus, CaptureReadModel, CaptureStartRequest, CaptureSummaryView, ComposerRequest,
    ComposerResult, ComposerSnapshot, ExportRequest, ExportResult, ImportRequest, ProxyRoute,
    ProxyStartRequest, SessionDetail, SessionHint, SessionPage, SessionQueryInput,
    SystemReplayExecutor,
};
use transmog_app_webui::{AppRenderer, ShellView, UiError, UiResponse};
use transmog_host_windows::{CurrentUserCertificateStore, WindowsProxyIntegration};

const UI_HOST: &str = "transmog-ui.localhost";

#[derive(Clone)]
struct DesktopState {
    application: Application,
    host: Arc<WindowsProxyIntegration>,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn app_status(state: State<'_, DesktopState>) -> AppStatus {
    state.application.status()
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
fn install_certificate(path: PathBuf, sha256: String) -> Result<(), String> {
    CurrentUserCertificateStore
        .install(&path, &sha256)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_certificate(sha256: String) -> Result<(), String> {
    CurrentUserCertificateStore
        .remove(&sha256)
        .map(|_| ())
        .map_err(|error| error.to_string())
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
    state.application.start_capture(request).await
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
    state.application.import_capture(request).await
}

#[tauri::command]
async fn export_capture(
    request: ExportRequest,
    state: State<'_, DesktopState>,
) -> Result<ExportResult, AppError> {
    state.application.export_capture(request).await
}

/// Runs the Windows `WebView2` host with a fixed embedded origin.
///
/// # Panics
///
/// Panics if the embedded assets, application service, or Tauri host cannot initialize.
pub fn run() {
    let replay = SystemReplayExecutor::new(ProxyRoute::Auto)
        .expect("operating-system trust must initialize for composer replay");
    let application = Application::new(AppConfig {
        replay_executor: Some(Arc::new(replay)),
        ..AppConfig::default()
    })
    .expect("application must initialize");
    let renderer = AppRenderer::new().expect("embedded WebUI assets must be valid");
    let journal_root = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Transmog");
    let host = Arc::new(WindowsProxyIntegration::system(
        journal_root.join("proxy-recovery-v1.json"),
    ));
    let protocol_application = application.clone();
    let close_application = application.clone();
    let close_started = Arc::new(AtomicBool::new(false));
    let close_guard = Arc::clone(&close_started);

    tauri::Builder::default()
        .manage(DesktopState {
            application: application.clone(),
            host,
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
            start_proxy,
            stop_application,
            retry_host_restore,
            recover_windows_proxy,
            certificate_is_trusted,
            install_certificate,
            remove_certificate,
            query_sessions,
            session_detail,
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
        .setup(|app| {
            let url = tauri::Url::parse("transmog-ui://localhost/")
                .expect("fixed application URL must parse");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
                .title("Transmog")
                .inner_size(1180.0, 760.0)
                .min_inner_size(760.0, 520.0)
                .on_navigation(is_allowed_navigation)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && !close_guard.swap(true, Ordering::AcqRel)
            {
                api.prevent_close();
                let application = close_application.clone();
                let window = window.clone();
                let close_guard = Arc::clone(&close_guard);
                tauri::async_runtime::spawn(async move {
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
    use super::is_allowed_navigation;

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
}
