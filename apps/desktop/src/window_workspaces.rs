//! Saved-capture windows own independent catalogs. Window roles are checked at
//! the IPC boundary, so hidden buttons are not a proxy-control permission.
use super::{ARTIFACT_SEQUENCE, DesktopState, create_main_window, is_allowed_navigation};
use std::sync::atomic::Ordering;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, WebviewWindow, WebviewWindowBuilder, utils::config::WebviewUrl};
use transmog_app::{
    AppConfig, AppError, Application, BodyStoreConfig, ErrorCategory, ProxyRoute,
    SystemReplayExecutor,
};

pub(super) struct Viewer {
    pub application: Application,
    _root: tempfile::TempDir,
}
#[derive(Default)]
pub(super) struct WindowWorkspaces {
    main_open: Arc<tokio::sync::Mutex<()>>,
    pub viewers: Mutex<HashMap<String, Viewer>>,
    pending: Mutex<HashMap<String, Vec<PathBuf>>>,
}
impl WindowWorkspaces {
    pub fn application(&self, main: &Application, label: &str) -> Result<Application, AppError> {
        if label == "main" {
            return Ok(main.clone());
        }
        self.viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(label)
            .map(|viewer| viewer.application.clone())
            .ok_or_else(|| error("This capture window is no longer available"))
    }
    pub fn queue(&self, label: &str, paths: Vec<PathBuf>) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let queue = pending.entry(label.into()).or_default();
        queue.extend(paths.into_iter().take(16_usize.saturating_sub(queue.len())));
    }
    pub fn take(&self, label: &str) -> Vec<PathBuf> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(label)
            .unwrap_or_default()
    }
}

pub(super) fn viewer_command_allowed(command: &str) -> bool {
    matches!(
        command,
        "app_status"
            | "product_state"
            | "save_workspace_preferences"
            | "desktop_bootstrap"
            | "record_frontend_diagnostic"
            | "query_sessions"
            | "session_detail"
            | "request_command"
            | "copy_all_headers"
            | "composer_source"
            | "save_request_body"
            | "inspect_body"
            | "save_response_body"
            | "watch_sessions"
            | "execute_composer"
            | "composer_history"
            | "remove_traffic_entries"
            | "remove_unselected_traffic_entries"
            | "search_traffic"
            | "matching_traffic_ids"
            | "cancel_traffic_search"
            | "import_trace"
            | "cancel_trace_import"
            | "trace_metadata_list"
            | "trace_metadata"
            | "pick_trace_path"
            | "open_trace_viewer"
            | "open_main_window"
            | "take_opened_traces"
    )
}
pub(super) fn opened_files(arguments: &[String], cwd: &Path) -> Vec<PathBuf> {
    arguments
        .iter()
        .skip(1)
        .filter_map(|argument| {
            let path = PathBuf::from(argument);
            if !path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("saz") || extension.eq_ignore_ascii_case("tmcap")
            }) {
                return None;
            }
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            path.is_file().then_some(path)
        })
        .take(16)
        .collect()
}

pub(super) fn create_viewer(
    app: &tauri::AppHandle,
    paths: Vec<PathBuf>,
) -> Result<String, AppError> {
    let state = app.state::<DesktopState>();
    let mut viewers = state
        .workspaces
        .viewers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if viewers.len() >= 8 {
        return Err(error(
            "Close a capture viewer before opening another (maximum 8)",
        ));
    }
    let root = tempfile::Builder::new()
        .prefix("transmog-viewer-")
        .tempdir()
        .map_err(|_| error("Viewer storage could not be created"))?;
    let replay = SystemReplayExecutor::new(ProxyRoute::Auto)
        .map_err(|_| error("Composer trust could not be initialized"))?;
    let application = Application::new(AppConfig {
        replay_executor: Some(Arc::new(replay)),
        body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
        preview_worker_executable: super::packaged_preview_worker(),
        ..AppConfig::default()
    })?;
    application.save_product_state(state.application.product_state())?;
    let label = format!(
        "viewer-{}",
        ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    // These windows all render the trusted application shell. Sharing its
    // browser profile avoids a browser process per viewer; evidence and IPC
    // permissions remain isolated by the window's Rust application instance.
    let profile = state.webview_data_path.clone();
    viewers.insert(
        label.clone(),
        Viewer {
            application,
            _root: root,
        },
    );
    drop(viewers);
    state.workspaces.queue(&label, paths);
    let result = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::CustomProtocol(
            tauri::Url::parse("transmog-ui://localhost/").expect("fixed URL"),
        ),
    )
    .title("Transmog — Capture viewer")
    .inner_size(1200.0, 820.0)
    .min_inner_size(760.0, 520.0)
    .data_directory(profile)
    .on_navigation(is_allowed_navigation)
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .build();
    if result.is_err() {
        state
            .workspaces
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&label);
        state.workspaces.take(&label);
        return Err(error("Capture viewer could not be opened"));
    }
    Ok(label)
}

pub(super) fn activate_files(app: &tauri::AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        if let Some(window) = app.get_webview_window("main") {
            focus(&window);
        }
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        app.state::<DesktopState>().workspaces.queue("main", paths);
        focus(&window);
        let _ = window.emit("trace-open-request", ());
    } else {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = create_viewer(&app, paths) {
                app.state::<DesktopState>().application.record_diagnostic(
                    transmog_app::DiagnosticLevel::Error,
                    "viewer",
                    "open-failed",
                    &error.message,
                );
            }
        });
    }
}
fn focus(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}
#[tauri::command]
pub(super) async fn open_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let operation = app.state::<DesktopState>().workspaces.main_open.clone();
    let _operation = operation.lock().await;
    // WebView2 window creation must not run in a synchronous IPC callback.
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(window) = app.get_webview_window("main") {
            focus(&window);
            return Ok(());
        }
        let state = app.state::<DesktopState>();
        create_main_window(
            &app,
            state.application.product_state().window,
            state.webview_data_path.clone(),
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "Main window worker failed".to_owned())?
}
#[tauri::command]
pub(super) async fn open_trace_viewer(
    paths: Vec<PathBuf>,
    app: tauri::AppHandle,
) -> Result<String, AppError> {
    if paths.len() > 16
        || paths
            .iter()
            .any(|path| !path.is_absolute() || !path.is_file())
    {
        return Err(error("Choose existing capture files"));
    }
    tauri::async_runtime::spawn_blocking(move || create_viewer(&app, paths))
        .await
        .map_err(|_| error("Capture viewer worker failed"))?
}
#[tauri::command]
pub(super) fn take_opened_traces(
    window: WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Vec<PathBuf> {
    state.workspaces.take(window.label())
}
#[tauri::command]
pub(super) async fn pick_trace_path(window: WebviewWindow) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Open saved traffic")
        .add_filter("Traffic captures", &["saz", "tmcap"])
        .pick_file()
        .await
        .map(|file| file.path().to_string_lossy().into_owned())
}
fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewer_permissions_exclude_every_proxy_and_certificate_mutation() {
        for command in [
            "start_proxy",
            "stop_application",
            "retry_host_restore",
            "recover_windows_proxy",
            "install_certificate",
            "remove_certificate",
            "create_ca",
            "reset_ca",
            "start_capture",
            "stop_capture",
            "activate_automation",
            "enable_breakpoints",
            "save_product_state",
        ] {
            assert!(!viewer_command_allowed(command), "{command}");
        }
        for command in [
            "import_trace",
            "execute_composer",
            "request_command",
            "open_main_window",
        ] {
            assert!(viewer_command_allowed(command));
        }
    }
}
