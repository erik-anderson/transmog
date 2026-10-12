//! Observed long-lived application stream lifecycle, independent of HTTP completion.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Application protocol carried by an exchange.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamProtocol {
    /// An upgraded, bidirectional WebSocket.
    WebSocket,
    /// A server-sent event response.
    Sse,
}

/// Lifetime of the application stream, distinct from its HTTP handshake.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamStatus {
    /// The upgrade is being established.
    Connecting,
    /// The stream is open, including periods with no messages.
    Open,
    /// A peer initiated closure.
    Closing,
    /// The stream ended normally.
    Closed,
    /// The transport or protocol failed.
    Failed,
    /// The application stopped observation before normal closure.
    Interrupted,
}

impl StreamStatus {
    /// Whether this state still represents an unfinished stream.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Connecting | Self::Open | Self::Closing)
    }
}

/// Timestamped stream state retained and saved with its originating exchange.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamState {
    /// Application protocol.
    pub protocol: StreamProtocol,
    /// Observed lifetime state.
    pub status: StreamStatus,
    /// Observation time in Unix milliseconds.
    pub at_millis: u64,
    /// Optional operator-safe close or failure explanation.
    pub reason: Option<String>,
}

impl StreamState {
    /// Creates an observation at the current wall-clock time.
    pub fn now(protocol: StreamProtocol, status: StreamStatus, reason: Option<String>) -> Self {
        Self {
            protocol,
            status,
            at_millis: observed_millis(),
            reason,
        }
    }
}

/// Current wall-clock time used by stream observations.
pub fn observed_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
