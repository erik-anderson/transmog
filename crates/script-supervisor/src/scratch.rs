//! An empty, supervised Console process with the same authenticated debugger transport.
use super::{
    DebugSession, Output, STARTUP_TIMEOUT, ScriptFailure, ScriptHostConfig, Slot, bind_debug,
    enable_debug_reader, kill, launch, unavailable,
};
use std::{
    io::BufReader,
    process::Child,
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
};
use transmog_script::{
    SCRIPT_HOST_PROTOCOL_VERSION, ScriptHostReply, ScriptHostRequest, ScriptHostToken,
    read_host_reply_frame, write_host_frame,
};

/// A window-owned Scratch heap, independent of traffic scripts and debugger surfaces.
pub struct ScratchRunner {
    process: Arc<Mutex<Option<Child>>>,
}
impl ScratchRunner {
    /// Starts an isolated empty context and waits for its authenticated ready barrier.
    /// # Errors
    /// Rejects unavailable isolation, exhausted process slots, and failed initialization.
    pub fn start(
        config: &ScriptHostConfig,
        session: &Arc<DebugSession>,
    ) -> Result<Self, ScriptFailure> {
        let slot = Slot::acquire()?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| unavailable("Scratch identity unavailable"))?;
        let token = ScriptHostToken(random);
        let mut child = launch(config, 64 * 1024 * 1024)?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("Scratch input unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| unavailable("Scratch output unavailable"))?;
        let process = Arc::new(Mutex::new(Some(child)));
        let runner = Self {
            process: Arc::clone(&process),
        };
        let input = Arc::new(Mutex::new(input));
        let failed = Arc::new(AtomicBool::new(false));
        bind_debug(session, &input, &process, &failed, token)?;
        let binding = Arc::new(Mutex::new(Arc::downgrade(session)));
        let mut output = Output::new(BufReader::new(output), |reader| {
            read_host_reply_frame(reader).map_err(|_| "Scratch output closed".into())
        });
        enable_debug_reader(&mut output, &binding, token)?;
        let initialize = ScriptHostRequest::InitializeScratch {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
            debug: session.options(),
        };
        let written = {
            let mut writer = input
                .lock()
                .map_err(|_| unavailable("Scratch input unavailable"))?;
            write_host_frame(&mut *writer, &initialize)
        };
        if written.is_err() {
            session.stop("Scratch initialization failed");
            return Err(unavailable("Scratch initialization failed"));
        }
        let (ready, started) = mpsc::sync_channel(1);
        let held = Arc::clone(&process);
        let owner = Arc::downgrade(session);
        std::thread::Builder::new().name("transmog-scratch".into()).spawn(move || {
            let _slot = slot;
            let valid = matches!(output.read(), Ok(ScriptHostReply::Ready { protocol_version, token: echoed })
                if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && echoed == token);
            let _ = ready.send(valid);
            if valid {
                // Evaluation replies are consumed by the debugger pump. No ordinary
                // script operations are accepted by a Scratch process.
                let _ = output.read();
            }
            if let Some(session) = owner.upgrade() {
                session.stop("Scratch context ended");
            }
            kill(&held);
        }).map_err(|_| {
            session.stop("Scratch worker could not start");
            unavailable("Scratch worker could not start")
        })?;
        if started.recv_timeout(STARTUP_TIMEOUT) != Ok(true) {
            session.stop("Scratch initialization failed or timed out");
            return Err(unavailable("Scratch could not start. Try again."));
        }
        Ok(runner)
    }
}
impl Drop for ScratchRunner {
    fn drop(&mut self) {
        kill(&self.process);
    }
}
