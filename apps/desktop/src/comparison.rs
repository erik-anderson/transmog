//! Window-owned read-only comparison commands.
use super::DesktopState;
use tauri::{State, WebviewWindow};
use transmog_app::{
    AppError, ErrorCategory,
    comparison::{
        CaptureComparison, CaptureEntryChoice, ComparisonBytes, ComparisonPage, ComparisonRequest,
        ComparisonResult, ComparisonScope, ComparisonSubject, PairingOptions, StructuredComparison,
    },
};

#[tauri::command]
pub(super) async fn compare_evidence(
    input: ComparisonRequest,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ComparisonResult, AppError> {
    let app = state.window_application(&window)?;
    if window.label() != "main"
        && [&input.left, &input.right].iter().any(|s| {
            matches!(
                s,
                ComparisonSubject::Asset { .. } | ComparisonSubject::Draft { .. }
            )
        })
    {
        return Err(error("Response authoring belongs to the main window"));
    }
    let a = super::captured_preview::comparison_evidence(window.label(), &input.left)?;
    let b = super::captured_preview::comparison_evidence(window.label(), &input.right)?;
    // The scene owners outlive the worker's file acquisition.
    let result = app.compare_evidence(input, [a.1, b.1]).await;
    drop((a.0, b.0));
    result
}
#[tauri::command]
pub(super) async fn comparison_fields(
    id: String,
    section: String,
    format: String,
    operation_id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<StructuredComparison, AppError> {
    state
        .window_application(&window)?
        .comparison_fields(&id, &section, &format, &operation_id)
        .await
}
#[tauri::command]
pub(super) fn close_comparison(
    id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.window_application(&window)?.close_comparison(&id);
    Ok(())
}
#[tauri::command]
pub(super) fn cancel_comparison(
    operation_id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .cancel_comparison(&operation_id);
    Ok(())
}
#[tauri::command]
pub(super) fn comparison_page(
    id: String,
    offset: usize,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ComparisonPage, AppError> {
    state
        .window_application(&window)?
        .comparison_page(&id, offset)
}
#[tauri::command]
pub(super) fn comparison_stale(
    id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<bool, AppError> {
    state.window_application(&window)?.comparison_stale(&id)
}
#[tauri::command]
pub(super) async fn comparison_bytes(
    id: String,
    section: String,
    offset: u64,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ComparisonBytes, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || app.comparison_bytes(&id, &section, offset))
        .await
        .map_err(|_| error("Byte range read failed"))?
}
#[tauri::command]
pub(super) async fn compare_captures(
    operation_id: String,
    left: ComparisonScope,
    right: ComparisonScope,
    options: PairingOptions,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<CaptureComparison, AppError> {
    state
        .window_application(&window)?
        .compare_captures(operation_id, left, right, options)
        .await
}
#[tauri::command]
pub(super) async fn reopen_case_capture_comparison(
    id: String,
    operation_id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<CaptureComparison, AppError> {
    let app = state.window_application(&window)?;
    let input = super::portable_cases::resolve_case_comparison(id, window, state).await?;
    let transmog_app::comparison::CaseComparisonOpen::Captures {
        left,
        right,
        options,
        corrections,
    } = input
    else {
        return Err(error("Select a saved capture-scope comparison"));
    };
    app.reopen_case_capture_comparison(operation_id, left, right, options, corrections)
        .await
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(super) async fn capture_comparison_page(
    id: String,
    offset: usize,
    options: Option<PairingOptions>,
    left: Option<String>,
    right: Option<String>,
    action: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<CaptureComparison, AppError> {
    let app = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || {
        app.capture_comparison_page(&id, offset, options, left, right, &action)
    })
    .await
    .map_err(|_| error("Pairing worker failed"))?
}
#[tauri::command]
pub(super) fn capture_comparison_entries(
    id: String,
    side: String,
    search: String,
    offset: usize,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<CaptureEntryChoice>, AppError> {
    state
        .window_application(&window)?
        .capture_comparison_entries(&id, &side, &search, offset)
}
#[tauri::command]
pub(super) fn close_capture_comparison(
    id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state
        .window_application(&window)?
        .close_capture_comparison(&id);
    Ok(())
}

fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: false,
    }
}
