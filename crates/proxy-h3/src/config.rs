use std::time::Duration;

use thiserror::Error;
use transmog_tls::{TrustError, UpstreamTlsContextFactory};

/// Bounded transport settings for an HTTP/3 client connection.
#[derive(Clone, Copy, Debug)]
pub struct H3TransportLimits {
    /// Idle timeout for a connection that makes no progress.
    pub idle_timeout: Duration,
    /// Aggregate receive-flow-control window.
    pub connection_window: u64,
    /// Receive-flow-control window for each client-initiated stream.
    pub stream_window: u64,
    /// Maximum peer-created bidirectional streams.
    pub peer_bidirectional_streams: u64,
    /// Maximum peer-created unidirectional streams, including H3 controls.
    pub peer_unidirectional_streams: u64,
    /// Maximum number of requests queued for one pooled origin connection.
    pub pending_requests_per_connection: usize,
    /// Maximum number of authority/trust-generation entries in the client pool.
    pub max_pool_entries: usize,
}

impl Default for H3TransportLimits {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30),
            connection_window: 4 * 1024 * 1024,
            stream_window: 1024 * 1024,
            peer_bidirectional_streams: 16,
            peer_unidirectional_streams: 16,
            pending_requests_per_connection: 256,
            max_pool_entries: 128,
        }
    }
}

impl H3TransportLimits {
    /// Validates all bounds before a connection driver can allocate queues.
    ///
    /// # Errors
    ///
    /// Returns [`H3ConfigError::InvalidLimits`] for a zero or unusable bound.
    pub fn validate(self) -> Result<(), H3ConfigError> {
        if self.idle_timeout.is_zero() {
            return Err(H3ConfigError::InvalidLimits("idle timeout must be nonzero"));
        }
        if self.connection_window == 0 || self.stream_window == 0 {
            return Err(H3ConfigError::InvalidLimits(
                "connection and stream flow-control windows must be nonzero",
            ));
        }
        if self.peer_unidirectional_streams == 0 {
            return Err(H3ConfigError::InvalidLimits(
                "HTTP/3 requires at least one peer unidirectional control stream",
            ));
        }
        if self.pending_requests_per_connection == 0 || self.max_pool_entries == 0 {
            return Err(H3ConfigError::InvalidLimits(
                "pool and per-connection queue limits must be nonzero",
            ));
        }
        Ok(())
    }
}

/// Builds quiche configuration from the shared `BoringSSL` trust factory.
///
/// Peer verification is enabled, active migration is disabled, and 0-RTT is
/// left disabled. Flow-control values are finite and explicit.
///
/// # Errors
///
/// Returns [`H3ConfigError`] when the shared TLS context or quiche
/// configuration cannot be created.
pub fn build_quiche_config(
    factory: &UpstreamTlsContextFactory,
    limits: H3TransportLimits,
) -> Result<quiche::Config, H3ConfigError> {
    limits.validate()?;
    let tls = factory.quiche_context_builder()?;
    let mut config = quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls)?;
    // The shared factory already enables peer verification and installs the
    // identity callback. Calling `Config::verify_peer` here would replace
    // that callback with quiche's callback-free default.
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
    config.set_max_idle_timeout(u64::try_from(limits.idle_timeout.as_millis()).unwrap_or(u64::MAX));
    config.set_initial_max_data(limits.connection_window);
    config.set_initial_max_stream_data_bidi_local(limits.stream_window);
    config.set_initial_max_stream_data_bidi_remote(limits.stream_window);
    config.set_initial_max_stream_data_uni(limits.stream_window);
    config.set_initial_max_streams_bidi(limits.peer_bidirectional_streams);
    config.set_initial_max_streams_uni(limits.peer_unidirectional_streams);
    config.set_disable_active_migration(true);
    Ok(config)
}

/// Shared-context or quiche configuration failure.
#[derive(Debug, Error)]
pub enum H3ConfigError {
    /// A zero or otherwise unusable transport bound was supplied.
    #[error("invalid HTTP/3 transport limits: {0}")]
    InvalidLimits(&'static str),
    /// Shared TLS policy could not create the `BoringSSL` context.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// quiche rejected the context or HTTP/3 transport settings.
    #[error(transparent)]
    Quiche(#[from] quiche::Error),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use transmog_tls::{
        SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    };

    use super::*;

    #[test]
    fn shared_snapshot_builds_verified_quiche_context() {
        let snapshot = Arc::new(TrustSnapshot::load(&SystemTrustSource, 7).unwrap());
        let factory = UpstreamTlsContextFactory::new(snapshot, UpstreamTlsPolicy::default());
        build_quiche_config(&factory, H3TransportLimits::default()).unwrap();
    }

    #[test]
    fn zero_queue_and_pool_limits_are_rejected_before_driver_start() {
        let snapshot = Arc::new(TrustSnapshot::load(&SystemTrustSource, 8).unwrap());
        let factory = UpstreamTlsContextFactory::new(snapshot, UpstreamTlsPolicy::default());
        for limits in [
            H3TransportLimits {
                pending_requests_per_connection: 0,
                ..H3TransportLimits::default()
            },
            H3TransportLimits {
                max_pool_entries: 0,
                ..H3TransportLimits::default()
            },
        ] {
            assert!(matches!(
                build_quiche_config(&factory, limits),
                Err(H3ConfigError::InvalidLimits(_))
            ));
        }
    }
}
