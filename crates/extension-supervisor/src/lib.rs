//! Explicit worker activation, exact input grants, authenticated IPC and bounded termination.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::BufReader,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use transmog_extensions::{
    BytePage, ContextRequest, Direction, Failure, FailureKind, HOST_PROTOCOL_VERSION, HostReply,
    HostRequest, HostToken, Invocation, PAYLOAD_CHUNK_BYTES, ReadRequest, Snapshot,
    VisualizationInput, VisualizationStep, Visualizer, check_body_access, decimal, project_headers,
    read_frame, valid_id, write_frame,
};
const MAX_HOSTS: u64 = 16;
mod context;
mod decoding;
#[cfg(test)]
mod tests;
static ACTIVE_HOSTS: AtomicU64 = AtomicU64::new(0);
static PACKAGES: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
struct Slot {
    package: String,
}
impl Slot {
    fn acquire(package: &str) -> Result<Self, Failure> {
        let mut packages = PACKAGES.lock().map_err(|_| unavailable())?;
        if packages.get(package).copied().unwrap_or(0) >= 8 {
            return Err(fail(
                FailureKind::Limit,
                "This extension is using all its worker slots",
            ));
        }
        ACTIVE_HOSTS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_HOSTS).then_some(n + 1)
            })
            .map_err(|_| fail(FailureKind::Limit, "All visualizer worker slots are in use"))?;
        *packages.entry(package.into()).or_default() += 1;
        Ok(Self {
            package: package.into(),
        })
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut packages) = PACKAGES.lock()
            && let Some(count) = packages.get_mut(&self.package)
        {
            *count -= 1;
            if *count == 0 {
                packages.remove(&self.package);
            }
        }
        ACTIVE_HOSTS.fetch_sub(1, Ordering::AcqRel);
    }
}
/// Exact helper executable. Production execution always requires the native sandbox.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Absolute installed helper path.
    pub executable: PathBuf,
}
/// One active instance; drop/stop removes its code and parser state immediately.
pub struct Runner {
    sender: SyncSender<Option<Work>>,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
    validated_document: AtomicBool,
    next_id: AtomicU64,
    visualizer: Visualizer,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Runner {
    /// Starts only the explicitly selected contribution from an exact verified revision.
    /// # Errors
    /// Returns containment, validation, capacity or initialization failures.
    pub fn start(
        config: &HostConfig,
        snapshot: Snapshot,
        visualizer_id: &str,
    ) -> Result<Arc<Self>, Failure> {
        Self::start_cancellable(config, snapshot, visualizer_id, || false)
    }
    /// Starts a contribution while allowing its owner to cancel pending initialization.
    /// # Errors
    /// Returns canceled, containment, validation, capacity or initialization failures.
    pub fn start_cancellable(
        config: &HostConfig,
        snapshot: Snapshot,
        visualizer_id: &str,
        canceled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, Failure> {
        if canceled() {
            return Err(unavailable());
        }
        let expected = snapshot.digest.clone();
        let snapshot = Snapshot::new(snapshot.manifest, snapshot.files)
            .map_err(|_| fail(FailureKind::Validation, "Invalid execution snapshot"))?;
        if snapshot.digest != expected {
            return Err(fail(
                FailureKind::Validation,
                "Execution revision identity changed",
            ));
        }
        let visualizer = snapshot
            .manifest
            .visualizers
            .iter()
            .find(|v| v.id == visualizer_id)
            .ok_or_else(|| fail(FailureKind::Validation, "Unknown visualizer"))?
            .clone();
        let slot = Slot::acquire(&snapshot.manifest.id)?;
        let mut token = [0; 32];
        getrandom::fill(&mut token).map_err(|_| {
            fail(
                FailureKind::Unavailable,
                "Worker authentication is unavailable",
            )
        })?;
        let token = HostToken(token);
        let child = launch(config, visualizer.limits.heap_bytes)?;
        let process = Arc::new(Mutex::new(Some(child)));
        let pipes = (|| {
            let mut guard = process.lock().map_err(|_| unavailable())?;
            let child = guard.as_mut().ok_or_else(unavailable)?;
            Ok::<_, Failure>((
                child.stdin.take().ok_or_else(unavailable)?,
                child.stdout.take().ok_or_else(unavailable)?,
            ))
        })();
        let (stdin, stdout) = match pipes {
            Ok(p) => p,
            Err(e) => {
                kill(&process);
                return Err(e);
            }
        };
        let failed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(8);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = Worker {
            _slot: slot,
            input: stdin,
            output: BufReader::new(stdout),
            token,
            snapshot,
            visualizer: visualizer.clone(),
            receiver,
            ready: ready_tx,
            process: Arc::clone(&process),
            failed: Arc::clone(&failed),
        };
        let thread = std::thread::Builder::new()
            .name("transmog-visualizer-io".into())
            .spawn(move || worker.run())
            .map_err(|_| {
                kill(&process);
                fail(
                    FailureKind::Unavailable,
                    "Worker supervisor thread unavailable",
                )
            })?;
        let ready = wait_ready(&ready_rx, canceled);
        match ready {
            Ok(()) => Ok(Arc::new(Self {
                sender,
                process,
                failed,
                validated_document: AtomicBool::new(false),
                next_id: AtomicU64::new(1),
                visualizer,
                thread: Mutex::new(Some(thread)),
            })),
            Err(e) => {
                failed.store(true, Ordering::Release);
                kill(&process);
                // Initialization may have completed concurrently with cancellation.
                // Close the idle work queue as well as the process before joining.
                drop(sender);
                let _ = thread.join();
                Err(e)
            }
        }
    }
    /// Runs one bounded synchronous parser operation asynchronously to its UI owner.
    /// # Errors
    /// Returns safe script, contract, resource or deadline failures.
    pub async fn invoke(&self, invocation: Invocation) -> Result<VisualizationStep, Failure> {
        if self.failed.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(Some(Work {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                invocation,
                reply: tx,
            }))
            .map_err(|_| fail(FailureKind::Limit, "The visualizer operation queue is full"))?;
        match tokio::time::timeout(
            Duration::from_millis(self.visualizer.limits.duration_ms),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => {
                if matches!(&result, Ok(VisualizationStep::Document { .. })) {
                    self.validated_document.store(true, Ordering::Release);
                }
                result
            }
            Ok(Err(_)) => {
                self.stop();
                Err(unavailable())
            }
            Err(_) => {
                self.stop();
                Err(fail(
                    FailureKind::Timeout,
                    "The visualizer operation timed out",
                ))
            }
        }
    }
    /// Cancels queued work and terminates the worker; it cannot be resumed after this call.
    pub fn stop(&self) {
        self.failed.store(true, Ordering::Release);
        let _ = self.sender.try_send(None);
        kill(&self.process);
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
    /// Whether the owning view must create a fresh instance before invoking again.
    pub fn is_stopped(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// Whether this exact live worker has returned a host-validated document.
    /// Initialization alone does not qualify a development revision for publication.
    pub fn has_validated_document(&self) -> bool {
        self.validated_document.load(Ordering::Acquire) && !self.is_stopped()
    }
}
fn wait_ready(
    receiver: &Receiver<Result<(), Failure>>,
    canceled: impl Fn() -> bool,
) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if canceled() {
            return Err(unavailable());
        }
        if std::time::Instant::now() >= deadline {
            return Err(fail(
                FailureKind::Timeout,
                "Visualizer initialization timed out",
            ));
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => {
                return if canceled() {
                    Err(unavailable())
                } else {
                    result
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(unavailable()),
        }
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Work {
    id: u64,
    invocation: Invocation,
    reply: tokio::sync::oneshot::Sender<Result<VisualizationStep, Failure>>,
}
struct Worker {
    _slot: Slot,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    token: HostToken,
    snapshot: Snapshot,
    visualizer: Visualizer,
    receiver: Receiver<Option<Work>>,
    ready: SyncSender<Result<(), Failure>>,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
}
impl Worker {
    fn run(mut self) {
        let init = self.initialize();
        let initialized = init.is_ok();
        let _ = self.ready.send(init);
        if initialized {
            let mut grants = BTreeMap::new();
            let mut pending = Pending::default();
            let mut remaining = self.visualizer.limits.input_bytes;
            let mut continuations = 0;
            while let Ok(Some(work)) = self.receiver.recv() {
                if self.failed.load(Ordering::Acquire) {
                    let _ = work.reply.send(Err(unavailable()));
                    break;
                }
                let result = (|| -> Result<VisualizationStep, Failure> {
                    let invocation = prepare(
                        work.invocation,
                        &self.visualizer,
                        &mut grants,
                        &mut pending,
                        &mut remaining,
                        &mut continuations,
                    )?;
                    write_frame(
                        &mut self.input,
                        &HostRequest::Invoke {
                            token: self.token,
                            request_id: work.id,
                            invocation,
                        },
                    )
                    .map_err(|_| protocol())?;
                    let result = match read_frame::<HostReply>(&mut self.output) {
                        Ok(HostReply::Result {
                            token,
                            request_id,
                            result,
                        }) if token == self.token && request_id == work.id => result,
                        _ => Err(protocol()),
                    }?;
                    validate_step(&result, &self.visualizer, &grants, &mut pending, remaining)?;
                    Ok(result)
                })();
                let fatal = result
                    .as_ref()
                    .is_err_and(|e| matches!(e.kind, FailureKind::Protocol | FailureKind::Limit));
                if fatal {
                    self.failed.store(true, Ordering::Release);
                }
                let _ = work.reply.send(result);
                if fatal {
                    break;
                }
            }
        }
        self.failed.store(true, Ordering::Release);
        kill(&self.process);
    }
    fn initialize(&mut self) -> Result<(), Failure> {
        write_frame(
            &mut self.input,
            &HostRequest::Initialize {
                protocol_version: HOST_PROTOCOL_VERSION,
                token: self.token,
                manifest: self.snapshot.manifest.clone(),
                visualizer_id: self.visualizer.id.clone(),
            },
        )
        .map_err(|_| protocol())?;
        for (path, bytes) in &self.snapshot.files {
            for (index, chunk) in bytes.chunks(PAYLOAD_CHUNK_BYTES).enumerate() {
                write_frame(
                    &mut self.input,
                    &HostRequest::Payload {
                        token: self.token,
                        path: path.clone(),
                        offset: (index * PAYLOAD_CHUNK_BYTES).to_string(),
                        bytes_base64: STANDARD.encode(chunk),
                    },
                )
                .map_err(|_| protocol())?;
            }
        }
        write_frame(&mut self.input, &HostRequest::Start { token: self.token })
            .map_err(|_| protocol())?;
        match read_frame::<HostReply>(&mut self.output) {
            Ok(HostReply::Ready { token }) if token == self.token => Ok(()),
            Ok(HostReply::Fatal {
                token: Some(token),
                failure,
            }) if token == self.token => Err(failure),
            _ => Err(protocol()),
        }
    }
}
#[derive(Default)]
struct Pending {
    token: Option<String>,
    reads: Vec<ReadRequest>,
    context: Option<ContextRequest>,
    decode: Option<String>,
    decodable: BTreeSet<String>,
    decoded: BTreeMap<String, transmog_extensions::DecodedBody>,
    context_total: usize,
    traffic_sequence: u64,
}
impl Pending {
    fn clear(&mut self) {
        self.token = None;
        self.reads.clear();
        self.context = None;
        self.decode = None;
    }
}
fn prepare(
    mut invocation: Invocation,
    visualizer: &Visualizer,
    grants: &mut BTreeMap<String, u64>,
    pending: &mut Pending,
    remaining: &mut usize,
    continuations: &mut usize,
) -> Result<Invocation, Failure> {
    match &mut invocation {
        Invocation::Visualize { input } => {
            grants.clear();
            pending.clear();
            pending.decodable.clear();
            pending.decoded.clear();
            pending.context_total = 0;
            pending.traffic_sequence = 0;
            *remaining = visualizer.limits.input_bytes;
            *continuations = 0;
            project(input, visualizer, grants, &mut pending.decodable, remaining)?;
        }
        Invocation::Traffic {
            events,
            missed_hints,
        } => {
            if visualizer.live_updates.is_none()
                || decimal(missed_hints).is_err()
                || events.is_empty()
                || events.len() > 32
            {
                return Err(fail(FailureKind::Validation, "Invalid live update batch"));
            }
            pending.clear();
            *remaining = visualizer.limits.input_bytes;
            *continuations = 0;
            for event in events {
                check_traffic(event, visualizer, &mut pending.traffic_sequence)?;
                project(
                    &mut event.input,
                    visualizer,
                    grants,
                    &mut pending.decodable,
                    remaining,
                )?;
            }
        }
        Invocation::Resume { token, pages } => {
            if pending.token.as_ref() != Some(token)
                || pending.context.is_some()
                || pending.decode.is_some()
                || pages.len() != pending.reads.len()
            {
                return Err(protocol());
            }
            *continuations += 1;
            if *continuations > 512 {
                return Err(fail(FailureKind::Limit, "Too many parser continuations"));
            }
            for (page, request) in pages.iter().zip(pending.reads.iter()) {
                check_page(page, request, grants, remaining)?;
            }
            pending.clear();
        }
        Invocation::ResumeContext { token, page } => {
            context::resume(
                token,
                page,
                visualizer,
                pending,
                grants,
                (remaining, continuations),
            )?;
            pending.clear();
        }
        Invocation::ResumeDecoded { token, result } => {
            decoding::resume(
                token,
                result,
                pending,
                grants,
                (remaining, continuations),
                visualizer.limits.input_bytes,
            )?;
            pending.clear();
        }
        Invocation::Expand { token } => {
            if !valid_id(token) {
                return Err(protocol());
            }
            pending.clear();
            *remaining = visualizer.limits.input_bytes;
            *continuations = 0;
        }
    }
    Ok(invocation)
}
fn check_traffic(
    event: &transmog_extensions::TrafficEvent,
    visualizer: &Visualizer,
    prior: &mut u64,
) -> Result<(), Failure> {
    use transmog_extensions::{BodyAccess, TrafficPhase};
    let live = visualizer.live_updates.as_ref().ok_or_else(protocol)?;
    let sequence = decimal(&event.sequence).map_err(|_| protocol())?;
    if sequence <= *prior
        || decimal(&event.revision).is_err()
        || event.revision != event.input.generation
        || !valid_id(&event.exchange_id)
        || !live.events.contains(&event.direction)
        || event.input.direction != event.direction
        || event.input.exchange_id != event.exchange_id
        || (event.phase == TrafficPhase::Body && !live.body_progress)
        || (matches!(
            event.phase,
            TrafficPhase::Body | TrafficPhase::BodyCompleted
        ) && visualizer.inputs.side(event.direction).body == BodyAccess::None)
    {
        return Err(protocol());
    }
    *prior = sequence;
    Ok(())
}
fn project(
    input: &mut VisualizationInput,
    visualizer: &Visualizer,
    grants: &mut BTreeMap<String, u64>,
    decodable: &mut BTreeSet<String>,
    remaining: &mut usize,
) -> Result<(), Failure> {
    if decimal(&input.request_number).is_err()
        || !valid_id(&input.instance_id)
        || !valid_id(&input.exchange_id)
        || decimal(&input.generation).is_err()
    {
        return Err(protocol());
    }
    for (side, message) in [
        (Direction::Request, &mut input.request),
        (Direction::Response, &mut input.response),
    ] {
        if let Some(message) = message {
            message.headers =
                project_headers(&visualizer.inputs, side, &message.headers).map_err(|_| {
                    fail(
                        FailureKind::Limit,
                        "Header input exceeds the declared budget",
                    )
                })?;
            if message.method.as_ref().is_some_and(|s| s.len() > 128)
                || message.path.as_ref().is_some_and(|s| s.len() > 4096)
            {
                return Err(fail(
                    FailureKind::Limit,
                    "Message metadata exceeds its budget",
                ));
            }
            if !valid_id(&message.evidence_id) {
                return Err(protocol());
            }
            for header in &mut message.headers {
                let bytes = header
                    .name
                    .len()
                    .saturating_add(header.value.as_ref().map_or(0, Vec::len));
                *remaining = remaining.checked_sub(bytes).ok_or_else(|| {
                    fail(FailureKind::Limit, "Declared header input budget exhausted")
                })?;
                if header
                    .original_bytes
                    .as_ref()
                    .is_some_and(|n| decimal(n).is_err())
                {
                    return Err(protocol());
                }
                if let Some(value) = &header.value {
                    let id = transmog_extensions::header_source_id(
                        side,
                        &message.evidence_id,
                        header.index,
                    );
                    grants.insert(id.clone(), value.len() as u64);
                    header.source_id = Some(id);
                }
            }
            if check_body_access(&visualizer.inputs, side).is_err() {
                message.body = None;
            }
            if let Some(body) = &message.body {
                if !valid_id(&body.source_id)
                    || body.source_id.starts_with("header-")
                    || body.source_id.starts_with("decoded-")
                {
                    return Err(protocol());
                }
                let length = decimal(&body.length).map_err(|_| protocol())?;
                if grants
                    .get(&body.source_id)
                    .is_some_and(|prior| *prior != length)
                {
                    return Err(protocol());
                }
                grants.insert(body.source_id.clone(), length);
                if visualizer.inputs.side(side).decode_http_content {
                    decodable.insert(body.source_id.clone());
                }
            }
        }
    }
    if grants.len() > 4096 {
        return Err(fail(
            FailureKind::Limit,
            "Too many retained source capabilities",
        ));
    }
    Ok(())
}
fn check_page(
    page: &BytePage,
    request: &ReadRequest,
    grants: &BTreeMap<String, u64>,
    remaining: &mut usize,
) -> Result<(), Failure> {
    if page.source_id != request.source_id
        || page.offset != request.offset
        || page.bytes.len() != request.max_bytes
    {
        return Err(protocol());
    }
    let offset = decimal(&page.offset).map_err(|_| protocol())?;
    if !grants.get(&page.source_id).is_some_and(|len| {
        offset
            .checked_add(page.bytes.len() as u64)
            .is_some_and(|end| end <= *len)
    }) {
        return Err(protocol());
    }
    *remaining = remaining
        .checked_sub(page.bytes.len())
        .ok_or_else(|| fail(FailureKind::Limit, "Retained input budget exhausted"))?;
    Ok(())
}
fn validate_step(
    step: &VisualizationStep,
    visualizer: &Visualizer,
    grants: &BTreeMap<String, u64>,
    pending: &mut Pending,
    remaining: usize,
) -> Result<(), Failure> {
    match step {
        VisualizationStep::Document { document } => {
            document.validate(grants, visualizer.limits).map_err(|_| {
                fail(
                    FailureKind::Validation,
                    "Visualizer document failed host validation",
                )
            })
        }
        VisualizationStep::NeedInput { reads, token } => {
            if !valid_id(token) || reads.is_empty() || reads.len() > 8 {
                return Err(protocol());
            }
            let mut requested = 0_usize;
            for request in reads {
                let offset = decimal(&request.offset).map_err(|_| protocol())?;
                if request.max_bytes == 0
                    || request.max_bytes > 64 * 1024
                    || !grants.get(&request.source_id).is_some_and(|len| {
                        offset
                            .checked_add(request.max_bytes as u64)
                            .is_some_and(|end| end <= *len)
                    })
                {
                    return Err(fail(
                        FailureKind::Validation,
                        "Visualizer requested undeclared or out-of-range bytes",
                    ));
                }
                requested = requested
                    .checked_add(request.max_bytes)
                    .ok_or_else(protocol)?;
            }
            if requested > remaining {
                return Err(fail(FailureKind::Limit, "Retained input budget exhausted"));
            }
            pending.reads.clone_from(reads);
            pending.token = Some(token.clone());
            Ok(())
        }
        VisualizationStep::NeedContext { request, token } => {
            let context = visualizer.context.as_ref().ok_or_else(|| {
                fail(
                    FailureKind::Validation,
                    "Related traffic access was not declared",
                )
            })?;
            if !valid_id(token) {
                return Err(protocol());
            }
            request
                .validate(context)
                .map_err(|_| fail(FailureKind::Validation, "Invalid related context request"))?;
            pending.reads.clear();
            pending.context = Some(request.clone());
            pending.token = Some(token.clone());
            Ok(())
        }
        VisualizationStep::NeedDecoded { source_id, token } => {
            if !valid_id(token) || !pending.decodable.contains(source_id) {
                return Err(fail(
                    FailureKind::Validation,
                    "HTTP decoding was not declared for this body",
                ));
            }
            if grants
                .get(source_id)
                .is_none_or(|length| *length > remaining as u64)
            {
                return Err(fail(
                    FailureKind::Limit,
                    "HTTP decode input budget exhausted",
                ));
            }
            pending.clear();
            pending.decode = Some(source_id.clone());
            pending.token = Some(token.clone());
            Ok(())
        }
    }
}
fn kill(process: &Mutex<Option<Child>>) {
    if let Ok(mut process) = process.lock() {
        if let Some(child) = process.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        process.take();
    }
}
fn fail(kind: FailureKind, message: &str) -> Failure {
    Failure::new(kind, message)
}
fn protocol() -> Failure {
    fail(
        FailureKind::Protocol,
        "Invalid visualizer IPC or source capability",
    )
}
fn unavailable() -> Failure {
    fail(
        FailureKind::Unavailable,
        "The visualizer worker is no longer active",
    )
}
fn launch(config: &HostConfig, heap: usize) -> Result<Child, Failure> {
    if !config.executable.is_absolute() || !config.executable.is_file() {
        return Err(fail(
            FailureKind::Unavailable,
            "The installed visualizer helper is missing",
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = heap;
        Err(fail(
            FailureKind::Unavailable,
            "The visualizer OS sandbox is unavailable on this platform",
        ))
    }
    #[cfg(windows)]
    {
        use std::{
            os::windows::process::CommandExt as _,
            process::{Command, Stdio},
        };
        let mut command = Command::new(&config.executable);
        command
            .creation_flags(0x0800_0000)
            .arg("--extension")
            .arg("--sandbox-bootstrap")
            .arg(heap.to_string())
            .env_clear()
            .current_dir(config.executable.parent().ok_or_else(unavailable)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in [
            "LOCALAPPDATA",
            "SystemRoot",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "WINDIR",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.spawn().map_err(|_| {
            fail(
                FailureKind::Unavailable,
                "The isolated visualizer process could not start",
            )
        })
    }
}
