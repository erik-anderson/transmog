//! Native path selection for capture tasks. Dialogs do not create or replace files.
use transmog_app::ExportFormat;

#[tauri::command]
pub(crate) async fn pick_capture_path(kind: String, format: Option<ExportFormat>) -> Result<Option<String>, String> {
    let dialog = rfd::AsyncFileDialog::new();
    let selected = match kind.as_str() {
        "source" => dialog.set_title("Choose a TMCap capture").add_filter("Transmog capture", &["tmcap"]).pick_file().await,
        "record" => dialog.set_title("Choose a new capture file").add_filter("Transmog capture", &["tmcap"]).set_file_name("session.tmcap").save_file().await,
        "export" => {
            let (label, extension) = match format.unwrap_or(ExportFormat::Native) {
                ExportFormat::Native => ("Transmog capture", "tmcap"),
                ExportFormat::JsonLines => ("JSON lines", "jsonl"),
                ExportFormat::SazStrict | ExportFormat::SazExtended => ("SAZ archive", "saz"),
            };
            dialog.set_title("Choose a new export file").add_filter(label, &[extension]).set_file_name(format!("session-copy.{extension}")).save_file().await
        }
        _ => return Err("Unknown capture file task".to_owned()),
    };
    Ok(selected.map(|file| file.path().to_string_lossy().into_owned()))
}
