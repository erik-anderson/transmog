//! Auth commands always resolve evidence through the calling window's application.
use super::DesktopState;
use tauri::State;
use transmog_app::{
    AppError,
    auth::{AuthHandle, AuthRequest, AuthSource, AuthView, RelatedAuth},
};
#[tauri::command]
pub(super) async fn auth_converter_input(
    handle: AuthHandle,
    finding: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::format_converter::ConverterText, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.auth_converter_input(&handle, &finding))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Auth converter input is unavailable".into(),
            retryable: false,
        })?
}
#[tauri::command]
pub(super) async fn inspect_auth(
    request: AuthRequest,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<AuthView, AppError> {
    state
        .window_application(&window)?
        .inspect_auth(request)
        .await
}
#[tauri::command]
pub(super) async fn auth_value(
    handle: AuthHandle,
    field: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.auth_value(&handle, &field))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Auth value worker unavailable".into(),
            retryable: false,
        })?
}
#[tauri::command]
pub(super) async fn auth_values(
    handle: AuthHandle,
    fields: Vec<String>,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.auth_values(&handle, &fields))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Auth copy worker unavailable".into(),
            retryable: false,
        })?
}
#[tauri::command]
pub(super) fn auth_source(
    handle: AuthHandle,
    finding: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<AuthSource, AppError> {
    state
        .window_application(&window)?
        .auth_source(&handle, &finding)
}
#[tauri::command]
pub(super) async fn related_auth(
    handle: AuthHandle,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<RelatedAuth, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.related_auth(&handle))
        .await
        .map_err(|_| AppError {
            category: transmog_app::ErrorCategory::Unavailable,
            message: "Related auth worker unavailable".into(),
            retryable: false,
        })?
}
#[tauri::command]
pub(super) fn cancel_auth(
    slot: String,
    generation: String,
    page_id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .cancel_auth(&slot, &generation, &page_id);
    Ok(())
}
