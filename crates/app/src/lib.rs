#![deny(missing_docs)]

//! UI-neutral product operations and bounded presentation read models.
//!
//! This crate is the application boundary shared by the desktop shell and
//! future command-line frontends. It deliberately has no Tauri, `WebUI`,
//! `WebView`, or operating-system dependency.

use serde::Serialize;
use thiserror::Error;
use transmog_session::{ApplicationSessionService, ServiceConfig, ServiceError, ServiceStatus};

/// Stable application failure category suitable for presentation boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCategory {
    /// Caller input failed validation.
    InvalidInput,
    /// Requested operation conflicts with current application state.
    Conflict,
    /// A finite resource or result limit was reached.
    Limit,
    /// An external resource was unavailable.
    Unavailable,
    /// An operation failed but may be retried without changing its meaning.
    Retryable,
    /// An internal operation failed with a redaction-safe description.
    Internal,
}

/// Bounded, redaction-safe application error.
#[derive(Clone, Debug, Eq, Error, PartialEq, Serialize)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    /// Stable machine-readable category.
    pub category: ErrorCategory,
    /// Operator-safe message.
    pub message: String,
    /// Whether retrying the same action can be useful.
    pub retryable: bool,
}

impl AppError {
    fn new(category: ErrorCategory, message: impl Into<String>, retryable: bool) -> Self {
        let message = message.into().chars().take(512).collect();
        Self {
            category,
            message,
            retryable,
        }
    }
}

impl From<ServiceError> for AppError {
    fn from(error: ServiceError) -> Self {
        let (category, retryable) = match error {
            ServiceError::AlreadyRunning | ServiceError::FailedRunNeedsStop => {
                (ErrorCategory::Conflict, false)
            }
            ServiceError::HostRestore(_) | ServiceError::HostRestorePending => {
                (ErrorCategory::Retryable, true)
            }
            ServiceError::ListenerAddress(_)
            | ServiceError::Runtime(_)
            | ServiceError::Capture(_)
            | ServiceError::HostApply(_) => (ErrorCategory::Unavailable, true),
        };
        Self::new(category, error.to_string(), retryable)
    }
}

/// Public application lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppLifecycle {
    /// No proxy run is active.
    Stopped,
    /// The proxy listener is active.
    Running,
    /// Graceful shutdown is in progress.
    Stopping,
    /// The last proxy run failed.
    Failed,
}

/// Bounded status read model returned to every presentation layer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// Current lifecycle category.
    pub lifecycle: AppLifecycle,
    /// Bound listener endpoint, when one exists.
    pub listener: Option<String>,
    /// Operator-safe status summary.
    pub summary: String,
    /// Whether exact host restoration remains pending.
    pub host_restore_pending: bool,
}

/// Construction settings for the application facade.
#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    /// Session-service limits and same-build identity.
    pub service: ServiceConfig,
}

/// Cloneable owner of authoritative application state.
#[derive(Clone)]
pub struct Application {
    service: ApplicationSessionService,
}

impl std::fmt::Debug for Application {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Application")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Application {
    /// Creates an idle application with finite service state.
    ///
    /// # Errors
    ///
    /// Returns a bounded application error if the owned service cannot start.
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        Ok(Self {
            service: ApplicationSessionService::new(config.service).map_err(AppError::from)?,
        })
    }

    /// Returns the authoritative session service for product use-case modules.
    pub fn session_service(&self) -> &ApplicationSessionService {
        &self.service
    }

    /// Returns a bounded point-in-time lifecycle read model.
    pub fn status(&self) -> AppStatus {
        let pending = self.service.has_pending_host_restore();
        match self.service.status() {
            ServiceStatus::Stopped => AppStatus {
                lifecycle: AppLifecycle::Stopped,
                listener: None,
                summary: "Proxy stopped".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Running { local_addr } => AppStatus {
                lifecycle: AppLifecycle::Running,
                listener: Some(local_addr.to_string()),
                summary: "Proxy listening".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Stopping { local_addr } => AppStatus {
                lifecycle: AppLifecycle::Stopping,
                listener: Some(local_addr.to_string()),
                summary: "Proxy stopping".to_owned(),
                host_restore_pending: pending,
            },
            ServiceStatus::Failed {
                local_addr,
                message,
            } => AppStatus {
                lifecycle: AppLifecycle::Failed,
                listener: local_addr.map(|address| address.to_string()),
                summary: message.chars().take(256).collect(),
                host_restore_pending: pending,
            },
        }
    }

    /// Gracefully stops the service and restores any active host transaction.
    ///
    /// # Errors
    ///
    /// Returns a retryable safe error when drain or restoration fails.
    pub async fn shutdown(&self) -> Result<(), AppError> {
        self.service.stop().await.map_err(AppError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_application_is_stopped_and_ui_neutral() {
        let application = Application::new(AppConfig::default()).unwrap();
        assert_eq!(
            application.status(),
            AppStatus {
                lifecycle: AppLifecycle::Stopped,
                listener: None,
                summary: "Proxy stopped".to_owned(),
                host_restore_pending: false,
            }
        );
        let json = serde_json::to_value(application.status()).unwrap();
        assert_eq!(json["lifecycle"], "stopped");
        assert!(json.get("tauri").is_none());
    }

    #[tokio::test]
    async fn stopped_shutdown_is_idempotent() {
        let application = Application::new(AppConfig::default()).unwrap();
        application.shutdown().await.unwrap();
        application.shutdown().await.unwrap();
        assert_eq!(application.status().lifecycle, AppLifecycle::Stopped);
    }
}
