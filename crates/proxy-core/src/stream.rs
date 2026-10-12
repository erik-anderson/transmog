//! Observed long-lived application stream lifecycle, independent of HTTP completion.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum retained payload for one inspected message; original length is preserved.
pub const MESSAGE_BYTES: usize = 256 * 1024;

/// A message, control frame, or SSE event observed without changing forwarded bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamMessage {
    /// Local observation time.
    pub at_millis: u64,
    /// `client` or `server`, identifying the sender.
    pub sender: String,
    /// Text, binary, ping, pong, close, event, heartbeat, or unavailable.
    pub kind: String,
    /// Original decoded payload length.
    pub byte_count: u64,
    /// Bounded decoded bytes; never HTML.
    pub payload: bytes::Bytes,
    /// True when the payload is only a retained prefix.
    pub truncated: bool,
    /// Whether WebSocket compression was negotiated for this message.
    pub compressed: bool,
    /// SSE event name, when applicable.
    pub event: Option<String>,
    /// SSE last event ID, when applicable.
    pub event_id: Option<String>,
    /// SSE reconnection delay in milliseconds.
    pub retry: Option<u64>,
    /// Exact decoded SSE event block, including field syntax.
    pub raw: Option<String>,
    /// WebSocket close code, when supplied by the peer.
    pub close_code: Option<u16>,
}

impl StreamMessage {
    /// Creates a bounded message and preserves its original length.
    pub fn new(sender: &str, kind: &str, payload: bytes::Bytes) -> Self {
        let byte_count = payload.len() as u64;
        let truncated = payload.len() > MESSAGE_BYTES;
        // Copy the prefix so it cannot pin an arbitrarily larger allocation.
        let payload = bytes::Bytes::copy_from_slice(&payload[..payload.len().min(MESSAGE_BYTES)]);
        Self {
            at_millis: observed_millis(),
            sender: sender.into(),
            kind: kind.into(),
            byte_count,
            payload,
            truncated,
            compressed: false,
            event: None,
            event_id: None,
            retry: None,
            raw: None,
            close_code: None,
        }
    }

    /// Memory charged to the bounded history, including metadata.
    pub fn retained_bytes(&self) -> usize {
        self.payload.len()
            + self.raw.as_ref().map_or(0, String::len)
            + self.event.as_ref().map_or(0, String::len)
            + self.event_id.as_ref().map_or(0, String::len)
            + 256
    }
}

/// Stable observed item identity, unaffected by filtering or retention eviction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamItem {
    /// Per-exchange observer sequence.
    pub sequence: u64,
    /// Observed message content.
    pub message: StreamMessage,
}

/// Bounded recent stream history and lifetime counters.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHistory {
    /// Retained recent items, in observed order.
    pub items: std::collections::VecDeque<StreamItem>,
    /// Total observed items including evicted history.
    pub observed: u64,
    /// Items evicted by retention.
    pub evicted: u64,
    /// Total decoded bytes sent by the client.
    pub client_bytes: u64,
    /// Total decoded bytes sent by the server.
    pub server_bytes: u64,
    /// Time the stream was first observed.
    pub opened_at: Option<u64>,
    /// Most recent observed message time.
    pub last_activity: Option<u64>,
    /// Bytes charged to retained items.
    pub retained_bytes: usize,
}

impl StreamHistory {
    /// Retains a bounded recent history; eviction never changes lifetime state.
    pub fn push(&mut self, sequence: u64, message: StreamMessage) {
        self.observed = self.observed.saturating_add(1);
        self.last_activity = Some(message.at_millis);
        if message.sender == "client" {
            self.client_bytes = self.client_bytes.saturating_add(message.byte_count);
        } else {
            self.server_bytes = self.server_bytes.saturating_add(message.byte_count);
        }
        self.retained_bytes = self.retained_bytes.saturating_add(message.retained_bytes());
        self.items.push_back(StreamItem { sequence, message });
        while self.items.len() > 1024 || self.retained_bytes > 8 * 1024 * 1024 {
            let Some(item) = self.items.pop_front() else {
                break;
            };
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(item.message.retained_bytes());
            self.evicted = self.evicted.saturating_add(1);
        }
    }
}

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

/// Recognizes an SSE media type without depending on transfer framing.
pub fn is_event_stream(headers: &crate::HeaderBlock) -> bool {
    headers.values("content-type").any(|value| {
        std::str::from_utf8(value).is_ok_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/event-stream")
        })
    })
}
