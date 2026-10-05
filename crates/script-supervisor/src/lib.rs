#![deny(missing_docs)]

//! Bounded lifecycle and authenticated IPC for one script host per revision.

use std::{
    io::BufReader,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::Duration,
};

use transmog_script::{
    BoxScriptFuture, CompiledScript, SCRIPT_HOST_PROTOCOL_VERSION, ScriptAction, ScriptFailure,
    ScriptFailureCategory, ScriptHostReply, ScriptHostRequest, ScriptHostToken, ScriptInvocation,
    ScriptRunner, read_host_frame, write_host_frame,
};

/// Maximum simultaneously active isolated script processes.
pub const MAX_ACTIVE_SCRIPT_HOSTS: usize = 32;
const HOST_QUEUE_DEPTH: usize = 64;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

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
    sender: SyncSender<Work>,
    process: Arc<Mutex<Option<Child>>>,
    failed: Arc<AtomicBool>,
    next_request_id: AtomicU64,
    deadline: Duration,
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
    pub fn start(
        config: &ScriptHostConfig,
        compiled: CompiledScript,
    ) -> Result<Arc<Self>, ScriptFailure> {
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token).map_err(|_| unavailable("channel token generation failed"))?;
        let token = ScriptHostToken(token);
        let deadline = Duration::from_millis(compiled.manifest.limits.max_duration_ms);
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
        let failed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(HOST_QUEUE_DEPTH);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let process_for_worker = Arc::clone(&process);
        let failed_for_worker = Arc::clone(&failed);
        std::thread::Builder::new()
            .name(format!(
                "transmog-script-{}-{}",
                compiled.manifest.id, compiled.manifest.revision
            ))
            .spawn(move || {
                worker(
                    stdin,
                    stdout,
                    token,
                    compiled,
                    receiver,
                    &ready_sender,
                    &failed_for_worker,
                );
                failed_for_worker.store(true, Ordering::Release);
                kill(&process_for_worker);
            })
            .map_err(|_| unavailable("script host I/O worker could not start"))?;
        match ready_receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(())) => Ok(Arc::new(Self {
                sender,
                process,
                failed,
                next_request_id: AtomicU64::new(1),
                deadline,
            })),
            Ok(Err(error)) => {
                kill(&process);
                Err(error)
            }
            Err(_) => {
                failed.store(true, Ordering::Release);
                kill(&process);
                Err(ScriptFailure {
                    category: ScriptFailureCategory::Timeout,
                    message: "script host initialization timed out".to_owned(),
                    line: None,
                    column: None,
                })
            }
        }
    }

    /// Returns whether this host has failed and will reject future traffic.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

impl ScriptRunner for SupervisedScriptRunner {
    fn invoke(&self, invocation: ScriptInvocation) -> BoxScriptFuture<'_> {
        if self.failed.load(Ordering::Acquire) {
            return Box::pin(async { Err(unavailable("script host is unavailable")) });
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (reply_sender, reply_receiver) = tokio::sync::oneshot::channel();
        match self.sender.try_send(Work {
            request_id,
            invocation,
            reply: reply_sender,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Box::pin(async {
                    Err(ScriptFailure {
                        category: ScriptFailureCategory::ResourceLimit,
                        message: "script host queue is full".to_owned(),
                        line: None,
                        column: None,
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
        Box::pin(async move {
            match tokio::time::timeout(deadline, reply_receiver).await {
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
                        category: ScriptFailureCategory::Timeout,
                        message: "script invocation exceeded its deadline".to_owned(),
                        line: None,
                        column: None,
                    })
                }
            }
        })
    }
}

impl Drop for SupervisedScriptRunner {
    fn drop(&mut self) {
        kill(&self.process);
    }
}

struct Work {
    request_id: u64,
    invocation: ScriptInvocation,
    reply: tokio::sync::oneshot::Sender<Result<ScriptAction, ScriptFailure>>,
}

fn worker(
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    token: ScriptHostToken,
    compiled: CompiledScript,
    receiver: mpsc::Receiver<Work>,
    ready: &SyncSender<Result<(), ScriptFailure>>,
    failed: &AtomicBool,
) {
    let mut stdout = BufReader::new(stdout);
    let initialize = ScriptHostRequest::Initialize {
        protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
        token,
        manifest: compiled.manifest,
        javascript: compiled.javascript,
    };
    if write_host_frame(&mut stdin, &initialize).is_err() {
        let _ = ready.send(Err(protocol("script host initialization write failed")));
        return;
    }
    match read_host_frame::<ScriptHostReply>(&mut stdout) {
        Ok(ScriptHostReply::Ready {
            protocol_version,
            token: echoed,
        }) if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && echoed == token => {
            let _ = ready.send(Ok(()));
        }
        Ok(ScriptHostReply::Fatal { failure, .. }) => {
            let _ = ready.send(Err(failure));
            return;
        }
        _ => {
            let _ = ready.send(Err(protocol("script host handshake was invalid")));
            return;
        }
    }
    for work in receiver {
        if failed.load(Ordering::Acquire) {
            let _ = work
                .reply
                .send(Err(unavailable("script host is unavailable")));
            continue;
        }
        if write_host_frame(
            &mut stdin,
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
        let result = match read_host_frame::<ScriptHostReply>(&mut stdout) {
            Ok(ScriptHostReply::Result {
                protocol_version,
                token: echoed,
                request_id,
                result,
            }) if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION
                && echoed == token
                && request_id == work.request_id =>
            {
                result
            }
            _ => Err(protocol("script host reply was invalid")),
        };
        let stop = result
            .as_ref()
            .is_err_and(|error| error.category == ScriptFailureCategory::Protocol);
        let _ = work.reply.send(result);
        if stop {
            return;
        }
    }
    let _ = write_host_frame(
        &mut stdin,
        &ScriptHostRequest::Shutdown {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
        },
    );
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
    Command::new(executable)
        .current_dir(working_directory)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable("script host process could not start"))
}

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
        category: ScriptFailureCategory::Unavailable,
        message: message.to_owned(),
        line: None,
        column: None,
    }
}

fn protocol(message: &str) -> ScriptFailure {
    ScriptFailure {
        category: ScriptFailureCategory::Protocol,
        message: message.to_owned(),
        line: None,
        column: None,
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        path::Path,
        process::{Child, Command, Stdio},
    };

    use super::{ScriptFailure, unavailable};

    pub(super) fn launch_sandboxed(
        executable: &Path,
        max_heap_bytes: usize,
    ) -> Result<Child, ScriptFailure> {
        let working_directory = executable
            .parent()
            .ok_or_else(|| unavailable("script host directory is unavailable"))?;
        Command::new(executable)
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
