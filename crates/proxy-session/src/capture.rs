use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use thiserror::Error;
use transmog_capture::{
    CaptureBodyRetention, CaptureLimits, CapturePolicy, CaptureWriter, loss_record,
    record_from_observer,
};
use transmog_core::observe::ObserverEvent;

/// Settings for a new native capture artifact.
#[derive(Clone, Debug)]
pub struct CaptureStart {
    /// Independent compression and optional password-derived encryption.
    pub encoding: transmog_capture::CaptureEncoding,
    /// New destination path. Existing files are never overwritten.
    pub path: PathBuf,
    /// Finite artifact, record, and recovery bounds.
    pub limits: CaptureLimits,
    /// Redaction and body-sample retention policy.
    pub policy: CapturePolicy,
    /// Original capture-level context, collected before recording starts.
    pub metadata: Option<serde_json::Value>,
}

/// Successfully sealed artifact metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedCapture {
    /// Artifact path.
    pub path: PathBuf,
    /// Bytes written to the operating system, including the seal.
    pub bytes_written: u64,
}

/// Persistent, operator-safe capture failure state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFailure {
    /// Artifact path, when a capture had already started.
    pub path: Option<PathBuf>,
    /// Redacted failure description.
    pub message: String,
}

/// Current dynamic capture state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureStatus {
    /// No capture has started since construction or reset.
    Idle,
    /// Events are being appended to this artifact.
    Active {
        /// Artifact path.
        path: PathBuf,
        /// Bytes published by the most recent checkpoint.
        bytes_written: u64,
    },
    /// Capture stopped cleanly with a final seal.
    Sealed(SealedCapture),
    /// Capture stopped accepting events after a visible failure.
    Failed(CaptureFailure),
    /// The worker has shut down.
    Shutdown,
}

/// Typed dynamic-capture command failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CaptureServiceError {
    /// A capture is already active.
    #[error("a capture is already active")]
    AlreadyActive,
    /// No capture is active.
    #[error("no capture is active")]
    NotActive,
    /// Destination already exists and will not be overwritten.
    #[error("capture destination already exists")]
    DestinationExists,
    /// The bounded capture queue saturated.
    #[error("capture event queue saturated")]
    QueueSaturated,
    /// Capture worker is shutting down or unavailable.
    #[error("capture worker is unavailable")]
    WorkerUnavailable,
    /// Capture writer failed. The text is safe for operator diagnostics.
    #[error("capture writer failed: {0}")]
    Writer(String),
}

#[derive(Debug)]
enum Command {
    Start(CaptureStart, mpsc::Sender<Result<(), CaptureServiceError>>),
    Mark {
        note: String,
        origin: transmog_capture::support::MarkerOrigin,
        at: Instant,
        unix_millis: u64,
        reply: mpsc::Sender<Result<transmog_capture::support::IncidentMarker, CaptureServiceError>>,
    },
    Record(ObserverEvent, Option<tokio::sync::OwnedSemaphorePermit>),
    Stop(mpsc::Sender<Result<SealedCapture, CaptureServiceError>>),
    Shutdown(mpsc::Sender<()>),
}

struct ActiveCapture {
    run_id: String,
    began: Instant,
    markers: usize,
    path: PathBuf,
    writer: CaptureWriter<File>,
    policy: CapturePolicy,
    retention: CaptureBodyRetention,
    last_sequences: HashMap<u128, u64>,
    // Value records whether the HTTP handshake has completed. Its terminal
    // event and the stream terminal event can arrive in either order.
    open_websockets: HashMap<u128, bool>,
}

struct CaptureInner {
    sender: mpsc::SyncSender<Command>,
    byte_budget: Arc<tokio::sync::Semaphore>,
    status: Arc<Mutex<CaptureStatus>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for CaptureInner {
    fn drop(&mut self) {
        // Dropping the sender disconnects the worker even if its finite queue is
        // currently full. Joining guarantees no file handle outlives the owner.
        let (placeholder, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let sender = std::mem::replace(&mut self.sender, placeholder);
        drop(sender);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// Cloneable controller for one bounded dynamic-capture worker.
#[derive(Clone)]
pub struct CaptureManager {
    inner: Arc<CaptureInner>,
}

impl std::fmt::Debug for CaptureManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaptureManager")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl CaptureManager {
    /// Starts a dedicated capture writer with a finite event queue.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureServiceError::WorkerUnavailable`] if the operating
    /// system cannot create the dedicated worker thread.
    pub fn new(queue_capacity: NonZeroUsize) -> Result<Self, CaptureServiceError> {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.get());
        let status = Arc::new(Mutex::new(CaptureStatus::Idle));
        let inner = Arc::new(CaptureInner {
            sender,
            byte_budget: Arc::new(tokio::sync::Semaphore::new(32 * 1024 * 1024)),
            status: Arc::clone(&status),
            worker: Mutex::new(None),
        });
        let worker = std::thread::Builder::new()
            .name("transmog-capture".into())
            .spawn(move || capture_worker(&receiver, &status))
            .map_err(|_| CaptureServiceError::WorkerUnavailable)?;
        *inner
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(worker);
        Ok(Self { inner })
    }

    /// Returns the latest worker-published status.
    pub fn status(&self) -> CaptureStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Creates a new capture without ever overwriting an existing path.
    ///
    /// # Errors
    ///
    /// Returns a typed state, destination, worker, or writer failure.
    pub async fn start(&self, request: CaptureStart) -> Result<(), CaptureServiceError> {
        let sender = self.inner.sender.clone();
        run_command(move || {
            let (reply, response) = mpsc::channel();
            sender
                .send(Command::Start(request, reply))
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?;
            response
                .recv()
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?
        })
        .await
    }

    /// Records a user incident through the bounded writer queue, then checkpoints it.
    /// # Errors
    /// Returns invalid input, marker/queue bounds, inactive/sealed state or writer failure.
    pub async fn mark(
        &self,
        note: String,
        origin: transmog_capture::support::MarkerOrigin,
    ) -> Result<transmog_capture::support::IncidentMarker, CaptureServiceError> {
        transmog_capture::support::text(
            "Incident note",
            &note,
            transmog_capture::support::MAX_NOTE_BYTES,
            false,
        )
        .map_err(|e| CaptureServiceError::Writer(e.to_string()))?;
        if !matches!(self.status(), CaptureStatus::Active { .. }) {
            return Err(CaptureServiceError::NotActive);
        }
        let sender = self.inner.sender.clone();
        let at = Instant::now();
        let unix_millis = transmog_capture::support::unix_millis();
        run_command(move || {
            let (reply, response) = mpsc::channel();
            sender
                .try_send(Command::Mark {
                    note,
                    origin,
                    at,
                    unix_millis,
                    reply,
                })
                .map_err(|e| match e {
                    mpsc::TrySendError::Full(_) => CaptureServiceError::QueueSaturated,
                    mpsc::TrySendError::Disconnected(_) => CaptureServiceError::WorkerUnavailable,
                })?;
            response
                .recv()
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?
        })
        .await
    }

    /// Seals and flushes the active artifact.
    ///
    /// # Errors
    ///
    /// Returns a typed state, worker, or writer failure.
    pub async fn stop(&self) -> Result<SealedCapture, CaptureServiceError> {
        let sender = self.inner.sender.clone();
        run_command(move || {
            let (reply, response) = mpsc::channel();
            sender
                .send(Command::Stop(reply))
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?;
            response
                .recv()
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?
        })
        .await
    }

    /// Stops the worker after sealing any healthy active capture.
    ///
    /// This operation is idempotent.
    pub async fn shutdown(&self) {
        if matches!(self.status(), CaptureStatus::Shutdown) {
            return;
        }
        let sender = self.inner.sender.clone();
        let _ = run_command(move || {
            let (reply, response) = mpsc::channel();
            sender
                .send(Command::Shutdown(reply))
                .map_err(|_| CaptureServiceError::WorkerUnavailable)?;
            response
                .recv()
                .map_err(|_| CaptureServiceError::WorkerUnavailable)
        })
        .await;
    }

    /// Enqueues one observer event, waiting for the bounded writer queue when
    /// it is full. The wait runs off the async executor; saturation is flow
    /// control rather than a failed capture or a missing record.
    pub async fn record(&self, event: ObserverEvent) {
        if !matches!(self.status(), CaptureStatus::Active { .. }) {
            return;
        }
        let charge =
            u32::try_from(event.retained_bytes().min(32 * 1024 * 1024)).unwrap_or(32 * 1024 * 1024);
        let Ok(permit) = self
            .inner
            .byte_budget
            .clone()
            .acquire_many_owned(charge)
            .await
        else {
            return;
        };
        match self
            .inner
            .sender
            .try_send(Command::Record(event, Some(permit)))
        {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(command)) => {
                let sender = self.inner.sender.clone();
                if run_command(move || {
                    sender
                        .send(command)
                        .map_err(|_| CaptureServiceError::WorkerUnavailable)
                })
                .await
                .is_err()
                {
                    set_failure(&self.inner.status, "capture worker disconnected");
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                set_failure(&self.inner.status, "capture worker disconnected");
            }
        }
    }
}

async fn run_command<T: Send + 'static>(
    command: impl FnOnce() -> Result<T, CaptureServiceError> + Send + 'static,
) -> Result<T, CaptureServiceError> {
    tokio::task::spawn_blocking(command)
        .await
        .map_err(|_| CaptureServiceError::WorkerUnavailable)?
}

fn capture_worker(receiver: &mpsc::Receiver<Command>, status: &Mutex<CaptureStatus>) {
    let mut active: Option<ActiveCapture> = None;
    let mut last_checkpoint = Instant::now();
    loop {
        if last_checkpoint.elapsed() >= Duration::from_secs(1) {
            if let Some(capture) = active.as_mut()
                && matches!(current_status(status), CaptureStatus::Active { .. })
            {
                match capture.writer.checkpoint() {
                    Ok(()) => set_status(
                        status,
                        CaptureStatus::Active {
                            path: capture.path.clone(),
                            bytes_written: capture.writer.bytes_written(),
                        },
                    ),
                    Err(error) => set_failure(status, error.to_string()),
                }
            }
            last_checkpoint = Instant::now();
        }
        let command = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            Command::Start(request, reply) => {
                let result = start_capture(&mut active, status, request);
                let _ = reply.send(result);
            }
            Command::Record(event, _permit) => {
                if matches!(current_status(status), CaptureStatus::Failed(_)) {
                    continue;
                }
                if let Some(capture) = active.as_mut()
                    && let Err(error) = append_event(capture, &event)
                {
                    set_failure(status, error.to_string());
                } else if let Some(capture) = active.as_ref() {
                    set_status(
                        status,
                        CaptureStatus::Active {
                            path: capture.path.clone(),
                            bytes_written: capture.writer.bytes_written(),
                        },
                    );
                }
            }
            Command::Mark {
                note,
                origin,
                at,
                unix_millis,
                reply,
            } => {
                let result = if matches!(current_status(status), CaptureStatus::Active { .. }) {
                    active
                        .as_mut()
                        .ok_or(CaptureServiceError::NotActive)
                        .and_then(|capture| {
                            mark_capture(capture, note, origin, at, unix_millis, status)
                        })
                } else {
                    Err(CaptureServiceError::NotActive)
                };
                let _ = reply.send(result);
            }
            Command::Stop(reply) => {
                let result = stop_capture(&mut active, status);
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                if active.is_some() {
                    let _ = stop_capture(&mut active, status);
                }
                set_status(status, CaptureStatus::Shutdown);
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn start_capture(
    active: &mut Option<ActiveCapture>,
    status: &Mutex<CaptureStatus>,
    mut request: CaptureStart,
) -> Result<(), CaptureServiceError> {
    if active.is_some() {
        return Err(CaptureServiceError::AlreadyActive);
    }
    if matches!(current_status(status), CaptureStatus::Shutdown) {
        return Err(CaptureServiceError::WorkerUnavailable);
    }
    let run_id = transmog_capture::support::new_id()
        .map_err(|e| CaptureServiceError::Writer(e.to_string()))?;
    if let Some(metadata) = request.metadata.as_mut() {
        if !metadata.is_object() {
            return Err(CaptureServiceError::Writer(
                "Capture context must be an object".into(),
            ));
        }
        metadata["captureRunId"] = run_id.clone().into();
    }
    if request.metadata.as_ref().is_some_and(|value| {
        serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 4 * 1024 * 1024)
    }) {
        return Err(CaptureServiceError::Writer(
            "Trace context exceeds its finite limit".into(),
        ));
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request.path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                CaptureServiceError::DestinationExists
            } else {
                CaptureServiceError::Writer(error.to_string())
            }
        })?;
    let writer = match prepare_writer(file, request.limits, request.metadata, &request.encoding) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = std::fs::remove_file(&request.path);
            return Err(error);
        }
    };
    set_status(
        status,
        CaptureStatus::Active {
            path: request.path.clone(),
            bytes_written: writer.bytes_written(),
        },
    );
    *active = Some(ActiveCapture {
        run_id,
        began: Instant::now(),
        markers: 0,
        path: request.path,
        writer,
        policy: request.policy,
        retention: CaptureBodyRetention::default(),
        last_sequences: HashMap::new(),
        open_websockets: HashMap::new(),
    });
    Ok(())
}

fn mark_capture(
    capture: &mut ActiveCapture,
    note: String,
    origin: transmog_capture::support::MarkerOrigin,
    at: Instant,
    unix_millis: u64,
    status: &Mutex<CaptureStatus>,
) -> Result<transmog_capture::support::IncidentMarker, CaptureServiceError> {
    if capture.markers >= transmog_capture::support::MAX_MARKERS {
        return Err(CaptureServiceError::Writer(
            "Incident marker limit reached (256); no marker was written".into(),
        ));
    }
    let mut marker = transmog_capture::support::IncidentMarker::new(
        capture.run_id.clone(),
        Some(
            u64::try_from(at.saturating_duration_since(capture.began).as_millis())
                .unwrap_or(u64::MAX),
        ),
        origin,
        note,
    )
    .map_err(|e| CaptureServiceError::Writer(e.to_string()))?;
    marker.unix_millis = unix_millis;
    let record = marker
        .record()
        .map_err(|e| CaptureServiceError::Writer(e.to_string()))?;
    if let Err(e) = capture
        .writer
        .append(&record)
        .and_then(|()| capture.writer.checkpoint())
    {
        set_failure(status, e.to_string());
        return Err(CaptureServiceError::Writer(e.to_string()));
    }
    capture.markers += 1;
    set_status(
        status,
        CaptureStatus::Active {
            path: capture.path.clone(),
            bytes_written: capture.writer.bytes_written(),
        },
    );
    Ok(marker)
}

fn prepare_writer(
    file: File,
    limits: CaptureLimits,
    metadata: Option<serde_json::Value>,
    encoding: &transmog_capture::CaptureEncoding,
) -> Result<CaptureWriter<File>, CaptureServiceError> {
    let mut writer = CaptureWriter::with_encoding(file, limits, encoding)
        .map_err(|error| CaptureServiceError::Writer(error.to_string()))?;
    writer.enable_checkpoint_locators();
    if let Some(metadata) = metadata {
        writer
            .append(&transmog_capture::CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: transmog_capture::CaptureRecordKind::Unknown {
                    kind: "trace-metadata".into(),
                    payload: metadata,
                },
            })
            .map_err(|error| CaptureServiceError::Writer(error.to_string()))?;
    }
    writer
        .checkpoint()
        .map_err(|error| CaptureServiceError::Writer(error.to_string()))?;
    Ok(writer)
}

fn stop_capture(
    active: &mut Option<ActiveCapture>,
    status: &Mutex<CaptureStatus>,
) -> Result<SealedCapture, CaptureServiceError> {
    let Some(mut capture) = active.take() else {
        return Err(CaptureServiceError::NotActive);
    };
    if let Err(error) = capture.writer.seal() {
        let message = error.to_string();
        set_failure(status, &message);
        return Err(CaptureServiceError::Writer(message));
    }
    let mut bytes_written = capture.writer.bytes_written();
    let compact = capture.writer.compact_sealed_bytes();
    drop(capture.writer);
    // Compaction is optional. A failed replacement leaves the already sealed
    // original intact; no password/nonce context is reused for the replacement.
    if let Ok(Some(bytes)) = compact {
        let parent = capture
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        if let Ok(mut temporary) = tempfile::NamedTempFile::new_in(parent)
            && temporary.write_all(&bytes).is_ok()
            && temporary.persist(&capture.path).is_ok()
        {
            bytes_written = bytes.len() as u64;
        }
    }
    let sealed = SealedCapture {
        path: capture.path,
        bytes_written,
    };
    if matches!(current_status(status), CaptureStatus::Failed(_)) {
        return Err(CaptureServiceError::Writer(
            "capture stopped after an earlier writer failure".into(),
        ));
    }
    set_status(status, CaptureStatus::Sealed(sealed.clone()));
    Ok(sealed)
}

fn append_event(
    capture: &mut ActiveCapture,
    event: &ObserverEvent,
) -> Result<(), transmog_capture::CaptureError> {
    let exchange_id = event.exchange_id.0;
    let previous = capture
        .last_sequences
        .get(&exchange_id)
        .copied()
        .unwrap_or(0);
    if event.sequence > previous.saturating_add(1) {
        let first_missing = previous.saturating_add(1);
        capture.writer.append(&loss_record(
            exchange_id,
            first_missing,
            event.sequence.saturating_sub(first_missing),
            "observer-delivery-gap",
        ))?;
    } else if event.sequence <= previous {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture observer sequence moved backward",
        )
        .into());
    }
    if let Some(mut record) = record_from_observer(event, &capture.policy) {
        capture.retention.apply(&mut record, &capture.policy);
        capture.writer.append(&record)?;
    }
    let mut websocket_finished = false;
    if let transmog_core::observe::ObserverEventKind::StreamState(state) = &event.kind
        && state.protocol == transmog_core::stream::StreamProtocol::WebSocket
    {
        if state.status.is_active() {
            capture.open_websockets.entry(exchange_id).or_insert(false);
        } else {
            websocket_finished = capture
                .open_websockets
                .remove(&exchange_id)
                .unwrap_or(false);
        }
    }
    let http_finished = matches!(
        event.kind,
        transmog_core::observe::ObserverEventKind::Completed(_)
            | transmog_core::observe::ObserverEventKind::Failed(_)
    );
    if http_finished && let Some(done) = capture.open_websockets.get_mut(&exchange_id) {
        *done = true;
    }
    if websocket_finished || (http_finished && !capture.open_websockets.contains_key(&exchange_id))
    {
        capture.last_sequences.remove(&exchange_id);
    } else {
        capture.last_sequences.insert(exchange_id, event.sequence);
    }
    Ok(())
}

fn current_status(status: &Mutex<CaptureStatus>) -> CaptureStatus {
    status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn set_status(current: &Mutex<CaptureStatus>, status: CaptureStatus) {
    *current
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
}

fn set_failure(status: &Mutex<CaptureStatus>, message: impl Into<String>) {
    let current = current_status(status);
    let path = match current {
        CaptureStatus::Active { path, .. } => Some(path),
        CaptureStatus::Failed(failure) => failure.path,
        _ => None,
    };
    set_status(
        status,
        CaptureStatus::Failed(CaptureFailure {
            path,
            message: message.into(),
        }),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        sync::atomic::{AtomicU64, Ordering},
        time::SystemTime,
    };

    use transmog_capture::{CaptureRecordKind, recover};
    use transmog_core::{
        ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeId, ExchangeMetadata},
        observe::ObserverEventKind,
    };

    use super::*;

    #[tokio::test]
    async fn incident_ack_survives_checkpoint_and_stop_never_writes_after_seal() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits::default(),
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        let marker = manager
            .mark(
                "Issue happened before any request".into(),
                transmog_capture::support::MarkerOrigin::Cli,
            )
            .await
            .unwrap();
        let recovered = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(!recovered.sealed);
        assert!(recovered.records.iter().any(|r|matches!(&r.kind,CaptureRecordKind::Unknown{kind,payload} if kind==transmog_capture::support::MARKER_KIND&&payload["id"]==marker.id)));
        let marking = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .mark(
                        "racing stop".into(),
                        transmog_capture::support::MarkerOrigin::Desktop,
                    )
                    .await
            })
        };
        manager.stop().await.unwrap();
        let result = marking.await.unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            manager
                .mark(
                    "too late".into(),
                    transmog_capture::support::MarkerOrigin::Cli
                )
                .await
                .is_err()
        );
        assert_eq!(before, std::fs::read(&path).unwrap());
        let recovered = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(recovered.sealed);
        if let Ok(marker) = result {
            assert!(recovered.records.iter().any(|r|matches!(&r.kind,CaptureRecordKind::Unknown{payload,..}if payload["id"]==marker.id)));
        }
        manager.shutdown().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn marker_bound_refuses_without_poisoning_and_keeps_marker_only_capture() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits::default(),
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        for _ in 0..transmog_capture::support::MAX_MARKERS {
            manager
                .mark(
                    String::new(),
                    transmog_capture::support::MarkerOrigin::Desktop,
                )
                .await
                .unwrap();
        }
        assert!(
            manager
                .mark(
                    "overflow".into(),
                    transmog_capture::support::MarkerOrigin::Desktop
                )
                .await
                .is_err()
        );
        assert!(matches!(manager.status(), CaptureStatus::Active { .. }));
        manager.stop().await.unwrap();
        let recovered = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(recovered.sealed);
        assert_eq!(recovered.records.iter().filter(|r| matches!(&r.kind, CaptureRecordKind::Unknown {kind,..} if kind == transmog_capture::support::MARKER_KIND)).count(), 256);
        manager.shutdown().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn saturated_marker_queue_refuses_without_changing_status() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (reply, _) = mpsc::channel();
        sender.send(Command::Shutdown(reply)).unwrap();
        let status = CaptureStatus::Active {
            path: temp_path(),
            bytes_written: 0,
        };
        let manager = CaptureManager {
            inner: Arc::new(CaptureInner {
                sender,
                byte_budget: Arc::new(tokio::sync::Semaphore::new(1)),
                status: Arc::new(Mutex::new(status.clone())),
                worker: Mutex::new(None),
            }),
        };
        assert_eq!(
            manager
                .mark(
                    "bounded".into(),
                    transmog_capture::support::MarkerOrigin::Cli
                )
                .await,
            Err(CaptureServiceError::QueueSaturated)
        );
        assert_eq!(manager.status(), status);
        drop(receiver);
    }

    static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn websocket_recording_keeps_sequence_through_handshake_and_releases_on_close() {
        use transmog_core::{
            HeaderBlock, RequestHead, ResponseHead,
            intercept::CompletedExchange,
            stream::{StreamProtocol, StreamState, StreamStatus},
        };
        let path = temp_path();
        let mut capture = ActiveCapture {
            run_id: transmog_capture::support::new_id().unwrap(),
            began: Instant::now(),
            markers: 0,
            path: path.clone(),
            writer: CaptureWriter::new(File::create(&path).unwrap(), CaptureLimits::default())
                .unwrap(),
            policy: CapturePolicy::default(),
            retention: CaptureBodyRetention::default(),
            last_sequences: HashMap::new(),
            open_websockets: HashMap::new(),
        };
        let mut event = start_event(1);
        let ObserverEventKind::ExchangeStarted { metadata } = event.kind.clone() else {
            unreachable!()
        };
        append_event(&mut capture, &event).unwrap();
        event.sequence = 2;
        event.kind = ObserverEventKind::StreamState(StreamState::now(
            StreamProtocol::WebSocket,
            StreamStatus::Connecting,
            None,
        ));
        append_event(&mut capture, &event).unwrap();
        event.sequence = 3;
        event.kind = ObserverEventKind::Completed(CompletedExchange {
            request_head: RequestHead {
                method: "GET".into(),
                target: metadata.original_target.as_target().clone(),
                headers: HeaderBlock::default(),
                source_version: HttpLegVersion::Http1,
            },
            response_head: ResponseHead {
                status: 101,
                headers: HeaderBlock::default(),
                source_version: HttpLegVersion::Http1,
            },
            metadata,
        });
        append_event(&mut capture, &event).unwrap();
        for (sequence, status) in [(4, StreamStatus::Open), (5, StreamStatus::Closed)] {
            event.sequence = sequence;
            event.kind = ObserverEventKind::StreamState(StreamState::now(
                StreamProtocol::WebSocket,
                status,
                None,
            ));
            append_event(&mut capture, &event).unwrap();
        }
        let pending = capture.last_sequences.len();
        capture.writer.seal().unwrap();
        let saved = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        let loss = saved
            .records
            .iter()
            .filter(|record| matches!(record.kind, CaptureRecordKind::Loss { .. }))
            .collect::<Vec<_>>();
        assert_eq!(loss, [] as [&transmog_capture::CaptureRecord; 0]);
        assert_eq!(pending, 0);
        std::fs::remove_file(path).unwrap();
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "transmog-session-{}-{}.tmcap",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn start_event(sequence: u64) -> ObserverEvent {
        let metadata = Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(7),
                downstream_connection_id: ConnectionId(8),
                stream_id: StreamId(9),
                client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
                ingress_version: HttpLegVersion::Http1,
                egress_version: None,
            },
            Target {
                scheme: "https".into(),
                authority: "example.test".into(),
                host: "example.test".into(),
                port: 443,
                path: "/".into(),
                query: None,
            },
            SystemTime::UNIX_EPOCH,
        ));
        ObserverEvent {
            exchange_id: ExchangeId(7),
            sequence,
            kind: ObserverEventKind::ExchangeStarted { metadata },
        }
    }

    #[test]
    fn terminal_exchanges_release_sequence_accounting() {
        let path = temp_path();
        let file = File::create(&path).unwrap();
        let mut capture = ActiveCapture {
            run_id: transmog_capture::support::new_id().unwrap(),
            began: Instant::now(),
            markers: 0,
            path: path.clone(),
            writer: CaptureWriter::new(file, CaptureLimits::default()).unwrap(),
            policy: CapturePolicy::default(),
            retention: CaptureBodyRetention::default(),
            last_sequences: HashMap::new(),
            open_websockets: HashMap::new(),
        };
        for id in 1..=100 {
            let mut event = start_event(1);
            event.exchange_id = ExchangeId(id);
            let ObserverEventKind::ExchangeStarted { metadata } = event.kind.clone() else {
                unreachable!()
            };
            append_event(&mut capture, &event).unwrap();
            event.sequence = 2;
            if id % 3 != 0 {
                use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
                let protocol = if id % 3 == 1 {
                    StreamProtocol::Sse
                } else {
                    StreamProtocol::WebSocket
                };
                for status in [StreamStatus::Open, StreamStatus::Failed] {
                    event.kind =
                        ObserverEventKind::StreamState(StreamState::now(protocol, status, None));
                    append_event(&mut capture, &event).unwrap();
                    event.sequence += 1;
                }
            }
            event.kind = ObserverEventKind::Failed(transmog_core::intercept::ExchangeFailure {
                metadata,
                stage: transmog_core::intercept::ExchangeStage::Upstream,
                kind: transmog_core::intercept::ExchangeFailureKind::Upstream,
                request_committed: false,
                response_committed: false,
                message: "fixture failure".into(),
            });
            append_event(&mut capture, &event).unwrap();
        }
        let pending = capture.last_sequences.len();
        assert!(capture.open_websockets.is_empty());
        capture.writer.seal().unwrap();
        drop(capture);
        let saved = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(
            !saved
                .records
                .iter()
                .any(|record| matches!(record.kind, CaptureRecordKind::Loss { .. }))
        );
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            pending, 0,
            "Finished traffic still consumes recording memory"
        );
    }

    #[tokio::test]
    async fn active_capture_publishes_a_checkpoint_then_compacts_atomically_on_stop() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits::default(),
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        manager.record(start_event(1)).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut reader = transmog_capture::CaptureReader::new(
                    File::open(&path).unwrap(),
                    CaptureLimits::default(),
                )
                .unwrap();
                if reader.read_next_indexed().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        let sealed = manager.stop().await.unwrap();
        assert!(sealed.bytes_written < before);
        assert!(sealed.bytes_written < 5 * 1024);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            sealed.bytes_written
        );
        let capture = recover(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(capture.sealed);
        assert!(matches!(
            capture.records[0].kind,
            CaptureRecordKind::ExchangeStarted { .. }
        ));
        manager.shutdown().await;
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn saturated_writer_queue_waits_instead_of_failing_the_recording() {
        use std::{future::poll_fn, task::Poll, time::Duration};

        let (sender, receiver) = mpsc::sync_channel(1);
        let manager = CaptureManager {
            inner: Arc::new(CaptureInner {
                sender,
                byte_budget: Arc::new(tokio::sync::Semaphore::new(32 * 1024 * 1024)),
                status: Arc::new(Mutex::new(CaptureStatus::Active {
                    path: temp_path(),
                    bytes_written: 0,
                })),
                worker: Mutex::new(None),
            }),
        };
        manager
            .inner
            .sender
            .send(Command::Record(start_event(1), None))
            .unwrap();
        let mut record = Box::pin(manager.record(start_event(2)));
        let early = poll_fn(|cx| Poll::Ready(record.as_mut().poll(cx))).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let late = if early.is_pending() {
            poll_fn(|cx| Poll::Ready(record.as_mut().poll(cx))).await
        } else {
            Poll::Pending
        };
        assert!(
            matches!(receiver.recv().unwrap(), Command::Record(event, _) if event.sequence == 1)
        );
        let waited = early.is_pending() && late.is_pending();
        if waited {
            record.as_mut().await;
        }
        drop(record);
        assert!(waited, "recording failed instead of slowing the producer");
        assert!(matches!(manager.status(), CaptureStatus::Active { .. }));
        assert!(
            matches!(receiver.recv().unwrap(), Command::Record(event, _) if event.sequence == 2)
        );
        drop(receiver);
    }

    #[tokio::test]
    async fn capture_is_create_new_streaming_and_sealed() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits::default(),
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        manager.record(start_event(1)).await;
        let sealed = manager.stop().await.unwrap();
        assert_eq!(sealed.path, path);
        let recovered =
            recover(File::open(&sealed.path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(recovered.sealed);
        assert!(matches!(
            recovered.records[0].kind,
            CaptureRecordKind::ExchangeStarted { .. }
        ));
        std::fs::remove_file(sealed.path).unwrap();
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn context_precedes_exchange_records_without_changing_their_sequence() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager.start(CaptureStart{encoding:transmog_capture::CaptureEncoding::default(),path:path.clone(),limits:CaptureLimits::default(),policy:CapturePolicy::default(),metadata:Some(serde_json::json!({"networkContext":{"platform":"fixture","output":"original machine"}}))}).await.unwrap();
        manager.record(start_event(1)).await;
        let sealed = manager.stop().await.unwrap();
        let capture = recover(File::open(&sealed.path).unwrap(), CaptureLimits::default()).unwrap();
        assert!(capture.sealed);
        assert_eq!(capture.records[0].exchange_id, 0);
        assert!(
            matches!(&capture.records[0].kind,CaptureRecordKind::Unknown{kind,payload} if kind=="trace-metadata" && payload["networkContext"]["output"]=="original machine")
        );
        assert_eq!(capture.records[1].sequence, 1);
        std::fs::remove_file(path).unwrap();
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn context_that_does_not_fit_never_leaves_a_retry_blocking_destination() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(1).unwrap()).unwrap();
        assert!(
            manager
                .start(CaptureStart {
                    encoding: transmog_capture::CaptureEncoding::default(),
                    path: path.clone(),
                    limits: CaptureLimits {
                        max_file_bytes: 1024,
                        max_record_bytes: 16,
                        max_records: 2
                    },
                    policy: CapturePolicy::default(),
                    metadata: Some(serde_json::json!({"networkContext":"fixture"}))
                })
                .await
                .is_err()
        );
        assert!(!path.exists());
        assert!(matches!(manager.status(), CaptureStatus::Idle));
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn existing_destination_and_idle_stop_are_typed() {
        let path = temp_path();
        File::create(&path).unwrap();
        let manager = CaptureManager::new(NonZeroUsize::new(1).unwrap()).unwrap();
        assert_eq!(
            manager
                .start(CaptureStart {
                    encoding: transmog_capture::CaptureEncoding::default(),
                    metadata: None,
                    path: path.clone(),
                    limits: CaptureLimits::default(),
                    policy: CapturePolicy::default(),
                })
                .await,
            Err(CaptureServiceError::DestinationExists)
        );
        assert_eq!(manager.stop().await, Err(CaptureServiceError::NotActive));
        std::fs::remove_file(path).unwrap();
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn quota_failure_is_visible_and_does_not_panic_caller() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                encoding: transmog_capture::CaptureEncoding::default(),
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits {
                    max_file_bytes: 64 * 1024,
                    max_record_bytes: 16,
                    max_records: 2,
                },
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        manager.record(start_event(1)).await;
        // A serialized start record exceeds the deliberately tiny record bound.
        while matches!(manager.status(), CaptureStatus::Active { .. }) {
            tokio::task::yield_now().await;
        }
        assert!(matches!(manager.status(), CaptureStatus::Failed(_)));
        assert!(manager.stop().await.is_err());
        std::fs::remove_file(path).unwrap();
        manager.shutdown().await;
    }
}
