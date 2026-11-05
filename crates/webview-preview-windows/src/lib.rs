#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Native `WebView2` boundary for captured rendering and trusted-window shortcuts.
//! Captured bytes are served through COM streams; this crate has no proxy,
//! certificate, application IPC or network API.
#[cfg(windows)]
mod shortcuts;
#[cfg(windows)]
pub use shortcuts::reserve_debugger_toggle;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{CapturedResponse, Lookup, PreviewRequest, attach};
