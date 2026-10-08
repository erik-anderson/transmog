use std::{any::Any, net::SocketAddr, sync::Arc};

use thiserror::Error;

/// Opaque, adapter-owned state required to restore host configuration exactly.
#[derive(Clone)]
pub struct HostRestoreToken {
    value: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for HostRestoreToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostRestoreToken(..)")
    }
}

impl HostRestoreToken {
    /// Wraps adapter-specific restore state without exposing it to the service.
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self {
            value: Arc::new(value),
        }
    }

    /// Recovers adapter-specific restore state by type.
    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.value.downcast_ref()
    }
}

/// Redaction-safe host integration implementation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("host integration failed: {message}")]
pub struct HostIntegrationError {
    /// Operator-safe description.
    pub message: String,
}

impl HostIntegrationError {
    /// Creates an operator-safe host integration error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Transactional adapter for caller-owned system proxy or trust setup.
///
/// Implementations must make `restore` safe to retry with the same token.
/// The session crate ships no operating-system implementation.
pub trait HostIntegration: Send + Sync {
    /// Applies host configuration for one already-bound loopback endpoint and
    /// returns the exact prior state needed for restoration.
    ///
    /// # Errors
    ///
    /// Returns a redaction-safe implementation failure. The implementation is
    /// responsible for rolling back any partial apply before returning an error.
    fn apply(&self, endpoint: SocketAddr) -> Result<HostRestoreToken, HostIntegrationError>;

    /// Restores exactly the state represented by `token`.
    ///
    /// # Errors
    ///
    /// Returns a retryable redaction-safe implementation failure.
    fn restore(&self, token: &HostRestoreToken) -> Result<(), HostIntegrationError>;
}

/// Caller-supplied host transaction implementation.
#[derive(Clone)]
pub struct HostIntegrationPlan {
    /// Transactional adapter.
    pub integration: Arc<dyn HostIntegration>,
}

impl std::fmt::Debug for HostIntegrationPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostIntegrationPlan")
            .finish_non_exhaustive()
    }
}

pub(crate) struct HostTransaction {
    integration: Arc<dyn HostIntegration>,
    token: HostRestoreToken,
    armed: bool,
}

impl HostTransaction {
    pub(crate) fn plan(&self) -> HostIntegrationPlan {
        HostIntegrationPlan {
            integration: self.integration.clone(),
        }
    }
    pub(crate) fn apply(
        plan: HostIntegrationPlan,
        endpoint: SocketAddr,
    ) -> Result<Self, HostIntegrationError> {
        let token = plan.integration.apply(endpoint)?;
        Ok(Self {
            integration: plan.integration,
            token,
            armed: true,
        })
    }

    pub(crate) fn restore(mut self) -> Result<(), (Self, String)> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.integration.restore(&self.token)
        }));
        match result {
            Ok(Ok(())) => {
                self.armed = false;
                Ok(())
            }
            Ok(Err(error)) => Err((self, error.message)),
            Err(_) => Err((self, "host integration panicked".into())),
        }
    }
}

impl Drop for HostTransaction {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.integration.restore(&self.token)
            }));
        }
    }
}
