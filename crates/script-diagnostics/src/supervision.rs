//! Native debugger ownership, independent leases and bounded evidence.
//! No engine, filesystem, network or application input is accessible here.

use crate::debugger::{
    ARM_LEASE_MS, BreakpointResolution, DebugAction, DebugCommand, DebugEvent, DebugEventKind,
    DebugOptions, DebugSource, MAX_DEBUG_COMMANDS, MAX_DEBUG_REPLY_BYTES, PAUSE_LEASE_MS,
    SESSION_LEASE_MS,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
#[cfg(test)]
mod tests;

const HISTORY_BYTES: usize = 128 * 1024;
type Writer = Arc<dyn Fn(DebugCommand) -> bool + Send + Sync>;
type Stopper = Arc<dyn Fn() + Send + Sync>;

/// Independent cumulative pause clock. Only authenticated native transitions update it.
#[derive(Default)]
pub struct PauseClock(Mutex<(Duration, Option<Instant>)>);
impl PauseClock {
    /// Cumulative authorized pause time, including the current bounded pause.
    pub fn elapsed(&self) -> Duration {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.0 + state.1.map_or(Duration::ZERO, |start| start.elapsed())
    }
    fn pause(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1 = Some(Instant::now());
    }
    fn resume(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(start) = state.1.take() {
            state.0 += start.elapsed();
        }
    }
}
struct Pending {
    since: Instant,
    inspection: bool,
    started: bool,
    expected: Expected,
}
enum Expected {
    Control,
    Evaluate(u64),
    Complete(u64),
    Configure,
    Resume,
    Release,
    Detach,
    Scopes(u64, u32),
    Properties(u64, u32, u32),
}
impl Expected {
    fn from_action(action: &DebugAction) -> Self {
        match action {
            DebugAction::SkipPauses { .. } | DebugAction::PauseNext { .. } => Self::Control,
            DebugAction::Evaluate { pause, .. } | DebugAction::Inspect { pause, .. } => {
                Self::Evaluate(*pause)
            }
            DebugAction::Complete { pause, .. } => Self::Complete(*pause),
            DebugAction::Scopes { pause, frame } => Self::Scopes(*pause, *frame),
            DebugAction::Properties {
                pause,
                handle,
                offset,
            } => Self::Properties(*pause, *handle, *offset),
            DebugAction::Resume { .. } | DebugAction::ContinueTo { .. } => Self::Resume,
            DebugAction::Release { .. } => Self::Release,
            DebugAction::Detach => Self::Detach,
            _ => Self::Configure,
        }
    }
    fn accepts(&self, event: &DebugEventKind) -> bool {
        match event {
            DebugEventKind::Control { .. } => matches!(self, Self::Control),
            DebugEventKind::Completions { pause, .. } => {
                matches!(self, Self::Complete(p) if p == pause)
            }
            DebugEventKind::Evaluating | DebugEventKind::Output { .. } => {
                matches!(self, Self::Evaluate(_))
            }
            DebugEventKind::Evaluated { pause, .. } => {
                matches!(self, Self::Evaluate(p) if p == pause)
            }
            DebugEventKind::Rejected { .. } => true,
            DebugEventKind::Acknowledged => {
                matches!(self, Self::Configure | Self::Resume | Self::Release)
            }
            DebugEventKind::Sources { .. } => matches!(self, Self::Configure),
            DebugEventKind::Detached => matches!(self, Self::Detach),
            DebugEventKind::Scopes { pause, frame, .. } => {
                matches!(self,Self::Scopes(p,f) if p==pause&&f==frame)
            }
            DebugEventKind::Properties {
                pause,
                handle,
                offset,
                ..
            } => matches!(self,Self::Properties(p,h,o) if p==pause&&h==handle&&o==offset),
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Connection {
    Attaching,
    Attached,
    Detaching(u64),
    Detached,
    Stopped,
}
impl Connection {
    fn closed(self) -> bool {
        matches!(self, Self::Detached | Self::Stopped)
    }
    fn detaching(self) -> bool {
        matches!(self, Self::Detaching(_))
    }
    fn attached(self) -> bool {
        matches!(self, Self::Attached | Self::Detaching(_))
    }
}
/// Current native execution controls, independent of saved breakpoint intent.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionControls {
    /// Future pauses and logpoint hits are suppressed.
    pub skip_pauses: bool,
    /// A pause is scheduled at the next verified user callback.
    pub pause_next: bool,
}
struct State {
    controls: ExecutionControls,
    logpoint_at: Option<Instant>,
    exceptions: crate::debugger::ExceptionPolicy,
    sources: BTreeMap<String, DebugSource>,
    breakpoints: BTreeMap<u32, BreakpointResolution>,
    breakpoint_request: u64,
    logs: VecDeque<Arc<crate::LogCapture>>,
    connection: Connection,
    active: bool,
    armed_at: Option<Instant>,
    arm_expired: bool,
    exchange: Option<String>,
    invocation: u64,
    pause: u64,
    paused_at: Option<Instant>,
    pause_snapshot: Option<DebugEvent>,
    detach_requested: bool,
    request: u64,
    sequence: u64,
    bytes: usize,
    events: VecDeque<(u64, DebugEvent, usize)>,
    pending: BTreeMap<u64, Pending>,
    writer: Option<Writer>,
    stop: Option<Stopper>,
    finished: Option<Stopper>,
}
/// One local controller, created by native code with an unpredictable identity.
/// Drop the owning controller explicitly with `stop` when its window disappears.
pub struct Session {
    options: DebugOptions,
    created: Instant,
    state: Mutex<State>,
    clock: Arc<PauseClock>,
}
/// Cursor-qualified bounded local evidence. Never persist or expose this through MCP.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    /// Current native callback attribution, including silent callbacks.
    pub operation: Option<crate::LogContext>,
    /// Last native pause epoch, including while idle. Object handles expire across epochs.
    pub value_generation: u64,
    /// Current native execution controls.
    #[serde(flatten)]
    pub controls: ExecutionControls,
    /// Current exception pause policy for this target.
    pub exceptions: crate::debugger::ExceptionPolicy,
    /// Current pause, retained even when transient inspection replies expire from history.
    pub paused: Option<DebugEvent>,
    /// Complete bounded inventory on first read or after unseen history expired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inventory: Option<DebugInventory>,
    /// Whether a native initializer/callback is currently admitted.
    pub active: bool,
    /// Selected live exchange; absent while armed or in a sandbox.
    pub exchange_id: Option<String>,
    /// Remaining bounded wait for the first matching live exchange.
    pub arm_remaining_ms: Option<u64>,
    /// Bounded initializer/recent console streams, independent of callback completion.
    pub logs: Vec<crate::LogRun>,
    /// Opaque native ownership identity.
    pub session: String,
    /// Last native event sequence, for the next poll.
    pub cursor: u64,
    /// Earlier unseen events expired from the finite history.
    pub omitted: bool,
    /// Current terminal state.
    pub closed: bool,
    /// Remaining independent pause lease; absent while running.
    pub pause_remaining_ms: Option<u64>,
    /// Remaining absolute session lease.
    pub session_remaining_ms: u64,
    /// Accepted ordered typed events only.
    pub events: Vec<DebugEvent>,
}
/// Source and breakpoint metadata, retained independently of transient value replies.
#[derive(Serialize)]
pub struct DebugInventory {
    /// Actual compiled module identities, with no internal invocation scripts.
    pub sources: Vec<DebugSource>,
    /// Current resolution, including pending and unbound requests.
    pub breakpoints: Vec<BreakpointResolution>,
}
impl Session {
    /// Creates a controller after native code has generated the session ID.
    /// # Errors
    /// Rejects excessive/invalid initial configuration.
    #[allow(clippy::too_many_lines)] // Initialize state and install its independent watchdog together.
    pub fn new(options: DebugOptions) -> Result<Arc<Self>, &'static str> {
        if !options.is_bounded() {
            return Err("Invalid debugger configuration");
        }
        let breakpoints = options
            .breakpoints
            .iter()
            .map(|value| {
                (
                    value.id,
                    BreakpointResolution {
                        id: value.id,
                        requested: value.location.clone(),
                        resolved: None,
                        issue: Some("Pending source resolution".into()),
                    },
                )
            })
            .collect();
        let exceptions = options.exceptions;
        let skip_pauses = options.skip_pauses;
        let session = Arc::new(Self {
            options,
            created: Instant::now(),
            clock: Arc::default(),
            state: Mutex::new(State {
                controls: ExecutionControls {
                    skip_pauses,
                    pause_next: false,
                },
                logpoint_at: None,
                exceptions,
                sources: BTreeMap::new(),
                breakpoints,
                breakpoint_request: 0,
                logs: VecDeque::new(),
                connection: Connection::Attaching,
                active: false,
                armed_at: None,
                arm_expired: false,
                exchange: None,
                invocation: 0,
                pause: 0,
                paused_at: None,
                pause_snapshot: None,
                detach_requested: false,
                request: 0,
                sequence: 0,
                bytes: 0,
                events: VecDeque::new(),
                pending: BTreeMap::new(),
                writer: None,
                stop: None,
                finished: None,
            }),
        });
        let weak = Arc::downgrade(&session);
        std::thread::Builder::new()
            .name("script-debug-watchdog".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(10));
                    let Some(session) = weak.upgrade() else { break };
                    let (expired, arm_expired) = {
                        let mut state = session
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if state.connection.closed() {
                            break;
                        }
                        let expired = state
                            .logpoint_at
                            .is_some_and(|since| since.elapsed() >= Duration::from_secs(1))
                            || (!session.options.persistent
                                && (session.created.elapsed()
                                    >= Duration::from_millis(SESSION_LEASE_MS)
                                    || state.paused_at.is_some_and(|start| {
                                        start.elapsed() >= Duration::from_millis(PAUSE_LEASE_MS)
                                    })))
                            || state.pending.values().any(|pending| {
                                (!session.options.persistent
                                    || !state.active
                                    || state.paused_at.is_some()
                                    || pending.started)
                                    && (pending.started
                                        || !state.pending.values().any(|active| active.started))
                                    && pending.since.elapsed()
                                        >= Duration::from_millis(if pending.inspection {
                                            500
                                        } else {
                                            1000
                                        })
                            });
                        let arm_expired = state.armed_at.is_some_and(|start| {
                            start.elapsed() >= Duration::from_millis(ARM_LEASE_MS)
                        });
                        if arm_expired {
                            state.armed_at = None;
                            state.arm_expired = true;
                        }
                        (expired, arm_expired)
                    };
                    if expired {
                        session.stop("Debugger lease or inspection deadline expired");
                        break;
                    }
                    if arm_expired && session.command(DebugAction::Detach).is_err() {
                        session.stop("Debugger arming lease expired");
                        break;
                    }
                }
            })
            .map_err(|_| "Debugger watchdog unavailable")?;
        Ok(session)
    }
    /// Exact bounded initial configuration, copied only into authenticated host initialization.
    pub fn options(&self) -> DebugOptions {
        self.options.clone()
    }
    /// Exact opaque session identity.
    pub fn id(&self) -> &str {
        &self.options.session
    }
    /// Shared native clock for this worker's explicitly authorized operations.
    pub fn clock(&self) -> Arc<PauseClock> {
        Arc::clone(&self.clock)
    }
    /// Releases native application evidence immediately when this session ends.
    /// The callback must not own the session; use a weak application-run reference.
    pub fn on_closed(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: Stopper = Arc::new(callback);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed() {
            drop(state);
            callback();
        } else {
            let previous = state.finished.take();
            state.finished = Some(Arc::new(move || {
                if let Some(previous) = &previous {
                    previous();
                }
                callback();
            }));
        }
    }
    /// Shares an already bounded native log capture without copying its accepted records.
    pub fn track_logs(&self, capture: Arc<crate::LogCapture>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.logs.len() >= 9 {
            state.logs.pop_front();
        }
        state.logs.push_back(capture);
    }
    /// Installs nonblocking control submission and independent process termination.
    pub fn bind(
        &self,
        writer: impl Fn(DebugCommand) -> bool + Send + Sync + 'static,
        stop: impl Fn() + Send + Sync + 'static,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.writer = Some(Arc::new(writer));
        state.stop = Some(Arc::new(stop));
        let closed = state.connection == Connection::Stopped;
        let stop = state.stop.clone();
        drop(state);
        if closed && let Some(stop) = stop {
            stop();
        }
    }
    /// Installs independent termination immediately after accepting an attachment,
    /// before the worker's I/O thread has created its command transport.
    pub fn bind_stop(&self, stop: impl Fn() + Send + Sync + 'static) {
        let stop: Stopper = Arc::new(stop);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection == Connection::Detached {
            return;
        }
        state.stop = Some(Arc::clone(&stop));
        let stopped = state.connection == Connection::Stopped;
        drop(state);
        if stopped {
            stop();
        }
    }
    /// Attributes the single admitted operation before sending its frame to the worker.
    pub fn begin(&self, invocation: u64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed() || state.active || state.connection.detaching() {
            return false;
        }
        state.active = true;
        state.invocation = invocation;
        true
    }
    /// Ends an ordinary operation without manufacturing a successful debugger outcome.
    pub fn completed(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.logpoint_at.is_some() {
            drop(state);
            self.stop("Debugger inspection did not finish before the callback reply");
            return;
        }
        state.active = false;
        for pending in state.pending.values_mut() {
            if !pending.started {
                pending.since = Instant::now();
            }
        }
    }
    /// Whether a session can still receive commands.
    pub fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            .closed()
    }
    /// Whether ordinary debugger commands can be sent at this attachment boundary.
    pub fn ready_for_commands(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            == Connection::Attached
    }
    /// Whether native confirmation ended this session with an idle detach.
    pub fn detached(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            == Connection::Detached
    }
    /// Continues and detaches, including an attachment queued behind ordinary work.
    /// # Errors
    /// Returns an error when the connected native control transport is unavailable.
    pub fn request_detach(&self) -> Result<(), &'static str> {
        let mut state = self.state.lock().map_err(|_| "Debugger unavailable")?;
        if state.connection.closed() {
            return Ok(());
        }
        if state.connection == Connection::Attaching {
            state.detach_requested = true;
            return Ok(());
        }
        drop(state);
        self.command(DebugAction::Detach).map(|_| ())
    }
    /// Starts the independent bounded live arming interval.
    pub fn arm(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed()
            || state.active
            || state.armed_at.is_some()
            || state.exchange.is_some()
        {
            return false;
        }
        state.armed_at = Some(Instant::now());
        true
    }
    /// Claims one native exchange atomically with arming expiry.
    pub fn claim_exchange(&self, exchange: &str) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed()
            || state.arm_expired
            || state
                .armed_at
                .is_none_or(|start| start.elapsed() >= Duration::from_millis(ARM_LEASE_MS))
            || exchange.is_empty()
            || exchange.len() > 128
        {
            return false;
        }
        state.armed_at = None;
        state.exchange = Some(exchange.into());
        true
    }
    /// Starts an operation after any in-flight idle detach has finished.
    /// False means a healthy detached worker can execute normally without Inspector.
    /// # Errors
    /// Rejects ended ownership and independently stops an unresponsive detach.
    pub fn begin_ready(&self, invocation: u64) -> Result<bool, &'static str> {
        let started = Instant::now();
        loop {
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.connection.closed() {
                    return if state.connection == Connection::Detached {
                        Ok(false)
                    } else {
                        Err("Debugger worker stopped")
                    };
                }
                if !state.connection.detaching() {
                    if state.active {
                        return Err("Debugger operation is already active");
                    }
                    state.active = true;
                    state.invocation = invocation;
                    return Ok(true);
                }
            }
            if started.elapsed() >= Duration::from_secs(1) {
                self.stop("Debugger detach timed out");
                return Err("Debugger detach timed out");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    /// Waits for the native attachment-complete barrier before admitting a callback.
    /// # Errors
    /// Stops an unresponsive worker after a finite attachment deadline.
    pub fn wait_attached(&self) -> Result<(), &'static str> {
        let started = Instant::now();
        loop {
            {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.connection.closed() {
                    return Err("Debugger attachment ended");
                }
                if state.connection.attached() {
                    return Ok(());
                }
            }
            if started.elapsed() >= Duration::from_secs(1) {
                self.stop("Debugger attachment timed out");
                return Err("Debugger attachment timed out");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    /// Releases ended-session evidence under the application's shared history budget.
    pub fn clear_evidence(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed() {
            state.events.clear();
            state.sources.clear();
            state.breakpoints.clear();
            state.bytes = 0;
            state.logs.clear();
        }
    }
    /// Native-generated monotonic command identity; callers cannot supply Inspector messages.
    /// # Errors
    /// Rejects stale epochs, ended ownership, pending floods or unavailable transport.
    pub fn command(&self, action: DebugAction) -> Result<u64, &'static str> {
        if matches!(action, DebugAction::Stop) {
            self.stop("Stopped by its owner");
            return Ok(0);
        }
        let mut state = self.state.lock().map_err(|_| "Debugger unavailable")?;
        if state.connection.closed() || !state.connection.attached() || state.connection.detaching()
        {
            return Err("Debugger session is unavailable");
        }
        let pause = match &action {
            DebugAction::Evaluate { pause, .. }
            | DebugAction::Inspect { pause, .. }
            | DebugAction::Complete { pause, .. }
            | DebugAction::Resume { pause, .. }
            | DebugAction::ContinueTo { pause, .. }
            | DebugAction::Scopes { pause, .. }
            | DebugAction::Properties { pause, .. }
            | DebugAction::Release { pause, .. } => Some(*pause),
            _ => None,
        };
        if pause.is_some_and(|pause| {
            if pause == 0 {
                state.paused_at.is_some()
            } else {
                state.paused_at.is_none() || pause != state.pause
            }
        }) {
            return Err("Pause has expired or resumed");
        }
        if matches!(action, DebugAction::Detach)
            && !self.options.persistent
            && (state.active || state.paused_at.is_some())
        {
            return Err("Finish the current operation before detaching");
        }
        if state.pending.len() >= MAX_DEBUG_COMMANDS {
            return Err("Debugger command queue is full");
        }
        let request = state
            .request
            .checked_add(1)
            .ok_or("Debugger request limit reached")?;
        let command = DebugCommand {
            session: self.id().to_owned(),
            request,
            action,
        };
        if !command.is_bounded() {
            return Err("Invalid debugger command");
        }
        let writer = state.writer.clone().ok_or("Debugger control unavailable")?;
        state.pending.insert(
            request,
            Pending {
                started: false,
                since: Instant::now(),
                expected: Expected::from_action(&command.action),
                inspection: matches!(
                    command.action,
                    DebugAction::Scopes { .. }
                        | DebugAction::Inspect { .. }
                        | DebugAction::Complete { .. }
                        | DebugAction::Properties { .. }
                        | DebugAction::Release { .. }
                ),
            },
        );
        state.request = request;
        if let DebugAction::Exceptions { policy } = command.action {
            state.exceptions = policy;
        }
        if let DebugAction::Breakpoints { values } = &command.action {
            state.breakpoint_request = request;
            state.breakpoints = values
                .iter()
                .map(|value| {
                    (
                        value.id,
                        BreakpointResolution {
                            id: value.id,
                            requested: value.location.clone(),
                            resolved: None,
                            issue: Some("Pending source resolution".into()),
                        },
                    )
                })
                .collect();
        }
        if matches!(command.action, DebugAction::Detach) {
            state.connection = Connection::Detaching(request);
        }
        drop(state);
        if !writer(command) {
            self.stop("Debugger control transport closed");
            return Err("Debugger control transport closed");
        }
        Ok(request)
    }
    /// Accepts only already authenticated host events and checks session/operation/pause ordering.
    /// # Errors
    /// Rejects excessive, stale or impossible transitions; caller must terminate the worker.
    #[allow(clippy::too_many_lines)] // Validate correlated transitions and commit their evidence under one lock.
    pub fn accept(&self, mut event: DebugEvent) -> Result<(), &'static str> {
        let bytes = serde_json::to_vec(&event)
            .map_err(|_| "Invalid debugger event")?
            .len();
        if bytes > MAX_DEBUG_REPLY_BYTES
            || event.session != self.id()
            || event.value_generation.is_some()
        {
            return Err("Invalid debugger event identity or extent");
        }
        let mut state = self.state.lock().map_err(|_| "Debugger unavailable")?;
        if state.connection.closed() {
            return Err("Debugger event after session closure");
        }
        let global = matches!(
            event.event,
            DebugEventKind::Evaluating
                | DebugEventKind::Output { .. }
                | DebugEventKind::Evaluated { .. }
                | DebugEventKind::Completions { .. }
                | DebugEventKind::Control { .. }
                | DebugEventKind::Logpoint { .. }
                | DebugEventKind::LogpointStarted
                | DebugEventKind::Properties { pause: 0, .. }
                | DebugEventKind::Sources { .. }
                | DebugEventKind::Acknowledged
                | DebugEventKind::Rejected { .. }
                | DebugEventKind::Stopped { .. }
                | DebugEventKind::Detached
        );
        if event.invocation != state.invocation && (!global || event.invocation > state.invocation)
        {
            return Err("Invalid debugger operation identity");
        }
        if event.request != 0
            && !state
                .pending
                .get(&event.request)
                .is_some_and(|pending| pending.expected.accepts(&event.event))
        {
            return Err("Uncorrelated debugger reply");
        }
        if event.request == 0
            && matches!(
                event.event,
                DebugEventKind::Scopes { .. }
                    | DebugEventKind::Completions { .. }
                    | DebugEventKind::Properties { .. }
                    | DebugEventKind::Acknowledged
                    | DebugEventKind::Rejected { .. }
            )
        {
            return Err("Debugger reply requires a request identity");
        }
        match &mut event.event {
            DebugEventKind::LogpointStarted => {
                if event.request != 0
                    || !state.active
                    || state.paused_at.is_some()
                    || state.logpoint_at.is_some()
                    || state.pending.values().any(|pending| pending.started)
                {
                    return Err("Invalid logpoint inspection start");
                }
                state.logpoint_at = Some(Instant::now());
                self.clock.pause();
            }
            DebugEventKind::Control {
                skip_pauses,
                pause_next,
            } => {
                if *skip_pauses && *pause_next {
                    return Err("Invalid debugger execution controls");
                }
                state.controls.skip_pauses = *skip_pauses;
                state.controls.pause_next = *pause_next;
            }
            DebugEventKind::Logpoint {
                location,
                original,
                text,
                time_unix_ms,
                omitted,
                ..
            } => {
                if event.request != 0
                    || original.is_some()
                    || text.len() > crate::debugger::MAX_VALUE_BYTES
                    || *time_unix_ms > 9_007_199_254_740_991
                    || location.as_ref().is_some_and(|location| {
                        !valid_location(location) || !state.sources.contains_key(&location.source)
                    })
                {
                    return Err("Invalid debugger logpoint output");
                }
                if location.is_some() {
                    if state.logpoint_at.take().is_none() {
                        return Err("Logpoint output arrived before inspection started");
                    }
                    self.clock.resume();
                } else if *omitted == 0 {
                    return Err("Invalid logpoint omission notice");
                }
            }
            DebugEventKind::Completions { pause, matches, .. } => {
                if (if *pause == 0 {
                    state.paused_at.is_some()
                } else {
                    state.paused_at.is_none() || *pause != state.pause
                }) || matches.len() > 64
                    || matches
                        .iter()
                        .any(|name| name.len() > 256 || name.chars().any(char::is_control))
                {
                    return Err("Invalid debugger completion reply");
                }
            }
            DebugEventKind::Output { text, .. } => {
                if text.len() > crate::MAX_LOG_RECORD_BYTES
                    || !state
                        .pending
                        .get(&event.request)
                        .is_some_and(|pending| pending.started)
                {
                    return Err("Invalid console output");
                }
            }
            DebugEventKind::Evaluating => {
                if state.logpoint_at.is_some()
                    || state.pending.values().any(|pending| pending.started)
                {
                    return Err("Overlapping debugger evaluation");
                }
                let pending = state
                    .pending
                    .get_mut(&event.request)
                    .ok_or("Uncorrelated console start")?;
                if pending.started {
                    return Err("Duplicate console start");
                }
                pending.started = true;
                pending.since = Instant::now();
                if state.paused_at.is_none() {
                    self.clock.pause();
                }
            }
            DebugEventKind::Evaluated { pause, value, .. } => {
                if !state
                    .pending
                    .get(&event.request)
                    .is_some_and(|pending| pending.started)
                {
                    return Err("Console result arrived before evaluation started");
                }
                if if *pause == 0 {
                    state.paused_at.is_some()
                } else {
                    state.paused_at.is_none() || *pause != state.pause
                } {
                    return Err("Stale console reply");
                }
                if value.display.len() > crate::debugger::MAX_VALUE_BYTES
                    || value.kind.len() > 128
                    || value.handle == Some(0)
                {
                    return Err("Console result exceeds its budget");
                }
                if state.paused_at.is_none() {
                    self.clock.resume();
                }
            }
            DebugEventKind::Attached => {
                if state.connection.attached() {
                    return Err("Duplicate debugger attachment");
                }
                state.connection = Connection::Attached;
            }
            DebugEventKind::Paused {
                pause,
                frames,
                reason,
                ..
            } => {
                if !state.connection.attached()
                    || !state.active
                    || state.logpoint_at.is_some()
                    || state.paused_at.is_some()
                    || *pause <= state.pause
                    || frames.len() > crate::debugger::MAX_DEBUG_FRAMES
                    || reason.len() > 128
                    || frames.iter().enumerate().any(|(index, frame)| {
                        usize::try_from(frame.index) != Ok(index)
                            || frame.function.len() > 2048
                            || frame.original.is_some()
                            || frame
                                .location
                                .as_ref()
                                .is_some_and(|location| !valid_location(location))
                    })
                {
                    return Err("Invalid debugger pause transition");
                }
                state.pause = *pause;
                state.controls.pause_next = false;
                for pending in state.pending.values_mut() {
                    if !pending.started {
                        pending.since = Instant::now();
                    }
                }
                state.paused_at = Some(Instant::now());
                self.clock.pause();
            }
            DebugEventKind::Resumed => {
                if state.paused_at.take().is_none() {
                    return Err("Invalid debugger resume transition");
                }
                self.clock.resume();
            }
            DebugEventKind::Scopes { pause, scopes, .. } => {
                if state.paused_at.is_none() || *pause != state.pause {
                    return Err("Stale debugger value reply");
                }
                if scopes.len() > 16
                    || scopes.iter().any(|scope| {
                        scope.kind.len() > 128 || scope.name.len() > 2048 || scope.handle == Some(0)
                    })
                {
                    return Err("Excessive debugger scopes");
                }
            }
            DebugEventKind::Properties {
                pause,
                properties,
                next,
                offset,
                ..
            } => {
                if if *pause == 0 {
                    state.paused_at.is_some()
                } else {
                    state.paused_at.is_none() || *pause != state.pause
                } {
                    return Err("Stale debugger value reply");
                }
                if properties.len() > crate::debugger::MAX_PROPERTIES
                    || next.is_some_and(|next| {
                        Some(next)
                            != offset
                                .checked_add(u32::try_from(properties.len()).unwrap_or(u32::MAX))
                            || properties.is_empty()
                    })
                    || properties.iter().any(|property| {
                        property.name.len() > 2048
                            || property.value.kind.len() > 128
                            || property.value.display.len() > crate::debugger::MAX_VALUE_BYTES
                            || property.value.handle == Some(0)
                    })
                {
                    return Err("Invalid debugger property page");
                }
            }
            DebugEventKind::Detached => {
                if !self.options.persistent && (state.active || state.paused_at.is_some()) {
                    return Err("Detach during an operation");
                }
                state.connection = Connection::Detached;
                state.paused_at = None;
                self.clock.resume();
                state.writer = None;
                state.stop = None;
            }
            DebugEventKind::Stopped { .. } => {
                state.connection = Connection::Stopped;
                state.paused_at = None;
                self.clock.resume();
            }
            DebugEventKind::Sources {
                sources,
                breakpoints,
            } => {
                if sources.len() > crate::debugger::MAX_DEBUG_SOURCES
                    || breakpoints.len() > crate::debugger::MAX_BREAKPOINTS
                {
                    return Err("Excessive debugger inventory");
                }
                for source in sources {
                    if source.source.is_empty()
                        || source.source.len() > 2048
                        || (!state.sources.contains_key(&source.source)
                            && state.sources.len() >= crate::debugger::MAX_DEBUG_SOURCES)
                    {
                        return Err("Invalid debugger module inventory");
                    }
                    state.sources.insert(source.source.clone(), source.clone());
                }
                if event.request != state.breakpoint_request {
                    breakpoints.clear();
                }
                for breakpoint in breakpoints {
                    if let Some(existing) = state.breakpoints.get(&breakpoint.id) {
                        if existing.requested != breakpoint.requested
                            || breakpoint
                                .resolved
                                .as_ref()
                                .is_some_and(|location| !valid_location(location))
                            || breakpoint
                                .issue
                                .as_ref()
                                .is_some_and(|issue| issue.len() > 2048)
                        {
                            return Err("Invalid debugger breakpoint resolution");
                        }
                        state.breakpoints.insert(breakpoint.id, breakpoint.clone());
                    }
                }
            }
            DebugEventKind::Rejected { .. } => {
                if state.connection == Connection::Detaching(event.request) {
                    state.connection = Connection::Attached;
                }
            }
            DebugEventKind::Acknowledged => {}
        }
        if event.request != 0
            && !matches!(
                event.event,
                DebugEventKind::Sources { .. }
                    | DebugEventKind::Evaluating
                    | DebugEventKind::Output { .. }
            )
        {
            state.pending.remove(&event.request);
            // One engine services this finite queue. Waiting commands receive
            // a fresh progress budget after each completed operation.
            for pending in state
                .pending
                .values_mut()
                .filter(|pending| !pending.started)
            {
                pending.since = Instant::now();
            }
        }
        let stopped = matches!(event.event, DebugEventKind::Stopped { .. });
        if state.connection.closed() {
            state.controls.pause_next = false;
            state.logpoint_at = None;
        }
        let stop = if stopped {
            state.writer = None;
            state.pending.clear();
            state.stop.take()
        } else {
            None
        };
        event.value_generation = Some(state.pause);
        match &event.event {
            DebugEventKind::Paused { .. } => state.pause_snapshot = Some(event.clone()),
            DebugEventKind::Resumed | DebugEventKind::Detached | DebugEventKind::Stopped { .. } => {
                state.pause_snapshot = None;
            }
            _ => {}
        }
        retain(&mut state, event, bytes);
        let finished = if state.connection.closed() {
            state.finished.take()
        } else {
            None
        };
        let detach = state.detach_requested && state.connection == Connection::Attached;
        if detach {
            state.detach_requested = false;
        }
        drop(state);
        if let Some(stop) = stop {
            stop();
        }
        if let Some(finished) = finished {
            finished();
        }
        if detach {
            self.command(DebugAction::Detach)?;
        }
        Ok(())
    }
    /// Stops independently of the engine, pipe writer and UI polling.
    pub fn stop(&self, reason: &'static str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.connection.closed() {
            return;
        }
        state.connection = Connection::Stopped;
        state.controls.pause_next = false;
        state.logpoint_at = None;
        state.paused_at = None;
        state.pause_snapshot = None;
        state.pending.clear();
        state.writer = None;
        self.clock.resume();
        let event = DebugEvent {
            value_generation: None,
            session: self.id().into(),
            invocation: state.invocation,
            request: 0,
            event: DebugEventKind::Stopped {
                reason: reason.into(),
            },
        };
        retain(&mut state, event, 256);
        let stop = state.stop.take();
        let finished = state.finished.take();
        drop(state);
        if let Some(stop) = stop {
            stop();
        }
        if let Some(finished) = finished {
            finished();
        }
    }
    /// Polls ephemeral evidence for the owning local desktop view.
    pub fn page(&self, after: u64) -> SessionPage {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let omitted = after < state.sequence
            && state
                .events
                .front()
                .is_none_or(|(sequence, ..)| after.saturating_add(1) < *sequence);
        SessionPage {
            operation: (state.active && !state.connection.closed())
                .then(|| {
                    state
                        .logs
                        .iter()
                        .rev()
                        .map(|capture| capture.context())
                        .find(|context| context.invocation == state.invocation)
                })
                .flatten(),
            value_generation: state.pause,
            controls: state.controls,
            exceptions: state.exceptions,
            paused: state.pause_snapshot.clone(),
            inventory: (after == 0 || omitted).then(|| DebugInventory {
                sources: state.sources.values().cloned().collect(),
                breakpoints: state.breakpoints.values().cloned().collect(),
            }),
            active: state.active,
            exchange_id: state.exchange.clone(),
            arm_remaining_ms: state.armed_at.map(|start| {
                ARM_LEASE_MS
                    .saturating_sub(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
            }),
            logs: state
                .logs
                .iter()
                .filter_map(|capture| capture.snapshot())
                .collect(),
            session: self.id().into(),
            cursor: state.sequence,
            omitted,
            closed: state.connection.closed(),
            pause_remaining_ms: state.paused_at.map(|start| {
                PAUSE_LEASE_MS
                    .saturating_sub(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
            }),
            session_remaining_ms: SESSION_LEASE_MS.saturating_sub(
                u64::try_from(self.created.elapsed().as_millis()).unwrap_or(u64::MAX),
            ),
            events: state
                .events
                .iter()
                .filter(|(sequence, ..)| *sequence > after)
                .map(|(_, event, _)| event.clone())
                .collect(),
        }
    }
}
fn valid_location(location: &crate::SourceLocation) -> bool {
    !location.source.is_empty()
        && location.source.len() <= 2048
        && !location.source.chars().any(char::is_control)
        && location.line > 0
        && location.column > 0
        && i32::try_from(location.line).is_ok()
        && i32::try_from(location.column).is_ok()
}
fn retain(state: &mut State, event: DebugEvent, bytes: usize) {
    state.sequence = state.sequence.saturating_add(1);
    state.bytes += bytes;
    state.events.push_back((state.sequence, event, bytes));
    while state.events.len() > 64 || state.bytes > HISTORY_BYTES {
        if let Some((_, _, bytes)) = state.events.pop_front() {
            state.bytes -= bytes;
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connection.closed()
            && let Some(stop) = state.stop.take()
        {
            stop();
        }
        if let Some(finished) = state.finished.take() {
            finished();
        }
    }
}

/// Synchronous ordinary output until debugging opts into a native read pump.
/// Only one reader ever owns the pipe. Typed debug events never enter normal reply queues.
pub struct ReplyReader<R, T> {
    direct: Option<R>,
    pumped: Option<std::sync::mpsc::Receiver<Result<T, String>>>,
    read: fn(&mut R) -> Result<T, String>,
}
impl<R: std::io::Read + Send + 'static, T: Send + 'static> ReplyReader<R, T> {
    /// Whether this worker has opted into the native debugger output pump.
    pub fn debugging(&self) -> bool {
        self.pumped.is_some()
    }
    /// Constructs the allocation-free ordinary reader path.
    pub fn new(input: R, read: fn(&mut R) -> Result<T, String>) -> Self {
        Self {
            direct: Some(input),
            pumped: None,
            read,
        }
    }
    /// Reads the next ordinary host result/console frame.
    /// # Errors
    /// Returns a safe transport failure after closure.
    pub fn read(&mut self) -> Result<T, String> {
        if let Some(input) = self.direct.as_mut() {
            (self.read)(input)
        } else {
            self.pumped
                .as_ref()
                .ok_or("Worker reader unavailable")?
                .recv()
                .map_err(|_| "Worker output closed")?
        }
    }
    /// Starts the debug-only pump. The callback consumes authenticated debug frames.
    /// # Errors
    /// Returns a safe reader resource failure.
    pub fn enable(
        &mut self,
        filter: impl Fn(T) -> Result<Option<T>, String> + Send + 'static,
        on_closed: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        if self.pumped.is_some() {
            return Ok(());
        }
        let mut input = self.direct.take().ok_or("Worker reader unavailable")?;
        let read = self.read;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("script-debug-output".into())
            .spawn(move || {
                loop {
                    match read(&mut input).and_then(&filter) {
                        Ok(Some(value)) => {
                            if sender.send(Ok(value)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            on_closed();
                            let _ = sender.send(Err(error));
                            break;
                        }
                    }
                }
            })
            .map_err(|_| "Debugger reader unavailable")?;
        self.pumped = Some(receiver);
        Ok(())
    }
}

/// Weak session binding shared by a worker reader and native controller.
pub type SessionBinding = Arc<Mutex<Weak<Session>>>;

/// Active execution budget, excluding only authenticated bounded native pause time.
/// The ordinary path delegates directly to Tokio's existing deadline behavior.
/// # Errors
/// Returns `BudgetExpired` when active execution exceeds the original budget.
pub fn wait_budget<T, F: std::future::Future<Output = T>>(
    duration: Duration,
    clock: Option<&Arc<PauseClock>>,
    future: F,
) -> impl std::future::Future<Output = Result<T, BudgetExpired>> + use<T, F> {
    use futures_util::FutureExt as _;
    match clock {
        None => futures_util::future::Either::Left(
            tokio::time::timeout(duration, future).map(|result| result.map_err(|_| BudgetExpired)),
        ),
        Some(clock) => futures_util::future::Either::Right(debug_budget(
            duration,
            Arc::clone(clock),
            Box::pin(future),
        )),
    }
}
/// Waits on work that depends on several workers, excluding their combined debug blocking.
/// Shared clocks are counted once; overlapping pauses never subtract more than wall time.
/// # Errors
/// Returns `BudgetExpired` when active execution exhausts the original budget.
pub async fn wait_budgets<T, F: std::future::Future<Output = T>>(
    duration: Duration,
    clocks: &[Arc<PauseClock>],
    future: F,
) -> Result<T, BudgetExpired> {
    if clocks.is_empty() {
        return wait_budget(duration, None, future).await;
    }
    let mut unique: Vec<Arc<PauseClock>> = Vec::new();
    for clock in clocks {
        if !unique.iter().any(|previous| Arc::ptr_eq(previous, clock)) {
            unique.push(Arc::clone(clock));
        }
    }
    let mut snapshots: Vec<_> = unique.iter().map(|clock| clock.elapsed()).collect();
    let mut previous = Instant::now();
    let mut active = Duration::ZERO;
    tokio::pin!(future);
    loop {
        let now = Instant::now();
        let delta = now.duration_since(previous);
        previous = now;
        let mut paused = Duration::ZERO;
        for (clock, snapshot) in unique.iter().zip(&mut snapshots) {
            let elapsed = clock.elapsed();
            paused = paused.saturating_add(elapsed.saturating_sub(*snapshot));
            *snapshot = elapsed;
        }
        active = active.saturating_add(delta.saturating_sub(paused));
        let remaining = duration.checked_sub(active).ok_or(BudgetExpired)?;
        tokio::select! {value=&mut future=>return Ok(value),()=tokio::time::sleep(remaining.min(Duration::from_millis(10)))=>{}}
    }
}
async fn debug_budget<T, F: std::future::Future<Output = T>>(
    duration: Duration,
    clock: Arc<PauseClock>,
    mut future: std::pin::Pin<Box<F>>,
) -> Result<T, BudgetExpired> {
    let started = Instant::now();
    let paused = clock.elapsed();
    loop {
        let elapsed = started
            .elapsed()
            .saturating_sub(clock.elapsed().saturating_sub(paused));
        let remaining = duration.checked_sub(elapsed).ok_or(BudgetExpired)?;
        tokio::select! {value=&mut future=>return Ok(value),()=tokio::time::sleep(remaining.min(Duration::from_millis(10)))=>{}}
    }
}
/// A finite active execution budget expired.
#[derive(Debug)]
pub struct BudgetExpired;
