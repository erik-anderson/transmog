//! Bounded native input routing; reader threads never touch an isolate.

use super::DebugTransport;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use transmog_script_diagnostics::debugger::{
    DebugCommand, DebugEvent, DebugEventKind, DebugOptions, MAX_DEBUG_COMMANDS,
};

/// Ordinary blocking input until an explicit debugger attachment starts a native reader.
/// The reader owns only framed host messages; it never owns an engine or a context.
pub struct HostInput<R, T> {
    direct: Option<R>,
    inputs: Option<DebugInputs<T>>,
    read: fn(&mut R) -> Result<T, String>,
}
impl<R: std::io::Read + Send + 'static, T: Send + 'static> HostInput<R, T> {
    /// Keeps the ordinary path synchronous, without a debugger reader thread.
    pub fn new(input: R, read: fn(&mut R) -> Result<T, String>) -> Self {
        Self {
            direct: Some(input),
            inputs: None,
            read,
        }
    }
    /// Reads an ordinary request or a coalesced debugger wake.
    /// # Errors
    /// Returns a safe framing/closure failure, never authored payloads.
    pub fn read_input(&mut self) -> Result<Input<T>, String> {
        if let Some(input) = self.direct.as_mut() {
            (self.read)(input).map(Input::Message)
        } else {
            self.inputs
                .as_ref()
                .and_then(DebugInputs::recv)
                .ok_or_else(|| "Worker input closed".into())
        }
    }
    /// Starts authenticated bounded routing before any debugged initializer executes.
    /// # Errors
    /// Rejects repeated setup, invalid configuration, or unavailable reader resources.
    pub fn enable(
        &mut self,
        options: Option<DebugOptions>,
        sink: impl Fn(DebugEvent) -> bool + Send + Sync + 'static,
        route: impl Fn(T) -> Result<Routed<T>, String> + Send + 'static,
    ) -> Result<Option<DebugAttachment>, String> {
        let (router, inputs, initial) = InputRouter::new(options, sink).map_err(str::to_owned)?;
        let mut input = self
            .direct
            .take()
            .ok_or("Debugger input is already enabled")?;
        let read = self.read;
        std::thread::Builder::new()
            .name("script-debug-input".into())
            .spawn(move || {
                while let Ok(value) = read(&mut input) {
                    let Ok(value) = route(value) else { break };
                    if !router.route(value).unwrap_or(false) {
                        break;
                    }
                }
                // Dropping the router closes the paused engine's control receiver on EOF.
            })
            .map_err(|_| "Debugger input reader unavailable")?;
        self.inputs = Some(inputs);
        Ok(initial)
    }
}

type Sink = Arc<dyn Fn(DebugEvent) -> bool + Send + Sync>;
struct Shared {
    active: Mutex<Option<(String, SyncSender<DebugCommand>)>>,
    wake: AtomicBool,
}
/// Host-authenticated request classification; the generic payload stays with its host.
pub enum Routed<T> {
    /// Ordinary host request, never executed by an Inspector pause loop.
    Message(T),
    /// Final ordinary request (such as shutdown); the reader then closes.
    Last(T),
    /// Explicit attachment on an existing idle worker.
    Attach {
        /// Native random session and bounded breakpoint/policy configuration.
        options: DebugOptions,
        /// Controller request to acknowledge after engine-thread attachment.
        request: u64,
    },
    /// Typed control for exactly one active authorized session.
    Control(DebugCommand),
}
/// One V8-thread wake or application request. It cannot cross into a nested pause loop.
pub enum Input<T> {
    /// Original authenticated host operation.
    Message(T),
    /// Attachment prepared by the native reader; construct Inspector on the V8 thread.
    Attach(DebugAttachment),
    /// Coalesced hint to service queued typed controls while idle.
    Wake,
}
/// Finite input prepared without creating any V8 objects.
pub struct DebugAttachment {
    options: DebugOptions,
    request: u64,
    commands: Receiver<DebugCommand>,
    sink: Sink,
}
impl DebugAttachment {
    /// Moves its receiver into the V8-thread transport; no engine access occurs here.
    pub fn into_parts(self) -> (DebugOptions, DebugTransport, u64) {
        let sink = self.sink;
        (
            self.options,
            DebugTransport::new(self.commands, move |event| sink(event)),
            self.request,
        )
    }
}
/// Reader-thread half. Dropping it closes the active control receiver even while paused.
pub struct InputRouter<T> {
    sender: SyncSender<Input<T>>,
    shared: Arc<Shared>,
    sink: Sink,
}
/// Engine-thread half; normal messages are admitted only outside the pause loop.
pub struct DebugInputs<T> {
    receiver: Receiver<Input<T>>,
    shared: Arc<Shared>,
}
impl<T> DebugInputs<T> {
    /// Waits for an application operation/idle control hint or reader closure.
    pub fn recv(&self) -> Option<Input<T>> {
        let value = self.receiver.recv().ok()?;
        if matches!(value, Input::Wake) {
            self.shared.wake.store(false, Ordering::Release);
        }
        Some(value)
    }
}
impl<T> InputRouter<T> {
    /// Creates finite routing and optionally reserves the initializer's debug session.
    /// # Errors
    /// Rejects malformed initial options before accepting controller work.
    pub fn new(
        initial: Option<DebugOptions>,
        sink: impl Fn(DebugEvent) -> bool + Send + Sync + 'static,
    ) -> Result<(Self, DebugInputs<T>, Option<DebugAttachment>), &'static str> {
        let (sender, receiver) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
        let shared = Arc::new(Shared {
            active: Mutex::new(None),
            wake: AtomicBool::new(false),
        });
        let state = Arc::clone(&shared);
        let sink: Sink = Arc::new(move |event| {
            if matches!(
                event.event,
                DebugEventKind::Detached | DebugEventKind::Stopped { .. }
            ) {
                let mut active = state
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if active
                    .as_ref()
                    .is_some_and(|(session, _)| *session == event.session)
                {
                    active.take();
                }
            }
            sink(event)
        });
        let router = Self {
            sender,
            shared: Arc::clone(&shared),
            sink,
        };
        let initial = initial
            .map(|options| router.attachment(options, 0))
            .transpose()?;
        Ok((router, DebugInputs { receiver, shared }, initial))
    }
    fn attachment(
        &self,
        options: DebugOptions,
        request: u64,
    ) -> Result<DebugAttachment, &'static str> {
        if !options.is_bounded() {
            return Err("Invalid debugger configuration");
        }
        let mut active = self
            .shared
            .active
            .lock()
            .map_err(|_| "Debugger input unavailable")?;
        if active.is_some() {
            return Err("Worker already has a debugger owner");
        }
        let (sender, commands) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
        *active = Some((options.session.clone(), sender));
        Ok(DebugAttachment {
            options,
            request,
            commands,
            sink: Arc::clone(&self.sink),
        })
    }
    /// Routes authenticated, bounded work without blocking on JavaScript.
    /// Returns false after a final request; the caller should drop this router.
    /// # Errors
    /// Rejects malformed work or queue overflow; closure stops an attached paused engine.
    pub fn route(&self, value: Routed<T>) -> Result<bool, &'static str> {
        match value {
            Routed::Message(value) => self
                .sender
                .try_send(Input::Message(value))
                .map_err(|_| "Host input queue full")?,
            Routed::Last(value) => {
                self.sender
                    .try_send(Input::Message(value))
                    .map_err(|_| "Host input queue full")?;
                return Ok(false);
            }
            Routed::Attach { options, request } => {
                if request == 0 || !options.is_bounded() {
                    return Err("Invalid debugger attachment");
                }
                let session = options.session.clone();
                match self.attachment(options, request) {
                    Ok(attachment) => self
                        .sender
                        .try_send(Input::Attach(attachment))
                        .map_err(|_| "Host input queue full")?,
                    Err(message) => {
                        self.reject(session, request, message)?;
                    }
                }
            }
            Routed::Control(command) => {
                if !command.is_bounded() {
                    return Err("Invalid debugger command");
                }
                let active = self
                    .shared
                    .active
                    .lock()
                    .map_err(|_| "Debugger input unavailable")?;
                if let Some((session, sender)) = active
                    .as_ref()
                    .filter(|(session, _)| *session == command.session)
                {
                    let _ = session;
                    sender
                        .try_send(command)
                        .map_err(|_| "Debugger control queue full")?;
                    drop(active);
                    // A paused engine consumes commands directly. It must not accumulate one
                    // unconsumed main-loop wake for each successful scope/property query.
                    if !self.shared.wake.swap(true, Ordering::AcqRel) {
                        self.sender
                            .try_send(Input::Wake)
                            .map_err(|_| "Host input queue full")?;
                    }
                } else {
                    drop(active);
                    self.reject(
                        command.session,
                        command.request,
                        "Debugger session is unavailable",
                    )?;
                }
            }
        }
        Ok(true)
    }
    fn reject(&self, session: String, request: u64, message: &str) -> Result<(), &'static str> {
        (self.sink)(DebugEvent {
            value_generation: None,
            session,
            invocation: 0,
            request,
            event: DebugEventKind::Rejected {
                message: message.to_owned(),
            },
        })
        .then_some(())
        .ok_or("Debugger output unavailable")
    }
}
impl<T> Drop for InputRouter<T> {
    fn drop(&mut self) {
        self.shared
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_script_diagnostics::debugger::{DebugAction, ExceptionPolicy};
    fn options() -> DebugOptions {
        DebugOptions {
            skip_pauses: false,
            pause_on_start: false,
            persistent: false,
            session: "a".repeat(32),
            exceptions: ExceptionPolicy::None,
            breakpoints: vec![],
        }
    }
    #[test]
    fn pause_queries_coalesce_wakes_and_reader_eof_closes_the_control_channel() {
        let (router, inputs, initial) = InputRouter::<()>::new(Some(options()), |_| true).unwrap();
        let (_, transport, _) = initial.unwrap().into_parts();
        for request in 1..=1000 {
            assert!(
                router
                    .route(Routed::Control(DebugCommand {
                        session: options().session,
                        request,
                        action: DebugAction::Exceptions {
                            policy: ExceptionPolicy::None
                        }
                    }))
                    .unwrap()
            );
            assert_eq!(transport.commands.try_recv().unwrap().request, request);
        }
        assert!(matches!(inputs.receiver.try_recv().unwrap(), Input::Wake));
        assert!(inputs.receiver.try_recv().is_err());
        drop(router);
        assert!(matches!(
            transport.commands.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
    #[test]
    fn a_second_attach_cannot_replace_the_current_session_sender() {
        let (router, _inputs, initial) = InputRouter::<()>::new(Some(options()), |_| true).unwrap();
        let (_, transport, _) = initial.unwrap().into_parts();
        let mut other = options();
        other.session = "b".repeat(32);
        assert!(
            router
                .route(Routed::Attach {
                    options: other,
                    request: 1
                })
                .unwrap()
        );
        router
            .route(Routed::Control(DebugCommand {
                session: options().session,
                request: 2,
                action: DebugAction::Stop,
            }))
            .unwrap();
        assert_eq!(transport.commands.try_recv().unwrap().request, 2);
    }
}
