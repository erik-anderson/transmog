//! Optional local debugger. Only typed native controls reach Inspector.
//! All engine access, including paused scope reads, stays on its owning thread.

use crate::{Error, ErrorKind, Runtime};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};
use transmog_script_diagnostics::{
    SourceLocation,
    debugger::{
        BreakpointResolution, DebugAction, DebugCommand, DebugEvent, DebugEventKind, DebugFrame,
        DebugOptions, DebugScope, DebugSource, ExceptionPolicy, MAX_DEBUG_COMMANDS,
        MAX_DEBUG_FRAMES, MAX_DEBUG_REPLY_BYTES, MAX_DEBUG_SOURCES, MAX_VALUE_BYTES,
        PAUSE_LEASE_MS, ResumeAction, SESSION_LEASE_MS,
    },
};
use v8::inspector::{
    Channel, ChannelImpl, StringBuffer, StringView, V8Inspector, V8InspectorClient,
    V8InspectorClientImpl, V8InspectorClientTrustLevel, V8InspectorSession,
};
mod input;
#[cfg(test)]
mod tests;
mod values;
pub use input::{DebugAttachment, DebugInputs, HostInput, Input, InputRouter, Routed};

const MAX_INSPECTOR_BYTES: usize = 1024 * 1024;
const MAX_SCOPE_CHAIN: usize = 16;
type Sink = Box<dyn Fn(DebugEvent) -> bool>;

/// Finite control input and nonblocking, bounded native event delivery.
/// Hosts authenticate commands before forwarding them; Inspector wire messages are private.
pub struct DebugTransport {
    commands: Receiver<DebugCommand>,
    sink: Sink,
}
impl DebugTransport {
    /// Uses an already bounded receiver and a native event sink which returns false on closure/full.
    pub fn new(
        commands: Receiver<DebugCommand>,
        sink: impl Fn(DebugEvent) -> bool + 'static,
    ) -> Self {
        Self {
            commands,
            sink: Box::new(sink),
        }
    }
}

pub(crate) struct Debugger {
    inspector: V8Inspector,
    client: Rc<Client>,
}
struct Client {
    runtime_state: Rc<super::State>,
    options: RefCell<DebugOptions>,
    commands: Receiver<DebugCommand>,
    sink: Sink,
    session: RefCell<Option<Rc<V8InspectorSession>>>,
    context: v8::Global<v8::Context>,
    raw_isolate: v8::UnsafeRawIsolatePtr,
    isolate: v8::IsolateHandle,
    created: Cell<Instant>,
    detached: Cell<bool>,
    stopped: Cell<bool>,
    paused: Cell<bool>,
    invocation: Cell<u64>,
    epoch: Cell<u64>,
    last_request: Cell<u64>,
    inspector_request: Cell<i32>,
    context_id: Cell<i64>,
    internal_resume: Cell<bool>,
    condition_failed: Cell<bool>,
    response: RefCell<Option<Value>>,
    pending_pause: RefCell<Option<Value>>,
    frames: RefCell<Vec<Value>>,
    sources: RefCell<BTreeMap<i32, DebugSource>>,
    inspector_breakpoints: RefCell<BTreeMap<u32, String>>,
    startup_breakpoints: RefCell<Vec<String>>,
    startup_armed: Cell<bool>,
    values: RefCell<values::Values>,
}

impl Runtime {
    /// Attaches the shared native debugger before module initialization or at an idle boundary.
    /// # Errors
    /// Rejects an existing connection, malformed options, or Inspector initialization failure.
    pub fn enable_debugger(
        &mut self,
        options: DebugOptions,
        transport: DebugTransport,
    ) -> Result<(), Error> {
        if self.debugger.is_some() || !options.is_bounded() {
            return Err(debug_error("Invalid debugger configuration"));
        }
        let connection = Debugger::new(self, options, transport)?;
        // An existing worker keeps the actual module instances and heap when attached.
        {
            v8::scope!(let scope, &mut self.isolate);
            let state = crate::state(scope);
            for (name, module) in state.modules.borrow().iter() {
                let module = v8::Local::new(scope, module);
                if let Some(id) = module.script_id() {
                    connection.register(id, name.as_str(), None);
                }
            }
        }
        connection.modules_instantiated();
        // This is an attachment-complete barrier: every existing module and
        // breakpoint event precedes it before another operation may be admitted.
        connection.client.emit(0, DebugEventKind::Attached);
        if connection.client.stopped.get() {
            v8::scope!(let scope,&mut self.isolate);
            connection.close(v8::Local::new(scope, &self.context));
            return Err(debug_error("Debugger event transport unavailable"));
        }
        self.debugger = Some(connection);
        Ok(())
    }

    /// Attributes the following initializer/callback and services finite queued native controls.
    /// Does not admit another JavaScript callback while paused.
    pub fn begin_debug_operation(&mut self, invocation: u64) {
        if let Some(debugger) = &self.debugger {
            debugger.client.invocation.set(invocation);
        }
        self.poll_debugger();
    }

    /// Services breakpoint/policy controls while the host is idle. Paused reads use V8 callbacks.
    pub fn poll_debugger(&mut self) {
        if let Some(debugger) = &self.debugger {
            debugger.poll();
        }
        if self
            .debugger
            .as_ref()
            .is_some_and(|debugger| debugger.client.detached.get())
        {
            self.disable_debugger();
        }
    }

    /// Detaches an idle connection while keeping the actual worker context and globals.
    pub fn disable_debugger(&mut self) {
        if let Some(debugger) = self.debugger.take() {
            v8::scope!(let scope, &mut self.isolate);
            debugger.close(v8::Local::new(scope, &self.context));
        }
    }

    /// Whether an attached session has stopped or expired and its worker must be retired.
    pub fn debugger_stopped(&self) -> bool {
        self.debugger
            .as_ref()
            .is_some_and(|debugger| debugger.client.stopped.get())
    }
}

fn debug_error(message: &str) -> Error {
    Error::new(ErrorKind::Initialization, message)
}

impl Debugger {
    #[allow(unsafe_code)] // A copied isolate token is used only by this isolate's Inspector callback.
    fn new(
        runtime: &mut Runtime,
        options: DebugOptions,
        transport: DebugTransport,
    ) -> Result<Self, Error> {
        let context = {
            v8::scope!(let scope, &mut runtime.isolate);
            let context = v8::Local::new(scope, &runtime.context);
            v8::Global::new(scope, context)
        };
        let pause_on_start = options.pause_on_start;
        let client = Rc::new(Client {
            runtime_state: super::state(&runtime.isolate),
            options: RefCell::new(options),
            commands: transport.commands,
            sink: transport.sink,
            session: RefCell::new(None),
            context,
            // SAFETY: no ownership is transferred. The connection is dropped before Runtime's isolate.
            raw_isolate: unsafe { runtime.isolate.as_raw_isolate_ptr() },
            isolate: runtime.isolate.thread_safe_handle(),
            created: Cell::new(Instant::now()),
            detached: Cell::new(false),
            stopped: Cell::new(false),
            paused: Cell::new(false),
            invocation: Cell::new(0),
            epoch: Cell::new(0),
            last_request: Cell::new(0),
            inspector_request: Cell::new(0),
            context_id: Cell::new(0),
            internal_resume: Cell::new(false),
            condition_failed: Cell::new(false),
            response: RefCell::new(None),
            pending_pause: RefCell::new(None),
            frames: RefCell::new(Vec::new()),
            sources: RefCell::new(BTreeMap::new()),
            inspector_breakpoints: RefCell::new(BTreeMap::new()),
            startup_breakpoints: RefCell::new(Vec::new()),
            startup_armed: Cell::new(pause_on_start),
            values: RefCell::new(values::Values::default()),
        });
        let inspector = V8Inspector::create(
            &mut runtime.isolate,
            V8InspectorClient::new(Box::new(InspectorClient(Rc::clone(&client)))),
        );
        {
            v8::scope!(let scope, &mut runtime.isolate);
            inspector.context_created(
                v8::Local::new(scope, &runtime.context),
                1,
                StringView::from(&b"Transmog script"[..]),
                StringView::from(&b"{}"[..]),
            );
        }
        *client.session.borrow_mut() = Some(Rc::new(inspector.connect(
            1,
            Channel::new(Box::new(Output(Rc::clone(&client)))),
            StringView::from(&b"{}"[..]),
            V8InspectorClientTrustLevel::Untrusted,
        )));
        let connection = Self { inspector, client };
        let configured = connection
            .client
            .dispatch("Debugger.enable", json!({}))
            .and_then(|_| connection.client.dispatch("Runtime.enable", json!({})))
            .and_then(|_| connection.client.policy());
        if let Err(error) = configured {
            v8::scope!(let scope, &mut runtime.isolate);
            connection.close(v8::Local::new(scope, &runtime.context));
            return Err(error);
        }
        Ok(connection)
    }
    pub(crate) fn register(&self, id: i32, name: &str, bytes: Option<usize>) {
        if self.client.stopped.get() {
            return;
        }
        if name.len() > 2048 || self.client.sources.borrow().len() >= MAX_DEBUG_SOURCES {
            self.client
                .stop(0, "Debugger source registry limit reached");
            return;
        }
        let source = DebugSource {
            source: name.to_owned(),
            bytes: bytes.and_then(|bytes| u32::try_from(bytes).ok()),
        };
        self.client.sources.borrow_mut().insert(id, source.clone());
        self.client.emit(
            0,
            DebugEventKind::Sources {
                sources: vec![source],
                breakpoints: vec![],
            },
        );
        // Module locations become executable only after graph instantiation.
    }
    pub(crate) fn modules_instantiated(&self) {
        self.client.resolve(None, 0);
        if self.client.startup_armed.get() && self.client.startup_breakpoints.borrow().is_empty() {
            let ids: Vec<_> = self
                .client
                .sources
                .borrow()
                .iter()
                .filter(|(_, source)| source.source.starts_with("file:///"))
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Ok(reply) = self.client.dispatch(
                    "Debugger.setBreakpoint",
                    json!({"location":{"scriptId":id.to_string(),"lineNumber":0,"columnNumber":0}}),
                ) && let Some(id) = reply["breakpointId"].as_str()
                {
                    self.client.startup_breakpoints.borrow_mut().push(id.into());
                }
            }
        }
    }
    fn poll(&self) {
        // Detach retires the input router's sender. Its resulting EOF belongs
        // to the old controller, not to the healthy execution context.
        if self.client.detached.get() || self.client.stopped.get() {
            return;
        }
        if !self.client.options.borrow().persistent
            && self.client.created.get().elapsed() >= Duration::from_millis(SESSION_LEASE_MS)
        {
            self.client.stop(0, "Debugger session lifetime expired");
            return;
        }
        for _ in 0..MAX_DEBUG_COMMANDS {
            match self.client.commands.try_recv() {
                Ok(command) => {
                    self.client.command(command, None);
                    if self.client.detached.get() || self.client.stopped.get() {
                        break;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.client.stop(0, "Debugger controller disconnected");
                    break;
                }
            }
        }
    }
    pub(crate) fn close(&self, context: v8::Local<v8::Context>) {
        self.client.values.borrow_mut().clear();
        self.client.session.borrow_mut().take();
        self.inspector.context_destroyed(context);
    }
}
impl Drop for Debugger {
    fn drop(&mut self) {
        // Break the client -> session -> output -> client cycle on every return path.
        self.client.session.borrow_mut().take();
        self.client.values.borrow_mut().clear();
    }
}

impl Client {
    fn emit(&self, request: u64, event: DebugEventKind) {
        let mut event = DebugEvent {
            session: self.options.borrow().session.clone(),
            invocation: self.invocation.get(),
            request,
            event,
        };
        // Native typed replies have a smaller ceiling than the raw Inspector staging buffer.
        while serde_json::to_vec(&event).is_ok_and(|bytes| bytes.len() > MAX_DEBUG_REPLY_BYTES) {
            if let DebugEventKind::Paused {
                frames,
                exception,
                truncated,
                ..
            } = &mut event.event
            {
                *truncated = true;
                if exception.take().is_some() {
                    continue;
                }
                if frames.pop().is_some() {
                    continue;
                }
            }
            self.stop(request, "Debugger evidence exceeded its native budget");
            return;
        }
        if let DebugEventKind::Paused { frames, .. } = &event.event {
            self.frames.borrow_mut().truncate(frames.len());
        }
        if !(self.sink)(event) {
            self.stop(0, "Debugger event transport closed");
        }
    }
    fn stop(&self, request: u64, reason: &str) {
        if self.stopped.replace(true) {
            return;
        }
        self.paused.set(false);
        self.frames.borrow_mut().clear();
        self.values.borrow_mut().clear();
        self.isolate.terminate_execution();
        self.emit(
            request,
            DebugEventKind::Stopped {
                reason: reason.to_owned(),
            },
        );
    }
    fn reject(&self, request: u64, message: &str) {
        self.emit(
            request,
            DebugEventKind::Rejected {
                message: message.to_owned(),
            },
        );
    }
    fn dispatch(&self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self
            .inspector_request
            .get()
            .checked_add(1)
            .ok_or_else(|| debug_error("Inspector request limit reached"))?;
        self.inspector_request.set(id);
        self.response.borrow_mut().take();
        let session = self
            .session
            .borrow()
            .clone()
            .ok_or_else(|| debug_error("Inspector connection unavailable"))?;
        // Only private fixed method/parameter constructions call this function.
        let mut command = json!({"id":id,"method":method});
        command["params"] = params;
        let command = serde_json::to_vec(&command)
            .map_err(|_| debug_error("Inspector command unavailable"))?;
        session.dispatch_protocol_message(StringView::from(command.as_slice()));
        let reply = self
            .response
            .borrow_mut()
            .take()
            .ok_or_else(|| debug_error("Inspector response unavailable"))?;
        if reply.get("error").is_some() {
            Err(debug_error("Inspector rejected this operation"))
        } else {
            Ok(reply["result"].clone())
        }
    }
    fn policy(&self) -> Result<(), Error> {
        let mode = match self.options.borrow().exceptions {
            ExceptionPolicy::None => "none",
            ExceptionPolicy::Uncaught => "uncaught",
            ExceptionPolicy::All => "all",
        };
        self.dispatch("Debugger.setPauseOnExceptions", json!({"state":mode}))
            .map(|_| ())
    }
    fn resolve(&self, script: Option<i32>, request: u64) {
        let values = self.options.borrow().breakpoints.clone();
        for value in values {
            let source = self
                .sources
                .borrow()
                .iter()
                .find(|(id, source)| {
                    source.source == value.location.source
                        && script.is_none_or(|script| script == **id)
                })
                .map(|(id, _)| *id);
            let Some(id) = source else {
                if script.is_none() {
                    self.emit(
                        request,
                        DebugEventKind::Sources {
                            sources: vec![],
                            breakpoints: vec![BreakpointResolution {
                                id: value.id,
                                requested: value.location,
                                resolved: None,
                                issue: Some("Source has not been registered in this worker".into()),
                            }],
                        },
                    );
                }
                continue;
            };
            if let Some(previous) = self.inspector_breakpoints.borrow_mut().remove(&value.id) {
                let _ = self.dispatch(
                    "Debugger.removeBreakpoint",
                    json!({"breakpointId":previous}),
                );
            }
            let response = self.dispatch("Debugger.setBreakpoint",json!({"location":{"scriptId":id.to_string(),"lineNumber":value.location.line-1,"columnNumber":value.location.column-1}}));
            let mut resolution = BreakpointResolution {
                id: value.id,
                requested: value.location.clone(),
                resolved: None,
                issue: None,
            };
            if let Ok(response) = response {
                if let Some(breakpoint) = response["breakpointId"].as_str() {
                    self.inspector_breakpoints
                        .borrow_mut()
                        .insert(value.id, breakpoint.to_owned());
                }
                resolution.resolved = self.location(&response["actualLocation"]);
            }
            if resolution.resolved.is_none() {
                resolution.issue =
                    Some("No executable position resolved for this source request".into());
            }
            self.emit(
                request,
                DebugEventKind::Sources {
                    sources: vec![],
                    breakpoints: vec![resolution],
                },
            );
        }
    }
    fn location(&self, location: &Value) -> Option<SourceLocation> {
        let id = location["scriptId"].as_str()?.parse::<i32>().ok()?;
        let sources = self.sources.borrow();
        let source = &sources.get(&id)?.source;
        Some(SourceLocation {
            source: source.clone(),
            line: u32::try_from(location["lineNumber"].as_u64()?)
                .ok()?
                .checked_add(1)?,
            column: u32::try_from(location["columnNumber"].as_u64()?)
                .ok()?
                .checked_add(1)?,
        })
    }
    #[allow(clippy::too_many_lines)] // Keep each typed action's epoch validation with its execution.
    fn command(&self, command: DebugCommand, scope: Option<&mut v8::PinScope<'_, '_>>) {
        if self.stopped.get() {
            return;
        }
        if !command.is_bounded()
            || command.session != self.options.borrow().session
            || command.request <= self.last_request.get()
        {
            self.reject(command.request, "Stale or invalid debugger command");
            return;
        }
        self.last_request.set(command.request);
        let request = command.request;
        match command.action {
            DebugAction::Stop => self.stop(request, "Debugger stopped by its owner"),
            DebugAction::Detach => {
                if self.paused.get() && !self.options.borrow().persistent {
                    self.reject(
                        request,
                        "A paused debugger must resume or stop before detach",
                    );
                    return;
                }
                // Inspector can deliver its resumed notification after the detach
                // acknowledgement. Retire notification delivery before resuming:
                // the authenticated supervisor has closed this control stream.
                self.detached.set(true);
                let _ = self.dispatch("Debugger.setSkipAllPauses", json!({"skip":true}));
                if self.paused.replace(false) {
                    self.frames.borrow_mut().clear();
                    self.values.borrow_mut().clear();
                    let _ = self.dispatch("Debugger.resume", json!({}));
                }
                self.emit(request, DebugEventKind::Detached);
            }
            DebugAction::Breakpoints { values } => {
                let previous = std::mem::take(&mut *self.inspector_breakpoints.borrow_mut());
                for id in previous.values() {
                    let _ = self.dispatch("Debugger.removeBreakpoint", json!({"breakpointId":id}));
                }
                self.options.borrow_mut().breakpoints = values;
                self.resolve(None, request);
                self.emit(request, DebugEventKind::Acknowledged);
            }
            DebugAction::Evaluate {
                expression,
                pause,
                frame,
            } => {
                if (pause == 0 && self.paused.get()) || (pause > 0 && !self.current_pause(pause)) {
                    self.reject(request, "The selected console scope has changed");
                    return;
                }
                self.evaluate(request, &expression, pause, frame);
            }
            DebugAction::Exceptions { policy } => {
                self.options.borrow_mut().exceptions = policy;
                if self.policy().is_ok() {
                    self.emit(request, DebugEventKind::Acknowledged);
                } else {
                    self.reject(request, "Exception policy could not be changed");
                }
            }
            DebugAction::Resume { pause, action } => {
                if !self.current_pause(pause) {
                    self.reject(request, "Pause has expired or resumed");
                    return;
                }
                let method = match action {
                    ResumeAction::Continue => "Debugger.resume",
                    ResumeAction::Into => "Debugger.stepInto",
                    ResumeAction::Over => "Debugger.stepOver",
                    ResumeAction::Out => "Debugger.stepOut",
                };
                if self.dispatch(method, json!({})).is_ok() {
                    self.emit(request, DebugEventKind::Acknowledged);
                } else {
                    self.reject(request, "This pause cannot resume");
                }
            }
            DebugAction::Scopes { pause, frame } => {
                if !self.current_pause(pause) {
                    self.reject(request, "Pause has expired or resumed");
                    return;
                }
                if let Some(scope) = scope {
                    self.scopes(scope, request, pause, frame);
                } else {
                    self.reject(request, "Scope inspection requires a paused engine");
                }
            }
            DebugAction::Properties {
                pause,
                handle,
                offset,
            } => {
                if !self.current_pause(pause) {
                    self.reject(request, "Pause has expired or resumed");
                    return;
                }
                if let Some(scope) = scope {
                    let event = self
                        .values
                        .borrow_mut()
                        .properties(scope, pause, handle, offset);
                    if let Some(event) = event {
                        self.emit(request, event);
                    } else {
                        self.reject(request, "Value handle is unavailable in this pause");
                    }
                } else {
                    self.reject(request, "Property inspection requires a paused engine");
                }
            }
            DebugAction::Release { pause, handles } => {
                if !self.current_pause(pause) {
                    self.reject(request, "Pause has expired or resumed");
                    return;
                }
                self.values.borrow_mut().release(&handles);
                self.emit(request, DebugEventKind::Acknowledged);
            }
        }
    }
    fn current_pause(&self, pause: u64) -> bool {
        self.paused.get() && self.epoch.get() == pause
    }
    fn condition_matches(&self, pause: &Value) -> bool {
        self.condition_failed.set(false);
        let Some(hits) = pause["hitBreakpoints"]
            .as_array()
            .filter(|hits| !hits.is_empty())
        else {
            return true;
        };
        let Some(frame) = pause["callFrames"][0]["callFrameId"].as_str() else {
            return true;
        };
        for hit in hits {
            let condition = {
                let bindings = self.inspector_breakpoints.borrow();
                let id = bindings
                    .iter()
                    .find(|(_, value)| Some(value.as_str()) == hit.as_str())
                    .map(|(id, _)| *id);
                id.and_then(|id| {
                    self.options
                        .borrow()
                        .breakpoints
                        .iter()
                        .find(|b| b.id == id)
                        .and_then(|b| b.condition.clone())
                })
            };
            let Some(condition) = condition else {
                return true;
            };
            let result=self.dispatch("Debugger.evaluateOnCallFrame",json!({"callFrameId":frame,"expression":format!("!!(\n{condition}\n)"),"throwOnSideEffect":true,"silent":true,"returnByValue":true,"timeout":50}));
            match result {
                Ok(result) if result.get("exceptionDetails").is_none() => {
                    if result["result"]["value"].as_bool() != Some(false) {
                        return true;
                    }
                }
                _ => {
                    self.condition_failed.set(true);
                    return true;
                }
            }
        }
        false
    }
    fn evaluate(&self, request: u64, expression: &str, pause: u64, frame: Option<u32>) {
        let mut params = json!({"expression":expression,"silent":true,"generatePreview":false,"returnByValue":false,"timeout":500,"disableBreaks":true,"objectGroup":"transmog-console"});
        let method = if let Some(frame) = frame {
            let id = self
                .frames
                .borrow()
                .get(frame as usize)
                .and_then(|frame| frame["callFrameId"].as_str())
                .map(str::to_owned);
            let Some(id) = id else {
                self.reject(request, "Console frame is unavailable");
                return;
            };
            params["callFrameId"] = json!(id);
            "Debugger.evaluateOnCallFrame"
        } else {
            params["contextId"] = json!(self.context_id.get());
            "Runtime.evaluate"
        };
        self.emit(request, DebugEventKind::Evaluating);
        let output = Rc::new(RefCell::new(Vec::new()));
        let capture = Rc::clone(&output);
        let dropped = Rc::new(Cell::new(0_u64));
        let omitted = Rc::clone(&dropped);
        let previous = self
            .runtime_state
            .console
            .replace(super::console::Console::capture(move |_, event| {
                if let transmog_script_diagnostics::ConsoleEvent::Record(record) = event {
                    if capture.borrow().len() >= 16 {
                        omitted.set(omitted.get().saturating_add(1));
                        return false;
                    }
                    capture.borrow_mut().push(record);
                }
                true
            }));
        let _ = self.dispatch("Debugger.setSkipAllPauses", json!({"skip":true}));
        let result = self.dispatch(method, params);
        self.runtime_state.console.replace(previous);
        for record in output.borrow_mut().drain(..) {
            self.emit(
                request,
                DebugEventKind::Output {
                    level: record.level,
                    text: record.text,
                },
            );
        }
        if dropped.get() > 0 {
            self.emit(
                request,
                DebugEventKind::Output {
                    level: transmog_script_diagnostics::LogLevel::Warn,
                    text: format!("{} additional console records omitted", dropped.get()),
                },
            );
        }
        let _ = self.dispatch("Debugger.setSkipAllPauses", json!({"skip":false}));
        let (value, thrown) = console_value(result);
        let _ = self.dispatch(
            "Runtime.releaseObjectGroup",
            json!({"objectGroup":"transmog-console"}),
        );
        self.emit(
            request,
            DebugEventKind::Evaluated {
                pause,
                value,
                thrown,
            },
        );
    }
    fn scopes(&self, scope: &mut v8::PinScope<'_, '_>, request: u64, pause: u64, frame: u32) {
        let chain = self
            .frames
            .borrow()
            .get(frame as usize)
            .and_then(|frame| frame["scopeChain"].as_array())
            .map(|chain| {
                chain
                    .iter()
                    .take(MAX_SCOPE_CHAIN)
                    .cloned()
                    .collect::<Vec<_>>()
            });
        let Some(chain) = chain else {
            self.reject(request, "Scopes are unavailable in this frame");
            return;
        };
        let Some(session) = self.session.borrow().clone() else {
            return;
        };
        let scopes = chain
            .iter()
            .take(MAX_SCOPE_CHAIN)
            .enumerate()
            .map(|(index, entry)| {
                let handle = entry["object"]["objectId"]
                    .as_str()
                    .filter(|id| id.len() <= 2048)
                    .and_then(|id| {
                        self.values
                            .borrow_mut()
                            .scope(scope, &session, frame, index, id)
                    });
                DebugScope {
                    kind: short(entry["type"].as_str().unwrap_or("unavailable"), 64),
                    name: short(entry["name"].as_str().unwrap_or(""), 256),
                    handle,
                }
            })
            .collect();
        self.emit(
            request,
            DebugEventKind::Scopes {
                pause,
                frame,
                scopes,
            },
        );
    }
    fn paused(&self, scope: &mut v8::PinScope<'_, '_>) {
        if self.stopped.get() {
            return;
        }
        let Some(raw) = self.pending_pause.borrow_mut().take() else {
            self.stop(0, "Inspector pause metadata unavailable");
            return;
        };
        let startup = self.startup_armed.replace(false);
        if startup {
            let values = std::mem::take(&mut *self.startup_breakpoints.borrow_mut());
            for id in values {
                let _ = self.dispatch("Debugger.removeBreakpoint", json!({"breakpointId":id}));
            }
        }
        if !startup && !self.condition_matches(&raw) {
            self.internal_resume.set(true);
            if self.dispatch("Debugger.resume", json!({})).is_ok() {
                return;
            }
            self.internal_resume.set(false);
        }
        let Some(epoch) = self.epoch.get().checked_add(1) else {
            self.stop(0, "Pause generation exhausted");
            return;
        };
        self.epoch.set(epoch);
        self.paused.set(true);
        self.values.borrow_mut().clear();
        let frames = raw["callFrames"].as_array().cloned().unwrap_or_default();
        let truncated = frames.len() > MAX_DEBUG_FRAMES;
        let frames = frames
            .into_iter()
            .take(MAX_DEBUG_FRAMES)
            .collect::<Vec<_>>();
        let visible = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| DebugFrame {
                index: u32::try_from(index).unwrap_or_default(),
                function: short(
                    frame["functionName"].as_str().unwrap_or(""),
                    MAX_VALUE_BYTES,
                ),
                location: self.location(&frame["location"]),
                original: None,
                unavailable: frame["scopeChain"]
                    .as_array()
                    .is_none_or(|scopes| scopes.len() > MAX_SCOPE_CHAIN),
            })
            .collect();
        let exception = raw["data"]["objectId"]
            .as_str()
            .filter(|id| id.len() <= 2048)
            .and_then(|id| {
                let session = self.session.borrow().clone()?;
                let (value, _, _) = session
                    .unwrap_object(scope, StringView::from(id.as_bytes()))
                    .ok()?;
                Some(Box::new(crate::exception::capture(scope, value)))
            });
        *self.frames.borrow_mut() = frames;
        let reason = if self.condition_failed.replace(false) {
            "Breakpoint condition failed"
        } else {
            match raw["reason"].as_str() {
                Some("exception" | "promiseRejection") => "exception",
                Some("step") => "step",
                _ if startup => "first user statement",
                _ => "breakpoint or debugger statement",
            }
        }
        .to_owned();
        self.emit(
            0,
            DebugEventKind::Paused {
                pause: epoch,
                reason,
                frames: visible,
                exception,
                truncated,
            },
        );
    }
}

fn console_value(
    result: Result<Value, Error>,
) -> (transmog_script_diagnostics::debugger::DebugValue, bool) {
    match result {
        Ok(result) => {
            let raw = &result["result"];
            let kind = raw["type"].as_str().unwrap_or("unavailable");
            let display = if let Some(description) = raw["description"]
                .as_str()
                .filter(|_| result.get("exceptionDetails").is_some())
            {
                description.into()
            } else if let Some(value) = raw.get("value") {
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned)
            } else if let Some(value) = raw["unserializableValue"].as_str() {
                value.into()
            } else {
                format!("[{kind}]")
            };
            (
                transmog_script_diagnostics::debugger::DebugValue {
                    kind: kind.into(),
                    truncated: display.len() > MAX_VALUE_BYTES,
                    display: short(&display, MAX_VALUE_BYTES),
                    handle: None,
                },
                result.get("exceptionDetails").is_some(),
            )
        }
        Err(_) => (
            transmog_script_diagnostics::debugger::DebugValue {
                kind: "unavailable".into(),
                display: "Evaluation failed or exceeded its execution deadline".into(),
                handle: None,
                truncated: false,
            },
            true,
        ),
    }
}
fn short(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

struct InspectorClient(Rc<Client>);
impl V8InspectorClientImpl for InspectorClient {
    #[allow(unsafe_code)] // V8 invokes this callback on the owning isolate thread while paused.
    fn run_message_loop_on_pause(&self, _group: i32) {
        let client = &self.0;
        if client.stopped.get() {
            return;
        }
        let mut raw = client.raw_isolate;
        // SAFETY: the Inspector and this callback belong to this still-live isolate.
        // The copied wrapper grants no ownership and never crosses an engine thread.
        let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr_mut(&mut raw) };
        // SAFETY: CallbackScope is created only inside V8's Inspector pause callback.
        let callback = std::pin::pin!(unsafe { v8::CallbackScope::new(isolate) });
        let scope = &mut callback.init();
        let context = v8::Local::new(scope, &client.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        client.paused(scope);
        let pause_started = Instant::now();
        while client.paused.get() && !client.stopped.get() {
            if !client.options.borrow().persistent
                && (pause_started.elapsed() >= Duration::from_millis(PAUSE_LEASE_MS)
                    || client.created.get().elapsed() >= Duration::from_millis(SESSION_LEASE_MS))
            {
                client.stop(0, "Debugger lease expired");
                break;
            }
            match client.commands.recv_timeout(Duration::from_millis(25)) {
                Ok(command) => client.command(command, Some(scope)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    client.stop(0, "Debugger controller disconnected");
                    break;
                }
            }
        }
    }
    fn quit_message_loop_on_pause(&self) {
        self.0.paused.set(false);
    }
    fn value_subtype<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Option<v8::UniquePtr<StringBuffer>> {
        // Hide default object/error descriptions, which can inspect authored properties.
        value
            .is_object()
            .then(|| StringBuffer::create(StringView::from(&b"error"[..])))
    }
    fn description_for_value_subtype<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        _value: v8::Local<'s, v8::Value>,
    ) -> Option<v8::UniquePtr<StringBuffer>> {
        Some(StringBuffer::create(StringView::from(
            &b"[Object: inspect explicitly]"[..],
        )))
    }
}
struct Output(Rc<Client>);
impl Output {
    fn decode(&self, message: &v8::UniquePtr<StringBuffer>) -> Option<Value> {
        let message = message.as_ref()?;
        if message.string().len() > MAX_INSPECTOR_BYTES / 3 {
            self.0
                .stop(0, "Inspector evidence exceeded its staging budget");
            return None;
        }
        serde_json::from_str(&message.string().to_string()).ok()
    }
}
impl ChannelImpl for Output {
    fn send_response(&self, _id: i32, message: v8::UniquePtr<StringBuffer>) {
        *self.0.response.borrow_mut() = self.decode(&message);
    }
    fn send_notification(&self, message: v8::UniquePtr<StringBuffer>) {
        if self.0.detached.get() {
            return;
        }
        let Some(event) = self.decode(&message) else {
            return;
        };
        match event["method"].as_str() {
            Some("Runtime.executionContextCreated") => {
                if let Some(id) = event["params"]["context"]["id"].as_i64() {
                    self.0.context_id.set(id);
                }
            }
            Some("Debugger.paused") => {
                *self.0.pending_pause.borrow_mut() = Some(event["params"].clone());
            }
            Some("Debugger.resumed") => {
                if self.0.internal_resume.replace(false) {
                    return;
                }
                self.0.paused.set(false);
                self.0.frames.borrow_mut().clear();
                self.0.values.borrow_mut().clear();
                self.0.emit(0, DebugEventKind::Resumed);
            }
            _ => {} // scriptParsed URLs and all raw protocol descriptions are deliberately ignored.
        }
    }
    fn flush_protocol_notifications(&self) {}
}
