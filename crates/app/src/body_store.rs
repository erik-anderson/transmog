use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use transmog_core::{
    HeaderBlock, ResponseHead,
    intercept::ExchangeId,
    observe::{
        BodyDirection, BodyObservation, BoxObserverFuture, ExchangeBoundary, ObservationInterest,
        ObservedBodyChunk, Observer, ObserverConfig, ObserverDeliveryPolicy, ObserverEvent,
        ObserverEventKind,
    },
};

/// Default aggregate size of the response-body cache.
pub const DEFAULT_BODY_STORE_BYTES: u64 = 1024 * 1024 * 1024;
/// Default maximum bytes returned by one inspector range read.
pub const DEFAULT_BODY_READ_BYTES: usize = 256 * 1024;
/// Hard maximum bytes returned by one inspector range read.
pub const MAX_BODY_READ_BYTES: usize = 16 * 1024 * 1024;

/// Policy used when retained body bytes reach their aggregate quota.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionMode {
    /// Keep metadata and byte counts but do not retain body bytes.
    Off,
    /// Keep existing bytes and omit new bytes after the quota is full.
    StopWhenFull,
    /// Evict the oldest terminal body blobs to admit newer bytes.
    Circular,
}

/// Location of live retained body bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferStorage {
    /// Do not create body-cache files.
    Memory,
    /// Retain body bytes in application-owned files.
    Disk,
}

/// Persisted aggregate buffer limit. Automatic uses half of installed RAM.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum BufferLimit {
    /// Half installed RAM, retained entirely in memory.
    #[default]
    Automatic,
    /// An explicit byte budget; budgets over half installed RAM use disk.
    Custom {
        /// Aggregate byte budget.
        bytes: u64,
    },
    /// No aggregate byte cap, using disk storage.
    Unlimited,
}

/// Resolved live buffer configuration and usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferStatus {
    /// Installed physical RAM, when the platform reports it.
    pub installed_ram: Option<u64>,
    /// Aggregate limit; None means no maximum.
    pub max_bytes: Option<u64>,
    /// Where new body boundaries are stored.
    pub storage: BufferStorage,
    /// Retained live body bytes, excluding imported file references.
    pub retained_bytes: u64,
}

impl BufferLimit {
    /// Resolve the policy against installed RAM; unavailable RAM uses a conservative fallback.
    pub fn resolve(self, installed_ram: Option<u64>) -> BufferStatus {
        let half = installed_ram
            .filter(|bytes| *bytes > 1)
            .map_or(DEFAULT_BODY_STORE_BYTES, |bytes| bytes / 2);
        let max_bytes = match self {
            Self::Automatic => Some(half),
            Self::Custom { bytes } => Some(bytes),
            Self::Unlimited => None,
        };
        BufferStatus {
            installed_ram,
            max_bytes,
            storage: if max_bytes.is_some_and(|bytes| bytes <= half) {
                BufferStorage::Memory
            } else {
                BufferStorage::Disk
            },
            retained_bytes: 0,
        }
    }
}

/// Installed RAM reported without enumerating processes or files.
pub fn installed_ram() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    (system.total_memory() > 0).then(|| system.total_memory())
}

/// Validated body-store construction settings.
#[derive(Clone, Debug)]
pub struct BodyStoreConfig {
    /// Application-owned directory containing only ephemeral body-cache files.
    pub root: PathBuf,
    /// Apply persisted product buffer choices when an application creates this store.
    /// Explicit embedding configurations can keep their construction quota instead.
    pub use_product_preferences: bool,
    /// Initial retention behavior.
    pub mode: RetentionMode,
    /// Storage location for new bodies.
    pub storage: BufferStorage,
    /// Aggregate retained-byte quota.
    pub max_bytes: u64,
    /// Per-boundary retained-byte quota.
    pub max_body_bytes: u64,
    /// Finite queue between the observer callback and storage worker.
    pub queue_capacity: NonZeroUsize,
    /// Maximum bytes accepted in one range read.
    pub max_read_bytes: usize,
    /// Whether request boundaries are retained in addition to responses.
    pub retain_requests: bool,
}

impl BodyStoreConfig {
    /// Creates a memory-only circular cache using half installed RAM.
    pub fn product_default(root: PathBuf) -> Self {
        Self {
            root,
            use_product_preferences: true,
            mode: RetentionMode::Circular,
            storage: BufferStorage::Memory,
            max_bytes: BufferLimit::Automatic
                .resolve(installed_ram())
                .max_bytes
                .unwrap_or(DEFAULT_BODY_STORE_BYTES),
            max_body_bytes: u64::MAX,
            queue_capacity: NonZeroUsize::new(2_048).unwrap_or(NonZeroUsize::MIN),
            max_read_bytes: MAX_BODY_READ_BYTES,
            retain_requests: true,
        }
    }
}

/// Why an exact body cannot currently be presented.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyAvailability {
    /// Bytes are still arriving.
    Capturing,
    /// A complete exact body is retained.
    Complete,
    /// A terminal prefix is retained because a configured limit was reached.
    Truncated,
    /// Delivery or storage loss made the retained bytes incomplete.
    Lost,
    /// The terminal blob was removed by circular eviction.
    Evicted,
    /// Retention was disabled for this boundary.
    Disabled,
    /// Stop-when-full policy omitted this boundary.
    QuotaOmitted,
}

/// Immutable metadata for one exchange boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredBodyMetadata {
    /// Whether byte counts exclude any deferred HTTP transfer framing.
    pub length_known: bool,
    /// Stable exchange identifier.
    pub exchange_id: String,
    /// Explicit observation boundary.
    pub boundary: &'static str,
    /// Total bytes reported by the runtime observer.
    pub observed_bytes: u64,
    /// Complete observed entity bytes, still content-encoded; excludes HTTP framing and TLS.
    pub wire_body_bytes: Option<u64>,
    /// Bytes currently retained in memory or the body cache.
    pub retained_bytes: u64,
    /// Current availability state.
    pub availability: BodyAvailability,
    /// Declared media type without parameters, when present.
    pub media_type: Option<String>,
    /// Declared charset, when present.
    pub charset: Option<String>,
    /// Ordered content-coding tokens.
    pub content_codings: Vec<String>,
    /// SHA-256 of retained bytes when the boundary became terminal.
    pub sha256: Option<String>,
    /// Redaction-safe reason for loss, truncation, omission, or eviction.
    pub reason: Option<String>,
}

/// One bounded byte range returned to an inspector or exporter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyRange {
    /// Offset of the first returned byte.
    pub offset: u64,
    /// Retained bytes available when the read began.
    pub retained_bytes: u64,
    /// Returned bytes.
    pub bytes: Vec<u8>,
    /// Availability state observed when the read began.
    pub availability: BodyAvailability,
}

/// File-backed read lease that prevents circular eviction until dropped.
pub struct BodyReadLease {
    reader: Box<dyn Read + Send>,
    key: BodyKey,
    inner: Arc<BodyStoreInner>,
    metadata: StoredBodyMetadata,
}

impl std::fmt::Debug for BodyReadLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BodyReadLease")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl BodyReadLease {
    /// Immutable complete-body metadata captured when the lease opened.
    pub fn metadata(&self) -> &StoredBodyMetadata {
        &self.metadata
    }
}

impl Read for BodyReadLease {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl Drop for BodyReadLease {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = state.records.get_mut(&self.key) {
            record.leases = record.leases.saturating_sub(1);
        }
        enforce_quota(&self.inner, &mut state);
    }
}

/// Aggregate body-store pressure and loss state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyStoreCounters {
    /// Bytes represented by live retained bodies.
    pub retained_bytes: u64,
    /// Terminal blobs removed by circular eviction.
    pub evicted_bodies: u64,
    /// Observer events rejected by the finite storage queue.
    pub dropped_events: u64,
    /// Filesystem operations that failed.
    pub storage_failures: u64,
}

/// Body-store construction, lookup, or bounded-read failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyStoreError {
    /// A configured bound is zero, inconsistent, or excessive.
    #[error("body-store limits are invalid")]
    InvalidLimits,
    /// The cache directory could not be prepared.
    #[error("body-store directory is unavailable")]
    DirectoryUnavailable,
    /// No body metadata exists at the requested boundary.
    #[error("body boundary is unavailable")]
    UnknownBody,
    /// The requested range exceeds the configured per-read ceiling.
    #[error("body range exceeds the configured read limit")]
    ReadLimitExceeded,
    /// The requested offset is outside retained bytes.
    #[error("body range offset is outside retained bytes")]
    InvalidOffset,
    /// The retained file disappeared or could not be read.
    #[error("retained body bytes are unavailable")]
    ReadUnavailable,
    /// The storage worker is no longer available.
    #[error("body-store worker is unavailable")]
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum BoundaryKey {
    ClientRequest,
    UpstreamRequest,
    UpstreamResponse,
    ClientResponse,
}

impl BoundaryKey {
    const fn from_boundary(boundary: ExchangeBoundary) -> Self {
        match boundary {
            ExchangeBoundary::ClientRequest => Self::ClientRequest,
            ExchangeBoundary::UpstreamRequest => Self::UpstreamRequest,
            ExchangeBoundary::UpstreamResponse => Self::UpstreamResponse,
            ExchangeBoundary::ClientResponse => Self::ClientResponse,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::ClientRequest => "client-request",
            Self::UpstreamRequest => "upstream-request",
            Self::UpstreamResponse => "upstream-response",
            Self::ClientResponse => "client-response",
        }
    }

    const fn direction(self) -> BodyDirection {
        match self {
            Self::ClientRequest | Self::UpstreamRequest => BodyDirection::Request,
            Self::UpstreamResponse | Self::ClientResponse => BodyDirection::Response,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BodyKey {
    exchange_id: ExchangeId,
    boundary: BoundaryKey,
}

impl Hash for BodyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.exchange_id.0.hash(state);
        self.boundary.hash(state);
    }
}

#[derive(Debug)]
struct StoredBody {
    source: Option<Arc<dyn SavedBodySource>>,
    source_size_deferred: bool,
    path: Option<PathBuf>,
    memory: Option<Vec<bytes::Bytes>>,
    observed_bytes: u64,
    retained_bytes: u64,
    availability: BodyAvailability,
    media_type: Option<String>,
    charset: Option<String>,
    content_codings: Vec<String>,
    response_head: Option<ResponseHead>,
    sha256: Option<String>,
    reason: Option<String>,
    hasher: Sha256,
    leases: usize,
    terminal: bool,
}

impl StoredBody {
    fn new(availability: BodyAvailability, storage: BufferStorage) -> Self {
        Self {
            source: None,
            source_size_deferred: false,
            path: None,
            memory: (storage == BufferStorage::Memory).then(Vec::new),
            observed_bytes: 0,
            retained_bytes: 0,
            availability,
            media_type: None,
            charset: None,
            content_codings: Vec::new(),
            response_head: None,
            sha256: None,
            reason: None,
            hasher: Sha256::new(),
            leases: 0,
            terminal: false,
        }
    }

    fn metadata(&self, key: BodyKey) -> StoredBodyMetadata {
        StoredBodyMetadata {
            length_known: self
                .source
                .as_ref()
                .is_none_or(|source| source.length().is_some()),
            exchange_id: format!("{:032x}", key.exchange_id.0),
            boundary: key.boundary.name(),
            observed_bytes: if self.source_size_deferred {
                self.source
                    .as_ref()
                    .and_then(|source| source.length())
                    .unwrap_or(self.observed_bytes)
            } else {
                self.observed_bytes
            },
            wire_body_bytes: (self.terminal
                && self.availability != BodyAvailability::Lost
                && !self.source_size_deferred)
                .then_some(self.observed_bytes),
            retained_bytes: self
                .source
                .as_ref()
                .and_then(|source| source.length())
                .unwrap_or(self.retained_bytes),
            availability: self.availability,
            media_type: self.media_type.clone(),
            charset: self.charset.clone(),
            content_codings: self.content_codings.clone(),
            sha256: self.sha256.clone(),
            reason: self.reason.clone(),
        }
    }
}

struct MemoryBodyReader {
    chunks: std::vec::IntoIter<bytes::Bytes>,
    current: bytes::Bytes,
    offset: usize,
}
impl MemoryBodyReader {
    fn new(chunks: Vec<bytes::Bytes>) -> Self {
        Self {
            chunks: chunks.into_iter(),
            current: bytes::Bytes::new(),
            offset: 0,
        }
    }
}
impl Read for MemoryBodyReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.offset == self.current.len() {
            let Some(chunk) = self.chunks.next() else {
                return Ok(0);
            };
            self.current = chunk;
            self.offset = 0;
        }
        let take = buffer.len().min(self.current.len() - self.offset);
        buffer[..take].copy_from_slice(&self.current[self.offset..self.offset + take]);
        self.offset += take;
        Ok(take)
    }
}

pub(crate) trait SavedBodySource: Send + Sync + std::fmt::Debug {
    fn open(&self) -> std::io::Result<Box<dyn Read + Send>>;
    fn length(&self) -> Option<u64>;
}

#[derive(Debug)]
struct StoreState {
    mode: RetentionMode,
    buffer: BufferStatus,
    exchange_modes: HashMap<ExchangeId, ExchangeRetention>,
    records: HashMap<BodyKey, StoredBody>,
    terminal_order: VecDeque<BodyKey>,
    lossy_exchanges: HashSet<ExchangeId>,
    last_sequences: HashMap<ExchangeId, u64>,
    counters: BodyStoreCounters,
}

#[derive(Clone, Copy, Debug)]
struct ExchangeRetention {
    mode: RetentionMode,
    requests: bool,
    responses: bool,
    request_limit: u64,
}

impl ExchangeRetention {
    fn for_direction(self, direction: BodyDirection) -> RetentionMode {
        let enabled = match direction {
            BodyDirection::Request => self.requests,
            BodyDirection::Response => self.responses,
        };
        if enabled {
            self.mode
        } else {
            RetentionMode::Off
        }
    }
}

enum WorkerCommand {
    Event(Box<ObserverEvent>),
    Flush(mpsc::Sender<()>),
    Shutdown,
}

struct BodyStoreInner {
    redact_sensitive: AtomicBool,
    retain_requests: AtomicBool,
    request_limit: AtomicU64,
    retain_responses: AtomicBool,
    config: BodyStoreConfig,
    state: Mutex<StoreState>,
    sender: mpsc::SyncSender<WorkerCommand>,
}

impl Drop for BodyStoreInner {
    fn drop(&mut self) {
        let _ = self.sender.try_send(WorkerCommand::Shutdown);
    }
}

/// Cloneable product-layer response-body cache and runtime observer.
#[derive(Clone)]
pub struct BodyStore {
    inner: Arc<BodyStoreInner>,
}

impl std::fmt::Debug for BodyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BodyStore")
            .field("root", &self.inner.config.root)
            .field("mode", &self.mode())
            .field("counters", &self.counters())
            .finish_non_exhaustive()
    }
}

impl BodyStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register_saved(
        &self,
        id: ExchangeId,
        boundary: ExchangeBoundary,
        headers: &HeaderBlock,
        response: Option<ResponseHead>,
        source: Arc<dyn SavedBodySource>,
        bytes: u64,
        complete: bool,
    ) {
        let mut state = self.lock_state();
        let key = BodyKey {
            exchange_id: id,
            boundary: BoundaryKey::from_boundary(boundary),
        };
        observe_headers(&self.inner, &mut state, id, boundary, headers);
        let record = state.records.get_mut(&key).expect("registered head");
        record.source_size_deferred = source.length().is_none();
        record.source = Some(source);
        record.response_head = response;
        record.observed_bytes = bytes;
        record.retained_bytes = bytes;
        record.terminal = true;
        record.availability = if complete {
            BodyAvailability::Complete
        } else {
            BodyAvailability::Lost
        };
        record.reason =
            (!complete).then(|| "The saved trace did not retain a complete body".into());
    }
    /// Creates the cache, removes stale temporary files, and starts its bounded
    /// storage worker.
    ///
    /// # Errors
    ///
    /// Returns a typed limit or directory error before starting a worker.
    pub fn new(config: BodyStoreConfig) -> Result<Self, BodyStoreError> {
        if config.max_bytes == 0
            || config.max_body_bytes == 0
            || config.max_read_bytes == 0
            || config.max_read_bytes > MAX_BODY_READ_BYTES
        {
            return Err(BodyStoreError::InvalidLimits);
        }
        if config.storage == BufferStorage::Disk {
            fs::create_dir_all(&config.root).map_err(|_| BodyStoreError::DirectoryUnavailable)?;
        }
        if config.root.exists() {
            cleanup_owned_files(&config.root)?;
        }
        let (sender, receiver) = mpsc::sync_channel(config.queue_capacity.get());
        let inner = Arc::new(BodyStoreInner {
            redact_sensitive: AtomicBool::new(false),
            retain_requests: AtomicBool::new(config.retain_requests),
            request_limit: AtomicU64::new(transmog_capture::DEFAULT_REQUEST_BODY_CAPTURE_BYTES),
            retain_responses: AtomicBool::new(true),
            state: Mutex::new(StoreState {
                mode: config.mode,
                buffer: BufferStatus {
                    installed_ram: installed_ram(),
                    max_bytes: (config.max_bytes != u64::MAX).then_some(config.max_bytes),
                    storage: config.storage,
                    retained_bytes: 0,
                },
                exchange_modes: HashMap::new(),
                records: HashMap::new(),
                terminal_order: VecDeque::new(),
                lossy_exchanges: HashSet::new(),
                last_sequences: HashMap::new(),
                counters: BodyStoreCounters::default(),
            }),
            config,
            sender,
        });
        let worker = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("transmog-body-store".to_owned())
            .spawn(move || worker_loop(&worker, &receiver))
            .map_err(|_| BodyStoreError::WorkerUnavailable)?;
        Ok(Self { inner })
    }

    /// Runtime observer settings for exact response bytes and optional request
    /// bytes with a finite backpressure deadline.
    pub fn observer_config(&self) -> ObserverConfig {
        ObserverConfig {
            interest: ObservationInterest {
                lifecycle: true,
                sensitive_headers: true,
                request_body: BodyObservation::Full,
                response_body: BodyObservation::Full,
            },
            queue_capacity: self.inner.config.queue_capacity,
            delivery: ObserverDeliveryPolicy::Backpressure {
                timeout: Duration::from_millis(25),
            },
            callback_timeout: Duration::from_secs(2),
        }
    }

    /// Current retention mode. Changes apply only to subsequently observed
    /// bytes and never silently delete prior blobs.
    pub fn mode(&self) -> RetentionMode {
        self.lock_state().mode
    }

    /// Changes retention behavior without deleting existing terminal blobs.
    pub fn set_mode(&self, mode: RetentionMode) {
        self.lock_state().mode = mode;
    }

    /// Changes the aggregate policy for future boundaries without spilling existing memory bodies.
    /// Existing leased or active bodies are protected from eviction.
    ///
    /// # Errors
    /// Returns a directory error before selecting disk storage.
    pub(crate) fn prepare_buffer_limit(
        &self,
        limit: BufferLimit,
    ) -> Result<BufferStatus, BodyStoreError> {
        if matches!(limit, BufferLimit::Custom { bytes: 0 }) {
            return Err(BodyStoreError::InvalidLimits);
        }
        let buffer = limit.resolve(self.lock_state().buffer.installed_ram);
        if buffer.storage == BufferStorage::Disk {
            fs::create_dir_all(&self.inner.config.root)
                .map_err(|_| BodyStoreError::DirectoryUnavailable)?;
        }
        Ok(buffer)
    }

    /// Changes the aggregate limit for new bodies; active reads remain protected.
    ///
    /// # Errors
    /// Returns invalid limits or a disk directory failure before changing the cache.
    pub fn set_buffer_limit(&self, limit: BufferLimit) -> Result<BufferStatus, BodyStoreError> {
        let buffer = self.prepare_buffer_limit(limit)?;
        let mut state = self.lock_state();
        state.buffer = buffer;
        enforce_quota(&self.inner, &mut state);
        Ok(BufferStatus {
            retained_bytes: state.counters.retained_bytes,
            ..state.buffer
        })
    }

    /// Current aggregate storage policy and live byte count.
    pub fn buffer_status(&self) -> BufferStatus {
        let state = self.lock_state();
        BufferStatus {
            retained_bytes: state.counters.retained_bytes,
            ..state.buffer
        }
    }

    /// Applies saved privacy choices to subsequently observed traffic.
    pub fn set_privacy(&self, requests: bool, responses: bool, redact: bool) {
        self.inner
            .retain_requests
            .store(requests, Ordering::Release);
        self.inner
            .retain_responses
            .store(responses, Ordering::Release);
        self.inner.redact_sensitive.store(redact, Ordering::Release);
    }

    /// Changes the per-request cap for subsequent exchanges; None is Unlimited.
    pub fn set_request_body_limit(&self, limit: Option<u64>) {
        self.inner
            .request_limit
            .store(limit.unwrap_or(0), Ordering::Release);
    }

    /// Returns aggregate retained-byte and loss counters.
    pub fn counters(&self) -> BodyStoreCounters {
        self.lock_state().counters
    }

    /// Lists known boundaries for one exchange in stable boundary order.
    pub fn metadata(&self, exchange_id: ExchangeId) -> Vec<StoredBodyMetadata> {
        let state = self.lock_state();
        [
            BoundaryKey::ClientRequest,
            BoundaryKey::UpstreamRequest,
            BoundaryKey::UpstreamResponse,
            BoundaryKey::ClientResponse,
        ]
        .into_iter()
        .filter_map(|boundary| {
            let key = BodyKey {
                exchange_id,
                boundary,
            };
            state.records.get(&key).map(|record| record.metadata(key))
        })
        .collect()
    }

    /// Returns one protected exact response head retained for asset creation.
    ///
    /// Values follow the persisted application redaction choice; display DTOs
    /// are bounded separately from this complete head.
    pub(crate) fn response_head(
        &self,
        exchange_id: ExchangeId,
        boundary: ExchangeBoundary,
    ) -> Option<ResponseHead> {
        self.lock_state()
            .records
            .get(&BodyKey {
                exchange_id,
                boundary: BoundaryKey::from_boundary(boundary),
            })
            .and_then(|record| record.response_head.clone())
    }

    /// Reads a bounded retained range while holding an eviction lease.
    ///
    /// # Errors
    ///
    /// Returns a typed lookup, range, limit, or storage failure.
    pub fn read_range(
        &self,
        exchange_id: ExchangeId,
        boundary: ExchangeBoundary,
        offset: u64,
        length: usize,
    ) -> Result<BodyRange, BodyStoreError> {
        if length == 0 || length > self.inner.config.max_read_bytes {
            return Err(BodyStoreError::ReadLimitExceeded);
        }
        let key = BodyKey {
            exchange_id,
            boundary: BoundaryKey::from_boundary(boundary),
        };
        let (path, source, memory, retained_bytes, availability) = {
            let mut state = self.lock_state();
            let record = state
                .records
                .get_mut(&key)
                .ok_or(BodyStoreError::UnknownBody)?;
            if offset >= record.retained_bytes && !(offset == 0 && record.retained_bytes == 0) {
                return Err(BodyStoreError::InvalidOffset);
            }
            let path = record.path.clone();
            let source = record.source.clone();
            record.leases = record.leases.saturating_add(1);
            (
                path,
                source,
                record.memory.clone(),
                record.retained_bytes,
                record.availability,
            )
        };
        let result = if let Some(source) = &source {
            source
                .open()
                .and_then(|mut reader| {
                    let skipped =
                        std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())?;
                    if skipped != offset {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    let mut bytes = Vec::new();
                    // One additional read observes the ZIP checksum/EOF when this
                    // range reaches the end, rather than hiding it behind Take.
                    reader.take(length as u64 + 1).read_to_end(&mut bytes)?;
                    bytes.truncate(length);
                    Ok(bytes)
                })
                .map_err(|_| BodyStoreError::ReadUnavailable)
        } else if let Some(chunks) = memory {
            let mut reader = MemoryBodyReader::new(chunks);
            std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())
                .and_then(|_| {
                    let mut bytes = Vec::new();
                    reader.take(length as u64).read_to_end(&mut bytes)?;
                    Ok(bytes)
                })
                .map_err(|_| BodyStoreError::ReadUnavailable)
        } else {
            path.as_ref()
                .ok_or(BodyStoreError::ReadUnavailable)
                .and_then(|path| read_file_range(path, offset, length))
        };
        let result = result.map(|bytes| BodyRange {
            offset,
            retained_bytes: source
                .as_ref()
                .and_then(|source| source.length())
                .unwrap_or(retained_bytes),
            bytes,
            availability,
        });
        let mut state = self.lock_state();
        if let Some(record) = state.records.get_mut(&key) {
            record.leases = record.leases.saturating_sub(1);
        }
        enforce_quota(&self.inner, &mut state);
        result
    }

    /// Opens a complete retained body while preventing circular eviction.
    ///
    /// # Errors
    /// Returns an unavailable error unless the exact body is terminal,
    /// complete, and readable.
    pub fn open_complete(
        &self,
        exchange_id: ExchangeId,
        boundary: ExchangeBoundary,
    ) -> Result<BodyReadLease, BodyStoreError> {
        let key = BodyKey {
            exchange_id,
            boundary: BoundaryKey::from_boundary(boundary),
        };
        let (path, source, memory, metadata) = {
            let mut state = self.lock_state();
            let record = state
                .records
                .get_mut(&key)
                .ok_or(BodyStoreError::UnknownBody)?;
            if record.availability != BodyAvailability::Complete || !record.terminal {
                return Err(BodyStoreError::ReadUnavailable);
            }
            let path = record.path.clone();
            let source = record.source.clone();
            record.leases = record.leases.saturating_add(1);
            (path, source, record.memory.clone(), record.metadata(key))
        };
        let reader = if let Some(source) = source {
            source.open()
        } else if let Some(chunks) = memory {
            Ok(Box::new(MemoryBodyReader::new(chunks)) as Box<dyn Read + Send>)
        } else if metadata.retained_bytes == 0 {
            Ok(Box::new(std::io::empty()) as Box<dyn Read + Send>)
        } else {
            path.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
                .and_then(std::fs::File::open)
                .map(|file| Box::new(file) as Box<dyn Read + Send>)
        };
        if let Ok(reader) = reader {
            Ok(BodyReadLease {
                reader,
                key,
                inner: Arc::clone(&self.inner),
                metadata,
            })
        } else {
            let mut state = self.lock_state();
            if let Some(record) = state.records.get_mut(&key) {
                record.leases = record.leases.saturating_sub(1);
            }
            Err(BodyStoreError::ReadUnavailable)
        }
    }

    /// Waits until all observer events accepted before this call have reached
    /// the storage worker.
    ///
    /// # Errors
    ///
    /// Returns [`BodyStoreError::WorkerUnavailable`] if the worker exited.
    pub fn flush(&self) -> Result<(), BodyStoreError> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(WorkerCommand::Flush(sender))
            .map_err(|_| BodyStoreError::WorkerUnavailable)?;
        receiver
            .recv()
            .map_err(|_| BodyStoreError::WorkerUnavailable)
    }

    /// Explicitly removes terminal cache blobs while leaving active writes
    /// alone. The count of deleted boundaries is returned.
    ///
    /// # Errors
    ///
    /// Returns a worker failure when preceding writes cannot be flushed.
    pub fn purge_terminal(&self) -> Result<usize, BodyStoreError> {
        self.flush()?;
        let mut state = self.lock_state();
        let keys = state
            .records
            .iter()
            .filter_map(|(key, record)| {
                (record.terminal && record.leases == 0 && record.source.is_none()).then_some(*key)
            })
            .collect::<Vec<_>>();
        for key in &keys {
            if let Some(record) = state.records.remove(key) {
                remove_record_file(&record);
                state.counters.retained_bytes = state
                    .counters
                    .retained_bytes
                    .saturating_sub(record.retained_bytes);
            }
        }
        state.terminal_order.retain(|key| !keys.contains(key));
        Ok(keys.len())
    }

    fn enqueue(&self, event: ObserverEvent) {
        let event = if self.inner.redact_sensitive.load(Ordering::Acquire) {
            event.redacted()
        } else {
            event
        };
        if let Err(
            mpsc::TrySendError::Full(WorkerCommand::Event(event))
            | mpsc::TrySendError::Disconnected(WorkerCommand::Event(event)),
        ) = self
            .inner
            .sender
            .try_send(WorkerCommand::Event(Box::new(event)))
        {
            mark_dropped_event(&self.inner, &event);
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Observer for BodyStore {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        self.enqueue(event);
        Box::pin(async { Ok(()) })
    }
}

fn worker_loop(inner: &std::sync::Weak<BodyStoreInner>, receiver: &mpsc::Receiver<WorkerCommand>) {
    while let Ok(command) = receiver.recv() {
        let Some(inner) = inner.upgrade() else {
            break;
        };
        match command {
            WorkerCommand::Event(event) => process_event(&inner, *event),
            WorkerCommand::Flush(done) => {
                let _ = done.send(());
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

fn process_event(inner: &BodyStoreInner, event: ObserverEvent) {
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior = state
        .last_sequences
        .insert(event.exchange_id, event.sequence)
        .unwrap_or(0);
    if prior > 0 && event.sequence > prior.saturating_add(1) {
        mark_exchange_loss(
            &mut state,
            event.exchange_id,
            "observer delivery sequence gap",
        );
    }
    match event.kind {
        ObserverEventKind::ExchangeStarted { .. } => {
            let retention = ExchangeRetention {
                mode: state.mode,
                requests: inner.retain_requests.load(Ordering::Acquire),
                request_limit: inner.request_limit.load(Ordering::Acquire),
                responses: inner.retain_responses.load(Ordering::Acquire),
            };
            state.exchange_modes.insert(event.exchange_id, retention);
        }
        ObserverEventKind::ResponseHeadObserved { boundary, head } => {
            observe_head(inner, &mut state, event.exchange_id, boundary, &head);
        }
        ObserverEventKind::RequestHeadObserved { boundary, head } => {
            observe_headers(
                inner,
                &mut state,
                event.exchange_id,
                boundary,
                &head.headers,
            );
        }
        ObserverEventKind::ResponseHeadFinalized(head) => observe_head(
            inner,
            &mut state,
            event.exchange_id,
            ExchangeBoundary::ClientResponse,
            &head,
        ),
        ObserverEventKind::BodyChunk(chunk) => {
            observe_chunk(inner, &mut state, event.exchange_id, chunk);
        }
        ObserverEventKind::Completed(_) => {
            finalize_exchange(inner, &mut state, event.exchange_id, true);
            state.exchange_modes.remove(&event.exchange_id);
            state.last_sequences.remove(&event.exchange_id);
            state.lossy_exchanges.remove(&event.exchange_id);
        }
        ObserverEventKind::Failed(failure) => {
            mark_exchange_loss(
                &mut state,
                event.exchange_id,
                &format!(
                    "exchange failed at {:?} ({:?}) before completion",
                    failure.stage, failure.kind
                ),
            );
            finalize_exchange(inner, &mut state, event.exchange_id, false);
            state.exchange_modes.remove(&event.exchange_id);
            state.last_sequences.remove(&event.exchange_id);
            state.lossy_exchanges.remove(&event.exchange_id);
        }
        _ => {}
    }
}

fn exchange_mode(
    inner: &BodyStoreInner,
    state: &StoreState,
    id: ExchangeId,
    direction: BodyDirection,
) -> RetentionMode {
    state
        .exchange_modes
        .get(&id)
        .copied()
        .unwrap_or(ExchangeRetention {
            mode: state.mode,
            requests: inner.retain_requests.load(Ordering::Acquire),
            request_limit: inner.request_limit.load(Ordering::Acquire),
            responses: inner.retain_responses.load(Ordering::Acquire),
        })
        .for_direction(direction)
}

fn observe_head(
    inner: &BodyStoreInner,
    state: &mut StoreState,
    exchange_id: ExchangeId,
    boundary: ExchangeBoundary,
    head: &ResponseHead,
) {
    observe_headers(inner, state, exchange_id, boundary, &head.headers);
    if let Some(record) = state.records.get_mut(&BodyKey {
        exchange_id,
        boundary: BoundaryKey::from_boundary(boundary),
    }) {
        record.response_head = Some(head.clone());
    }
}

fn observe_headers(
    inner: &BodyStoreInner,
    state: &mut StoreState,
    exchange_id: ExchangeId,
    boundary: ExchangeBoundary,
    headers: &HeaderBlock,
) {
    let key = BodyKey {
        exchange_id,
        boundary: BoundaryKey::from_boundary(boundary),
    };
    let mode = exchange_mode(inner, state, exchange_id, key.boundary.direction());
    let storage = state.buffer.storage;
    let record = state.records.entry(key).or_insert_with(|| {
        StoredBody::new(
            if mode == RetentionMode::Off {
                BodyAvailability::Disabled
            } else {
                BodyAvailability::Capturing
            },
            storage,
        )
    });
    let content_type = header_text(headers, "content-type");
    if let Some(content_type) = content_type {
        let mut parts = content_type.split(';');
        record.media_type = parts.next().map(|value| value.trim().to_ascii_lowercase());
        record.charset = parts.find_map(|parameter| {
            let (name, value) = parameter.split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case("charset")
                .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
        });
    }
    record.content_codings = crate::response_assets::asset_codings(headers);
}

fn observe_chunk(
    inner: &BodyStoreInner,
    state: &mut StoreState,
    exchange_id: ExchangeId,
    chunk: ObservedBodyChunk,
) {
    let key = BodyKey {
        exchange_id,
        boundary: BoundaryKey::from_boundary(chunk.boundary),
    };
    let mode = exchange_mode(inner, state, exchange_id, key.boundary.direction());
    let request_limit = state.exchange_modes.get(&exchange_id).map_or_else(
        || inner.request_limit.load(Ordering::Acquire),
        |retention| retention.request_limit,
    );
    let boundary_limit = if key.boundary.direction() == BodyDirection::Request && request_limit > 0
    {
        inner.config.max_body_bytes.min(request_limit)
    } else {
        inner.config.max_body_bytes
    };
    let lossy = state.lossy_exchanges.contains(&exchange_id);
    let storage = state.buffer.storage;
    let record = state.records.entry(key).or_insert_with(|| {
        StoredBody::new(
            if mode == RetentionMode::Off {
                BodyAvailability::Disabled
            } else {
                BodyAvailability::Capturing
            },
            storage,
        )
    });
    record.observed_bytes = record
        .observed_bytes
        .saturating_add(u64::try_from(chunk.byte_count).unwrap_or(u64::MAX));
    if mode == RetentionMode::Off {
        record.availability = BodyAvailability::Disabled;
        record.reason = Some("body retention was disabled".to_owned());
        return;
    }
    // An empty frame omits no bytes and needs no quota allocation.
    if chunk.byte_count == 0 && chunk.sample.as_ref().is_none_or(bytes::Bytes::is_empty) {
        return;
    }
    if lossy || chunk.truncated || chunk.sample.is_none() {
        record.availability = BodyAvailability::Lost;
        record
            .reason
            .get_or_insert_with(|| "observer did not provide every body byte".to_owned());
    }
    let Some(sample) = chunk.sample else {
        return;
    };
    if sample.is_empty() {
        return;
    }
    let per_body_remaining = boundary_limit.saturating_sub(record.retained_bytes);
    let desired = u64::try_from(sample.len())
        .unwrap_or(u64::MAX)
        .min(per_body_remaining);
    if desired == 0 {
        record.availability = BodyAvailability::Truncated;
        record.reason = Some("per-body retention limit reached".to_owned());
        return;
    }
    make_capacity(inner, state, desired, Some(key), mode);
    let aggregate_remaining = state
        .buffer
        .max_bytes
        .unwrap_or(u64::MAX)
        .saturating_sub(state.counters.retained_bytes);
    let take = usize::try_from(desired.min(aggregate_remaining)).unwrap_or(sample.len());
    if take == 0 {
        let record = state.records.get_mut(&key).expect("record exists");
        record.availability = if mode == RetentionMode::StopWhenFull {
            BodyAvailability::QuotaOmitted
        } else {
            BodyAvailability::Truncated
        };
        record.reason = Some("aggregate retention quota reached".to_owned());
        return;
    }
    let record = state.records.get_mut(&key).expect("record exists");
    if append_body(record, &inner.config.root, key, &sample[..take]).is_err() {
        record.availability = BodyAvailability::Lost;
        record.reason = Some("body cache write failed".to_owned());
        state.counters.storage_failures = state.counters.storage_failures.saturating_add(1);
        return;
    }
    record.retained_bytes = record
        .retained_bytes
        .saturating_add(u64::try_from(take).unwrap_or(u64::MAX));
    record.hasher.update(&sample[..take]);
    state.counters.retained_bytes = state
        .counters
        .retained_bytes
        .saturating_add(u64::try_from(take).unwrap_or(u64::MAX));
    if take < sample.len() || desired < u64::try_from(sample.len()).unwrap_or(u64::MAX) {
        record.availability = BodyAvailability::Truncated;
        record.reason = Some("body retention quota reached".to_owned());
    }
}

fn append_body(
    record: &mut StoredBody,
    root: &Path,
    key: BodyKey,
    bytes: &[u8],
) -> std::io::Result<()> {
    if let Some(chunks) = &mut record.memory {
        // Own exactly these bytes: a short retained prefix must not pin a larger source sample.
        chunks.push(bytes::Bytes::copy_from_slice(bytes));
    } else {
        let path = partial_path(root, key);
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(bytes)?;
        record.path = Some(path);
    }
    Ok(())
}

fn make_capacity(
    _inner: &BodyStoreInner,
    state: &mut StoreState,
    desired: u64,
    protected: Option<BodyKey>,
    mode: RetentionMode,
) {
    if mode != RetentionMode::Circular {
        return;
    }
    while state.counters.retained_bytes.saturating_add(desired)
        > state.buffer.max_bytes.unwrap_or(u64::MAX)
    {
        let Some(candidate) = state.terminal_order.iter().copied().find(|key| {
            Some(*key) != protected
                && state
                    .records
                    .get(key)
                    .is_some_and(|record| record.terminal && record.leases == 0)
        }) else {
            break;
        };
        state.terminal_order.retain(|key| *key != candidate);
        if let Some(record) = state.records.get_mut(&candidate) {
            if let Some(path) = record.path.take() {
                let _ = fs::remove_file(path);
            }
            state.counters.retained_bytes = state
                .counters
                .retained_bytes
                .saturating_sub(record.retained_bytes);
            record.memory = None;
            record.retained_bytes = 0;
            record.availability = BodyAvailability::Evicted;
            record.reason = Some("evicted by circular response-body quota".to_owned());
            record.sha256 = None;
            state.counters.evicted_bodies = state.counters.evicted_bodies.saturating_add(1);
        }
    }
}

fn enforce_quota(inner: &BodyStoreInner, state: &mut StoreState) {
    make_capacity(inner, state, 0, None, RetentionMode::Circular);
}

fn finalize_exchange(
    inner: &BodyStoreInner,
    state: &mut StoreState,
    exchange_id: ExchangeId,
    completed: bool,
) {
    let keys = state
        .records
        .keys()
        .copied()
        .filter(|key| key.exchange_id == exchange_id)
        .collect::<Vec<_>>();
    for key in keys {
        let Some(record) = state.records.get_mut(&key) else {
            continue;
        };
        record.terminal = true;
        if let Some(path) = record.path.clone() {
            let final_path = final_path(&inner.config.root, key);
            if fs::rename(&path, &final_path).is_ok() {
                record.path = Some(final_path);
            } else {
                record.availability = BodyAvailability::Lost;
                record.reason = Some("body cache finalization failed".to_owned());
                state.counters.storage_failures = state.counters.storage_failures.saturating_add(1);
            }
        }
        if record.retained_bytes > 0 {
            record.sha256 = Some(format!("{:x}", record.hasher.clone().finalize()));
            state.terminal_order.push_back(key);
        }
        if record.availability == BodyAvailability::Capturing {
            record.availability = if completed {
                BodyAvailability::Complete
            } else {
                BodyAvailability::Lost
            };
        }
    }
    enforce_quota(inner, state);
}

fn mark_dropped_event(inner: &BodyStoreInner, event: &ObserverEvent) {
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.counters.dropped_events = state.counters.dropped_events.saturating_add(1);
    state.lossy_exchanges.insert(event.exchange_id);
    if let ObserverEventKind::BodyChunk(chunk) = &event.kind {
        let key = BodyKey {
            exchange_id: event.exchange_id,
            boundary: BoundaryKey::from_boundary(chunk.boundary),
        };
        let storage = state.buffer.storage;
        let record = state
            .records
            .entry(key)
            .or_insert_with(|| StoredBody::new(BodyAvailability::Lost, storage));
        record.observed_bytes = record
            .observed_bytes
            .saturating_add(u64::try_from(chunk.byte_count).unwrap_or(u64::MAX));
    }
    mark_exchange_loss(&mut state, event.exchange_id, "body-store queue saturated");
}

fn mark_exchange_loss(state: &mut StoreState, exchange_id: ExchangeId, reason: &str) {
    state.lossy_exchanges.insert(exchange_id);
    for (key, record) in &mut state.records {
        if key.exchange_id == exchange_id
            && !record.terminal
            && matches!(
                record.availability,
                BodyAvailability::Capturing | BodyAvailability::Complete | BodyAvailability::Lost
            )
        {
            record.availability = BodyAvailability::Lost;
            record.reason.get_or_insert_with(|| reason.to_owned());
        }
    }
}

fn header_text(headers: &HeaderBlock, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|field| field.name_eq(name))
        .and_then(|field| std::str::from_utf8(field.value()).ok())
        .map(ToOwned::to_owned)
}

fn partial_path(root: &Path, key: BodyKey) -> PathBuf {
    root.join(format!(
        "{:032x}-{}.part",
        key.exchange_id.0,
        key.boundary.name()
    ))
}

fn final_path(root: &Path, key: BodyKey) -> PathBuf {
    root.join(format!(
        "{:032x}-{}.body",
        key.exchange_id.0,
        key.boundary.name()
    ))
}

fn cleanup_owned_files(root: &Path) -> Result<(), BodyStoreError> {
    for entry in fs::read_dir(root).map_err(|_| BodyStoreError::DirectoryUnavailable)? {
        let entry = entry.map_err(|_| BodyStoreError::DirectoryUnavailable)?;
        let path = entry.path();
        let owned = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "part" | "body" | "preview"));
        if owned && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            fs::remove_file(path).map_err(|_| BodyStoreError::DirectoryUnavailable)?;
        }
    }
    Ok(())
}

fn read_file_range(path: &Path, offset: u64, length: usize) -> Result<Vec<u8>, BodyStoreError> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| BodyStoreError::ReadUnavailable)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| BodyStoreError::ReadUnavailable)?;
    let mut bytes = vec![0; length];
    let read = file
        .read(&mut bytes)
        .map_err(|_| BodyStoreError::ReadUnavailable)?;
    bytes.truncate(read);
    Ok(bytes)
}

fn remove_record_file(record: &StoredBody) {
    if let Some(path) = &record.path {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::SystemTime};

    use bytes::Bytes;
    use transmog_core::{
        ConnectionId, HeaderField, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{CompletedExchange, ExchangeMetadata},
        observe::ObservedBodyChunk,
    };

    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "transmog-body-store-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn config(root: PathBuf, max_bytes: u64) -> BodyStoreConfig {
        BodyStoreConfig {
            root,
            storage: BufferStorage::Disk,
            use_product_preferences: false,
            mode: RetentionMode::Circular,
            max_bytes,
            max_body_bytes: max_bytes,
            queue_capacity: NonZeroUsize::new(8).unwrap(),
            max_read_bytes: 1024,
            retain_requests: false,
        }
    }

    fn event(id: u128, sequence: u64, kind: ObserverEventKind) -> ObserverEvent {
        ObserverEvent {
            exchange_id: ExchangeId(id),
            sequence,
            kind,
        }
    }

    fn started(id: u128) -> ObserverEvent {
        event(
            id,
            1,
            ObserverEventKind::ExchangeStarted {
                metadata: metadata(id),
            },
        )
    }

    fn metadata(id: u128) -> Arc<ExchangeMetadata> {
        let session = SessionMetadata {
            session_id: SessionId(id),
            downstream_connection_id: ConnectionId(id),
            stream_id: StreamId(id),
            client_addr: "127.0.0.1:1".parse::<SocketAddr>().unwrap(),
            client_identity: transmog_core::ClientIdentity::default(),
            proxy_addr: "127.0.0.1:2".parse::<SocketAddr>().unwrap(),
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
        };
        Arc::new(ExchangeMetadata::from_session(
            &session,
            Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/".to_owned(),
                query: None,
            },
        ))
    }

    fn head(id: u128, sequence: u64) -> ObserverEvent {
        event(
            id,
            sequence,
            ObserverEventKind::ResponseHeadObserved {
                boundary: ExchangeBoundary::UpstreamResponse,
                head: transmog_core::ResponseHead {
                    status: 200,
                    source_version: HttpLegVersion::Http1,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("content-type", "text/plain; charset=utf-8").unwrap(),
                        HeaderField::try_new("content-encoding", "br").unwrap(),
                    ]),
                },
            },
        )
    }

    fn chunk(id: u128, sequence: u64, bytes: &'static [u8]) -> ObserverEvent {
        event(
            id,
            sequence,
            ObserverEventKind::BodyChunk(ObservedBodyChunk {
                boundary: ExchangeBoundary::UpstreamResponse,
                byte_count: bytes.len(),
                sample: Some(Bytes::from_static(bytes)),
                truncated: false,
            }),
        )
    }

    fn completed(id: u128, sequence: u64) -> ObserverEvent {
        event(
            id,
            sequence,
            ObserverEventKind::Completed(CompletedExchange {
                metadata: metadata(id),
                request_head: transmog_core::RequestHead {
                    method: "GET".to_owned(),
                    target: Target {
                        scheme: "https".to_owned(),
                        authority: "example.test".to_owned(),
                        host: "example.test".to_owned(),
                        port: 443,
                        path: "/".to_owned(),
                        query: None,
                    },
                    source_version: HttpLegVersion::Http1,
                    headers: HeaderBlock::default(),
                },
                response_head: transmog_core::ResponseHead {
                    status: 200,
                    source_version: HttpLegVersion::Http1,
                    headers: HeaderBlock::default(),
                },
            }),
        )
    }

    fn push(store: &BodyStore, events: impl IntoIterator<Item = ObserverEvent>) {
        for event in events {
            store.enqueue(event);
        }
        store.flush().unwrap();
    }

    #[test]
    fn memory_buffer_never_creates_files_and_leases_protect_chunks() {
        let root = root("memory-only").join("must-not-exist");
        let mut settings = config(root.clone(), 8);
        settings.storage = BufferStorage::Memory;
        let store = BodyStore::new(settings).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 1),
                chunk(1, 2, b"secret"),
                completed(1, 3),
            ],
        );
        assert!(!root.exists());
        let mut lease = store
            .open_complete(ExchangeId(1), ExchangeBoundary::UpstreamResponse)
            .unwrap();
        let mut text = String::new();
        lease.read_to_string(&mut text).unwrap();
        assert_eq!(text, "secret");
        assert_eq!(
            store
                .read_range(ExchangeId(1), ExchangeBoundary::UpstreamResponse, 2, 3)
                .unwrap()
                .bytes,
            b"cre"
        );
        push(
            &store,
            [
                started(2),
                head(2, 1),
                chunk(2, 2, b"newest"),
                completed(2, 3),
            ],
        );
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Complete
        );
        drop(lease);
        push(
            &store,
            [
                started(3),
                head(3, 1),
                chunk(3, 2, b"newest"),
                completed(3, 3),
            ],
        );
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Evicted
        );
        assert!(!root.exists());
    }

    #[test]
    fn buffer_policy_uses_installed_ram_and_only_large_limits_write_disk() {
        assert_eq!(
            BufferLimit::Automatic.resolve(Some(32_000)).max_bytes,
            Some(16_000)
        );
        assert_eq!(
            BufferLimit::Custom { bytes: 16_000 }
                .resolve(Some(32_000))
                .storage,
            BufferStorage::Memory
        );
        assert_eq!(
            BufferLimit::Custom { bytes: 16_001 }
                .resolve(Some(32_000))
                .storage,
            BufferStorage::Disk
        );
        assert_eq!(BufferLimit::Unlimited.resolve(Some(32_000)).max_bytes, None);
        assert_eq!(
            BufferLimit::Unlimited.resolve(Some(32_000)).storage,
            BufferStorage::Disk
        );
    }

    #[test]
    fn request_limit_snapshots_and_unlimited_keeps_the_aggregate_budget() {
        let root = root("request-cap");
        let store = BodyStore::new(config(root.clone(), 64)).unwrap();
        store.set_privacy(true, true, false);
        store.set_request_body_limit(Some(16));
        push(&store, [started(1)]);
        store.set_request_body_limit(None);
        let request_chunk = |id, sequence, len| {
            event(
                id,
                sequence,
                ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary: ExchangeBoundary::ClientRequest,
                    byte_count: len,
                    sample: Some(Bytes::from(vec![b'x'; len])),
                    truncated: false,
                }),
            )
        };
        push(&store, [request_chunk(1, 2, 32), completed(1, 3)]);
        let first = store.metadata(ExchangeId(1));
        assert_eq!(first[0].observed_bytes, 32);
        assert_eq!(first[0].retained_bytes, 16);
        assert_eq!(first[0].availability, BodyAvailability::Truncated);
        push(
            &store,
            [started(2), request_chunk(2, 2, 48), completed(2, 3)],
        );
        let second = store.metadata(ExchangeId(2));
        assert_eq!(second[0].retained_bytes, 48);
        assert_eq!(second[0].availability, BodyAvailability::Complete);
        push(
            &store,
            [started(3), request_chunk(3, 2, 80), completed(3, 3)],
        );
        let third = store.metadata(ExchangeId(3));
        assert_eq!(third[0].observed_bytes, 80);
        assert_eq!(third[0].retained_bytes, 64);
        assert_eq!(third[0].availability, BodyAvailability::Truncated);
        assert!(store.counters().retained_bytes <= 64);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn content_coding_metadata_preserves_repeated_header_fields_in_order() {
        let root = root("coding-stack");
        let store = BodyStore::new(config(root.clone(), 32)).unwrap();
        let mut response = head(1, 2);
        if let ObserverEventKind::ResponseHeadObserved { head, .. } = &mut response.kind {
            head.headers
                .push(HeaderField::try_new("Content-Encoding", "gzip, zstd").unwrap());
        }
        push(
            &store,
            [started(1), response, chunk(1, 3, b"hello"), completed(1, 4)],
        );
        assert_eq!(
            store.metadata(ExchangeId(1))[0].content_codings,
            ["br", "gzip", "zstd"]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn complete_response_is_range_readable_with_representation_metadata() {
        let root = root("read");
        let store = BodyStore::new(config(root.clone(), 32)).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 2),
                chunk(1, 3, b"hello"),
                completed(1, 4),
            ],
        );
        let metadata = store.metadata(ExchangeId(1));
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].availability, BodyAvailability::Complete);
        assert_eq!(metadata[0].media_type.as_deref(), Some("text/plain"));
        assert_eq!(metadata[0].charset.as_deref(), Some("utf-8"));
        assert_eq!(metadata[0].content_codings, ["br"]);
        assert_eq!(
            store
                .read_range(ExchangeId(1), ExchangeBoundary::UpstreamResponse, 1, 3)
                .unwrap()
                .bytes,
            b"ell"
        );
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_frames_do_not_truncate_complete_bodies_or_consume_quota() {
        let root = root("empty-frames");
        let store = BodyStore::new(config(root.clone(), 4)).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 2),
                chunk(1, 3, b""),
                chunk(1, 4, b"1234"),
                chunk(1, 5, b""),
                completed(1, 6),
            ],
        );
        let body = &store.metadata(ExchangeId(1))[0];
        assert_eq!(body.availability, BodyAvailability::Complete, "{body:?}");
        assert_eq!(body.observed_bytes, 4);
        assert_eq!(body.retained_bytes, 4);
        assert_eq!(body.reason, None);
        assert_eq!(
            store
                .read_range(ExchangeId(1), ExchangeBoundary::UpstreamResponse, 0, 4)
                .unwrap()
                .bytes,
            b"1234"
        );
        push(
            &store,
            [started(2), head(2, 2), chunk(2, 3, b""), completed(2, 4)],
        );
        let body = &store.metadata(ExchangeId(2))[0];
        assert_eq!(body.availability, BodyAvailability::Complete, "{body:?}");
        assert_eq!(body.retained_bytes, 0);
        assert_eq!(body.reason, None);
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Complete
        );
        assert_eq!(store.counters().retained_bytes, 4);
        assert_eq!(store.counters().evicted_bodies, 0);
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nonempty_html_with_an_empty_final_frame_stays_complete_under_default_limits() {
        let root = root("html-empty-final-frame");
        let store = BodyStore::new(BodyStoreConfig::product_default(root.clone())).unwrap();
        let body = Bytes::from(format!(
            "<!DOCTYPE html><html><title>Error 403</title><div>{}</div>",
            "x".repeat(1772)
        ));
        push(
            &store,
            [
                started(1),
                event(
                    1,
                    2,
                    ObserverEventKind::ResponseHeadObserved {
                        boundary: ExchangeBoundary::UpstreamResponse,
                        head: ResponseHead {
                            status: 403,
                            source_version: HttpLegVersion::Http2,
                            headers: HeaderBlock::from_fields(vec![
                                HeaderField::try_new("content-type", "text/html; charset=utf-8")
                                    .unwrap(),
                            ]),
                        },
                    },
                ),
                event(
                    1,
                    3,
                    ObserverEventKind::BodyChunk(ObservedBodyChunk {
                        boundary: ExchangeBoundary::UpstreamResponse,
                        byte_count: body.len(),
                        sample: Some(body.clone()),
                        truncated: false,
                    }),
                ),
                chunk(1, 4, b""),
                completed(1, 5),
            ],
        );
        let retained = store.metadata(ExchangeId(1));
        assert_eq!(
            retained[0].availability,
            BodyAvailability::Complete,
            "{:?}",
            retained[0]
        );
        assert_eq!(retained[0].observed_bytes, body.len() as u64);
        assert_eq!(retained[0].retained_bytes, body.len() as u64);
        assert_eq!(retained[0].reason, None);
        assert_eq!(
            store
                .read_range(
                    ExchangeId(1),
                    ExchangeBoundary::UpstreamResponse,
                    0,
                    body.len()
                )
                .unwrap()
                .bytes,
            body
        );
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delivery_gaps_preserve_the_specific_retention_reason() {
        let root = root("reasons");
        let store = BodyStore::new(config(root.clone(), 4)).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 2),
                chunk(1, 4, b"1234"),
                completed(1, 5),
            ],
        );
        let body = &store.metadata(ExchangeId(1))[0];
        assert_eq!(body.availability, BodyAvailability::Lost);
        assert_eq!(
            body.reason.as_deref(),
            Some("observer delivery sequence gap")
        );
        push(
            &store,
            [
                started(2),
                head(2, 2),
                chunk(2, 3, b"123456"),
                completed(2, 5),
            ],
        );
        let body = &store.metadata(ExchangeId(2))[0];
        assert_eq!(body.availability, BodyAvailability::Truncated);
        assert_eq!(body.reason.as_deref(), Some("body retention quota reached"));
        store.set_mode(RetentionMode::Off);
        push(
            &store,
            [
                started(3),
                head(3, 2),
                chunk(3, 4, b"1234"),
                completed(3, 5),
            ],
        );
        let body = &store.metadata(ExchangeId(3))[0];
        assert_eq!(body.availability, BodyAvailability::Disabled);
        assert_eq!(body.reason.as_deref(), Some("body retention was disabled"));
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn incomplete_compressed_body_explains_why_and_keeps_raw_hex_available() {
        let root = root("incomplete-preview");
        let store = BodyStore::new(config(root.clone(), 4)).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 2),
                chunk(1, 3, b"123456"),
                completed(1, 4),
            ],
        );
        let mut request = crate::BodyInspectionRequest {
            session_id: format!("{:032x}", 1),
            boundary: "upstream-response".to_owned(),
            representation: crate::BodyRepresentation::Auto,
            decode_content: true,
            offset: 0,
            max_bytes: Some(16),
        };
        let error = crate::inspector::inspect_body(Some(&store), request.clone())
            .await
            .unwrap_err();
        assert!(error.message.contains("4 of 6 observed bytes retained"));
        assert!(error.message.contains("body retention quota reached"));
        request.representation = crate::BodyRepresentation::Bytes;
        request.decode_content = false;
        let hex = crate::inspector::inspect_body(Some(&store), request.clone())
            .await
            .unwrap();
        assert_eq!(hex.display_bytes, 4);
        assert_eq!(hex.representation, "bytes");
        assert!(!hex.decoded);
        assert_eq!(hex.bytes_base64.as_deref(), Some("MTIzNA=="));
        assert_eq!(hex.byte_offset, 0);
        request.representation = crate::BodyRepresentation::Metadata;
        let metadata = crate::inspector::inspect_body(Some(&store), request)
            .await
            .unwrap()
            .metadata;
        assert_eq!(metadata.availability, BodyAvailability::Truncated);
        assert_eq!(metadata.retained_bytes, 4);
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn byte_preview_returns_exact_bounded_bytes_and_offsets() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let root = root("byte-preview");
        let store = BodyStore::new(config(root.clone(), 32)).unwrap();
        push(
            &store,
            [
                started(1),
                head(1, 2),
                chunk(1, 3, b"\0A\xffBCD"),
                completed(1, 4),
            ],
        );
        let mut request = crate::BodyInspectionRequest {
            session_id: format!("{:032x}", 1),
            boundary: "upstream-response".to_owned(),
            representation: crate::BodyRepresentation::Auto,
            decode_content: false,
            offset: 0,
            max_bytes: Some(4),
        };
        let automatic = crate::inspector::inspect_body(Some(&store), request.clone())
            .await
            .unwrap();
        assert_eq!(automatic.representation, "bytes");
        assert_eq!(
            STANDARD.decode(automatic.bytes_base64.unwrap()).unwrap(),
            b"\0A\xffB"
        );
        assert_eq!(automatic.byte_offset, 0);
        assert_eq!(automatic.display_bytes, 4);
        assert_eq!(automatic.next_offset, Some(4));
        assert!(automatic.truncated);
        request.representation = crate::BodyRepresentation::Bytes;
        request.offset = 2;
        request.max_bytes = Some(3);
        let range = crate::inspector::inspect_body(Some(&store), request.clone())
            .await
            .unwrap();
        assert_eq!(
            STANDARD.decode(range.bytes_base64.unwrap()).unwrap(),
            b"\xffBC"
        );
        assert_eq!(range.byte_offset, 2);
        assert_eq!(range.display_bytes, 3);
        assert_eq!(range.next_offset, Some(5));
        request.representation = crate::BodyRepresentation::Metadata;
        let metadata = crate::inspector::inspect_body(Some(&store), request)
            .await
            .unwrap();
        assert!(metadata.bytes_base64.is_none());
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn protected_client_response_head_survives_without_exposing_it_in_metadata() {
        let root = root("protected-head");
        let store = BodyStore::new(config(root.clone(), 32)).unwrap();
        let response_head = ResponseHead {
            status: 204,
            source_version: HttpLegVersion::Http1,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("set-cookie", "session=secret").unwrap(),
                HeaderField::try_new("content-type", "text/plain").unwrap(),
            ]),
        };
        push(
            &store,
            [
                started(9),
                event(
                    9,
                    2,
                    ObserverEventKind::ResponseHeadObserved {
                        boundary: ExchangeBoundary::ClientResponse,
                        head: response_head.clone(),
                    },
                ),
                completed(9, 3),
            ],
        );
        let metadata = store.metadata(ExchangeId(9));
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].boundary, "client-response");
        assert_eq!(metadata[0].availability, BodyAvailability::Complete);
        assert_eq!(metadata[0].retained_bytes, 0);
        let retained = store
            .response_head(ExchangeId(9), ExchangeBoundary::ClientResponse)
            .unwrap();
        assert_eq!(retained.status, 204);
        assert_eq!(
            retained.headers.values("set-cookie").next(),
            Some(&b"session=secret"[..])
        );
        assert!(!serde_json::to_string(&metadata).unwrap().contains("secret"));
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn circular_mode_evicts_oldest_terminal_blob_but_keeps_metadata() {
        let root = root("evict");
        let store = BodyStore::new(config(root.clone(), 5)).unwrap();
        push(&store, [started(1), chunk(1, 2, b"12345"), completed(1, 3)]);
        push(&store, [started(2), chunk(2, 2, b"abcde"), completed(2, 3)]);
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Evicted
        );
        assert_eq!(
            store.metadata(ExchangeId(2))[0].availability,
            BodyAvailability::Complete
        );
        assert_eq!(store.counters().retained_bytes, 5);
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disabled_mode_keeps_metadata_and_explicit_purge_is_separate() {
        let root = root("off");
        let mut settings = config(root.clone(), 32);
        settings.mode = RetentionMode::Off;
        let store = BodyStore::new(settings).unwrap();
        push(
            &store,
            [started(1), chunk(1, 2, b"secret"), completed(1, 3)],
        );
        let metadata = store.metadata(ExchangeId(1));
        assert_eq!(metadata[0].availability, BodyAvailability::Disabled);
        assert_eq!(metadata[0].retained_bytes, 0);
        assert_eq!(store.purge_terminal().unwrap(), 1);
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn per_body_and_read_limits_fail_closed() {
        let root = root("limits");
        let store = BodyStore::new(config(root.clone(), 4)).unwrap();
        push(
            &store,
            [started(1), chunk(1, 2, b"oversized"), completed(1, 3)],
        );
        let metadata = store.metadata(ExchangeId(1));
        assert_eq!(metadata[0].availability, BodyAvailability::Truncated);
        assert_eq!(metadata[0].retained_bytes, 4);
        assert_eq!(
            store
                .read_range(ExchangeId(1), ExchangeBoundary::UpstreamResponse, 0, 1025)
                .unwrap_err(),
            BodyStoreError::ReadLimitExceeded
        );
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retention_changes_apply_only_to_new_exchanges() {
        let root = root("policy-snapshot");
        let store = BodyStore::new(config(root.clone(), 32)).unwrap();
        store.enqueue(started(1));
        store.flush().unwrap();
        store.set_mode(RetentionMode::Off);
        push(&store, [chunk(1, 2, b"kept"), completed(1, 3)]);
        push(
            &store,
            [started(2), chunk(2, 2, b"omitted"), completed(2, 3)],
        );
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Complete
        );
        assert_eq!(
            store.metadata(ExchangeId(2))[0].availability,
            BodyAvailability::Disabled
        );
        drop(store);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn request_privacy_toggle_never_revives_a_partially_omitted_body() {
        let root = root("request-privacy");
        let store = BodyStore::new(config(root.clone(), 1024)).unwrap();
        push(&store, [started(1)]);
        store.set_privacy(true, true, false);
        let mut omitted = chunk(1, 2, b"omitted");
        if let ObserverEventKind::BodyChunk(ref mut chunk) = omitted.kind {
            chunk.boundary = ExchangeBoundary::ClientRequest;
        }
        push(&store, [omitted, completed(1, 3)]);
        assert_eq!(
            store.metadata(ExchangeId(1))[0].availability,
            BodyAvailability::Disabled
        );
        let mut kept = chunk(2, 2, b"kept");
        if let ObserverEventKind::BodyChunk(ref mut chunk) = kept.kind {
            chunk.boundary = ExchangeBoundary::ClientRequest;
        }
        push(&store, [started(2), kept, completed(2, 3)]);
        assert_eq!(
            store.metadata(ExchangeId(2))[0].availability,
            BodyAvailability::Complete
        );
        drop(store);
        let _ = fs::remove_dir_all(root);
    }
}
