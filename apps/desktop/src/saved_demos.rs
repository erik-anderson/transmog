//! Main-window commands for reviewed durable demo snapshots.
use super::DesktopState;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::Manager;
use transmog_app::{
    AppError, ErrorCategory, SaveDemoInput, SavedDemo, SavedDemoPreparation, SavedDemoRevision,
};

static OPERATIONS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
fn operations() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    OPERATIONS.get_or_init(Mutex::default)
}
struct Operation(String);
impl Drop for Operation {
    fn drop(&mut self) {
        operations()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.0);
    }
}
fn register(
    window: &tauri::WebviewWindow,
    operation_id: &str,
) -> Result<(Operation, Arc<AtomicBool>), AppError> {
    if window.label() != "main" || operation_id.is_empty() || operation_id.len() > 128 {
        return Err(error("Invalid saved demo operation"));
    }
    let key = format!("{}:{operation_id}", window.label());
    let canceled = Arc::new(AtomicBool::new(false));
    let mut pending = operations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending.len() >= 4 || pending.contains_key(&key) {
        return Err(error("Another saved demo operation is still running"));
    }
    pending.insert(key.clone(), canceled.clone());
    let weak = Arc::downgrade(&canceled);
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed)
            && let Some(canceled) = weak.upgrade()
        {
            canceled.store(true, Ordering::Release);
        }
    });
    Ok((Operation(key), canceled))
}

#[tauri::command]
pub(super) async fn saved_demos(
    owner_id: String,
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<SavedDemo>, AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || application.saved_demos(&owner_id))
        .await
        .map_err(|_| error("Saved demos could not be read"))?
}

#[tauri::command]
pub(super) async fn prepare_demo_save(
    input: SaveDemoInput,
    operation_id: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<SavedDemoPreparation, AppError> {
    let (_operation, canceled) = register(&window, &operation_id)?;
    let prepared = state
        .application
        .prepare_demo_save(input, canceled.clone())
        .await?;
    if canceled.load(Ordering::Acquire)
        || window
            .app_handle()
            .get_webview_window(window.label())
            .is_none()
    {
        state.application.cancel_demo_save(&prepared.token);
        return Err(AppError {
            category: ErrorCategory::Canceled,
            message: "Saving the demo was canceled".into(),
            retryable: false,
        });
    }
    let application = state.application.clone();
    let token = prepared.token.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let application = application.clone();
            let token = token.clone();
            tauri::async_runtime::spawn_blocking(move || application.cancel_demo_save(&token));
        }
    });
    Ok(prepared)
}

#[tauri::command]
pub(super) async fn commit_demo_save(
    token: String,
    reviewed_missing: bool,
    operation_id: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<SavedDemo, AppError> {
    let (_operation, canceled) = register(&window, &operation_id)?;
    state
        .application
        .commit_demo_save(token, reviewed_missing, canceled)
        .await
}

#[tauri::command]
pub(super) async fn cancel_demo_save(
    token: String,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || application.cancel_demo_save(&token))
        .await
        .map_err(|_| error("The prepared demo could not be released"))
}

#[tauri::command]
pub(super) fn cancel_demo_operation(operation_id: String, window: tauri::WebviewWindow) {
    if let Some(canceled) = operations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&format!("{}:{operation_id}", window.label()))
    {
        canceled.store(true, Ordering::Release);
    }
}

#[tauri::command]
pub(super) async fn delete_saved_demo(
    owner_id: String,
    demo: SavedDemoRevision,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || application.delete_saved_demo(&owner_id, &demo))
        .await
        .map_err(|_| error("The saved demo could not be deleted"))?
}

fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: true,
    }
}
