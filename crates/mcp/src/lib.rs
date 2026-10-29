#![deny(missing_docs)]
//! Local agent authorization shared by the desktop and CLI connector.

mod access;
pub use access::{AccessKey, AccessManager, AccessStatus, Grant, GrantInput, Permission, Scope};
mod transport;
pub use transport::{CallFuture, Endpoint, Handler, LocalClient, LocalServer};
mod catalog;
pub use catalog::{ToolSpec, tool_catalog};
#[cfg(feature = "desktop")]
mod application;
#[cfg(feature = "desktop")]
pub use application::{AgentApplication, ProxyHost, SessionReview};
#[cfg(feature = "connector")]
mod connector;
#[cfg(feature = "connector")]
pub use connector::Connector;
#[cfg(all(test, feature = "connector"))]
mod connector_tests;

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
