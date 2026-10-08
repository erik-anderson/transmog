//! Runtime orchestration with safe listener defaults.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use transmog_core::intercept::HookLimits;

use thiserror::Error;

mod activity;
mod provider;
mod proxy;

pub use activity::ProxyActivity;

pub use provider::{
    AtomicRuntimeIdGenerator, RuntimeClock, RuntimeIdGenerator, RuntimeIdKind, SystemRuntimeClock,
};
pub use proxy::{
    ExchangeEvidence, ProxyComponents, ProxyConfig, ProxyControl, ProxyRuntimeError, ProxyServer,
    RouteAttemptEvidence, WebSocketSessionEvidence, WebSocketSessionOutcome,
};
pub use transmog_client_identity::{ClientIdentityResolver, SystemClientIdentityResolver};
pub use transmog_network::{HappyEyeballsConfig, HappyEyeballsConfigError};

/// Network exposure configuration.
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// Address for the explicit proxy listener.
    pub listen_addr: SocketAddr,
    /// Must be explicitly true before a non-loopback bind is accepted.
    pub allow_remote_clients: bool,
}

/// Complete runtime policy for the explicit proxy.
#[derive(Clone, Debug)]
pub struct RuntimeLimits {
    /// Maximum request-body bytes accepted by streaming and buffered adapters.
    pub max_request_body_bytes: usize,
    /// Maximum response-body bytes accepted by streaming and buffered adapters.
    pub max_response_body_bytes: usize,
    /// Maximum bytes accepted from an application-owned streaming local response.
    pub max_local_response_body_bytes: usize,
    /// Maximum body frames queued between independently scheduled adapters.
    pub body_channel_capacity: usize,
    /// Maximum simultaneous downstream TCP connections.
    pub max_connections: usize,
    /// Maximum concurrent streams advertised on each downstream HTTP/2 connection.
    pub max_h2_streams: u32,
    /// Maximum number of fields accepted in one downstream HTTP/1 header block.
    pub max_header_count: usize,
    /// Maximum downstream header-buffer or HTTP/2 header-list bytes.
    pub max_header_bytes: usize,
    /// Maximum time allowed to receive a complete downstream HTTP/1 header block.
    pub header_read_timeout: std::time::Duration,
    /// Maximum idle gap while receiving any request or response body frame.
    pub body_idle_timeout: std::time::Duration,
    /// Maximum duration of a downstream TLS handshake.
    pub tls_handshake_timeout: std::time::Duration,
    /// Maximum time to drain accepted connection tasks after shutdown begins.
    pub shutdown_timeout: std::time::Duration,
    /// Maximum duration of one route-selector call.
    pub route_selection_timeout: std::time::Duration,
    /// Maximum duration allowed for an application-owned upstream attempt.
    pub application_upstream_timeout: std::time::Duration,
    /// Maximum generated leaf certificates retained in memory.
    pub leaf_cache_capacity: usize,
    /// Generated leaf validity in days.
    pub leaf_validity_days: u32,
    /// Hook callback concurrency and deadline policy.
    pub hooks: HookLimits,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_request_body_bytes: 4 * 1024 * 1024,
            max_response_body_bytes: 16 * 1024 * 1024,
            max_local_response_body_bytes: 1024 * 1024 * 1024,
            body_channel_capacity: 8,
            max_connections: 1_024,
            max_h2_streams: 128,
            max_header_count: 128,
            max_header_bytes: 64 * 1024,
            header_read_timeout: std::time::Duration::from_secs(15),
            body_idle_timeout: std::time::Duration::from_secs(30),
            tls_handshake_timeout: std::time::Duration::from_secs(10),
            shutdown_timeout: std::time::Duration::from_secs(10),
            route_selection_timeout: std::time::Duration::from_secs(5),
            application_upstream_timeout: std::time::Duration::from_secs(30),
            leaf_cache_capacity: 1_024,
            leaf_validity_days: 2,
            hooks: HookLimits::default(),
        }
    }
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            allow_remote_clients: false,
        }
    }
}

impl ListenerConfig {
    /// Rejects accidental remote exposure.
    ///
    /// # Errors
    ///
    /// Returns [`ListenerConfigError`] for a non-loopback address unless
    /// `allow_remote_clients` explicitly acknowledges the exposure.
    pub fn validate(&self) -> Result<(), ListenerConfigError> {
        if !self.listen_addr.ip().is_loopback() && !self.allow_remote_clients {
            return Err(ListenerConfigError::RemoteBindRequiresOptIn(
                self.listen_addr,
            ));
        }
        Ok(())
    }
}

/// Unsafe listener configuration.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ListenerConfigError {
    /// Non-loopback listener without explicit acknowledgement.
    #[error("remote listen address {0} requires allow_remote_clients=true")]
    RemoteBindRequiresOptIn(SocketAddr),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_default_and_remote_requires_opt_in() {
        assert!(ListenerConfig::default().validate().is_ok());
        let proxy = ProxyConfig::default();
        assert_eq!(
            proxy.happy_eyeballs.attempt_delay(),
            std::time::Duration::from_millis(250)
        );
        let remote = ListenerConfig {
            listen_addr: "0.0.0.0:8080".parse().unwrap(),
            allow_remote_clients: false,
        };
        assert!(matches!(
            remote.validate(),
            Err(ListenerConfigError::RemoteBindRequiresOptIn(_))
        ));
    }
}
