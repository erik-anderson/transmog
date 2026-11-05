//! One debugger controller with acknowledged dock/window ownership transfer.
use super::DesktopState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{collections::HashMap, sync::Mutex, time::Duration};
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
use tauri::{Emitter, Manager, State, WebviewWindowBuilder, utils::config::WebviewUrl};
use transmog_app::{
    Application, DebugAction, DebugSessionView, DebuggerWorkspaceView, WorkspaceBreakpoint,
};

/// Browser accelerators can consume F12 before the DOM receives a key event.
pub(super) fn install_shortcut(window: &tauri::WebviewWindow) {
    let app = window.app_handle().clone();
    let label = window.label().to_owned();
    let _ = window.with_webview(move |native| {
        let events = app.clone();
        let target = label.clone();
        if let Err(error) = transmog_webview_preview_windows::reserve_debugger_toggle(
            &native.controller(),
            move || {
                let _ = events.emit_to(&target, "debugger-toggle", ());
            },
        ) {
            let _ = app.emit_to(&label, "debugger-access-error", error.to_string());
        }
    });
}

#[derive(Default)]
pub(super) struct DebuggerWindows(Mutex<HashMap<String, Entry>>, AtomicBool);
struct Entry {
    paused: std::collections::BTreeSet<String>,
    application: Application,
    surface: String,
    placement: String,
    pending: Option<(String, String, u64)>,
    generation: u64,
    view_state: serde_json::Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadyView {
    #[serde(flatten)]
    workspace: DebuggerWorkspaceView,
    view_state: serde_json::Value,
}
impl DebuggerWindows {
    pub fn contains(&self, label: &str) -> bool {
        label.starts_with("debugger-") && self.application(label).is_some()
    }
    pub fn application(&self, label: &str) -> Option<Application> {
        self.0
            .lock()
            .ok()?
            .values()
            .find(|entry| {
                entry.surface == label || entry.pending.as_ref().is_some_and(|p| p.0 == label)
            })
            .map(|entry| entry.application.clone())
    }
    pub fn release(&self, label: &str, app: &tauri::AppHandle) {
        let removed: Vec<_> = {
            let mut entries = self.0.lock().unwrap();
            let owners: Vec<_> = entries
                .iter()
                .filter(|(owner, entry)| owner.as_str() == label || entry.surface == label)
                .map(|(owner, _)| owner.clone())
                .collect();
            let removed: Vec<_> = owners
                .into_iter()
                .filter_map(|owner| entries.remove(&owner).map(|entry| (owner, entry)))
                .collect();
            for (owner, entry) in &removed {
                let _ = entry.application.close_debugger_workspace();
                let _ = app.emit_to(owner.as_str(), "debugger-placement", "closed");
            }
            removed
        };
        for (_, entry) in removed {
            for surface in
                std::iter::once(entry.surface).chain(entry.pending.map(|pending| pending.0))
            {
                if surface.starts_with("debugger-")
                    && surface != label
                    && let Some(child) = app.get_webview_window(&surface)
                {
                    let _ = child.destroy();
                }
            }
        }
    }
    fn retire(
        &self,
        owner: &str,
        app: &tauri::AppHandle,
        application: &Application,
    ) -> Result<Option<Entry>, String> {
        // Closing ownership and its UI notification precede every subsequent Open.
        // Destroy native windows afterward, outside this lock, since destruction
        // itself calls release through the native window event handler.
        let mut entries = self.0.lock().unwrap();
        let previous = entries.remove(owner);
        application
            .close_debugger_workspace()
            .map_err(|error| error.message)?;
        app.emit_to(owner, "debugger-placement", "closed")
            .map_err(|error| error.to_string())?;
        Ok(previous)
    }
}
pub(super) fn command_allowed(command: &str) -> bool {
    matches!(
        command,
        "place_debugger"
            | "open_debugger"
            | "toggle_debugger"
            | "debugger_seen"
            | "acknowledge_debugger"
            | "debugger_workspace"
            | "debugger_file"
            | "debugger_breakpoint_positions"
            | "workspace_breakpoints"
            | "workspace_debug_session"
            | "workspace_debug_command"
            | "workspace_restart"
    )
}
/// Root entry points await the surface handshake before starting user code.
pub(super) async fn ensure_ready(window: &tauri::WebviewWindow) -> Result<Application, String> {
    let app = window.app_handle().clone();
    let owner = {
        let windows = app.state::<DebuggerWindows>();
        let entries = windows.0.lock().unwrap();
        entries
            .iter()
            .find(|(owner, entry)| {
                owner.as_str() == window.label() || entry.surface == window.label()
            })
            .map_or_else(|| window.label().to_owned(), |(owner, _)| owner.clone())
    };
    let exists = app
        .state::<DebuggerWindows>()
        .0
        .lock()
        .unwrap()
        .contains_key(&owner);
    if !exists {
        place_debugger(
            "window".into(),
            None,
            window.clone(),
            app.clone(),
            window.state::<DesktopState>(),
        )
        .await?;
    }
    let started = std::time::Instant::now();
    loop {
        {
            let windows = app.state::<DebuggerWindows>();
            let entries = windows.0.lock().unwrap();
            let entry = entries.get(&owner).ok_or("Debugger opening was canceled")?;
            if entry.pending.is_none() && entry.placement != "closed" {
                if let Some(surface) = app.get_webview_window(&entry.surface) {
                    let _ = surface.unminimize();
                    let _ = surface.show();
                    let _ = surface.set_focus();
                }
                let _ = app.emit_to(&entry.surface, "debugger-focus", ());
                return Ok(entry.application.clone());
            }
        }
        if started.elapsed() >= Duration::from_secs(30) {
            return Err("Debugger did not become ready".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
#[tauri::command]
pub(super) async fn open_debugger(window: tauri::WebviewWindow) -> Result<(), String> {
    ensure_ready(&window).await.map(|_| ())
}
/// Session visibility follows successful opens, independently of surface lifetime.
#[tauri::command]
pub(super) fn debugger_seen(window: tauri::WebviewWindow) -> bool {
    window.state::<DebuggerWindows>().1.load(Ordering::Acquire)
}
#[tauri::command]
pub(super) async fn toggle_debugger(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let open = app
        .state::<DebuggerWindows>()
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|(owner, entry)| owner.as_str() == window.label() || entry.surface == window.label());
    place_debugger(
        if open { "closed" } else { "window" }.into(),
        None,
        window,
        app,
        state,
    )
    .await
}
pub(super) fn opening_error(message: String) -> transmog_app::AppError {
    transmog_app::AppError {
        category: transmog_app::ErrorCategory::Conflict,
        message: message.chars().take(512).collect(),
        retryable: false,
        exception: None,
        logs: Vec::new(),
    }
}
fn controlled(window: &tauri::WebviewWindow, pending: bool) -> Result<Application, String> {
    window
        .state::<DebuggerWindows>()
        .0
        .lock()
        .unwrap()
        .values()
        .find(|entry| {
            entry.surface == window.label()
                || pending
                    && entry
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.0 == window.label())
        })
        .map(|entry| entry.application.clone())
        .ok_or_else(|| "Debugger is controlled by another surface".into())
}
#[tauri::command]
pub(super) async fn place_debugger(
    placement: String,
    view_state: Option<serde_json::Value>,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    if !["window", "bottom", "right", "closed"].contains(&placement.as_str()) {
        return Err("Unknown debugger placement".into());
    }
    let view_state = view_state.unwrap_or(serde_json::Value::Null);
    if serde_json::to_vec(&view_state)
        .map_err(|e| e.to_string())?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("Debugger view state is too large".into());
    }
    let windows = app.state::<DebuggerWindows>();
    let owner = {
        let entries = windows.0.lock().unwrap();
        entries
            .iter()
            .find(|(owner, entry)| {
                owner.as_str() == window.label() || entry.surface == window.label()
            })
            .map_or_else(|| window.label().into(), |(owner, _)| owner.clone())
    };
    let application = if window.label().starts_with("debugger-") {
        controlled(&window, false)?
    } else {
        state.window_application(&window).map_err(|e| e.message)?
    };
    let child = windows
        .0
        .lock()
        .unwrap()
        .get(&owner)
        .filter(|entry| entry.placement == "window")
        .map_or_else(
            || {
                format!(
                    "debugger-{owner}-{}",
                    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
                )
            },
            |entry| entry.surface.clone(),
        );
    if placement == "closed" {
        let previous = windows.retire(&owner, &app, &application)?;
        return destroy_surfaces(&app, previous);
    }
    let destination = if placement == "window" {
        child.clone()
    } else {
        owner.clone()
    };
    let generation = {
        let mut entries = windows.0.lock().unwrap();
        let entry = entries.entry(owner.clone()).or_insert_with(|| Entry {
            paused: std::collections::BTreeSet::new(),
            application,
            surface: owner.clone(),
            placement: "closed".into(),
            pending: None,
            generation: 0,
            view_state: serde_json::Value::Null,
        });
        if entry.surface == destination && entry.placement != "closed" {
            entry.placement.clone_from(&placement);
            drop(entries);
            if destination == owner {
                app.emit_to(&owner, "debugger-placement", &placement)
                    .map_err(|e| e.to_string())?;
            } else if let Some(surface) = app.get_webview_window(&destination) {
                surface.unminimize().map_err(|e| e.to_string())?;
                surface.show().map_err(|e| e.to_string())?;
                surface.set_focus().map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        if entry.pending.is_some() {
            return Err("Debugger is moving. Wait for it to finish.".into());
        }
        entry.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        entry.view_state = view_state;
        entry.pending = Some((destination.clone(), placement.clone(), entry.generation));
        entry.generation
    };
    let result = if placement == "window" {
        create_window(app.clone(), child.clone()).await
    } else {
        app.emit_to(&owner, "debugger-placement", &placement)
            .map_err(|e| e.to_string())
    };
    if result.is_err() {
        rollback(&app, &owner, generation);
        return result;
    }
    validate_destination(&windows, &app, &owner, &destination, generation, &placement)?;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        rollback(&app, &owner, generation);
    });
    Ok(())
}
fn validate_destination(
    windows: &DebuggerWindows,
    app: &tauri::AppHandle,
    owner: &str,
    destination: &str,
    generation: u64,
    placement: &str,
) -> Result<(), String> {
    let current = windows.0.lock().unwrap().get(owner).is_some_and(|entry| {
        entry.generation == generation
            && (entry
                .pending
                .as_ref()
                .is_some_and(|pending| pending.2 == generation)
                || entry.surface == destination)
    });
    if !current {
        if placement == "window"
            && let Some(child) = app.get_webview_window(destination)
        {
            let _ = child.destroy();
        }
        return Err("Debugger opening was canceled".into());
    }
    Ok(())
}
fn rollback(app: &tauri::AppHandle, owner: &str, generation: u64) {
    let windows = app.state::<DebuggerWindows>();
    let recovery = {
        let mut entries = windows.0.lock().unwrap();
        let recovery = entries.get_mut(owner).and_then(|entry| {
            if entry.pending.as_ref().is_some_and(|p| p.2 == generation) {
                let pending = entry.pending.take().unwrap();
                Some((
                    pending.0,
                    entry.placement.clone(),
                    entry.application.clone(),
                ))
            } else {
                None
            }
        });
        if recovery
            .as_ref()
            .is_some_and(|(_, placement, _)| placement == "closed")
        {
            entries.remove(owner);
            if let Some((_, _, application)) = &recovery {
                let _ = application.close_debugger_workspace();
            }
        }
        if let Some((_, placement, _)) = &recovery {
            let _ = app.emit_to(owner, "debugger-placement", placement);
        }
        recovery
    };
    if let Some((pending, _, _)) = recovery
        && pending.starts_with("debugger-")
        && let Some(child) = app.get_webview_window(&pending)
    {
        let _ = child.destroy();
    }
}
#[tauri::command]
pub(super) async fn acknowledge_debugger(
    window: tauri::WebviewWindow,
) -> Result<ReadyView, String> {
    let (owner, application, placement, previous, generation, view_state) = {
        let windows = window.state::<DebuggerWindows>();
        let entries = windows.0.lock().unwrap();
        let (owner, entry) = entries
            .iter()
            .find(|(_, entry)| {
                entry
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.0 == window.label())
                    || entry.surface == window.label()
            })
            .ok_or("Debugger handoff expired")?;
        let placement = entry
            .pending
            .as_ref()
            .map_or_else(|| entry.placement.clone(), |p| p.1.clone());
        (
            owner.clone(),
            entry.application.clone(),
            placement,
            entry.surface.clone(),
            entry.generation,
            entry.view_state.clone(),
        )
    };
    let task_app = application.clone();
    let workspace = tauri::async_runtime::spawn_blocking(move || {
        task_app.open_debugger_workspace().map_err(|e| e.message)
    })
    .await
    .map_err(|e| e.to_string())??;
    let app = window.app_handle();
    {
        let windows = window.state::<DebuggerWindows>();
        let mut entries = windows.0.lock().unwrap();
        let Some(entry) = entries.get_mut(&owner) else {
            // Keep reopen serialized until the orphan's attachment is retired.
            // Otherwise this close could race a fresh surface's successful Open.
            let _ = application.close_debugger_workspace();
            return Err("Debugger handoff expired".into());
        };
        if entry.generation != generation
            || entry
                .pending
                .as_ref()
                .is_some_and(|p| p.0 != window.label())
        {
            return Err("Debugger handoff superseded".into());
        }
        if placement == "window" {
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
        }
        entry.surface = window.label().into();
        entry.pending = None;
        entry.placement.clone_from(&placement);
    }
    window
        .state::<DebuggerWindows>()
        .1
        .store(true, Ordering::Release);
    app.emit("debugger-opened", true)
        .map_err(|error| error.to_string())?;
    if placement == "window" {
        app.emit_to(&owner, "debugger-placement", "window")
            .map_err(|e| e.to_string())?;
    }
    if previous != window.label()
        && previous.starts_with("debugger-")
        && let Some(old) = app.get_webview_window(&previous)
    {
        let _ = old.destroy();
    }
    Ok(ReadyView {
        workspace,
        view_state,
    })
}
#[tauri::command]
pub(super) fn debugger_workspace(
    window: tauri::WebviewWindow,
) -> Result<DebuggerWorkspaceView, String> {
    controlled(&window, true)?
        .debugger_workspace()
        .map_err(|e| e.message)
}
#[tauri::command]
pub(super) fn debugger_file(
    id: String,
    artifact: String,
    window: tauri::WebviewWindow,
) -> Result<String, String> {
    controlled(&window, true)?
        .debugger_file(&id, &artifact)
        .map_err(|e| e.message)
}
#[tauri::command]
pub(super) fn workspace_breakpoints(
    points: Vec<WorkspaceBreakpoint>,
    removed: Vec<WorkspaceBreakpoint>,
    window: tauri::WebviewWindow,
) -> Result<Vec<WorkspaceBreakpoint>, String> {
    controlled(&window, false)?
        .edit_workspace_breakpoints(points, &removed)
        .map_err(|e| e.message)
}
#[tauri::command]
pub(super) fn workspace_debug_session(
    session: String,
    after: u64,
    window: tauri::WebviewWindow,
) -> Result<DebugSessionView, String> {
    let view = controlled(&window, false)?
        .workspace_debug_session(&session, after)
        .map_err(|error| error.message)?;
    let windows = window.state::<DebuggerWindows>();
    let mut entries = windows.0.lock().unwrap();
    if let Some((owner, entry)) = entries
        .iter_mut()
        .find(|(_, entry)| entry.surface == window.label())
    {
        let changed = if view.stream.paused.is_some() && !view.stream.closed {
            entry.paused.insert(session)
        } else {
            entry.paused.remove(&session)
        };
        if changed {
            let _ =
                window
                    .app_handle()
                    .emit_to(owner.as_str(), "debugger-paused", entry.paused.len());
        }
    }
    Ok(view)
}
#[tauri::command]
pub(super) async fn workspace_restart(
    session: String,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    let application = controlled(&window, false)?;
    let view = application
        .workspace_debug_session(&session, 0)
        .map_err(|error| error.message)?;
    if view.info.mode == "Workspace script" {
        return tauri::async_runtime::spawn_blocking(move || {
            application
                .restart_workspace_script(&session)
                .map_err(|error| error.message)
        })
        .await
        .map_err(|error| error.to_string())?;
    }
    let instance = application.workspace_visualizer_instance(&session);
    let binding = application.workspace_visualizer_binding(&session);
    if !view.info.mode.starts_with("Sandbox")
        && binding.is_none()
        && view.info.mode != "Visualizer fixture"
    {
        return Err(
            "This visualizer is no longer active. Select it in Traffic to start a fresh instance."
                .into(),
        );
    }
    application
        .workspace_debug_command(&session, DebugAction::Stop)
        .map_err(|error| error.message)?;
    let windows = window.state::<DebuggerWindows>();
    let entries = windows.0.lock().unwrap();
    let (owner, _) = entries
        .iter()
        .find(|(_, entry)| entry.surface == window.label())
        .ok_or("Debugger surface is unavailable")?;
    window.app_handle().emit_to(owner.as_str(),"debugger-restart",serde_json::json!({"session":session,"instance":instance,"binding":binding,"exceptions":view.stream.exceptions})).map_err(|error|error.to_string())
}
#[tauri::command]
pub(super) fn workspace_debug_command(
    session: String,
    action: DebugAction,
    window: tauri::WebviewWindow,
) -> Result<u64, String> {
    let application = controlled(&window, false)?;
    let instance = if matches!(action, DebugAction::Stop) {
        application.workspace_visualizer_instance(&session)
    } else {
        None
    };
    let result = application
        .workspace_debug_command(&session, action)
        .map_err(|error| error.message)?;
    if let Some(instance) = instance {
        let windows = window.state::<DebuggerWindows>();
        let entries = windows.0.lock().unwrap();
        if let Some((owner, _)) = entries
            .iter()
            .find(|(_, entry)| entry.surface == window.label())
        {
            let _ = window.app_handle().emit_to(
                owner.as_str(),
                "debugger-stopped",
                serde_json::json!({"session":session,"instance":instance}),
            );
        }
    }
    Ok(result)
}

#[tauri::command]
pub(super) fn debugger_breakpoint_positions(
    id: String,
    artifact: String,
    window: tauri::WebviewWindow,
) -> Result<Vec<(u32, transmog_app::SourceLocation)>, String> {
    controlled(&window, true)?
        .debugger_breakpoint_positions(&id, &artifact)
        .map_err(|error| error.message)
}
fn destroy_surfaces(app: &tauri::AppHandle, previous: Option<Entry>) -> Result<(), String> {
    if let Some(previous) = previous {
        for label in
            std::iter::once(previous.surface).chain(previous.pending.map(|pending| pending.0))
        {
            if label.starts_with("debugger-")
                && let Some(child) = app.get_webview_window(&label)
            {
                child.destroy().map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
async fn create_window(app: tauri::AppHandle, label: String) -> Result<(), String> {
    let builder_app = app;
    tauri::async_runtime::spawn_blocking(move || {
        WebviewWindowBuilder::new(
            &builder_app,
            label,
            WebviewUrl::CustomProtocol(
                "transmog-ui://localhost/"
                    .parse()
                    .expect("constant UI origin"),
            ),
        )
        .title("Debugger — Transmog")
        .inner_size(1200.0, 800.0)
        .min_inner_size(640.0, 480.0)
        .visible(false)
        .data_directory(
            builder_app
                .state::<DesktopState>()
                .webview_data_path
                .clone(),
        )
        .on_navigation(super::is_allowed_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()
        .map(|window| install_shortcut(&window))
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(std::convert::identity)
}
