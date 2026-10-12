//! Connection partition identities and policies shared by transport adapters.
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use crate::{ClientIdentity, HttpLegVersion, intercept::ExchangeMetadata};

/// HTTP protocols admitted when creating a new origin connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedHttpProtocols {
    /// Permit new HTTP/1.1 connections.
    pub http1: bool,
    /// Permit new HTTP/2 connections.
    pub http2: bool,
    /// Permit new HTTP/3 connections.
    pub http3: bool,
}

impl Default for AllowedHttpProtocols {
    fn default() -> Self {
        Self {
            http1: true,
            http2: true,
            http3: true,
        }
    }
}

impl AllowedHttpProtocols {
    /// Whether a new connection may select this HTTP version.
    pub const fn permits(self, version: HttpLegVersion) -> bool {
        match version {
            HttpLegVersion::Http1 => self.http1,
            HttpLegVersion::Http2 => self.http2,
            HttpLegVersion::Http3 => self.http3,
        }
    }
    const fn bits(self) -> u8 {
        (self.http1 as u8) | ((self.http2 as u8) << 1) | ((self.http3 as u8) << 2)
    }
}

const REUSE_CONNECTIONS: u8 = 1 << 3;

/// Live connection-admission and reuse policy shared by origin transports.
#[derive(Clone, Debug)]
pub struct OutboundProtocolPolicy(Arc<AtomicU8>);

impl Default for OutboundProtocolPolicy {
    fn default() -> Self {
        Self::new(AllowedHttpProtocols::default())
    }
}

impl OutboundProtocolPolicy {
    /// Creates one policy shared by every caller partition and trust generation.
    pub fn new(allowed: AllowedHttpProtocols) -> Self {
        Self(Arc::new(AtomicU8::new(allowed.bits() | REUSE_CONNECTIONS)))
    }
    /// Takes a consistent snapshot at connection admission.
    pub fn snapshot(&self) -> AllowedHttpProtocols {
        let bits = self.0.load(Ordering::Acquire);
        AllowedHttpProtocols {
            http1: bits & 1 != 0,
            http2: bits & 2 != 0,
            http3: bits & 4 != 0,
        }
    }
    /// Whether subsequent requests may share physical origin connections.
    pub fn reuse_connections(&self) -> bool {
        self.0.load(Ordering::Acquire) & REUSE_CONNECTIONS != 0
    }
    /// Changes reuse for subsequent requests while admitted work finishes normally.
    pub fn set_reuse_connections(&self, enabled: bool) {
        if enabled {
            self.0.fetch_or(REUSE_CONNECTIONS, Ordering::AcqRel);
        } else {
            self.0.fetch_and(!REUSE_CONNECTIONS, Ordering::AcqRel);
        }
    }
    /// Changes admission for new connections without closing existing ones.
    pub fn set(&self, allowed: AllowedHttpProtocols) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some((current & REUSE_CONNECTIONS) | allowed.bits())
            });
    }
}

/// Opaque identity separating physical outbound connection pools.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NetworkIsolationKey(Arc<str>);

impl NetworkIsolationKey {
    /// Constructs an application-computed partition identity.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    /// Explicitly shares pools across all callers.
    pub fn shared() -> Self {
        Self(Arc::from("shared"))
    }

    /// Partitions by PID when available, otherwise source IP, and then by H2 session.
    ///
    /// H2 stream IDs do not enter the key: concurrent streams in one browser
    /// session share its pool, while independent sessions stay separate.
    pub fn for_caller(metadata: &ExchangeMetadata) -> Self {
        let caller = match metadata.client_identity {
            ClientIdentity::LocalProcess { pid, .. } => format!("process:{pid}"),
            _ => format!("address:{}", metadata.client_addr.ip()),
        };
        if metadata.ingress_version == HttpLegVersion::Http2 {
            Self::new(format!(
                "{caller}/h2:{}",
                metadata.downstream_connection_id.0
            ))
        } else {
            Self::new(caller)
        }
    }
}

/// Embedding policy for computing connection partitions from immutable caller evidence.
pub trait NetworkIsolationPolicy: Send + Sync {
    /// Returns the partition for this exchange; request hooks cannot change its caller.
    fn compute(&self, metadata: &ExchangeMetadata) -> NetworkIsolationKey;
}

/// The default process/source-IP plus browser H2 session partition.
#[derive(Clone, Copy, Debug, Default)]
pub struct CallerNetworkIsolation;

impl NetworkIsolationPolicy for CallerNetworkIsolation {
    fn compute(&self, metadata: &ExchangeMetadata) -> NetworkIsolationKey {
        NetworkIsolationKey::for_caller(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionId, SessionId, SessionMetadata, StreamId, Target};

    #[test]
    fn protocol_edits_and_clones_preserve_connection_reuse_choice() {
        let policy = OutboundProtocolPolicy::default();
        let clone = policy.clone();
        policy.set_reuse_connections(false);
        clone.set(AllowedHttpProtocols {
            http1: false,
            http2: true,
            http3: false,
        });
        assert!(!policy.reuse_connections());
        assert_eq!(
            policy.snapshot(),
            AllowedHttpProtocols {
                http1: false,
                http2: true,
                http3: false
            }
        );
        clone.set_reuse_connections(true);
        assert!(policy.reuse_connections());
        assert!(!policy.snapshot().http1);
    }

    fn metadata(
        pid: Option<u32>,
        address: &str,
        connection: u128,
        stream: u128,
        version: HttpLegVersion,
    ) -> ExchangeMetadata {
        ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(connection),
                stream_id: StreamId(stream),
                client_addr: address.parse().unwrap(),
                proxy_addr: "127.0.0.1:8080".parse().unwrap(),
                ingress_version: version,
                egress_version: None,
                client_identity: pid.map_or(ClientIdentity::LocalUnknown, |pid| {
                    ClientIdentity::LocalProcess { pid, name: None }
                }),
            },
            Target {
                scheme: "https".into(),
                host: "example.test".into(),
                port: 443,
                authority: "example.test".into(),
                path: "/".into(),
                query: None,
            },
        )
    }

    #[test]
    fn process_partitions_ignore_source_ports_and_h1_socket_ids() {
        let first = NetworkIsolationKey::for_caller(&metadata(
            Some(10),
            "127.0.0.1:1234",
            1,
            1,
            HttpLegVersion::Http1,
        ));
        assert_eq!(
            first,
            NetworkIsolationKey::for_caller(&metadata(
                Some(10),
                "127.0.0.1:5678",
                2,
                2,
                HttpLegVersion::Http1
            ))
        );
        assert_ne!(
            first,
            NetworkIsolationKey::for_caller(&metadata(
                Some(11),
                "127.0.0.1:1234",
                1,
                1,
                HttpLegVersion::Http1
            ))
        );
    }

    #[test]
    fn h2_streams_share_a_session_but_sessions_and_unknown_source_ips_do_not() {
        let first = NetworkIsolationKey::for_caller(&metadata(
            Some(10),
            "127.0.0.1:1234",
            1,
            1,
            HttpLegVersion::Http2,
        ));
        assert_eq!(
            first,
            NetworkIsolationKey::for_caller(&metadata(
                Some(10),
                "127.0.0.1:1234",
                1,
                3,
                HttpLegVersion::Http2
            ))
        );
        assert_ne!(
            first,
            NetworkIsolationKey::for_caller(&metadata(
                Some(10),
                "127.0.0.1:5678",
                2,
                1,
                HttpLegVersion::Http2
            ))
        );
        let unknown = NetworkIsolationKey::for_caller(&metadata(
            None,
            "192.0.2.1:1234",
            1,
            1,
            HttpLegVersion::Http1,
        ));
        assert_eq!(
            unknown,
            NetworkIsolationKey::for_caller(&metadata(
                None,
                "192.0.2.1:5678",
                2,
                1,
                HttpLegVersion::Http1
            ))
        );
        assert_ne!(
            unknown,
            NetworkIsolationKey::for_caller(&metadata(
                None,
                "192.0.2.2:1234",
                1,
                1,
                HttpLegVersion::Http1
            ))
        );
    }
}
