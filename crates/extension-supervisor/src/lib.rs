//! Explicit worker activation, exact input grants, authenticated IPC and bounded termination.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    read_reply as read_host_reply, valid_id, write_frame,
};
const MAX_HOSTS: u64 = 16;
mod context;
mod decoding;
mod source_maps;
/// Maps read-only debugger frames against only the exact executing package inventory.
pub fn map_debug_event(snapshot: &Snapshot, event: &mut transmog_extensions::debugger::DebugEvent) {
    source_maps::debug_event(snapshot, event);
}
/// Resolves requested original-source breakpoints through bounded inventory-declared maps.
pub fn map_debug_breakpoints(
    snapshot: &Snapshot,
    values: &[transmog_extensions::debugger::SourceBreakpoint],
) -> Vec<transmog_extensions::debugger::SourceBreakpoint> {
    source_maps::debug_breakpoints(snapshot, values)
}
use transmog_extensions::debugger::DebugCommand;
use transmog_script_diagnostics::supervision::{ReplyReader, SessionBinding, wait_budget};
pub use transmog_script_diagnostics::supervision::{
    Session as DebugSession, SessionPage as DebugSessionPage,
};
type Output = ReplyReader<BufReader<ChildStdout>, HostReply>;
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
    snapshot: Snapshot,
    worker: String,
    revision: String,
    startup_logs: Arc<transmog_extensions::LogCapture>,
    logs: Mutex<VecDeque<Arc<transmog_extensions::LogCapture>>>,
    log_runs_omitted: AtomicU64,
    sender: SyncSender<Option<Task>>,
    debug: SessionBinding,
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
    #[allow(clippy::too_many_lines)] // Keep startup ownership and every cancellation cleanup in one transaction.
    pub fn start_cancellable(
        config: &HostConfig,
        snapshot: Snapshot,
        visualizer_id: &str,
        canceled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, Failure> {
        Self::start_cancellable_debug(config, snapshot, visualizer_id, canceled, None)
    }
    /// Starts a fresh exact package with an explicitly owned initializer debugger.
    /// # Errors
    /// Returns ordinary validation, containment, capacity and initialization failures.
    pub fn start_debug(
        config: &HostConfig,
        snapshot: Snapshot,
        visualizer_id: &str,
        session: &Arc<DebugSession>,
    ) -> Result<Arc<Self>, Failure> {
        Self::start_cancellable_debug(
            config,
            snapshot,
            visualizer_id,
            || session.is_closed(),
            Some(Arc::clone(session)),
        )
    }
    #[allow(clippy::too_many_lines, clippy::needless_pass_by_value)] // Own startup session until cancellation and worker initialization settle.
    /// Starts an exact package with optional debugger ownership and native cancellation.
    /// # Errors
    /// Returns ordinary validation, containment, capacity and initialization failures.
    pub fn start_cancellable_debug(
        config: &HostConfig,
        snapshot: Snapshot,
        visualizer_id: &str,
        canceled: impl Fn() -> bool,
        initial_debug: Option<Arc<DebugSession>>,
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
        let revision = format!("{}:{}", snapshot.digest, visualizer.id);
        source_maps::cache_sources(&snapshot);
        let mut token = [0; 32];
        getrandom::fill(&mut token).map_err(|_| {
            fail(
                FailureKind::Unavailable,
                "Worker authentication is unavailable",
            )
        })?;
        let token = HostToken(token);
        let generation = transmog_extensions::sha256(&token.0);
        let startup_logs = transmog_extensions::LogCapture::new(transmog_extensions::LogContext {
            artifact: Some(snapshot.digest.clone()),
            worker: generation.clone(),
            revision: revision.clone(),
            callback: "initializer".into(),
            invocation: 0,
            exchange_id: None,
        });
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
        let stdin = Arc::new(Mutex::new(stdin));
        let debug: SessionBinding = Arc::new(Mutex::new(
            initial_debug
                .as_ref()
                .map_or_else(std::sync::Weak::new, Arc::downgrade),
        ));
        let mut output = Output::new(BufReader::new(stdout), |output| {
            read_host_reply(output).map_err(|_| "Worker output transport failed".into())
        });
        if let Some(session) = &initial_debug {
            session.track_logs(Arc::clone(&startup_logs));
            bind_debug(session, &stdin, &process, &failed, token)?;
            session.begin(0);
            enable_debug_reader(&mut output, &debug, token)?;
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = Worker {
            startup_logs: Arc::clone(&startup_logs),
            _slot: slot,
            input: stdin,
            output,
            debug: Arc::clone(&debug),
            initial_debug: initial_debug.clone(),
            token,
            snapshot: snapshot.clone(),
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
        let ready = wait_ready(&ready_rx, canceled, initial_debug.as_ref());
        match ready {
            Ok(()) => Ok(Arc::new(Self {
                snapshot,
                worker: generation,
                revision,
                startup_logs,
                logs: Mutex::new(VecDeque::new()),
                log_runs_omitted: AtomicU64::new(0),
                sender,
                process,
                failed,
                validated_document: AtomicBool::new(false),
                next_id: AtomicU64::new(1),
                visualizer,
                thread: Mutex::new(Some(thread)),
                debug,
            })),
            Err(mut e) => {
                failed.store(true, Ordering::Release);
                kill(&process);
                // Initialization may have completed concurrently with cancellation.
                // Close the idle work queue as well as the process before joining.
                drop(sender);
                let _ = thread.join();
                e.logs = startup_logs.snapshot().into_iter().collect();
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let callback = match &invocation {
            Invocation::Visualize { .. } => "visualize",
            Invocation::Resume { .. } => "resume",
            Invocation::ResumeContext { .. } => "resumeContext",
            Invocation::ResumeDecoded { .. } => "resumeDecoded",
            Invocation::Expand { .. } => "expand",
            Invocation::Traffic { .. } => "onTraffic",
        };
        let logs = transmog_extensions::LogCapture::new(transmog_extensions::LogContext {
            artifact: self
                .revision
                .split_once(':')
                .map(|(digest, _)| digest.to_owned()),
            worker: self.worker.clone(),
            revision: self.revision.clone(),
            callback: callback.into(),
            invocation: id,
            exchange_id: if let Invocation::Visualize { input } = &invocation {
                Some(input.exchange_id.clone())
            } else {
                None
            },
        });
        self.sender
            .try_send(Some(Task::Invoke(Work {
                id,
                logs: Arc::clone(&logs),
                invocation,
                reply: tx,
            })))
            .map_err(|_| fail(FailureKind::Limit, "The visualizer operation queue is full"))?;
        let mut pending = PendingInvocation {
            sender: self.sender.clone(),
            process: Arc::clone(&self.process),
            failed: Arc::clone(&self.failed),
            armed: true,
        };
        let clock = self
            .debug
            .lock()
            .ok()
            .and_then(|binding| binding.upgrade())
            .filter(|session| !session.is_closed())
            .map(|session| session.clock());
        let mut result = match wait_budget(
            Duration::from_millis(self.visualizer.limits.duration_ms),
            clock.as_ref(),
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
        };
        if logs.snapshot().is_some()
            && let Ok(mut history) = self.logs.lock()
        {
            if history.len() >= 8 {
                history.pop_front();
                self.log_runs_omitted.fetch_add(1, Ordering::AcqRel);
            }
            history.push_back(Arc::clone(&logs));
        }
        if let Err(error) = &mut result {
            error.logs = self.logs();
        }
        pending.disarm();
        result
    }

    /// Bounded initializer and recent operation logs for this exact instance.
    /// At most eight callback streams and one initializer are retained.
    pub fn logs(&self) -> Vec<transmog_extensions::LogRun> {
        self.startup_logs
            .snapshot()
            .into_iter()
            .chain(self.logs.lock().ok().into_iter().flat_map(|history| {
                history
                    .iter()
                    .filter_map(|capture| capture.snapshot())
                    .collect::<Vec<_>>()
            }))
            .collect()
    }

    /// Count of older operation streams evicted from this instance.
    pub fn log_runs_omitted(&self) -> u64 {
        self.log_runs_omitted.load(Ordering::Acquire)
    }
    /// Clears retained history; an operation currently running may publish fresh output.
    pub fn clear_logs(&self) {
        if let Ok(mut history) = self.logs.lock() {
            history.clear();
            self.startup_logs.clear();
            self.log_runs_omitted.store(0, Ordering::Release);
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
    /// Exact executing inventory; file bytes remain shared and immutable.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    /// Attaches to this actual instance at its next operation boundary without resetting globals.
    /// # Errors
    /// Rejects an existing controller, stopped worker or a full native queue.
    pub fn attach_debugger(&self, session: Arc<DebugSession>) -> Result<(), Failure> {
        let mut debug = self.debug.lock().map_err(|_| unavailable())?;
        if session.is_closed()
            || self.is_stopped()
            || debug.upgrade().is_some_and(|session| !session.is_closed())
        {
            return Err(fail(
                FailureKind::Limit,
                "Visualizer already has a debugger owner",
            ));
        }
        *debug = Arc::downgrade(&session);
        let binding = Arc::clone(&session);
        self.sender
            .try_send(Some(Task::Attach(session)))
            .map_err(|_| fail(FailureKind::Limit, "Visualizer is busy"))?;
        let process = Arc::downgrade(&self.process);
        let failed = Arc::downgrade(&self.failed);
        binding.bind_stop(move || {
            if let Some(failed) = failed.upgrade() {
                failed.store(true, Ordering::Release);
            }
            if let Some(process) = process.upgrade() {
                kill(&process);
            }
        });
        Ok(())
    }
}
fn wait_ready(
    receiver: &Receiver<Result<(), Failure>>,
    canceled: impl Fn() -> bool,
    debug: Option<&Arc<DebugSession>>,
) -> Result<(), Failure> {
    let started = std::time::Instant::now();
    loop {
        if canceled() {
            return Err(unavailable());
        }
        if started
            .elapsed()
            .saturating_sub(debug.map_or(Duration::ZERO, |session| session.clock().elapsed()))
            >= Duration::from_secs(15)
        {
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
    logs: Arc<transmog_extensions::LogCapture>,
    id: u64,
    invocation: Invocation,
    reply: tokio::sync::oneshot::Sender<Result<VisualizationStep, Failure>>,
}
#[allow(clippy::large_enum_variant)] // Keep ordinary work inline without adding a per-callback allocation.
enum Task {
    Invoke(Work),
    Attach(Arc<DebugSession>),
}

// A dropped invocation future must not remove the only deadline while its
// isolated worker continues running. The I/O thread releases its slot on exit.
struct PendingInvocation {
    sender: SyncSender<Option<Task>>,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
    armed: bool,
}

impl PendingInvocation {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingInvocation {
    fn drop(&mut self) {
        if self.armed {
            self.failed.store(true, Ordering::Release);
            // The worker may already have replied and be waiting for more work.
            let _ = self.sender.try_send(None);
            kill(&self.process);
        }
    }
}
struct Worker {
    startup_logs: Arc<transmog_extensions::LogCapture>,
    _slot: Slot,
    input: Arc<Mutex<ChildStdin>>,
    output: Output,
    debug: SessionBinding,
    initial_debug: Option<Arc<DebugSession>>,
    token: HostToken,
    snapshot: Snapshot,
    visualizer: Visualizer,
    receiver: Receiver<Option<Task>>,
    ready: SyncSender<Result<(), Failure>>,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
}
impl Worker {
    #[allow(clippy::too_many_lines)] // Ordered initialization, grant validation, invocation and fail-closed cleanup stay together.
    fn run(mut self) {
        let init = self.initialize();
        let initialized = init.is_ok();
        let _ = self.ready.send(init);
        if initialized {
            let mut grants = BTreeMap::new();
            let mut pending = Pending::default();
            let mut remaining = self.visualizer.limits.input_bytes;
            let mut continuations = 0;
            loop {
                let task = if self.output.debugging() {
                    if self.failed.load(Ordering::Acquire) {
                        break;
                    }
                    match self.receiver.recv_timeout(Duration::from_millis(25)) {
                        Ok(task) => task,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    match self.receiver.recv() {
                        Ok(task) => task,
                        Err(_) => break,
                    }
                };
                let Some(task) = task else { break };
                let work = match task {
                    Task::Invoke(work) => work,
                    Task::Attach(session) => {
                        if enable_debug_reader(&mut self.output, &self.debug, self.token).is_err()
                            || bind_debug(
                                &session,
                                &self.input,
                                &self.process,
                                &self.failed,
                                self.token,
                            )
                            .is_err()
                            || transmog_extensions::write_request(
                                &mut *self
                                    .input
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                                &HostRequest::AttachDebugger {
                                    protocol_version: HOST_PROTOCOL_VERSION,
                                    token: self.token,
                                    request_id: 1,
                                    options: session.options(),
                                },
                            )
                            .is_err()
                        {
                            session.stop("Debugger attachment failed");
                            break;
                        }
                        if session.wait_attached().is_err() {
                            break;
                        }
                        continue;
                    }
                };
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
                    )
                    .inspect_err(|_| {
                        let _ = work.logs.accept(
                            transmog_extensions::ConsoleEvent::End { dropped: 0 },
                            |_| {},
                        );
                    })?;
                    if let Some(session) = self
                        .debug
                        .lock()
                        .ok()
                        .and_then(|binding| binding.upgrade())
                        .filter(|session| !session.is_closed())
                        && session.begin_ready(work.id).map_err(|_| unavailable())?
                    {
                        session.track_logs(Arc::clone(&work.logs));
                    }
                    write_frame(
                        &mut *self
                            .input
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                        &HostRequest::Invoke {
                            token: self.token,
                            request_id: work.id,
                            invocation,
                        },
                    )
                    .map_err(|_| protocol())?;
                    let result = match self.read_reply(work.id, &work.logs) {
                        Ok(HostReply::Result {
                            token,
                            request_id,
                            result,
                        }) if token == self.token
                            && request_id == work.id
                            && work.logs.complete_or_silent() =>
                        {
                            result.map_err(|failure| {
                                source_maps::map_failure(validate_failure(failure), &self.snapshot)
                            })
                        }
                        _ => Err(protocol()),
                    }?;
                    validate_step(&result, &self.visualizer, &grants, &mut pending, remaining)?;
                    Ok(result)
                })();
                if let Some(session) = self.debug.lock().ok().and_then(|binding| binding.upgrade())
                {
                    session.completed();
                }
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
            &mut *self
                .input
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &HostRequest::Initialize {
                debug: self.initial_debug.as_ref().map(|session| session.options()),
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
                    &mut *self
                        .input
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
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
        write_frame(
            &mut *self
                .input
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &HostRequest::Start { token: self.token },
        )
        .map_err(|_| protocol())?;
        let logs = Arc::clone(&self.startup_logs);
        match self.read_reply(0, &logs) {
            Ok(HostReply::Ready { token }) if token == self.token && logs.complete_or_silent() => {
                if let Some(session) = &self.initial_debug {
                    session.completed();
                }
                Ok(())
            }
            Ok(HostReply::Fatal {
                token: Some(token),
                failure,
            }) if token == self.token => {
                logs.complete_or_silent();
                Err(source_maps::map_failure(
                    validate_failure(failure),
                    &self.snapshot,
                ))
            }
            _ => Err(protocol()),
        }
    }

    fn read_reply(
        &mut self,
        id: u64,
        logs: &Arc<transmog_extensions::LogCapture>,
    ) -> Result<HostReply, Failure> {
        let mut mapper = source_maps::Mapper::new(&self.snapshot);
        loop {
            if self.failed.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let reply = self.output.read().map_err(|_| protocol())?;
            if let HostReply::Console {
                token,
                request_id,
                event,
            } = reply
            {
                if token != self.token || request_id != id {
                    return Err(protocol());
                }
                logs.accept(event, |record| {
                    record.map_frames(|diagnostic| {
                        let mut failure = protocol();
                        failure.exception = Some(Box::new(diagnostic));
                        *mapper
                            .map_failure(failure)
                            .exception
                            .expect("validated bounded trace remains a diagnostic")
                    });
                })
                .map_err(|_| protocol())?;
            } else {
                return Ok(reply);
            }
        }
    }
}

fn bind_debug(
    session: &Arc<DebugSession>,
    input: &Arc<Mutex<ChildStdin>>,
    process: &Arc<Mutex<Option<Child>>>,
    failed: &Arc<AtomicBool>,
    token: HostToken,
) -> Result<(), Failure> {
    let (sender, receiver) =
        mpsc::sync_channel::<DebugCommand>(transmog_extensions::debugger::MAX_DEBUG_COMMANDS);
    let input = Arc::clone(input);
    let owner = Arc::downgrade(session);
    std::thread::Builder::new()
        .name("visualizer-debug-control".into())
        .spawn(move || {
            for command in receiver {
                if transmog_extensions::write_request(
                    &mut *input
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    &HostRequest::ControlDebugger {
                        protocol_version: HOST_PROTOCOL_VERSION,
                        token,
                        command,
                    },
                )
                .is_err()
                {
                    if let Some(session) = owner.upgrade() {
                        session.stop("Debugger control transport closed");
                    }
                    break;
                }
            }
        })
        .map_err(|_| unavailable())?;
    let process = Arc::downgrade(process);
    let failed = Arc::downgrade(failed);
    session.bind(
        move |command| sender.try_send(command).is_ok(),
        move || {
            if let Some(failed) = failed.upgrade() {
                failed.store(true, Ordering::Release);
            }
            if let Some(process) = process.upgrade() {
                kill(&process);
            }
        },
    );
    Ok(())
}
fn enable_debug_reader(
    output: &mut Output,
    debug: &SessionBinding,
    token: HostToken,
) -> Result<(), Failure> {
    let debug = Arc::clone(debug);
    let closing = Arc::clone(&debug);
    output
        .enable(
            move |reply| {
                if let HostReply::Debugger {
                    token: echoed,
                    event,
                } = reply
                {
                    let session = debug
                        .lock()
                        .ok()
                        .and_then(|binding| binding.upgrade())
                        .ok_or("Debugger owner unavailable")?;
                    if echoed != token || session.accept(event).is_err() {
                        session.stop("Debugger worker protocol failed");
                        return Err("Invalid debugger worker output".into());
                    }
                    Ok(None)
                } else {
                    Ok(Some(reply))
                }
            },
            move || {
                if let Some(session) = closing.lock().ok().and_then(|binding| binding.upgrade()) {
                    session.stop("Debugger worker exited");
                }
            },
        )
        .map_err(|_| unavailable())
}

fn validate_failure(failure: Failure) -> Failure {
    if failure
        .exception
        .as_ref()
        .is_some_and(|diagnostic| !diagnostic.is_bounded())
    {
        protocol()
    } else {
        failure
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
