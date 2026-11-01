//! Read-only visualizer commands scoped to the calling window's retained capture.
use super::{DesktopState, ScriptCommandError};
use tauri::{Emitter as _, Manager as _, State};
use transmog_app::{
    AppError, VisualizerActivation, VisualizerChoice, VisualizerHandle, VisualizerLiveStatus,
    VisualizerLiveUpdate, VisualizerReadRequest, VisualizerReload, VisualizerSelection,
    VisualizerSourcePage, VisualizerView,
};

#[tauri::command]
pub(super) async fn debug_visualizer(
    request: transmog_app::DebugVisualizerRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::DebugSessionInfo, AppError> {
    state
        .window_application(&window)?
        .debug_visualizer(window.label(), request)
}
#[tauri::command]
pub(super) async fn await_debug_visualizer(
    session: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerView, ScriptCommandError> {
    let view = state
        .window_application(&window)?
        .await_debug_visualizer(window.label(), &session)
        .await?;
    if view.metadata_changed {
        let _ = window.app_handle().emit("extensions-changed", ());
    }
    Ok(view)
}
#[tauri::command]
pub(super) async fn attach_visualizer_debugger(
    handle: VisualizerHandle,
    configuration: transmog_app::DebugConfiguration,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::DebugSessionInfo, AppError> {
    state
        .window_application(&window)?
        .attach_visualizer_debugger(window.label(), &handle, &configuration)
}

#[tauri::command]
pub(super) fn visualizer_page(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    Ok(state.window_application(&window)?.visualizer_page())
}
#[tauri::command]
pub(super) fn visualizer_choices(
    selection: VisualizerSelection,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<VisualizerChoice>, AppError> {
    state
        .window_application(&window)?
        .visualizer_choices(&selection)
}
#[tauri::command]
pub(super) async fn activate_visualizer(
    request: VisualizerActivation,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerView, ScriptCommandError> {
    let view = state
        .window_application(&window)?
        .activate_visualizer(request)
        .await?;
    if view.metadata_changed {
        let _ = window.app_handle().emit("extensions-changed", ());
    }
    Ok(view)
}
#[tauri::command]
pub(super) async fn visualizer_reload_status(
    handle: VisualizerHandle,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::extensions::ReloadStatus, AppError> {
    state
        .window_application(&window)?
        .visualizer_reload_status(&handle)
        .await
}
#[tauri::command]
pub(super) async fn reload_visualizer(
    request: VisualizerReload,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerView, ScriptCommandError> {
    let view = state
        .window_application(&window)?
        .reload_visualizer(request)
        .await?;
    if view.metadata_changed {
        let _ = window.app_handle().emit("extensions-changed", ());
    }
    Ok(view)
}
#[tauri::command]
pub(super) async fn expand_visualizer(
    handle: VisualizerHandle,
    token: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerView, ScriptCommandError> {
    state
        .window_application(&window)?
        .expand_visualizer(&handle, token)
        .await
        .map_err(ScriptCommandError::from)
}
#[tauri::command]
pub(super) async fn poll_visualizer_live(
    handle: VisualizerHandle,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerLiveUpdate, ScriptCommandError> {
    state
        .window_application(&window)?
        .poll_visualizer_live(&handle)
        .await
        .map_err(ScriptCommandError::from)
}
#[tauri::command]
pub(super) fn stop_visualizer_live(
    handle: VisualizerHandle,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerLiveStatus, AppError> {
    state
        .window_application(&window)?
        .stop_visualizer_live(&handle)
}
#[tauri::command]
pub(super) async fn visualizer_source(
    handle: VisualizerHandle,
    request: VisualizerReadRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerSourcePage, AppError> {
    state
        .window_application(&window)?
        .visualizer_source(&handle, request)
        .await
}
#[tauri::command]
pub(super) fn visualizer_active(
    handle: VisualizerHandle,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<bool, AppError> {
    Ok(state
        .window_application(&window)?
        .visualizer_active(&handle))
}
#[tauri::command]
pub(super) fn deactivate_visualizer(
    slot: String,
    generation: String,
    page_id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .deactivate_visualizer(&slot, &generation, &page_id);
    Ok(())
}
