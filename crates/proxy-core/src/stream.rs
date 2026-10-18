//! Observed long-lived application stream lifecycle, independent of HTTP completion.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// Complete decoded bytes when content retention is enabled; never HTML.
    pub payload: bytes::Bytes,
    /// True when the payload is only a retained prefix.
    pub truncated: bool,
    /// Whether WebSocket compression was negotiated for this message.
    pub compressed: bool,
    /// SSE event name, when applicable.
    pub event: Option<String>,
    /// SSE last event ID, when applicable.
    pub event_id: Option<std::sync::Arc<str>>,
    /// SSE reconnection delay in milliseconds.
    pub retry: Option<u64>,
    /// Decoded SSE event block and field syntax, with LF line endings.
    pub raw: Option<String>,
    /// WebSocket close code, when supplied by the peer.
    pub close_code: Option<u16>,
}

impl StreamMessage {
    /// Creates a message without silently truncating its content.
    pub fn new(sender: &str, kind: &str, payload: bytes::Bytes) -> Self {
        let byte_count = payload.len() as u64;
        Self {
            at_millis: observed_millis(),
            sender: sender.into(),
            kind: kind.into(),
            byte_count,
            payload,
            truncated: false,
            compressed: false,
            event: None,
            event_id: None,
            retry: None,
            raw: None,
            close_code: None,
        }
    }

    /// Memory occupied by one standalone message, including metadata.
    pub fn retained_bytes(&self) -> usize {
        self.payload.len()
            + self.raw.as_ref().map_or(0, String::len)
            + self.event.as_ref().map_or(0, String::len)
            + self.event_id.as_ref().map_or(0, |id| id.len())
            + self.sender.len()
            + self.kind.len()
            + 256
    }
}

/// Stable observed item identity, unaffected by filtering or retention eviction.
#[derive(Clone, Debug)]
pub struct StreamItem {
    /// Per-exchange observer sequence.
    pub sequence: u64,
    /// Observed message content.
    pub message: StreamMessage,
    /// Optional pinned capture source; payload bytes are read only on demand.
    pub source: Option<std::sync::Arc<dyn StreamContentSource>>,
}

/// Immutable, lazy payload source owned by an imported stream item.
pub trait StreamContentSource: std::fmt::Debug + Send + Sync {
    /// Reads and authenticates one message's content.
    /// # Errors
    /// Returns an explanation when the source is corrupt or unavailable.
    fn read(&self) -> Result<StreamMessage, String>;
}
impl StreamItem {
    /// Reads retained content without changing this item's identity or metadata.
    /// # Errors
    /// Returns an explanation when an imported payload cannot be read.
    pub fn read(&self) -> Result<StreamMessage, String> {
        self.source
            .as_ref()
            .map_or_else(|| Ok(self.message.clone()), |source| source.read())
    }
}

/// Retained stream history and lifetime counters.
#[derive(Clone, Debug, Default)]
pub struct StreamHistory {
    /// Retained items, in observed order.
    pub items: std::collections::VecDeque<std::sync::Arc<StreamItem>>,
    /// Total observed items including evicted history.
    pub observed: u64,
    /// Items evicted by retention.
    pub evicted: u64,
    /// Total decoded bytes sent by the client.
    pub client_bytes: u64,
    /// Total decoded bytes sent by the server.
    pub server_bytes: u64,
    /// Whether client totals were initialized live or explicitly recorded.
    pub client_bytes_known: bool,
    /// Whether server totals were initialized live or explicitly recorded.
    pub server_bytes_known: bool,
    /// First stream observation (legacy name; may be Connecting, not established).
    pub opened_at: Option<u64>,
    /// Explicit Open observation; never inferred from the legacy first observation.
    pub established_at: Option<u64>,
    /// Explicit Closed, Failed or Interrupted observation, never last message time.
    pub ended_at: Option<u64>,
    /// Known observation cutoff for an unfinished stream snapshot.
    pub observed_until: Option<u64>,
    /// Established lifetime measured monotonically, or a recorded source interval.
    pub elapsed_millis: Option<u64>,
    live_clock: Option<std::time::Instant>,
    /// Most recent observed message time.
    pub last_activity: Option<u64>,
    /// Bytes charged to retained items.
    pub retained_bytes: usize,
    event_ids: std::collections::HashSet<usize>,
}

impl StreamHistory {
    /// Records lifecycle evidence. Imported event timestamps retain their source clock.
    pub fn observe_state(&mut self, state: &StreamState, live: bool) {
        self.opened_at.get_or_insert(state.at_millis);
        if state.status == StreamStatus::Open
            && self.established_at.is_none()
            && state.at_millis > 0
        {
            self.established_at = Some(state.at_millis);
            if live {
                self.client_bytes_known = true;
                self.server_bytes_known = true;
            }
            self.elapsed_millis = live.then_some(0);
            self.live_clock = live.then(std::time::Instant::now);
        }
        if !state.status.is_active() && self.ended_at.is_none() {
            self.ended_at = (state.at_millis > 0).then_some(state.at_millis);
            self.elapsed_millis = if live {
                self.lifetime_millis()
            } else {
                self.ended_at
                    .zip(self.established_at)
                    .and_then(|(end, start)| end.checked_sub(start))
            };
            self.live_clock = None;
        }
        if self.established_at.is_some()
            && state.at_millis > 0
            && (live || state.status != StreamStatus::Open)
        {
            self.observed_until = Some(state.at_millis);
        }
    }

    /// Lifetime through this instant, including quiet periods on a live stream.
    pub fn lifetime_millis(&self) -> Option<u64> {
        self.lifetime_millis_at(std::time::Instant::now())
    }

    fn lifetime_millis_at(&self, now: std::time::Instant) -> Option<u64> {
        self.established_at?;
        self.elapsed_millis.map(|elapsed| {
            elapsed.saturating_add(self.live_clock.map_or(0, |clock| {
                u64::try_from(now.saturating_duration_since(clock).as_millis()).unwrap_or(u64::MAX)
            }))
        })
    }

    /// Freezes timing at the catalog read boundary, including a reliable capture cutoff.
    #[must_use]
    pub fn snapshot(&self, live: bool) -> Self {
        let mut history = self.clone();
        if live {
            history.elapsed_millis = self.lifetime_millis();
            if self.live_clock.is_some() {
                history.observed_until = Some(observed_millis());
            }
        }
        history.live_clock = None;
        history
    }

    /// Copies only lifetime/counter metadata at a shared catalog read boundary.
    #[must_use]
    pub fn metadata_snapshot(
        &self,
        live: bool,
        now: std::time::Instant,
        cutoff_millis: u64,
    ) -> Self {
        Self {
            observed: self.observed,
            evicted: self.evicted,
            client_bytes: self.client_bytes,
            server_bytes: self.server_bytes,
            client_bytes_known: self.client_bytes_known,
            server_bytes_known: self.server_bytes_known,
            opened_at: self.opened_at,
            established_at: self.established_at,
            ended_at: self.ended_at,
            observed_until: if live && self.live_clock.is_some() {
                Some(cutoff_millis)
            } else {
                self.observed_until
            },
            elapsed_millis: if live {
                self.lifetime_millis_at(now)
            } else {
                self.elapsed_millis
            },
            last_activity: self.last_activity,
            retained_bytes: self.retained_bytes,
            ..Self::default()
        }
    }

    /// Accounts for message storage, charging a shared persistent ID once.
    pub fn charge_message(&mut self, message: &StreamMessage) {
        let mut bytes = message.retained_bytes();
        if let Some(id) = &message.event_id
            && !self.event_ids.insert(id.as_ptr() as usize)
        {
            bytes = bytes.saturating_sub(id.len());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
    }

    /// Retains every observed item until the owning session is removed.
    pub fn push(&mut self, sequence: u64, message: StreamMessage) {
        self.observed = self.observed.saturating_add(1);
        self.last_activity = Some(message.at_millis);
        if message.kind == "unavailable" || message.kind == "incomplete" {
            // Diagnostic text is not application payload.
        } else if message.sender == "client" {
            self.client_bytes_known = true;
            self.client_bytes = self.client_bytes.saturating_add(message.byte_count);
        } else if message.sender == "server" {
            self.server_bytes_known = true;
            self.server_bytes = self.server_bytes.saturating_add(message.byte_count);
        }
        self.charge_message(&message);
        self.items.push_back(std::sync::Arc::new(StreamItem {
            sequence,
            message,
            source: None,
        }));
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

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    fn state(status: StreamStatus, at_millis: u64) -> StreamState {
        StreamState {
            protocol: StreamProtocol::WebSocket,
            status,
            at_millis,
            reason: None,
        }
    }
    #[test]
    fn byte_evidence_distinguishes_quiet_live_imports_and_each_direction() {
        let mut imported = StreamHistory::default();
        imported.observe_state(&state(StreamStatus::Open, 2000), false);
        assert!(!imported.client_bytes_known && !imported.server_bytes_known);
        imported.push(
            1,
            StreamMessage::new("server", "unavailable", bytes::Bytes::new()),
        );
        assert!(!imported.client_bytes_known && !imported.server_bytes_known);
        imported.push(2, StreamMessage::new("client", "text", bytes::Bytes::new()));
        assert!(imported.client_bytes_known && !imported.server_bytes_known);
        assert_eq!(imported.client_bytes, 0);
        let metadata = imported.metadata_snapshot(false, std::time::Instant::now(), 5000);
        assert!(metadata.client_bytes_known && !metadata.server_bytes_known);
        imported.push(
            3,
            StreamMessage::new("server", "text", b"hello".to_vec().into()),
        );
        assert!(imported.server_bytes_known);
        assert_eq!(imported.server_bytes, 5);
        let mut live = StreamHistory::default();
        live.observe_state(&state(StreamStatus::Open, 2000), true);
        let quiet = live.snapshot(true);
        assert!(quiet.client_bytes_known && quiet.server_bytes_known);
        assert_eq!((quiet.client_bytes, quiet.server_bytes), (0, 0));
    }
    #[test]
    fn actual_open_has_a_monotonic_clock_and_terminal_states_freeze_it() {
        for status in [
            StreamStatus::Closed,
            StreamStatus::Failed,
            StreamStatus::Interrupted,
        ] {
            let mut history = StreamHistory::default();
            history.observe_state(&state(StreamStatus::Connecting, 1000), true);
            assert_eq!(history.established_at, None);
            assert_eq!(history.lifetime_millis(), None);
            history.observe_state(&state(StreamStatus::Open, 2000), true);
            assert_eq!(history.opened_at, Some(1000));
            assert_eq!(history.established_at, Some(2000));
            history.live_clock =
                std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10));
            let quiet = history.snapshot(true);
            assert!(quiet.elapsed_millis.unwrap() >= 10_000);
            assert!(quiet.observed_until.unwrap() > 2000);
            // A wall-clock adjustment does not change the measured live interval.
            history.observe_state(&state(status, 1500), true);
            let frozen = history.lifetime_millis().unwrap();
            assert!(frozen >= 10_000);
            assert_eq!(history.ended_at, Some(1500));
            assert!(history.live_clock.is_none());
            assert_eq!(history.snapshot(true).elapsed_millis, Some(frozen));
        }
    }
    #[test]
    fn imported_timing_requires_explicit_open_and_never_uses_last_message_as_close() {
        let mut old = StreamHistory {
            opened_at: Some(1000),
            last_activity: Some(9000),
            ..Default::default()
        };
        old.observe_state(&state(StreamStatus::Closed, 10_000), false);
        assert_eq!(old.established_at, None);
        assert_eq!(old.lifetime_millis(), None);
        let mut imported = StreamHistory::default();
        imported.observe_state(&state(StreamStatus::Open, 2000), false);
        assert_eq!(imported.elapsed_millis, None);
        assert_eq!(imported.observed_until, None);
        imported.last_activity = Some(5000);
        assert_eq!(imported.ended_at, None);
        imported.observe_state(&state(StreamStatus::Closed, 12_000), false);
        assert_eq!(imported.lifetime_millis(), Some(10_000));
        assert_eq!(imported.ended_at, Some(12_000));
    }
}
