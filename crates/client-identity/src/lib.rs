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

/// Lists executable names available for process-inspection suggestions.
///
/// The inventory is a bounded, best-effort snapshot, sorted and deduplicated.
/// Processes that exit or cannot be queried are omitted. Full paths and process
/// identifiers are never returned. The result is only a convenience for authoring
/// process rules; live connections still use their own identity lookup.
///
/// # Errors
/// Returns an operating-system enumeration error, or [`std::io::ErrorKind::Unsupported`]
/// when process attribution is unavailable on this platform.
pub fn running_process_names() -> std::io::Result<Vec<String>> {
    #[cfg(windows)]
    {
        windows::running_process_names()
    }
    #[cfg(target_os = "linux")]
    {
        linux::running_process_names()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Process inspection is supported on Windows and Linux.",
        ))
    }
}

/// Identifies the current process using the same native image lookup as sockets.
///
/// The known PID is retained when its executable name is inaccessible or
/// ambiguous. Unsupported platforms return [`ClientIdentity::LocalUnknown`].
pub fn current_process_identity() -> ClientIdentity {
    #[cfg(windows)]
    {
        let pid = std::process::id();
        ClientIdentity::LocalProcess {
            pid,
            name: windows::process_name(pid),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let pid = std::process::id();
        ClientIdentity::LocalProcess {
            pid,
            name: linux::process_name(pid),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        ClientIdentity::LocalUnknown
    }
}

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

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn current_process_identity_uses_a_native_executable_name() {
        let ClientIdentity::LocalProcess { pid, name } = current_process_identity() else {
            panic!("The current process should be identifiable");
        };
        assert_eq!(pid, std::process::id());
        let name = name.expect("The current executable should be readable");
        assert_ne!(name, "");
        assert!(!name.contains(['/', '\\']));
    }

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
