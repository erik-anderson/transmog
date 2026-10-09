#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Native render-only browser boundary. Captured bytes are served through COM
//! streams; this crate has no proxy, certificate, application IPC or network API.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{CapturedResponse, Lookup, PreviewRequest, attach};
