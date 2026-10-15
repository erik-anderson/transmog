//! Window-owned incident intake, export review and active file-recording marks.
use super::DesktopState;
use tauri::{State, WebviewWindow};
use transmog_app::{
    AppError, ErrorCategory, comparison::ComparisonScope, export_review::ExportInventory,
    incidents::IncidentIntake,
};
#[tauri::command]
pub(super) async fn incident_intake(
    trace_id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<IncidentIntake, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.incident_intake(&trace_id))
        .await
        .map_err(|_| error("Incident intake failed"))?
}
#[tauri::command]
pub(super) async fn review_traffic_export(
    selection: Option<ComparisonScope>,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ExportInventory, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.review_traffic_export(selection))
        .await
        .map_err(|_| error("Export review failed"))?
}
#[tauri::command]
pub(super) async fn mark_incident(
    note: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::incidents::IncidentMarker, AppError> {
    if window.label() != "main" {
        return Err(error("Incident recording belongs to the main window"));
    }
    state.window_application(&window)?.mark_incident(note).await
}
fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: false,
    }
}
