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

/// Transport metadata shared with breakpoint handlers.
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
    /// Local listener address.
    pub proxy_addr: SocketAddr,
    /// Browser-facing HTTP version.
    pub ingress_version: HttpLegVersion,
    /// Selected origin-facing HTTP version, once known.
    pub egress_version: Option<HttpLegVersion>,
}
