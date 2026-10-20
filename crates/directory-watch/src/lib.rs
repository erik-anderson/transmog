//! Bounded owned directory notifications. This API reads no file contents and releases its bounded observer on drop.
#![deny(unsafe_op_in_unsafe_fn)]
use std::{io, path::Path};

/// One bounded completed OS notification buffer.
#[derive(Debug)]
pub struct Notifications {
    /// Directory-relative names; inert data which must never be used as unchecked filesystem paths.
    pub names: Vec<String>,
    /// Details were lost or malformed. The caller may revalidate its own bounded source inventory.
    pub overflowed: bool,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::DirectoryWatch;

/// Unsupported platforms expose no polling fallback or periodic file reads.
#[cfg(not(windows))]
pub struct DirectoryWatch;
#[cfg(not(windows))]
impl DirectoryWatch {
    /// Opens a directory notification handle.
    /// # Errors
    /// Returns unsupported on this platform.
    pub fn open(_path: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Directory notifications are unavailable",
        ))
    }
    /// Returns a completed metadata buffer or `None` without waiting.
    /// # Errors
    /// Returns unsupported on this platform.
    pub fn poll(&mut self) -> io::Result<Option<Notifications>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Directory notifications are unavailable",
        ))
    }
}
