//! Native extension installation and shared metadata management. No install hooks execute.
use super::DesktopState;
use tauri::{Emitter as _, Manager as _, State};
use transmog_app::extensions::{ExtensionStatus, Registry};

fn registry(window: &tauri::WebviewWindow, state: &DesktopState) -> Result<Registry, String> {
    if window.label() != "main" {
        return Err("Manage extensions in the main Transmog window.".into());
    }
    Ok(state.application.extensions())
}
fn changed(window: &tauri::WebviewWindow, registry: &Registry) -> ExtensionStatus {
    let _ = window.app_handle().emit("extensions-changed", ());
    registry.status()
}
#[tauri::command]
pub(super) fn extension_status(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ExtensionStatus, String> {
    Ok(registry(&window, &state)?.status())
}
#[tauri::command]
pub(super) async fn install_extension(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<ExtensionStatus>, String> {
    let registry = registry(&window, &state)?;
    let selected = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Install a Transmog extension")
        .add_filter("Transmog extension", &["tmext"])
        .pick_file()
        .await;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let installer = registry.clone();
    let path = selected.path().to_path_buf();
    let result = tauri::async_runtime::spawn_blocking(move || installer.install(&path))
        .await
        .map_err(|_| "Extension installation could not finish.".to_owned())?;
    let status = changed(&window, &registry);
    result.map_err(|error| error.to_string())?;
    Ok(Some(status))
}
#[tauri::command]
pub(super) async fn load_unpacked_extension(
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<ExtensionStatus>, String> {
    let registry = registry(&window, &state)?;
    let selected = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Choose an unpacked extension folder")
        .pick_folder()
        .await;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let installer = registry.clone();
    let path = selected.path().to_path_buf();
    let result =
        tauri::async_runtime::spawn_blocking(move || installer.register_directory(&path, false))
            .await
            .map_err(|_| "The unpacked extension could not be registered.".to_owned())?;
    let status = changed(&window, &registry);
    result.map_err(|error| error.to_string())?;
    Ok(Some(status))
}
#[tauri::command]
pub(super) async fn enable_extension(
    id: String,
    enabled: bool,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ExtensionStatus, String> {
    let registry = registry(&window, &state)?;
    let manager = registry.clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_enabled(&id, enabled))
        .await
        .map_err(|_| "Extension settings could not be saved.".to_owned())?
        .map_err(|error| error.to_string())?;
    Ok(changed(&window, &registry))
}
#[tauri::command]
pub(super) async fn extension_hot_reload(
    id: String,
    enabled: bool,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ExtensionStatus, String> {
    let registry = registry(&window, &state)?;
    let manager = registry.clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_hot_reload(&id, enabled))
        .await
        .map_err(|_| "Extension development settings could not be saved.".to_owned())?
        .map_err(|error| error.to_string())?;
    Ok(changed(&window, &registry))
}
#[tauri::command]
pub(super) async fn uninstall_extension(
    id: String,
    window: tauri::WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ExtensionStatus, String> {
    let registry = registry(&window, &state)?;
    let manager = registry.clone();
    let result = tauri::async_runtime::spawn_blocking(move || manager.uninstall(&id))
        .await
        .map_err(|_| "The extension could not be removed.".to_owned())?;
    let status = changed(&window, &registry);
    result.map_err(|error| error.to_string())?;
    Ok(status)
}
