//! Thin Tauri host for the Transmog application and `WebUI` renderer.

#[cfg(windows)]
mod file_dialogs;

#[cfg(windows)]
mod windows;

#[cfg(any(windows, test))]
mod update_policy;

#[cfg(windows)]
mod updates;

/// Starts the Windows desktop shell.
#[cfg(windows)]
pub use windows::run;

/// Reports the intentionally unsupported platform when built elsewhere.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("Transmog Desktop currently supports Windows with WebView2 only");
}
