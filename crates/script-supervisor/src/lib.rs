#![deny(missing_docs)]

//! Bounded lifecycle and authenticated IPC for one script host per revision.

use std::{
    collections::VecDeque,
    io::BufReader,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::Duration,
};

use transmog_script::debugger::DebugCommand;
use transmog_script::{
    BoxScriptFuture, CompiledScript, SCRIPT_HOST_PROTOCOL_VERSION, ScriptFailure,
    ScriptFailureCategory, ScriptHostReply, ScriptHostRequest, ScriptHostToken, ScriptInvocation,
    ScriptResult, ScriptRunner, read_host_reply_frame, write_host_frame,
};
use transmog_script_diagnostics::supervision::{ReplyReader, SessionBinding, wait_budget};
pub use transmog_script_diagnostics::supervision::{
    Session as DebugSession, SessionPage as DebugSessionPage,
};
type Output = ReplyReader<BufReader<ChildStdout>, ScriptHostReply>;
mod debugging;
mod scratch;
pub use scratch::ScratchRunner;

/// Maximum applied script revisions in one registry.
pub const MAX_ACTIVE_SCRIPT_HOSTS: usize = 32;
/// Maximum worker processes, including pinned generations and fresh test runs.
pub const MAX_SCRIPT_HOST_PROCESSES: usize = 64;
const HOST_QUEUE_DEPTH: usize = 64;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
static ACTIVE_HOSTS: AtomicUsize = AtomicUsize::new(0);
struct Slot;
impl Slot {
    fn acquire() -> Result<Self, ScriptFailure> {
        ACTIVE_HOSTS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_SCRIPT_HOST_PROCESSES).then_some(count + 1)
            })
            .map_err(|_| ScriptFailure {
                category: ScriptFailureCategory::ResourceLimit,
                message: "All script worker slots are in use".into(),
                line: None,
                column: None,
                exception: None,
                logs: Vec::new(),
            })?;
        Ok(Self)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE_HOSTS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod quota_tests {
    #[test]
    fn source_mapping_never_borrows_a_token_from_another_generated_line() {
        let map = sourcemap::decode_slice(
            br#"{"version":3,"sources":["main.ts"],"names":[],"mappings":"AAAA"}"#,
        )
        .unwrap();
        assert!(super::same_line_token(&map, 0, 0).is_some());
        assert!(super::same_line_token(&map, 1, 0).is_none());
    }
    #[test]
    fn process_slots_include_pinned_generations_and_release_on_drop() {
        let mut slots = (0..super::MAX_SCRIPT_HOST_PROCESSES)
            .map(|_| super::Slot::acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(
            matches!(super::Slot::acquire(),Err(error) if error.category==super::ScriptFailureCategory::ResourceLimit)
        );
        slots.pop();
        let replacement = super::Slot::acquire().unwrap();
        drop(replacement);
        drop(slots);
        assert_eq!(super::ACTIVE_HOSTS.load(super::Ordering::Acquire), 0);
    }
}

/// Process launch policy. Production callers must require operating-system
/// isolation; the explicit development mode exists for portable tests only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsolationPolicy {
    /// Require the platform sandbox and fail closed if it is unavailable.
    RequireSandbox,
    /// Launch with a cleared environment but without an OS security boundary.
    DevelopmentOnly,
}

/// Settings for one persistent one-script host process.
#[derive(Clone, Debug)]
pub struct ScriptHostConfig {
    /// Exact helper executable.
    pub executable: PathBuf,
    /// Required launch isolation.
    pub isolation: IsolationPolicy,
}

/// One persistent, serialized, fail-closed script runner.
pub struct SupervisedScriptRunner {
    admission: Mutex<debugging::Admission>,
    worker: String,
    sender: SyncSender<Option<Task>>,
    debug: SessionBinding,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
    next_request_id: AtomicU64,
    deadline: Duration,
    revision: String,
    startup_logs: Arc<transmog_script::LogCapture>,
    initial_reported: AtomicBool,
    recent_logs: Arc<Mutex<VecDeque<Arc<transmog_script::LogCapture>>>>,
    log_runs_omitted: Arc<AtomicU64>,
}

impl std::fmt::Debug for SupervisedScriptRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SupervisedScriptRunner")
            .field("failed", &self.failed.load(Ordering::Acquire))
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl SupervisedScriptRunner {
    /// Starts and authenticates an exact compiled script revision.
    ///
    /// # Errors
    /// Returns a fail-closed startup failure if process creation, sandboxing,
    /// initialization, or the handshake does not complete in time.
    #[allow(clippy::too_many_lines)] // Startup slot, process, reader and accepted evidence have one cleanup transaction.
    pub fn start(
        config: &ScriptHostConfig,
        compiled: CompiledScript,
    ) -> Result<Arc<Self>, ScriptFailure> {
        Self::start_debug(config, compiled, None)
    }
    /// Starts an exact build with an optional explicitly owned initializer debugger.
    /// # Errors
    /// Returns the same fail-closed startup failures as `start`.
    #[allow(clippy::too_many_lines, clippy::needless_pass_by_value)] // Own the optional session until startup and cancellation settle.
    pub fn start_debug(
        config: &ScriptHostConfig,
        compiled: CompiledScript,
        initial_debug: Option<Arc<DebugSession>>,
    ) -> Result<Arc<Self>, ScriptFailure> {
        let slot = Slot::acquire()?;
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token).map_err(|_| unavailable("channel token generation failed"))?;
        let token = ScriptHostToken(token);
        let generation = transmog_script::source_hash(&token.0);
        let deadline = Duration::from_millis(compiled.manifest.limits.max_duration_ms);
        let revision = format!(
            "{}:{}:{}:{}",
            compiled.manifest.id,
            compiled.manifest.revision,
            transmog_script::source_hash(compiled.javascript.as_bytes()),
            transmog_script::source_hash(&compiled.source_map)
        );
        cache_sources(&revision, &compiled);
        let startup_logs = transmog_script::LogCapture::new(transmog_script::LogContext {
            artifact: Some(revision.clone()),
            worker: generation.clone(),
            revision: revision.clone(),
            callback: "initializer".into(),
            invocation: 0,
            exchange_id: None,
        });
        let mut launched = launch(config, compiled.manifest.limits.max_heap_bytes)?;
        let stdin = launched
            .stdin
            .take()
            .ok_or_else(|| unavailable("script host input pipe is unavailable"))?;
        let stdout = launched
            .stdout
            .take()
            .ok_or_else(|| unavailable("script host output pipe is unavailable"))?;
        let process = Arc::new(Mutex::new(Some(launched)));
        let stdin = Arc::new(Mutex::new(stdin));
        let debug: SessionBinding = Arc::new(Mutex::new(
            initial_debug
                .as_ref()
                .map_or_else(std::sync::Weak::new, Arc::downgrade),
        ));
        let mut stdout = Output::new(BufReader::new(stdout), |output| {
            read_host_reply_frame(output).map_err(|_| "Worker output transport failed".into())
        });
        let failed = Arc::new(AtomicBool::new(false));
        if let Some(session) = &initial_debug {
            session.track_logs(Arc::clone(&startup_logs));
            bind_debug(session, &stdin, &process, &failed, token)?;
            session.begin(0);
            enable_debug_reader(&mut stdout, &debug, token)?;
        }
        let (sender, receiver) = mpsc::sync_channel(HOST_QUEUE_DEPTH);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let process_for_worker = Arc::clone(&process);
        let failed_for_worker = Arc::clone(&failed);
        let logs_for_worker = Arc::clone(&startup_logs);
        let recent_logs = Arc::new(Mutex::new(VecDeque::new()));
        let log_runs_omitted = Arc::new(AtomicU64::new(0));
        let history_for_worker = Arc::clone(&recent_logs);
        let omitted_for_worker = Arc::clone(&log_runs_omitted);
        let debug_for_worker = Arc::clone(&debug);
        let initial_for_worker = initial_debug.clone();
        std::thread::Builder::new()
            .name(format!(
                "transmog-script-{}-{}",
                compiled.manifest.id, compiled.manifest.revision
            ))
            .spawn(move || {
                let _slot = slot;
                worker(
                    &stdin,
                    stdout,
                    token,
                    compiled,
                    &receiver,
                    &ready_sender,
                    &failed_for_worker,
                    &logs_for_worker,
                    &history_for_worker,
                    &omitted_for_worker,
                    &debug_for_worker,
                    initial_for_worker.as_ref(),
                    &process_for_worker,
                );
                failed_for_worker.store(true, Ordering::Release);
                kill(&process_for_worker);
            })
            .map_err(|_| {
                kill(&process);
                unavailable("script host I/O worker could not start")
            })?;
        let ready = if let Some(session) = &initial_debug {
            let start = std::time::Instant::now();
            let clock = session.clock();
            loop {
                match ready_receiver.recv_timeout(Duration::from_millis(10)) {
                    Err(mpsc::RecvTimeoutError::Timeout)
                        if !session.is_closed()
                            && start.elapsed().saturating_sub(clock.elapsed())
                                < STARTUP_TIMEOUT => {}
                    value => break value,
                }
            }
        } else {
            ready_receiver.recv_timeout(STARTUP_TIMEOUT)
        };
        match ready {
            Ok(Ok(())) => Ok(Arc::new(Self {
                admission: Mutex::new(debugging::Admission::default()),
                worker: generation,
                sender,
                process,
                failed,
                next_request_id: AtomicU64::new(1),
                deadline,
                revision,
                startup_logs,
                initial_reported: AtomicBool::new(false),
                recent_logs,
                log_runs_omitted,
                debug,
            })),
            Ok(Err(mut error)) => {
                kill(&process);
                error.logs = startup_logs.snapshot().into_iter().collect();
                Err(error)
            }
            Err(_) => {
                failed.store(true, Ordering::Release);
                kill(&process);
                Err(ScriptFailure {
                    logs: startup_logs.snapshot().into_iter().collect(),
                    category: ScriptFailureCategory::Timeout,
                    message: "script host initialization timed out".to_owned(),
                    line: None,
                    column: None,
                    exception: None,
                })
            }
        }
    }

    /// Returns whether this host has failed and will reject future traffic.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// Attaches at the actual worker's next safe operation boundary, preserving its heap.
    /// # Errors
    /// Rejects an owned/unavailable worker or a bounded full operation queue.
    pub fn attach_debugger(&self, session: Arc<DebugSession>) -> Result<(), ScriptFailure> {
        let mut debug = self
            .debug
            .lock()
            .map_err(|_| unavailable("Debugger ownership unavailable"))?;
        if session.is_closed()
            || self.has_failed()
            || debug.upgrade().is_some_and(|session| !session.is_closed())
        {
            return Err(unavailable("Worker already has a debugger owner"));
        }
        *debug = Arc::downgrade(&session);
        let binding = Arc::clone(&session);
        self.sender
            .try_send(Some(Task::Attach(session)))
            .map_err(|_| unavailable("Worker is busy"))?;
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
    /// Independent native stop, including running code which never reaches another breakpoint.
    pub fn stop(&self) {
        self.failed.store(true, Ordering::Release);
        let _ = self.sender.try_send(None);
        kill(&self.process);
    }

    /// Bounded initializer output, available even before the first callback.
    pub fn startup_logs(&self) -> Vec<transmog_script::LogRun> {
        self.startup_logs.snapshot().into_iter().collect()
    }

    /// Initializer and the two most recent operations, including abandoned work.
    /// Capture outlives the invocation future, so cancellation retains accepted logs.
    pub fn logs(&self) -> Vec<transmog_script::LogRun> {
        self.startup_logs()
            .into_iter()
            .chain(
                self.recent_logs
                    .lock()
                    .ok()
                    .into_iter()
                    .flat_map(|history| {
                        history
                            .iter()
                            .filter_map(|capture| capture.snapshot())
                            .collect::<Vec<_>>()
                    }),
            )
            .collect()
    }
    /// Number of older operation streams evicted from this worker's history.
    pub fn log_runs_omitted(&self) -> u64 {
        self.log_runs_omitted.load(Ordering::Acquire)
    }
    /// Clears retained output; currently running work may publish fresh records.
    pub fn clear_logs(&self) {
        self.startup_logs.clear();
        if let Ok(history) = self.recent_logs.lock() {
            for capture in history.iter() {
                capture.clear();
            }
        }
        self.log_runs_omitted.store(0, Ordering::Release);
    }
}

impl ScriptRunner for SupervisedScriptRunner {
    fn exchange_open(&self) -> Result<(), ScriptFailure> {
        self.open_exchange()
    }
    fn exchange_closed(&self, exchange: &str, terminal: bool) {
        self.close_exchange(exchange, terminal);
    }
    fn debug_admit(
        &self,
        exchange: &str,
        method: &str,
        host: &str,
        path: &str,
    ) -> Result<(), ScriptFailure> {
        self.select_debug_exchange(exchange, method, host, path)
    }
    fn debug_pause_clock(
        &self,
        exchange: &str,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        self.clock_for_exchange(exchange)
    }
    fn invoke(&self, invocation: ScriptInvocation) -> BoxScriptFuture<'_> {
        if self.failed.load(Ordering::Acquire) {
            return Box::pin(async { Err(unavailable("script host is unavailable")) });
        }
        if let Err(failure) = self.check_debug_admission(&invocation.context.exchange_id) {
            return Box::pin(async move { Err(failure) });
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let logs = transmog_script::LogCapture::new(transmog_script::LogContext {
            artifact: Some(self.revision.clone()),
            worker: self.worker.clone(),
            revision: self.revision.clone(),
            callback: invocation.context.handler.export_name().into(),
            invocation: request_id,
            exchange_id: Some(invocation.context.exchange_id.clone()),
        });
        let initial = if self.initial_reported.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Arc::clone(&self.startup_logs))
        };
        let (reply_sender, reply_receiver) = tokio::sync::oneshot::channel();
        match self.sender.try_send(Some(Task::Invoke(Work {
            request_id,
            invocation,
            reply: reply_sender,
            logs: Arc::clone(&logs),
        }))) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Box::pin(async {
                    Err(ScriptFailure {
                        logs: Vec::new(),
                        category: ScriptFailureCategory::ResourceLimit,
                        message: "script host queue is full".to_owned(),
                        line: None,
                        column: None,
                        exception: None,
                    })
                });
            }
            Err(TrySendError::Disconnected(_)) => {
                self.failed.store(true, Ordering::Release);
                return Box::pin(async { Err(unavailable("script host is unavailable")) });
            }
        }
        let deadline = self.deadline;
        let process = Arc::clone(&self.process);
        let failed = Arc::clone(&self.failed);
        let debug = self.debug.lock().ok().and_then(|session| session.upgrade());
        let clock = debug
            .as_ref()
            .filter(|session| !session.is_closed())
            .map(|session| session.clock());
        // Submission is eager, so the guard must be captured before the future
        // is first polled. Dropping an unpolled future still cancels its work.
        let mut pending = PendingInvocation {
            sender: self.sender.clone(),
            process: Arc::clone(&self.process),
            failed: Arc::clone(&self.failed),
            armed: true,
        };
        Box::pin(async move {
            let mut result = match wait_budget(deadline, clock.as_ref(), reply_receiver).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => {
                    failed.store(true, Ordering::Release);
                    kill(&process);
                    Err(unavailable("script host exited during invocation"))
                }
                Err(_) => {
                    failed.store(true, Ordering::Release);
                    kill(&process);
                    Err(ScriptFailure {
                        logs: Vec::new(),
                        category: ScriptFailureCategory::Timeout,
                        message: "script invocation exceeded its deadline".to_owned(),
                        line: None,
                        column: None,
                        exception: None,
                    })
                }
            };
            let output = initial
                .and_then(|capture| capture.snapshot())
                .into_iter()
                .chain(logs.snapshot())
                .collect();
            match &mut result {
                Ok(result) => result.logs = output,
                Err(error) => error.logs = output,
            }
            pending.disarm();
            result
        })
    }
}

impl Drop for SupervisedScriptRunner {
    fn drop(&mut self) {
        kill(&self.process);
    }
}

struct Work {
    logs: Arc<transmog_script::LogCapture>,
    request_id: u64,
    invocation: ScriptInvocation,
    reply: tokio::sync::oneshot::Sender<Result<ScriptResult, ScriptFailure>>,
}
#[allow(clippy::large_enum_variant)] // Keep ordinary work inline without a new allocation on every callback.
enum Task {
    Invoke(Work),
    Attach(Arc<DebugSession>),
}

// The caller owns the timeout future. If it disappears while work is queued or
// executing, terminate the revision rather than leaving hostile code unobserved.
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
            // Wake an idle worker too: its reply may have raced cancellation.
            let _ = self.sender.try_send(None);
            kill(&self.process);
        }
    }
}

#[allow(clippy::too_many_arguments)] // One owned I/O loop, including its startup evidence sink.
#[allow(clippy::too_many_lines)] // Ordered initialization, stream barriers and fail-closed replies stay together.
fn worker(
    stdin: &Arc<Mutex<ChildStdin>>,
    mut stdout: Output,
    token: ScriptHostToken,
    compiled: CompiledScript,
    receiver: &mpsc::Receiver<Option<Task>>,
    ready: &SyncSender<Result<(), ScriptFailure>>,
    failed: &Arc<AtomicBool>,
    startup_logs: &Arc<transmog_script::LogCapture>,
    recent_logs: &Mutex<VecDeque<Arc<transmog_script::LogCapture>>>,
    omitted: &AtomicU64,
    debug: &SessionBinding,
    initial_debug: Option<&Arc<DebugSession>>,
    process: &Arc<Mutex<Option<Child>>>,
) {
    let artifact = format!(
        "{}:{}:{}:{}",
        compiled.manifest.id,
        compiled.manifest.revision,
        transmog_script::source_hash(compiled.javascript.as_bytes()),
        transmog_script::source_hash(&compiled.source_map)
    );
    let source_map = sourcemap::decode_slice(&compiled.source_map).ok();
    let initialize = ScriptHostRequest::Initialize {
        debug: initial_debug.map(|session| session.options()),
        protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
        token,
        manifest: compiled.manifest,
        javascript: compiled.javascript,
    };
    if write_host_frame(
        &mut *stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        &initialize,
    )
    .is_err()
    {
        let _ = ready.send(Err(protocol("script host initialization write failed")));
        return;
    }
    match read_reply(&mut stdout, token, 0, startup_logs, source_map.as_ref()) {
        Ok(ScriptHostReply::Ready {
            protocol_version,
            token: echoed,
        }) if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION
            && echoed == token
            && startup_logs.complete_or_silent() =>
        {
            if let Some(session) = initial_debug {
                session.completed();
            }
            let _ = ready.send(Ok(()));
        }
        Ok(ScriptHostReply::Fatal {
            protocol_version,
            token: Some(echoed),
            failure,
        }) if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && echoed == token => {
            startup_logs.complete_or_silent();
            let _ = ready.send(Err(map_failure(
                failure,
                source_map.as_ref(),
                Some(&artifact),
            )));
            return;
        }
        _ => {
            let _ = ready.send(Err(protocol("script host handshake was invalid")));
            return;
        }
    }
    loop {
        let work = if stdout.debugging() {
            if failed.load(Ordering::Acquire) {
                break;
            }
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(work) => work,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(work) => work,
                Err(_) => break,
            }
        };
        let Some(task) = work else { break };
        let work = match task {
            Task::Invoke(work) => work,
            Task::Attach(session) => {
                if enable_debug_reader(&mut stdout, debug, token).is_err()
                    || bind_debug(&session, stdin, process, failed, token).is_err()
                    || transmog_script::write_host_request_frame(
                        &mut *stdin
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                        &ScriptHostRequest::AttachDebugger {
                            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                            token,
                            request_id: 1,
                            options: session.options(),
                        },
                    )
                    .is_err()
                {
                    session.stop("Debugger attachment failed");
                    return;
                }
                if session.wait_attached().is_err() {
                    return;
                }
                continue;
            }
        };
        if failed.load(Ordering::Acquire) {
            let _ = work
                .reply
                .send(Err(unavailable("script host is unavailable")));
            break;
        }
        if let Ok(mut history) = recent_logs.lock() {
            if history.len() >= 2
                && history
                    .pop_front()
                    .is_some_and(|capture| capture.snapshot().is_some())
            {
                omitted.fetch_add(1, Ordering::AcqRel);
            }
            history.push_back(Arc::clone(&work.logs));
        }
        let session = debug
            .lock()
            .ok()
            .and_then(|binding| binding.upgrade())
            .filter(|session| !session.is_closed());
        if let Some(session) = &session {
            match session.begin_ready(work.request_id) {
                Ok(true) => session.track_logs(Arc::clone(&work.logs)),
                Ok(false) => {}
                Err(_) => {
                    let _ = work.reply.send(Err(unavailable("Debugger worker stopped")));
                    return;
                }
            }
        }
        if write_host_frame(
            &mut *stdin
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &ScriptHostRequest::Invoke {
                protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                token,
                request_id: work.request_id,
                invocation: work.invocation,
            },
        )
        .is_err()
        {
            let _ = work
                .reply
                .send(Err(protocol("script host request write failed")));
            return;
        }
        let result = match read_reply(
            &mut stdout,
            token,
            work.request_id,
            &work.logs,
            source_map.as_ref(),
        ) {
            Ok(ScriptHostReply::Result {
                protocol_version,
                token: echoed,
                request_id,
                result,
            }) if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION
                && echoed == token
                && request_id == work.request_id =>
            {
                if work.logs.complete_or_silent() {
                    result.map_err(|failure| {
                        map_failure(failure, source_map.as_ref(), Some(&artifact))
                    })
                } else {
                    Err(protocol("script host log stream did not end"))
                }
            }
            _ => Err(protocol("script host reply was invalid")),
        };
        let stop = result.as_ref().is_err_and(|error| {
            matches!(
                error.category,
                ScriptFailureCategory::Protocol | ScriptFailureCategory::ResourceLimit
            )
        });
        if let Some(session) = session {
            session.completed();
        }
        if stop {
            // Publish the fail-closed state before waking the invocation. This
            // prevents callers from briefly observing a reusable host after a
            // protocol violation or a potentially corrupted resource limit.
            failed.store(true, Ordering::Release);
        }
        let _ = work.reply.send(result);
        if stop {
            return;
        }
    }
    let _ = write_host_frame(
        &mut *stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        &ScriptHostRequest::Shutdown {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
        },
    );
}

fn read_reply(
    output: &mut Output,
    token: ScriptHostToken,
    request_id: u64,
    logs: &Arc<transmog_script::LogCapture>,
    source_map: Option<&sourcemap::DecodedMap>,
) -> Result<ScriptHostReply, ScriptFailure> {
    loop {
        let reply = output
            .read()
            .map_err(|_| protocol("script host output transport failed"))?;
        if let ScriptHostReply::Console {
            protocol_version,
            token: echoed,
            request_id: id,
            event,
        } = reply
        {
            if protocol_version != SCRIPT_HOST_PROTOCOL_VERSION
                || echoed != token
                || id != request_id
            {
                return Err(protocol("script host log identity was invalid"));
            }
            logs.accept(event, |record| {
                record.map_frames(|diagnostic| {
                    let mut failure = protocol("native console trace");
                    failure.exception = Some(Box::new(diagnostic));
                    *map_failure(failure, source_map, None)
                        .exception
                        .expect("validated bounded trace remains a diagnostic")
                });
            })
            .map_err(protocol)?;
        } else {
            return Ok(reply);
        }
    }
}

fn bind_debug(
    session: &Arc<DebugSession>,
    input: &Arc<Mutex<ChildStdin>>,
    process: &Arc<Mutex<Option<Child>>>,
    failed: &Arc<AtomicBool>,
    token: ScriptHostToken,
) -> Result<(), ScriptFailure> {
    let (sender, receiver) =
        mpsc::sync_channel::<DebugCommand>(transmog_script::debugger::MAX_DEBUG_COMMANDS);
    let input = Arc::clone(input);
    let owner = Arc::downgrade(session);
    std::thread::Builder::new()
        .name("script-debug-control".into())
        .spawn(move || {
            for command in receiver {
                if transmog_script::write_host_request_frame(
                    &mut *input
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    &ScriptHostRequest::ControlDebugger {
                        protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
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
        .map_err(|_| unavailable("Debugger control unavailable"))?;
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
    token: ScriptHostToken,
) -> Result<(), ScriptFailure> {
    let debug = Arc::clone(debug);
    let closing = Arc::clone(&debug);
    output
        .enable(
            move |reply| {
                if let ScriptHostReply::Debugger {
                    protocol_version,
                    token: echoed,
                    event,
                } = reply
                {
                    let session = debug
                        .lock()
                        .ok()
                        .and_then(|binding| binding.upgrade())
                        .ok_or("Debugger owner unavailable")?;
                    if protocol_version != SCRIPT_HOST_PROTOCOL_VERSION
                        || echoed != token
                        || session.accept(event).is_err()
                    {
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
        .map_err(|_| unavailable("Debugger output reader unavailable"))
}

fn same_line_token(
    map: &sourcemap::DecodedMap,
    line: u32,
    column: u32,
) -> Option<sourcemap::Token<'_>> {
    let sourcemap::DecodedMap::Regular(map) = map else {
        return None;
    };
    let mut low = 0;
    let mut high = usize::try_from(map.get_token_count()).ok()?;
    while low < high {
        let middle = low + (high - low) / 2;
        let token = map.get_token(middle)?;
        if (token.get_dst_line(), token.get_dst_col()) <= (line, column) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let token = map.get_token(low.checked_sub(1)?)?;
    (token.get_dst_line() == line).then_some(token)
}

fn map_failure(
    mut failure: ScriptFailure,
    source_map: Option<&sourcemap::DecodedMap>,
    artifact: Option<&str>,
) -> ScriptFailure {
    if failure.line == Some(0) || failure.column == Some(0) {
        return protocol("script host returned invalid source coordinates");
    }
    if let Some(diagnostic) = &mut failure.exception {
        if !diagnostic.is_bounded() {
            return protocol("script host returned invalid exception evidence");
        }
        diagnostic.artifact = artifact.map(str::to_owned);
        for frame in diagnostic.frames_mut() {
            frame.original = None;
            frame.mapping_issue = None;
        }
        if let Some(source_map) = source_map {
            // Mapping adds original locations without replacing executing ones.
            // Never map a helper or forged module identity through main's map.
            for frame in diagnostic.frames_mut() {
                if let Some(location) = &frame.generated
                    && location.source == "file:///transmog-script/main.js"
                {
                    let Some(token) =
                        same_line_token(source_map, location.line - 1, location.column - 1)
                    else {
                        frame.mapping_issue = Some(transmog_script::MappingIssue::Unmapped);
                        continue;
                    };
                    let Some(source) = token.get_source() else {
                        frame.mapping_issue = Some(transmog_script::MappingIssue::Unmapped);
                        continue;
                    };
                    let offset = if token.is_range() {
                        location.column - 1 - token.get_dst_col()
                    } else {
                        0
                    };
                    let Some((line, column)) = token.get_src_line().checked_add(1).zip(
                        token
                            .get_src_col()
                            .checked_add(offset)
                            .and_then(|column| column.checked_add(1)),
                    ) else {
                        frame.mapping_issue = Some(transmog_script::MappingIssue::Invalid);
                        continue;
                    };
                    if source.len() > 2048 {
                        frame.mapping_issue = Some(transmog_script::MappingIssue::Source);
                        continue;
                    }
                    frame.original = Some(transmog_script::SourceLocation {
                        source: source.to_owned(),
                        line,
                        column,
                    });
                }
            }
            // Mapped names also consume the shared evidence text budget.
            if !diagnostic.is_bounded() {
                for frame in diagnostic.frames_mut() {
                    frame.original = None;
                    frame.mapping_issue = Some(transmog_script::MappingIssue::Budget);
                }
                diagnostic.truncated = true;
            }
        }
    }
    let Some((line, column, source_map)) = failure
        .line
        .zip(failure.column)
        .and_then(|(line, column)| source_map.map(|map| (line, column, map)))
    else {
        return failure;
    };
    if let Some(token) =
        same_line_token(source_map, line.saturating_sub(1), column.saturating_sub(1))
        && let Some((line, column)) = token.get_src_line().checked_add(1).zip(
            token
                .get_src_col()
                .checked_add(if token.is_range() {
                    column - 1 - token.get_dst_col()
                } else {
                    0
                })
                .and_then(|column| column.checked_add(1)),
        )
    {
        failure.line = Some(line);
        failure.column = Some(column);
    }
    failure
}

fn cache_sources(artifact: &str, compiled: &CompiledScript) {
    let mut sources = transmog_script::diagnostic_sources::SourceBuilder::new(artifact.to_owned());
    sources.add("file:///transmog-script/main.js", &compiled.javascript);
    if let Ok(sourcemap::DecodedMap::Regular(map)) = sourcemap::decode_slice(&compiled.source_map) {
        for (index, name) in map.sources().enumerate().take(128) {
            if let Ok(index) = u32::try_from(index)
                && let Some(content) = map.get_source_contents(index)
            {
                sources.add(name, content);
            }
        }
    }
    sources.publish();
}

fn launch(config: &ScriptHostConfig, max_heap_bytes: usize) -> Result<Child, ScriptFailure> {
    if !config.executable.is_absolute() || !config.executable.is_file() {
        return Err(unavailable("script host executable is unavailable"));
    }
    match config.isolation {
        IsolationPolicy::DevelopmentOnly => launch_development(&config.executable),
        IsolationPolicy::RequireSandbox => {
            platform::launch_sandboxed(&config.executable, max_heap_bytes)
        }
    }
}

fn launch_development(executable: &Path) -> Result<Child, ScriptFailure> {
    let working_directory = executable
        .parent()
        .ok_or_else(|| unavailable("script host directory is unavailable"))?;
    let mut command = Command::new(executable);
    suppress_console_window(&mut command);
    command
        .current_dir(working_directory)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable("script host process could not start"))
}

#[cfg(windows)]
fn suppress_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn suppress_console_window(_command: &mut Command) {}

fn kill(process: &Mutex<Option<Child>>) {
    let Ok(mut process) = process.lock() else {
        return;
    };
    if let Some(child) = process.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    process.take();
}

fn unavailable(message: &str) -> ScriptFailure {
    ScriptFailure {
        logs: Vec::new(),
        category: ScriptFailureCategory::Unavailable,
        message: message.to_owned(),
        line: None,
        column: None,
        exception: None,
    }
}

fn protocol(message: &str) -> ScriptFailure {
    ScriptFailure {
        logs: Vec::new(),
        category: ScriptFailureCategory::Protocol,
        message: message.to_owned(),
        line: None,
        column: None,
        exception: None,
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        path::Path,
        process::{Child, Command, Stdio},
    };

    use super::{ScriptFailure, suppress_console_window, unavailable};

    pub(super) fn launch_sandboxed(
        executable: &Path,
        max_heap_bytes: usize,
    ) -> Result<Child, ScriptFailure> {
        let working_directory = executable
            .parent()
            .ok_or_else(|| unavailable("script host directory is unavailable"))?;
        let mut command = Command::new(executable);
        suppress_console_window(&mut command);
        command
            .arg("--sandbox-bootstrap")
            .arg(max_heap_bytes.to_string())
            .current_dir(working_directory)
            .env_clear()
            .envs(required_windows_environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| unavailable("script sandbox bootstrap could not start"))
    }

    fn required_windows_environment() -> Vec<(&'static str, std::ffi::OsString)> {
        [
            "LOCALAPPDATA",
            "SystemRoot",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "WINDIR",
        ]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::{path::Path, process::Child};

    use super::{ScriptFailure, unavailable};

    pub(super) fn launch_sandboxed(
        _executable: &Path,
        _max_heap_bytes: usize,
    ) -> Result<Child, ScriptFailure> {
        Err(unavailable(
            "the script sandbox is currently supported only on Windows",
        ))
    }
}
