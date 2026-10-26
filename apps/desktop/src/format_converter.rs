use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, State, WebviewWindowBuilder, utils::config::WebviewUrl};
use transmog_app::format_converter::{ConversionInput, ConversionOutput, MAX_INPUT, convert};

pub(super) struct ConverterWindows {
    windows: Mutex<HashMap<String, Handoff>>,
    jobs: Mutex<HashMap<String, (u64, Arc<AtomicBool>)>>,
    workers: Arc<tokio::sync::Semaphore>,
}

struct Handoff {
    draft: Option<serde_json::Value>,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ConverterWindows {
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            workers: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
    pub fn contains(&self, label: &str) -> bool {
        self.windows.lock().unwrap().contains_key(label)
    }
    pub fn release(&self, label: &str) {
        self.windows.lock().unwrap().remove(label);
        if let Some((_, cancelled)) = self.jobs.lock().unwrap().remove(label) {
            cancelled.store(true, Ordering::Relaxed);
        }
    }
    fn start(&self, label: &str, revision: u64) -> Result<Arc<AtomicBool>, String> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some((previous, cancelled)) = jobs.get(label) {
            if revision <= *previous {
                return Err("Conversion superseded.".into());
            }
            cancelled.store(true, Ordering::Relaxed);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        jobs.insert(label.into(), (revision, Arc::clone(&cancelled)));
        Ok(cancelled)
    }
}

pub(super) fn command_allowed(command: &str) -> bool {
    matches!(
        command,
        "convert_format"
            | "auth_conversion_catalog"
            | "cancel_format_conversion"
            | "open_converter_window"
            | "take_converter_draft"
            | "acknowledge_converter_window"
            | "save_converter_output"
            | "close_converter_window"
    )
}

#[tauri::command]
pub(super) fn auth_conversion_catalog() -> &'static [transmog_app::format_converter::AuthConversion]
{
    transmog_app::format_converter::AUTH_CONVERSIONS
}

#[tauri::command]
pub(super) async fn convert_format(
    input: ConversionInput,
    revision: u64,
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) -> Result<ConversionOutput, String> {
    let cancelled = converters.start(window.label(), revision)?;
    let permit = Arc::clone(&converters.workers)
        .try_acquire_owned()
        .map_err(|_| "Two conversions are running. Try Convert again shortly.")?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        convert(&input, &cancelled)
    })
    .await
    .map_err(|_| "Conversion worker failed. Try Convert again.")?
}

#[tauri::command]
pub(super) fn cancel_format_conversion(
    revision: u64,
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) {
    let mut jobs = converters.jobs.lock().unwrap();
    if let Some((previous, cancelled)) = jobs.get(window.label()) {
        if revision < *previous {
            return;
        }
        cancelled.store(true, Ordering::Relaxed);
    }
    jobs.insert(
        window.label().into(),
        (revision, Arc::new(AtomicBool::new(true))),
    );
}

#[tauri::command]
pub(super) async fn open_converter_window(
    draft: serde_json::Value,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if serde_json::to_vec(&draft)
        .map_err(|_| "Converter draft could not be transferred.")?
        .len()
        > MAX_INPUT * 4
    {
        return Err(
            "Converter draft is too large to transfer. Reduce input or undo history.".into(),
        );
    }
    let (ready, received) = tokio::sync::oneshot::channel();
    let sequence = super::ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let label = format!("converter-{sequence}");
    let converters = app.state::<ConverterWindows>();
    converters.windows.lock().unwrap().insert(
        label.clone(),
        Handoff {
            draft: Some(draft),
            ready: Some(ready),
        },
    );
    let create_app = app.clone();
    let create_label = label.clone();
    let created = tauri::async_runtime::spawn_blocking(move || {
        let state = create_app.state::<super::DesktopState>();
        if state.closing.load(Ordering::Acquire) {
            return Err("Transmog is closing. Try again after the update.".to_owned());
        }
        WebviewWindowBuilder::new(
            &create_app,
            &create_label,
            WebviewUrl::CustomProtocol(
                tauri::Url::parse("transmog-ui://localhost/").expect("fixed URL"),
            ),
        )
        .title(format!("Format converter {} — Transmog", sequence + 1))
        .inner_size(680.0, 720.0)
        .min_inner_size(400.0, 380.0)
        .visible(false)
        .data_directory(state.webview_data_path.clone())
        .on_navigation(super::is_allowed_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()
        .map(|_| ())
        .map_err(|_| "Converter window could not be opened.".to_owned())
    })
    .await
    .map_err(|_| "Converter window worker failed.".to_owned())
    .and_then(|result| result);
    let result = match created {
        Ok(()) => tokio::time::timeout(std::time::Duration::from_secs(20), received)
            .await
            .map_err(|_| {
                "Converter window did not become ready. Your draft is preserved.".to_owned()
            })
            .and_then(|result| {
                result.map_err(|_| "Converter window closed before receiving the draft.".to_owned())
            }),
        Err(error) => Err(error),
    };
    if result.is_err() {
        converters.release(&label);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    }
    result
}

#[tauri::command]
pub(super) fn take_converter_draft(
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) -> Result<serde_json::Value, String> {
    converters
        .windows
        .lock()
        .unwrap()
        .get(window.label())
        .and_then(|handoff| handoff.draft.clone())
        .ok_or_else(|| "Converter draft is no longer available.".into())
}

#[tauri::command]
pub(super) fn acknowledge_converter_window(
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) -> Result<(), String> {
    let mut windows = converters.windows.lock().unwrap();
    let handoff = windows
        .get_mut(window.label())
        .ok_or("Converter window is no longer available.")?;
    if handoff.ready.is_none() {
        return Err("Converter draft was already received.".into());
    }
    window
        .show()
        .and_then(|()| window.set_focus())
        .map_err(|_| "Converter window could not be shown.")?;
    handoff
        .ready
        .take()
        .ok_or("Converter draft was already received.")?
        .send(())
        .map_err(|()| "Converter source closed before handoff completed.")?;
    handoff.draft = None;
    Ok(())
}

#[tauri::command]
pub(super) fn close_converter_window(
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) -> Result<(), String> {
    if !converters.contains(window.label()) {
        return Err("This action requires a converter window.".into());
    }
    window
        .close()
        .map_err(|_| "Converter window could not be closed.".into())
}

#[tauri::command]
pub(super) async fn save_converter_output(
    input: ConversionInput,
    window: tauri::WebviewWindow,
    converters: State<'_, ConverterWindows>,
) -> Result<bool, String> {
    let permit = Arc::clone(&converters.workers)
        .try_acquire_owned()
        .map_err(|_| "Two conversions are running. Try saving again shortly.")?;
    let output = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        convert(&input, &AtomicBool::new(false))
    })
    .await
    .map_err(|_| "Output preparation failed.")??;
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Save converter output")
        .set_file_name("converted.bin")
        .save_file()
        .await
    else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        let destination = file.path();
        let mut staged =
            tempfile::NamedTempFile::new_in(destination.parent().ok_or(())?).map_err(|_| ())?;
        staged.write_all(&output.bytes).map_err(|_| ())?;
        staged.as_file().sync_all().map_err(|_| ())?;
        staged.persist(destination).map_err(|_| ())?;
        Ok::<(), ()>(())
    })
    .await
    .map_err(|_| "Output save worker failed.")?
    .map_err(|()| "Output could not be saved. Check the destination and try again.")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converter_role_is_narrow_and_jobs_reject_late_work() {
        for command in [
            "start_proxy",
            "query_sessions",
            "record_frontend_diagnostic",
            "open_trace_viewer",
            "change_response_transform",
            "auth_converter_input",
            "converter_header_input",
        ] {
            assert!(!command_allowed(command));
        }
        assert!(command_allowed("convert_format"));
        let converters = ConverterWindows::new();
        let first = converters.start("main", 1).unwrap();
        let other = converters.start("converter-1", 1).unwrap();
        let second = converters.start("main", 2).unwrap();
        assert!(first.load(Ordering::Relaxed));
        assert!(!other.load(Ordering::Relaxed));
        assert!(converters.start("main", 1).is_err());
        converters.release("main");
        assert!(second.load(Ordering::Relaxed));
    }
}
