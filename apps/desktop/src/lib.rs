//! Thin Tauri host for the Transmog application and `WebUI` renderer.

#[cfg(windows)]
mod file_dialogs;

#[cfg(all(windows, feature = "desktop-test-fixtures"))]
mod certificate_test_fixture;
#[cfg(windows)]
mod windows;

#[cfg(any(windows, test))]
mod body_page;

#[cfg(any(windows, test))]
mod bundled_extensions;

#[cfg(any(windows, test))]
mod window_geometry;

#[cfg(any(windows, test))]
mod update_policy;

#[cfg(windows)]
mod updates;

#[cfg(windows)]
mod loopback;

/// Starts the Windows desktop shell.
#[cfg(windows)]
pub use windows::run;

/// Reports the intentionally unsupported platform when built elsewhere.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("Transmog Desktop currently supports Windows with WebView2 only");
}
