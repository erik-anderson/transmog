#![deny(missing_docs)]
//! Local agent authorization shared by the desktop and CLI connector.

mod access;
pub use access::{AccessKey, AccessManager, AccessStatus, Grant, GrantInput, Permission, Scope};

use serde::Serialize;

/// Safe failure returned to an agent without credentials or filesystem paths.
#[derive(Clone, Debug, Serialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct Error {
    /// Stable machine-readable failure category.
    pub code: &'static str,
    /// User-facing explanation and useful next action.
    pub message: String,
}

impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
