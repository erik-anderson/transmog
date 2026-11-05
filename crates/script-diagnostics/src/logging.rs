//! Bounded snapshots of native console output. All content remains untrusted.

use crate::{ExceptionDiagnostic, ExceptionValue, StackFrame};
use serde::{Deserialize, Serialize};

/// Maximum accepted records per initializer or callback.
pub const MAX_LOG_RECORDS: usize = 128;
/// Maximum text bytes across one initializer or callback.
pub const MAX_LOG_BYTES: usize = 16 * 1024;
/// Maximum text bytes, including frame names, per record.
pub const MAX_LOG_RECORD_BYTES: usize = 2048;
/// Maximum native frames on a trace record.
pub const MAX_LOG_FRAMES: usize = 8;
/// Maximum retained trace frames across one complete operation.
pub const MAX_LOG_TOTAL_FRAMES: usize = 32;

/// Native console method, without browser-specific formatting behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// console.log and console.info.
    Info,
    /// console.debug.
    Debug,
    /// console.warn.
    Warn,
    /// console.error.
    Error,
    /// console.trace, with native call frames.
    Trace,
}

/// An inert snapshot; ordinary logs never retain live engine objects.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogRecord {
    /// Monotonic call sequence within one operation, including omitted calls.
    pub sequence: u64,
    /// Host wall-clock milliseconds since Unix epoch.
    pub time_unix_ms: u64,
    /// Console level.
    pub level: LogLevel,
    /// Space-separated bounded primitive values and inert object labels.
    pub text: String,
    /// Engine-owned trace frames; empty for ordinary messages.
    pub frames: Vec<StackFrame>,
    /// Some text, arguments or frames were omitted.
    pub truncated: bool,
}

impl LogRecord {
    /// Total retained UTF-8 evidence bytes, including source-map names.
    pub fn text_bytes(&self) -> usize {
        self.frames.iter().fold(self.text.len(), |bytes, frame| {
            bytes.saturating_add(frame.function.len()).saturating_add(
                [&frame.generated, &frame.original]
                    .into_iter()
                    .flatten()
                    .map(|location| location.source.len())
                    .sum::<usize>(),
            )
        })
    }

    /// Checks an untrusted worker record before mapping, cloning or retaining it.
    pub fn is_bounded(&self) -> bool {
        self.sequence > 0
            && self.frames.len() <= MAX_LOG_FRAMES
            && self.text_bytes() <= MAX_LOG_RECORD_BYTES
            && self.frames.iter().all(|frame| {
                [&frame.generated, &frame.original]
                    .into_iter()
                    .flatten()
                    .all(|location| location.line > 0 && location.column > 0)
            })
    }

    /// Reuses the exception mapper without parsing authored stack text.
    pub fn map_frames(&mut self, map: impl FnOnce(ExceptionDiagnostic) -> ExceptionDiagnostic) {
        let snapshot = ExceptionDiagnostic {
            artifact: None,
            exception: ExceptionValue {
                name: None,
                message: String::new(),
                frames: std::mem::take(&mut self.frames),
                unavailable: false,
            },
            causes: vec![],
            truncated: false,
        };
        self.frames = map(snapshot).exception.frames;
        if self.text_bytes() > MAX_LOG_RECORD_BYTES {
            for frame in &mut self.frames {
                if frame.original.take().is_some() {
                    frame.mapping_issue = Some(crate::MappingIssue::Budget);
                }
            }
            self.truncated = true;
        }
    }
}

/// Attribution bound by the native supervisor to the exact executing artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogContext {
    /// Native immutable source-cache identity, independent of display attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// Public identity of this isolated worker instance, separate from its secret IPC token.
    #[serde(default)]
    pub worker: String,
    /// Immutable script build hash or extension package digest.
    pub revision: String,
    /// Initializer or declared callback name.
    pub callback: String,
    /// Worker-local operation identity; zero denotes initialization.
    pub invocation: u64,
    /// Native exchange identity where applicable.
    pub exchange_id: Option<String>,
}

/// Supervisor-accepted log records for one bounded operation.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogRun {
    /// Native attribution; JavaScript cannot supply this metadata.
    pub context: LogContext,
    /// Accepted records in call order.
    pub records: Vec<LogRecord>,
    /// Known messages omitted by the worker or supervisor.
    pub dropped: u64,
    /// No authenticated stream-end arrived before worker loss or cancellation.
    pub incomplete: bool,
}
impl std::fmt::Debug for LogRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRun")
            .field("invocation", &self.context.invocation)
            .field("record_count", &self.records.len())
            .field("dropped", &self.dropped)
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

impl LogRun {
    /// Opens an initially incomplete log stream for one native operation.
    pub fn new(context: LogContext) -> Self {
        Self {
            context,
            records: vec![],
            dropped: 0,
            incomplete: true,
        }
    }
    /// Accepts an already validated, ordered record under the aggregate bound.
    /// Overflow never invalidates the callback result.
    pub fn push(&mut self, mut record: LogRecord) {
        let remaining = MAX_LOG_TOTAL_FRAMES
            .saturating_sub(self.records.iter().map(|record| record.frames.len()).sum());
        if record.frames.len() > remaining {
            record.frames.truncate(remaining);
            record.frames.shrink_to_fit();
            record.truncated = true;
        }
        if self.records.len() >= MAX_LOG_RECORDS
            || self.text_bytes().saturating_add(record.text_bytes()) > MAX_LOG_BYTES
        {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.records.push(record);
        }
    }
    /// Retained text, excluding the small native attribution header.
    pub fn text_bytes(&self) -> usize {
        self.records.iter().map(LogRecord::text_bytes).sum()
    }
    /// Checks retained evidence at projection and storage boundaries.
    pub fn is_bounded(&self) -> bool {
        self.context
            .artifact
            .as_ref()
            .is_none_or(|id| id.len() <= 512)
            && self.context.worker.len() <= 128
            && self.context.revision.len() <= 512
            && self.context.callback.len() <= 64
            && self
                .context
                .exchange_id
                .as_ref()
                .is_none_or(|id| id.len() <= 128)
            && self.records.len() <= MAX_LOG_RECORDS
            && self.text_bytes() <= MAX_LOG_BYTES
            && self.records.iter().all(LogRecord::is_bounded)
            && self
                .records
                .iter()
                .map(|record| record.frames.len())
                .sum::<usize>()
                <= MAX_LOG_TOTAL_FRAMES
            && self
                .records
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
    }
    /// Whether a local author has evidence or a stream-loss marker to inspect.
    pub fn has_output(&self) -> bool {
        !self.records.is_empty() || self.dropped > 0 || self.incomplete
    }
}

/// Worker-to-supervisor stream event; attribution remains supervisor-owned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ConsoleEvent {
    /// One accepted native snapshot.
    Record(LogRecord),
    /// End marker, including messages omitted before transport acceptance.
    End {
        /// Cumulative omitted call count.
        dropped: u64,
    },
}

/// Thread-safe capture shared by the pipe reader and independent timeout owner.
/// It retains only supervisor-accepted records, even when the worker never replies.
pub struct LogCapture {
    state: std::sync::Mutex<CaptureState>,
}
// A capture contains at most 48 KiB including native record/vector overhead.
// Shared weak ownership lets retained worker histories remain globally evictable.
static CAPTURES: std::sync::Mutex<std::collections::VecDeque<std::sync::Weak<LogCapture>>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
const MAX_RETAINED_CAPTURES: usize = 256; // 12 MiB across all traffic/visualizer workers.
struct CaptureState {
    run: LogRun,
    last_sequence: u64,
    sequence_gaps: u64,
    local_dropped: u64,
    retention_dropped: u64,
    registered: bool,
    retain_records: bool,
}
impl LogCapture {
    /// Opens a finite stream with native attribution.
    pub fn new(context: LogContext) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(CaptureState {
                run: LogRun::new(context),
                last_sequence: 0,
                sequence_gaps: 0,
                local_dropped: 0,
                retention_dropped: 0,
                registered: false,
                retain_records: true,
            }),
        })
    }
    /// Accepts an authenticated event and maps only its engine-owned frames.
    /// # Errors
    /// Rejects malformed, unordered or post-end records.
    pub fn accept(
        self: &std::sync::Arc<Self>,
        event: ConsoleEvent,
        map: impl FnOnce(&mut LogRecord),
    ) -> Result<(), &'static str> {
        if let ConsoleEvent::Record(record) = &event {
            {
                let state = self.state.lock().map_err(|_| "Log capture unavailable")?;
                if !state.run.incomplete
                    || !record.is_bounded()
                    || record.sequence <= state.last_sequence
                {
                    return Err("Invalid log record");
                }
            }
            self.register();
        }
        let mut state = self.state.lock().map_err(|_| "Log capture unavailable")?;
        if !state.run.incomplete {
            return Err("Log event followed stream end");
        }
        match event {
            ConsoleEvent::Record(mut record) => {
                if !record.is_bounded() || record.sequence <= state.last_sequence {
                    return Err("Invalid log record");
                }
                state.sequence_gaps = state
                    .sequence_gaps
                    .saturating_add(record.sequence - state.last_sequence - 1);
                state.last_sequence = record.sequence;
                if !state.retain_records {
                    state.local_dropped = state.local_dropped.saturating_add(1);
                    state.run.dropped = state
                        .local_dropped
                        .saturating_add(state.sequence_gaps)
                        .saturating_add(state.retention_dropped);
                    return Ok(());
                }
                map(&mut record);
                if !record.is_bounded() {
                    return Err("Invalid mapped log record");
                }
                let count = state.run.records.len();
                state.run.push(record);
                if state.run.records.len() == count {
                    state.local_dropped = state.local_dropped.saturating_add(1);
                }
                state.run.dropped = state
                    .local_dropped
                    .saturating_add(state.sequence_gaps)
                    .saturating_add(state.retention_dropped);
            }
            ConsoleEvent::End { dropped } => {
                if dropped < state.sequence_gaps {
                    return Err("Invalid log loss counter");
                }
                state.run.dropped = state
                    .local_dropped
                    .saturating_add(dropped)
                    .saturating_add(state.retention_dropped);
                state.run.incomplete = false;
            }
        }
        Ok(())
    }
    fn register(self: &std::sync::Arc<Self>) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.registered {
                return;
            }
            state.registered = true;
        }
        let mut pool = CAPTURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pool.retain(|entry| {
            let Some(capture) = entry.upgrade() else {
                return false;
            };
            let Ok(state) = capture.state.try_lock() else {
                return true;
            };
            state.run.incomplete || !state.run.records.is_empty()
        });
        while pool.len() >= MAX_RETAINED_CAPTURES {
            if let Some(previous) = pool.pop_front().and_then(|previous| previous.upgrade()) {
                let mut state = previous
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let removed = u64::try_from(state.run.records.len()).unwrap_or(u64::MAX);
                state.retention_dropped = state.retention_dropped.saturating_add(removed);
                state.run.records.clear();
                state.run.records.shrink_to_fit();
                state.retain_records = false;
                state.run.dropped = state.run.dropped.saturating_add(removed);
            }
        }
        pool.push_back(std::sync::Arc::downgrade(self));
    }
    /// Copies bounded accepted evidence without waiting for a callback result.
    pub fn snapshot(&self) -> Option<LogRun> {
        let state = self.state.lock().ok()?;
        state.run.has_output().then(|| state.run.clone())
    }
    /// Native attribution remains available even when the callback emits no output.
    pub fn context(&self) -> LogContext {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .run
            .context
            .clone()
    }
    /// Whether an authenticated stream-end has arrived.
    pub fn finished(&self) -> bool {
        self.state.lock().is_ok_and(|state| !state.run.incomplete)
    }
    /// Commits a validated ordinary result as the end of a completely silent stream.
    /// A stream that delivered any record still requires its explicit ordered end.
    pub fn complete_or_silent(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if !state.run.incomplete {
            return true;
        }
        if state.last_sequence != 0 || state.run.dropped != 0 {
            return false;
        }
        state.run.incomplete = false;
        true
    }
    /// Clears already retained output without changing the operation identity.
    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.run.records.clear();
            state.run.dropped = 0;
            state.local_dropped = 0;
            state.sequence_gaps = 0;
            state.retention_dropped = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static CAPTURE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[test]
    fn only_a_completely_silent_stream_can_complete_at_the_result_barrier() {
        let context = LogContext {
            artifact: None,
            worker: "worker".into(),
            revision: "revision".into(),
            callback: "callback".into(),
            invocation: 1,
            exchange_id: None,
        };
        let silent = LogCapture::new(context.clone());
        assert!(silent.complete_or_silent());
        assert!(silent.finished());
        assert_eq!(silent.snapshot(), None);
        let spoken = LogCapture::new(context);
        spoken
            .accept(
                ConsoleEvent::Record(LogRecord {
                    sequence: 1,
                    time_unix_ms: 0,
                    level: LogLevel::Info,
                    text: "accepted".into(),
                    frames: vec![],
                    truncated: false,
                }),
                |_| {},
            )
            .unwrap();
        assert!(!spoken.complete_or_silent());
        spoken.clear();
        assert!(!spoken.complete_or_silent());
        spoken
            .accept(ConsoleEvent::End { dropped: 0 }, |_| {})
            .unwrap();
        assert!(spoken.complete_or_silent());
    }
    #[test]
    fn global_capture_eviction_releases_records_and_keeps_loss_and_completion_truthful() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        let mut captures = Vec::new();
        for invocation in 0..=MAX_RETAINED_CAPTURES + 1 {
            let capture = LogCapture::new(LogContext {
                artifact: None,
                worker: String::new(),
                revision: "build".into(),
                callback: "callback".into(),
                invocation: u64::try_from(invocation).unwrap(),
                exchange_id: None,
            });
            capture
                .accept(
                    ConsoleEvent::Record(LogRecord {
                        sequence: 1,
                        time_unix_ms: 0,
                        level: LogLevel::Info,
                        text: "accepted".into(),
                        frames: vec![],
                        truncated: false,
                    }),
                    |_| {},
                )
                .unwrap();
            if invocation == 0 {
                capture
                    .accept(ConsoleEvent::End { dropped: 7 }, |_| {})
                    .unwrap();
            }
            captures.push(capture);
        }
        let ended = captures[0].snapshot().unwrap();
        assert_eq!(ended.records, Vec::new());
        assert_eq!(ended.dropped, 8);
        assert!(!ended.incomplete);
        let active = &captures[1];
        active
            .accept(
                ConsoleEvent::Record(LogRecord {
                    sequence: 2,
                    time_unix_ms: 0,
                    level: LogLevel::Info,
                    text: "later".into(),
                    frames: vec![],
                    truncated: false,
                }),
                |_| panic!("evicted capture must not map/retain more values"),
            )
            .unwrap();
        active
            .accept(ConsoleEvent::End { dropped: 0 }, |_| {})
            .unwrap();
        assert_eq!(active.snapshot().unwrap().records, Vec::new());
        assert_eq!(active.snapshot().unwrap().dropped, 2);
        assert_eq!(
            captures.last().unwrap().snapshot().unwrap().records[0].text,
            "accepted"
        );
        assert!(CAPTURES.lock().unwrap().len() <= MAX_RETAINED_CAPTURES);
    }
    #[test]
    fn trace_metadata_has_an_aggregate_frame_bound_even_without_text() {
        let mut run = LogRun::new(LogContext {
            artifact: None,
            worker: String::new(),
            revision: "build".into(),
            callback: "callback".into(),
            invocation: 1,
            exchange_id: None,
        });
        for sequence in 1..=128 {
            run.push(LogRecord {
                sequence,
                time_unix_ms: 0,
                level: LogLevel::Trace,
                text: String::new(),
                frames: vec![
                    StackFrame {
                        function: String::new(),
                        generated: None,
                        original: None,
                        mapping_issue: None
                    };
                    8
                ],
                truncated: false,
            });
        }
        assert_eq!(run.records.len(), 128);
        assert_eq!(
            run.records
                .iter()
                .map(|record| record.frames.len())
                .sum::<usize>(),
            MAX_LOG_TOTAL_FRAMES
        );
        assert!(run.records[4].truncated);
        assert!(run.is_bounded());
    }
    #[test]
    fn capture_rejects_stale_ordering_and_records_after_end() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        let capture = LogCapture::new(LogContext {
            artifact: None,
            worker: String::new(),
            revision: "build".into(),
            callback: "callback".into(),
            invocation: 7,
            exchange_id: None,
        });
        let record = LogRecord {
            sequence: 1,
            time_unix_ms: 0,
            level: LogLevel::Info,
            text: "accepted".into(),
            frames: vec![],
            truncated: false,
        };
        assert!(
            capture
                .accept(ConsoleEvent::Record(record.clone()), |_| {})
                .is_ok()
        );
        assert!(
            capture
                .accept(ConsoleEvent::Record(record.clone()), |_| {})
                .is_err()
        );
        assert!(capture.snapshot().unwrap().incomplete);
        let mut after_gap = record.clone();
        after_gap.sequence = 3;
        assert!(
            capture
                .accept(ConsoleEvent::Record(after_gap), |_| {})
                .is_ok()
        );
        assert_eq!(capture.snapshot().unwrap().dropped, 1);
        assert!(
            capture
                .accept(ConsoleEvent::End { dropped: 0 }, |_| {})
                .is_err()
        );
        assert!(
            capture
                .accept(ConsoleEvent::End { dropped: 3 }, |_| {})
                .is_ok()
        );
        assert!(!capture.snapshot().unwrap().incomplete);
        assert_eq!(capture.snapshot().unwrap().dropped, 3);
        assert!(
            capture
                .accept(ConsoleEvent::Record(record), |_| {})
                .is_err()
        );
        assert!(!format!("{:?}", capture.snapshot().unwrap()).contains("accepted"));
    }
    #[test]
    fn overflow_retains_prefix_and_counts_loss_without_invalidating_output() {
        let mut run = LogRun::new(LogContext {
            artifact: None,
            worker: String::new(),
            revision: "build".into(),
            callback: "initializer".into(),
            invocation: 0,
            exchange_id: None,
        });
        for sequence in 1..=200 {
            run.push(LogRecord {
                sequence,
                time_unix_ms: 0,
                level: LogLevel::Info,
                text: "x".repeat(2048),
                frames: vec![],
                truncated: false,
            });
        }
        assert_eq!(run.records.len(), 8);
        assert_eq!(run.dropped, 192);
        assert!(run.is_bounded());
        run.records[1].sequence = 1;
        assert!(!run.is_bounded());
    }
}
