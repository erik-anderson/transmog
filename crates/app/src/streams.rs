//! Bounded metadata pages and independently fetched stream payloads.
use crate::{AppError, Application, ErrorCategory};
use serde::{Deserialize, Serialize};
use transmog_core::{
    intercept::ExchangeId,
    stream::{StreamMessage, StreamState},
};
use transmog_session::SessionSnapshot;

/// Stream state and lifetime counters, without message payloads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSummary {
    /// Latest recorded stream state.
    pub state: StreamState,
    /// True for saved evidence rather than a currently running stream.
    pub imported: bool,
    /// Revision for refreshing the selected stream.
    pub revision: u64,
    /// Observed items, including evicted history.
    pub observed: u64,
    /// Items evicted by retention.
    pub evicted: u64,
    /// Missing observer records, reported separately from deliberate eviction.
    pub sequence_loss: u64,
    /// Retained items available for inspection.
    pub retained: usize,
    /// Total decoded client message bytes.
    pub client_bytes: u64,
    /// Total decoded server message bytes.
    pub server_bytes: u64,
    /// First observed stream timestamp.
    pub opened_at: Option<u64>,
    /// Most recent message timestamp.
    pub last_activity: Option<u64>,
}
impl StreamSummary {
    pub(crate) fn from_snapshot(snapshot: &SessionSnapshot) -> Option<Self> {
        let history = &snapshot.stream_history;
        Some(Self {
            state: snapshot.stream.clone()?,
            imported: snapshot.imported,
            revision: snapshot.last_sequence,
            observed: history.observed,
            evicted: history.evicted,
            sequence_loss: snapshot.sequence_loss,
            retained: history.items.len(),
            client_bytes: history.client_bytes,
            server_bytes: history.server_bytes,
            opened_at: history.opened_at,
            last_activity: history.last_activity,
        })
    }
}
/// A metadata-only window through a stream's retained messages.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamQuery {
    /// Parent traffic entry.
    pub session_id: String,
    /// Offset in the filtered collection.
    #[serde(default)]
    pub offset: usize,
    /// At most 200 rows per response.
    pub limit: usize,
    /// Request the newest rows.
    #[serde(default)]
    pub tail: bool,
    /// Preserve this first visible sequence as history changes.
    pub anchor: Option<u64>,
    /// Optional client/server filter.
    pub sender: Option<String>,
    /// Include ping, pong, close, heartbeat and diagnostic items.
    #[serde(default)]
    pub controls: bool,
    /// Explicit search over retained text, event names and IDs.
    #[serde(default)]
    pub search: String,
}
/// One stable list row; payload and raw event bytes are fetched separately.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRow {
    /// Per-exchange sequence.
    pub sequence: u64,
    /// Content-free message metadata.
    pub message: StreamMessage,
}
/// Window with stable identities and full filtered count.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamPage {
    /// Full matching count.
    pub total: usize,
    /// Actual start, accounting for retention and following latest.
    pub offset: usize,
    /// Metadata rows in observation order.
    pub rows: Vec<StreamRow>,
    /// Latest counters and state.
    pub summary: Option<StreamSummary>,
}
fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, false)
}
impl Application {
    /// Returns bounded metadata without serializing the history's payload bytes.
    /// # Errors
    /// Returns invalid input or an unavailable entry.
    pub fn stream_page(&self, query: &StreamQuery) -> Result<StreamPage, AppError> {
        if query.limit == 0
            || query.limit > 200
            || query.search.len() > 4096
            || query
                .sender
                .as_ref()
                .is_some_and(|s| s != "client" && s != "server")
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "Invalid stream query",
                false,
            ));
        }
        let snapshot = self
            .service
            .catalog()
            .get(ExchangeId(crate::inspector::parse_session_id(
                &query.session_id,
            )?))
            .ok_or_else(|| unavailable("Stream is unavailable or has been removed"))?;
        let search = query.search.to_lowercase();
        let mut matching = Vec::new();
        for item in &snapshot.stream_history.items {
            let message = &item.message;
            if query
                .sender
                .as_ref()
                .is_some_and(|sender| sender != &message.sender)
            {
                continue;
            }
            if !query.controls && !matches!(message.kind.as_str(), "text" | "binary" | "event") {
                continue;
            }
            if !search.is_empty() {
                let message = item.read().map_err(|e| unavailable(&e))?;
                if !String::from_utf8_lossy(&message.payload)
                    .to_lowercase()
                    .contains(&search)
                    && !message
                        .event
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&search)
                    && !message
                        .event_id
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&search)
                {
                    continue;
                }
            }
            matching.push(item);
        }
        let total = matching.len();
        let offset = if query.tail {
            total.saturating_sub(query.limit)
        } else {
            query
                .anchor
                .and_then(|anchor| matching.iter().position(|item| item.sequence >= anchor))
                .unwrap_or(query.offset)
                .min(total.saturating_sub(1))
        };
        let rows = matching
            .into_iter()
            .skip(offset)
            .take(query.limit)
            .map(|item| {
                let mut message = item.message.clone();
                message.payload.clear();
                message.raw = None;
                StreamRow {
                    sequence: item.sequence,
                    message,
                }
            })
            .collect();
        Ok(StreamPage {
            total,
            offset,
            rows,
            summary: StreamSummary::from_snapshot(&snapshot),
        })
    }
    /// Reads one retained payload, pinning imported source files during the read.
    /// # Errors
    /// Returns an unavailable/evicted item or corrupt capture error.
    pub fn stream_message(&self, id: &str, sequence: u64) -> Result<StreamMessage, AppError> {
        let snapshot = self
            .service
            .catalog()
            .get(ExchangeId(crate::inspector::parse_session_id(id)?))
            .ok_or_else(|| unavailable("Stream is unavailable"))?;
        snapshot
            .stream_history
            .items
            .iter()
            .find(|item| item.sequence == sequence)
            .ok_or_else(|| unavailable("This message has been evicted from retained history"))?
            .read()
            .map_err(|e| unavailable(&e))
    }
}

/// Atomically saves already retained message bytes chosen before opening a file dialog.
/// # Errors
/// Returns a destination or filesystem error, preserving an existing file on failure.
pub fn save_stream_content(destination: &std::path::Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    if !destination.is_absolute() {
        return Err(unavailable("Choose an absolute message destination"));
    }
    let mut output = tempfile::NamedTempFile::new_in(
        destination
            .parent()
            .ok_or_else(|| unavailable("Message destination is unavailable"))?,
    )
    .map_err(|e| unavailable(&e.to_string()))?;
    output
        .write_all(bytes)
        .and_then(|()| output.as_file().sync_all())
        .map_err(|e| unavailable(&e.to_string()))?;
    output
        .persist(destination)
        .map_err(|e| unavailable(&e.to_string()))?;
    Ok(())
}
