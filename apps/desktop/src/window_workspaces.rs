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
    pub fn prepare_network_profile(
        &self,
        profile: transmog_app::NetworkProfileSnapshot,
    ) -> Result<Vec<Box<dyn FnOnce() + Send>>, AppError> {
        self.viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|viewer| {
                viewer
                    .application
                    .prepare_replay_network_profile(profile.clone())
            })
            .collect()
    }
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
            | "converter_header_value"
            | "client_certificate_status"
            | "product_state"
            | "save_workspace_preferences"
            | "desktop_bootstrap"
            | "record_frontend_diagnostic"
            | "query_sessions"
            | "query_traffic_view"
            | "traffic_facets"
            | "traffic_view_rows"
            | "extension_traffic_columns"
            | "classify_traffic_rows"
            | "traffic_timeline_rows"
            | "release_traffic_view"
            | "session_detail"
            | "incident_intake"
            | "trace_marker_reviews"
            | "change_trace_marker"
            | "add_trace_marker"
            | "discard_marker_changes"
            | "set_marker_draft_dirty"
            | "close_trace_window"
            | "review_traffic_export"
            | "case_export_choices"
            | "prepare_case_export"
            | "save_case_export"
            | "save_open_case"
            | "choose_case"
            | "open_case"
            | "saved_file_is_case"
            | "inspect_case_source"
            | "read_case_context"
            | "save_case_working_copy"
            | "cancel_case_operation"
            | "close_case"
            | "compare_evidence"
            | "close_comparison"
            | "cancel_comparison"
            | "comparison_page"
            | "comparison_stale"
            | "comparison_bytes"
            | "compare_captures"
            | "capture_comparison_page"
            | "capture_comparison_entries"
            | "close_capture_comparison"
            | "stream_page"
            | "stream_message"
            | "save_stream_message"
            | "inspect_certificates"
            | "inspect_headers"
            | "copy_message_headers"
            | "copy_message_url"
            | "request_command"
            | "copy_all_headers"
            | "composer_source"
            | "save_request_body"
            | "inspect_body"
            | "visualizer_page"
            | "visualizer_choices"
            | "activate_visualizer"
            | "reload_visualizer"
            | "visualizer_reload_status"
            | "expand_visualizer"
            | "poll_visualizer_live"
            | "stop_visualizer_live"
            | "visualizer_source"
            | "visualizer_active"
            | "deactivate_visualizer"
            | "read_body_range"
            | "save_response_body"
            | "save_traffic_trace"
            | "open_captured_page"
            | "captured_page_report"
            | "captured_page_choices"
            | "cancel_captured_page"
            | "watch_sessions"
            | "execute_composer"
            | "composer_history"
            | "remove_traffic_entries"
            | "clear_traffic"
            | "remove_unselected_traffic_entries"
            | "search_traffic"
            | "traffic_search_entry"
            | "matching_traffic_ids"
            | "cancel_traffic_search"
            | "buffer_status"
            | "import_trace"
            | "cancel_trace_import"
            | "trace_metadata_list"
            | "trace_metadata"
            | "inspect_trace_metadata"
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
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            transmog_app::detect_trace_format(&path)
                .is_some()
                .then_some(path)
        })
        .take(16)
        .collect()
}

pub(super) fn create_viewer(
    app: &tauri::AppHandle,
    paths: Vec<PathBuf>,
) -> Result<String, AppError> {
    let state = app.state::<DesktopState>();
    // Creation runs on the setup thread or an owned blocking worker. Serialize
    // with profile activation so no new viewer starts with a retired recipe.
    let _profile_operation = state.certificate_operation.blocking_lock();
    let mut viewers = state
        .workspaces
        .viewers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.closing.load(Ordering::Acquire) {
        return Err(error(
            "Transmog is closing. Open the viewer after the update.",
        ));
    }
    if viewers.len() >= 8 {
        return Err(error(
            "Close a capture viewer before opening another (maximum 8)",
        ));
    }
    let root = tempfile::Builder::new()
        .prefix("transmog-viewer-")
        .tempdir()
        .map_err(|_| error("Viewer storage could not be created"))?;
    let label = format!(
        "viewer-{}",
        ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    #[cfg(feature = "desktop-test-fixtures")]
    let replay = match &state.certificate_test_fixture {
        Some(fixture) => {
            SystemReplayExecutor::with_tls_factory(ProxyRoute::Auto, fixture.factory())
        }
        None => SystemReplayExecutor::new(ProxyRoute::Auto),
    };
    #[cfg(not(feature = "desktop-test-fixtures"))]
    let replay = SystemReplayExecutor::new(ProxyRoute::Auto);
    let replay = replay
        .map_err(|_| error("Composer trust could not be initialized"))?
        .with_client_certificate_resolver(
            state
                .application
                .client_certificates()
                .expect("desktop certificate controller")
                .resolver(),
            format!("composer:{label}"),
        )
        .map_err(|_| error("Composer certificate transport could not be initialized"))?;
    let application = Application::new(AppConfig {
        replay_executor: Some(Arc::new(replay)),
        body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
        preview_worker_executable: super::packaged_preview_worker(),
        extensions: Some(state.application.extensions()),
        ..AppConfig::default()
    })?;
    application.save_product_state(state.application.product_state())?;
    application.prepare_replay_network_profile(
        state
            .application
            .product_state()
            .preferences
            .network_profiles
            .active_profile,
    )?();
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
        .add_filter(
            "Traffic captures",
            &["saz", "tmcap", "har", "netlog", "json", "jsonl"],
        )
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
    #[test]
    fn file_launch_keeps_saz_and_native_inputs_and_ignores_bare_case_zips() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "traffic.saz",
            "traffic.SAZ",
            "old.tmcase.zip",
            "old.tmrules.zip",
        ] {
            std::fs::write(root.path().join(name), b"PK\x03\x04fixture").unwrap();
        }
        std::fs::write(root.path().join("case.tmcase.tmcap"), b"TMCAP001\0").unwrap();
        let arguments: Vec<_> = [
            "transmog",
            "traffic.saz",
            "traffic.SAZ",
            "case.tmcase.tmcap",
            "old.tmcase.zip",
            "old.tmrules.zip",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            super::opened_files(&arguments, root.path()),
            ["traffic.saz", "traffic.SAZ", "case.tmcase.tmcap"].map(|name| root.path().join(name))
        );
    }
    use super::*;
    #[tokio::test]
    async fn profile_updates_reach_open_viewers_without_replacing_their_workspace() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        use transmog_app::{AllowedHttpProtocols, ComposerRequest};
        let workspaces = WindowWorkspaces::default();
        let mut applications = Vec::new();
        for index in 0..2 {
            let application = Application::new(AppConfig {
                replay_executor: Some(Arc::new(
                    SystemReplayExecutor::new(ProxyRoute::Http1).unwrap(),
                )),
                ..AppConfig::default()
            })
            .unwrap();
            workspaces.viewers.lock().unwrap().insert(
                format!("viewer-{index}"),
                Viewer {
                    application: application.clone(),
                    _root: tempfile::tempdir().unwrap(),
                },
            );
            applications.push((application.clone(), application.product_state()));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = ComposerRequest {
            method: "GET".into(),
            url: format!("http://{}/", listener.local_addr().unwrap()),
            headers: vec![],
            body: String::new(),
            body_is_hex: false,
            body_source: None,
            source_entry_id: None,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        };
        let mut blocked = transmog_app::NetworkProfileSnapshot::default();
        blocked.settings.protocols = AllowedHttpProtocols {
            http1: false,
            http2: false,
            http3: false,
        };
        for update in workspaces.prepare_network_profile(blocked).unwrap() {
            update();
        }
        for (application, original) in &applications {
            assert!(application.execute_composer(input.clone()).await.is_err());
            assert_eq!(&application.product_state(), original);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(socket.read_u8().await.unwrap());
                }
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
        });
        for update in workspaces
            .prepare_network_profile(transmog_app::NetworkProfileSnapshot::default())
            .unwrap()
        {
            update();
        }
        for (application, original) in applications {
            assert_eq!(
                application
                    .execute_composer(input.clone())
                    .await
                    .unwrap()
                    .body,
                "ok"
            );
            assert_eq!(application.product_state(), original);
        }
        server.await.unwrap();
    }

    #[test]
    fn viewer_permissions_exclude_every_proxy_and_certificate_mutation() {
        for command in [
            "start_proxy",
            "cancel_requests",
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
            "change_header_rule",
            "change_response_transform",
            "test_response_transform",
            "test_header_rule",
            "change_response_set",
            "response_set_overlaps",
            "enable_breakpoints",
            "save_product_state",
            "save_client_certificate_policy",
            "client_certificate_identities",
            "client_certificate_request",
            "decide_client_certificate",
        ] {
            assert!(!viewer_command_allowed(command), "{command}");
        }
        for command in [
            "import_trace",
            "execute_composer",
            "request_command",
            "read_body_range",
            "visualizer_choices",
            "activate_visualizer",
            "reload_visualizer",
            "visualizer_reload_status",
            "expand_visualizer",
            "poll_visualizer_live",
            "stop_visualizer_live",
            "visualizer_source",
            "visualizer_active",
            "deactivate_visualizer",
            "visualizer_page",
            "inspect_certificates",
            "copy_message_url",
            "open_main_window",
        ] {
            assert!(viewer_command_allowed(command));
        }
    }
}
