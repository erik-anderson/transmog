//! Pool authorization and physical-handshake ownership shared by TCP and QUIC.
use crate::{
    TlsCapture, UpstreamTlsContextFactory,
    client_authentication::{
        ClientAuthenticationAttempt, ClientAuthenticationContext, ClientAuthenticationResolver,
    },
};
use std::{fmt, sync::Arc};

/// Prevents cached authentication from bypassing a fresh `CertificateRequest`.
/// # Errors
/// Returns a native TLS configuration failure.
pub fn clear_cached_session(
    ssl: &mut boring::ssl::SslRef,
) -> Result<(), boring::error::ErrorStack> {
    transmog_tls_native::clear_client_session(ssl)
}

pub(crate) struct AuthenticationScope {
    pub resolver: Arc<dyn ClientAuthenticationResolver>,
    pub caller: String,
    pub generation: Option<u64>,
}
impl fmt::Debug for AuthenticationScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthenticationScope")
            .field("caller", &self.caller)
            .finish_non_exhaustive()
    }
}
impl UpstreamTlsContextFactory {
    /// Whether a request-driven controller is installed, including epoch zero.
    pub fn has_client_certificate_resolver(&self) -> bool {
        self.authentication.is_some()
    }
    /// Installs request-driven authentication for one immutable caller partition.
    /// Existing pools must be replaced when attaching a different controller or caller.
    #[must_use]
    pub fn with_client_certificate_resolver(
        mut self,
        resolver: Arc<dyn ClientAuthenticationResolver>,
        caller: impl Into<String>,
    ) -> Self {
        self.authentication = Some(Arc::new(AuthenticationScope {
            resolver,
            caller: caller.into(),
            generation: None,
        }));
        self
    }
    /// Selects a caller without sharing its certificate decisions with another pool.
    #[must_use]
    pub fn for_client_certificate_caller(mut self, caller: impl Into<String>) -> Self {
        if let Some(scope) = &self.authentication {
            self.authentication = Some(Arc::new(AuthenticationScope {
                resolver: scope.resolver.clone(),
                caller: caller.into(),
                generation: scope.generation,
            }));
        }
        self
    }
    /// Current authorization epoch; absent controllers also report zero.
    pub fn client_certificate_generation(&self) -> u64 {
        self.authentication
            .as_ref()
            .map_or(0, |scope| scope.resolver.generation())
    }
    /// Freezes the authorization epoch admitted by a transport pool.
    #[must_use]
    pub fn for_client_certificate_epoch(mut self, generation: u64) -> Self {
        if let Some(scope) = &self.authentication {
            self.authentication = Some(Arc::new(AuthenticationScope {
                resolver: scope.resolver.clone(),
                caller: scope.caller.clone(),
                generation: Some(generation),
            }));
        }
        self
    }
    /// Epoch captured by this connector, independently of later controller updates.
    pub fn client_certificate_admission_generation(&self) -> u64 {
        self.authentication.as_ref().map_or(0, |s| {
            s.generation.unwrap_or_else(|| s.resolver.generation())
        })
    }
    /// Whether an origin may reuse its physical connection within the current epoch.
    pub fn permits_client_certificate_reuse(&self, host: &str, port: u16) -> bool {
        self.authentication
            .as_ref()
            .is_none_or(|scope| scope.resolver.permits_connection_reuse(host, port))
    }
    /// Checks the identity actually installed, rather than a merely configured selector.
    pub fn permits_selected_client_certificate_reuse(
        &self,
        host: &str,
        port: u16,
        identity: Option<&str>,
    ) -> bool {
        self.authentication.as_ref().is_none_or(|scope| {
            scope.resolver.generation() == self.client_certificate_admission_generation()
                && scope
                    .resolver
                    .permits_selected_connection_reuse(host, port, identity)
        })
    }
    /// Creates ownership for an actual connection; inventory is queried only after its peer asks.
    pub fn client_certificate_attempt(
        &self,
        host: &str,
        port: u16,
        generation: u64,
        capture: TlsCapture,
    ) -> Option<ClientAuthenticationAttempt> {
        self.authentication.as_ref().map(|scope| {
            ClientAuthenticationAttempt::new(
                scope.resolver.clone(),
                ClientAuthenticationContext {
                    host: host.into(),
                    port,
                    caller: scope.caller.clone(),
                    generation,
                },
                capture,
            )
        })
    }
}
