use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::HttpLegVersion;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        pub struct $name(pub u128);
    };
}

id_type!(
    SessionId,
    "Stable identifier for one request/response exchange."
);
id_type!(
    ConnectionId,
    "Stable identifier for one transport connection."
);
id_type!(
    StreamId,
    "Protocol stream identifier normalized to an unsigned value."
);

/// Best-effort identity of the process that opened the downstream connection.
///
/// Process attribution is intentionally a snapshot: process IDs can be reused
/// after a connection is accepted, and access controls or connection teardown
/// can prevent an operating-system lookup from succeeding.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClientIdentity {
    /// A loopback connection whose owning process was resolved.
    LocalProcess {
        /// Operating-system process identifier at connection-accept time.
        pid: u32,
        /// Executable file name when it could be queried without elevation.
        name: Option<String>,
    },
    /// A loopback connection whose owning process could not be resolved.
    #[default]
    LocalUnknown,
    /// A connection from a non-loopback peer; no local process lookup applies.
    Remote,
}

/// Transport metadata used to construct exchange hook context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Exchange identifier.
    pub session_id: SessionId,
    /// Downstream connection identifier.
    pub downstream_connection_id: ConnectionId,
    /// Stream identifier; HTTP/1 exchanges receive a locally allocated value.
    pub stream_id: StreamId,
    /// Browser/client peer address.
    pub client_addr: SocketAddr,
    /// Best-effort caller process identity captured when the connection opened.
    pub client_identity: ClientIdentity,
    /// Local listener address.
    pub proxy_addr: SocketAddr,
    /// Browser-facing HTTP version.
    pub ingress_version: HttpLegVersion,
    /// Selected origin-facing HTTP version, once known.
    pub egress_version: Option<HttpLegVersion>,
}
