//! Thin Tauri host for the Transmog application and `WebUI` renderer.

#[cfg(windows)]
mod windows;

/// Starts the Windows desktop shell.
#[cfg(windows)]
pub use windows::run;

/// Reports the intentionally unsupported platform when built elsewhere.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("Transmog Desktop currently supports Windows with WebView2 only");
}
