use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};

use thiserror::Error;
use transmog_capture::{
    CaptureLimits, CapturePolicy, CaptureWriter, loss_record, record_from_observer,
};
use transmog_core::observe::ObserverEvent;

/// Settings for a new native capture artifact.
#[derive(Clone, Debug)]
pub struct CaptureStart {
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
    /// Bytes durably written, including the seal.
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
        /// Bytes committed as of the most recent append.
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
    Record(ObserverEvent),
    Stop(mpsc::Sender<Result<SealedCapture, CaptureServiceError>>),
    Shutdown(mpsc::Sender<()>),
}

struct ActiveCapture {
    path: PathBuf,
    writer: CaptureWriter<File>,
    policy: CapturePolicy,
    last_sequences: HashMap<u128, u64>,
}

struct CaptureInner {
    sender: mpsc::SyncSender<Command>,
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

    /// Enqueues one observer event without waiting for disk I/O.
    pub(crate) fn record(&self, event: ObserverEvent) {
        if !matches!(self.status(), CaptureStatus::Active { .. }) {
            return;
        }
        match self.inner.sender.try_send(Command::Record(event)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                set_failure(
                    &self.inner.status,
                    "capture event queue saturated; artifact has a recoverable prefix",
                );
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
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Start(request, reply) => {
                let result = start_capture(&mut active, status, request);
                let _ = reply.send(result);
            }
            Command::Record(event) => {
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
    request: CaptureStart,
) -> Result<(), CaptureServiceError> {
    if active.is_some() {
        return Err(CaptureServiceError::AlreadyActive);
    }
    if matches!(current_status(status), CaptureStatus::Shutdown) {
        return Err(CaptureServiceError::WorkerUnavailable);
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
    let writer = match prepare_writer(file, request.limits, request.metadata) {
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
        path: request.path,
        writer,
        policy: request.policy,
        last_sequences: HashMap::new(),
    });
    Ok(())
}

fn prepare_writer(
    file: File,
    limits: CaptureLimits,
    metadata: Option<serde_json::Value>,
) -> Result<CaptureWriter<File>, CaptureServiceError> {
    let mut writer = CaptureWriter::new(file, limits)
        .map_err(|error| CaptureServiceError::Writer(error.to_string()))?;
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
    let sealed = SealedCapture {
        path: capture.path,
        bytes_written: capture.writer.bytes_written(),
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
    if let Some(record) = record_from_observer(event, &capture.policy) {
        capture.writer.append(&record)?;
    }
    capture.last_sequences.insert(exchange_id, event.sequence);
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

    static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

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

    #[tokio::test]
    async fn capture_is_create_new_streaming_and_sealed() {
        let path = temp_path();
        let manager = CaptureManager::new(NonZeroUsize::new(8).unwrap()).unwrap();
        manager
            .start(CaptureStart {
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits::default(),
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        manager.record(start_event(1));
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
        manager.start(CaptureStart{path:path.clone(),limits:CaptureLimits::default(),policy:CapturePolicy::default(),metadata:Some(serde_json::json!({"networkContext":{"platform":"fixture","output":"original machine"}}))}).await.unwrap();
        manager.record(start_event(1));
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
                    path: path.clone(),
                    limits: CaptureLimits {
                        max_file_bytes: 32,
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
                metadata: None,
                path: path.clone(),
                limits: CaptureLimits {
                    max_file_bytes: 32,
                    max_record_bytes: 16,
                    max_records: 2,
                },
                policy: CapturePolicy::default(),
            })
            .await
            .unwrap();
        manager.record(start_event(1));
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
