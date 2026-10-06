#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Best-effort, bounded attribution of a loopback TCP client to its process.
//!
//! Attribution is captured at accept time and is never an authorization input:
//! operating-system visibility, races, namespaces, and PID reuse can all make it
//! incomplete. Non-loopback peers are classified as remote without enumeration.

use std::net::SocketAddr;

use transmog_core::ClientIdentity;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(windows)]
mod windows;

/// Resolves caller identity for one newly accepted downstream TCP connection.
pub trait ClientIdentityResolver: Send + Sync {
    /// Returns a best-effort identity. Implementations must treat lookup
    /// failures as [`ClientIdentity::LocalUnknown`] rather than failing traffic.
    fn resolve(&self, client_addr: SocketAddr, proxy_addr: SocketAddr) -> ClientIdentity;
}

/// Production resolver for the current operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClientIdentityResolver;

impl ClientIdentityResolver for SystemClientIdentityResolver {
    fn resolve(&self, client_addr: SocketAddr, proxy_addr: SocketAddr) -> ClientIdentity {
        if !client_addr.ip().is_loopback() {
            return ClientIdentity::Remote;
        }

        #[cfg(windows)]
        {
            windows::resolve(client_addr, proxy_addr).unwrap_or(ClientIdentity::LocalUnknown)
        }
        #[cfg(target_os = "linux")]
        {
            linux::resolve(client_addr, proxy_addr).unwrap_or(ClientIdentity::LocalUnknown)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = proxy_addr;
            ClientIdentity::LocalUnknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_peers_never_trigger_local_attribution() {
        assert_eq!(
            SystemClientIdentityResolver.resolve(
                "192.0.2.10:12345".parse().unwrap(),
                "192.0.2.20:8080".parse().unwrap(),
            ),
            ClientIdentity::Remote
        );
    }
}
