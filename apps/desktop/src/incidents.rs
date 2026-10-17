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

#[tauri::command]
pub(super) fn trace_marker_reviews(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<transmog_app::incidents::MarkerReview>, AppError> {
    state.window_application(&window)?.trace_marker_reviews()
}
#[tauri::command]
pub(super) fn change_trace_marker(
    trace_id: String,
    revision: u64,
    change: transmog_app::incidents::MarkerChange,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::incidents::MarkerReview, AppError> {
    state
        .window_application(&window)?
        .change_trace_marker(&trace_id, revision, change)
}
#[tauri::command]
pub(super) fn discard_marker_changes(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.window_application(&window)?.discard_marker_changes()
}

#[tauri::command]
pub(super) fn set_marker_draft_dirty(
    dirty: bool,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .set_marker_draft_dirty(dirty);
    Ok(())
}

#[tauri::command]
pub(super) fn add_trace_marker(
    exchange_id: String,
    boundary: String,
    note: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::incidents::MarkerReview, AppError> {
    state
        .window_application(&window)?
        .add_trace_marker(&exchange_id, &boundary, note)
}

#[tauri::command]
pub(super) fn close_trace_window(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.window_application(&window)?;
    window
        .close()
        .map_err(|_| error("Window could not be closed"))
}
