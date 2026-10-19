//! Read-only visualizer commands scoped to the calling window's retained capture.
use super::DesktopState;
use tauri::State;
use transmog_app::{
    AppError, VisualizerActivation, VisualizerChoice, VisualizerHandle, VisualizerReadRequest,
    VisualizerSelection, VisualizerSourcePage, VisualizerView,
};

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
) -> Result<VisualizerView, AppError> {
    state
        .window_application(&window)?
        .activate_visualizer(request)
        .await
}
#[tauri::command]
pub(super) async fn expand_visualizer(
    handle: VisualizerHandle,
    token: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VisualizerView, AppError> {
    state
        .window_application(&window)?
        .expand_visualizer(&handle, token)
        .await
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
