//! Native-only debugger commands, including explicit bounded local evaluation.
//! The Inspector wire protocol is never exposed to the UI.

use crate::{ExceptionDiagnostic, SourceLocation};
use serde::{Deserialize, Serialize};

/// Source breakpoints per session, including unresolved requests.
pub const MAX_BREAKPOINTS: usize = 128;
/// Aggregate source-name bytes in one breakpoint configuration.
pub const MAX_BREAKPOINT_TEXT_BYTES: usize = 8192;
/// Registered module identities per debugger session.
pub const MAX_DEBUG_SOURCES: usize = 128;
/// Stack frames retained for one pause.
pub const MAX_DEBUG_FRAMES: usize = 32;
/// Own properties returned by one read.
pub const MAX_PROPERTIES: usize = 100;
/// Retained engine values between pause/resume boundaries, including idle Console results.
pub const MAX_OBJECT_HANDLES: usize = 64;
/// Maximum serialized native debug reply.
pub const MAX_DEBUG_REPLY_BYTES: usize = 32 * 1024;
/// Maximum preview/name text; reads never invoke conversion on objects.
pub const MAX_VALUE_BYTES: usize = 2048;
/// Finite controller commands awaiting the engine thread.
pub const MAX_DEBUG_COMMANDS: usize = 32;
/// One native pause lease; UI heartbeats cannot extend it.
pub const PAUSE_LEASE_MS: u64 = 60_000;
/// Absolute session lifetime, independent of continued controller activity.
pub const SESSION_LEASE_MS: u64 = 10 * 60_000;
/// Maximum explicit live reservation wait for an exchange.
pub const ARM_LEASE_MS: u64 = 60_000;

/// Inspector exception policy, selected explicitly by the local author.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExceptionPolicy {
    /// Pause only at source breakpoints or authored debugger statements.
    #[default]
    None,
    /// Pause on exceptions that escape a handler.
    Uncaught,
    /// Pause on caught and uncaught exceptions.
    All,
}

/// Requested generated-code position. Supervisors map original sources before transport.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBreakpoint {
    /// Side-effect-checked value to log instead of pausing when its condition matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_expression: Option<String>,
    /// Optional side-effect-free JavaScript predicate evaluated in the hit frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Client identity, unique within this finite breakpoint list.
    pub id: u32,
    /// Exact registered module identity, never a URL pattern or file-read permission.
    pub location: SourceLocation,
}

/// Explicit opt-in before initialization or on an idle existing worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugOptions {
    /// Temporarily suppress all engine pauses, including logpoint hits.
    #[serde(default)]
    pub skip_pauses: bool,
    /// Fresh fixture runs can stop at the first executable statement in verified user modules.
    #[serde(default)]
    pub pause_on_start: bool,
    /// A visible workspace owns this attachment until explicitly detached.
    #[serde(default)]
    pub persistent: bool,
    /// Native random 128-bit session identity, encoded as 32 lowercase hexadecimal digits.
    pub session: String,
    /// Initial pause-on-exception policy.
    pub exceptions: ExceptionPolicy,
    /// Initial generated positions, resolved against actual compiled engine script IDs.
    pub breakpoints: Vec<SourceBreakpoint>,
}
impl DebugOptions {
    /// Validates finite native configuration before creating an Inspector session.
    pub fn is_bounded(&self) -> bool {
        self.session.len() == 32
            && self
                .session
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && valid_breakpoints(&self.breakpoints)
    }
}
/// Validates exact generated breakpoint coordinates and distinct client IDs.
pub fn valid_breakpoints(values: &[SourceBreakpoint]) -> bool {
    values.len() <= MAX_BREAKPOINTS
        && values
            .iter()
            .map(|value| value.location.source.len())
            .sum::<usize>()
            <= MAX_BREAKPOINT_TEXT_BYTES
        && values.iter().all(|value| {
            value
                .log_expression
                .as_ref()
                .is_none_or(|text| !text.trim().is_empty() && text.len() <= 4096)
                && value
                    .condition
                    .as_ref()
                    .is_none_or(|text| !text.is_empty() && text.len() <= 4096)
                && !value.location.source.is_empty()
                && value.location.source.len() <= 2048
                && !value.location.source.chars().any(char::is_control)
                && value.location.line > 0
                && value.location.column > 0
                && i32::try_from(value.location.line).is_ok()
                && i32::try_from(value.location.column).is_ok()
        })
        && values
            .iter()
            .map(|value| value.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == values.len()
}

/// A scoped execution action; no expression can be supplied as a step condition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResumeAction {
    /// Continue until another breakpoint, exception or debugger statement.
    Continue,
    /// Enter the next called function.
    Into,
    /// Execute the current statement in the selected call frame.
    Over,
    /// Return from the current call frame.
    Out,
}

/// Narrow typed command. The containing host envelope authenticates its worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugCommand {
    /// Exact authorized session identity.
    pub session: String,
    /// Monotonic native request identity, independently correlated from invocations.
    pub request: u64,
    /// Allowed operation; unknown fields and arbitrary protocol methods are rejected.
    pub action: DebugAction,
}
impl DebugCommand {
    /// Validates finite work before accepting it into a native control queue.
    pub fn is_bounded(&self) -> bool {
        self.request > 0
            && self.session.len() == 32
            && self
                .session
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && match &self.action {
                DebugAction::ContinueTo { pause, location } => {
                    *pause > 0
                        && valid_breakpoints(&[SourceBreakpoint {
                            id: 0,
                            condition: None,
                            log_expression: None,
                            location: location.clone(),
                        }])
                }
                DebugAction::Breakpoints { values } => valid_breakpoints(values),
                DebugAction::Release { handles, .. } => handles.len() <= MAX_OBJECT_HANDLES,
                DebugAction::Scopes { pause, frame } => {
                    *pause > 0 && (*frame as usize) < MAX_DEBUG_FRAMES
                }
                DebugAction::Properties { handle, .. } => *handle > 0,
                DebugAction::Resume { pause, .. } => *pause > 0,
                DebugAction::Evaluate {
                    expression,
                    pause,
                    frame,
                }
                | DebugAction::Inspect {
                    expression,
                    pause,
                    frame,
                } => {
                    !expression.trim().is_empty()
                        && expression.len() <= 8192
                        && ((*pause == 0 && frame.is_none())
                            || (*pause > 0
                                && frame.is_some_and(|frame| (frame as usize) < MAX_DEBUG_FRAMES)))
                }
                DebugAction::Complete {
                    expression,
                    pause,
                    frame,
                } => {
                    expression.len() <= 1024
                        && !expression.chars().any(char::is_control)
                        && expression.split('.').count() <= 8
                        && ((*pause == 0 && frame.is_none())
                            || (*pause > 0
                                && frame.is_some_and(|frame| (frame as usize) < MAX_DEBUG_FRAMES)))
                }
                DebugAction::Exceptions { .. }
                | DebugAction::SkipPauses { .. }
                | DebugAction::PauseNext { .. }
                | DebugAction::Stop
                | DebugAction::Detach => true,
            }
    }
}

/// Commands execute on the engine's own thread; Stop also has independent native enforcement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DebugAction {
    /// Suppress future pauses without discarding breakpoint or exception choices.
    SkipPauses {
        /// Whether to suppress all pause reasons and logpoints.
        skip: bool,
    },
    /// Pause at the next statement in a verified user callback.
    PauseNext {
        /// False cancels an earlier request.
        enabled: bool,
    },
    /// Resume to one exact loaded generated position, without saving a breakpoint.
    ContinueTo {
        /// Current pause generation.
        pause: u64,
        /// Exact registered generated source and position.
        location: SourceLocation,
    },
    /// Side-effect-checked expression preview for watches and automatic inspection.
    Inspect {
        /// Bounded expression; mutation is rejected by the engine.
        expression: String,
        /// Current pause generation, or zero for idle globals.
        pause: u64,
        /// Exact paused lexical frame, absent for idle globals.
        frame: Option<u32>,
    },
    /// Inert identifier/property completion; never evaluates an expression.
    Complete {
        /// Dot-separated data-property path and final name prefix.
        expression: String,
        /// Current pause generation, or zero for idle globals.
        pause: u64,
        /// Exact paused lexical frame, absent for idle globals.
        frame: Option<u32>,
    },
    /// Explicit local JavaScript, in a paused frame or an idle global context.
    Evaluate {
        /// Bounded command text, retained only in local developer memory.
        expression: String,
        /// Zero selects the idle global context.
        pause: u64,
        /// Required for a paused lexical scope.
        frame: Option<u32>,
    },
    /// Atomically replace source breakpoints, returning actual resolved positions.
    Breakpoints {
        /// Finite exact-source requests.
        values: Vec<SourceBreakpoint>,
    },
    /// Select caught/uncaught exception handling.
    Exceptions {
        /// Explicit policy.
        policy: ExceptionPolicy,
    },
    /// Continue or step in this exact pause epoch.
    Resume {
        /// Current pause generation, invalidated on every resume.
        pause: u64,
        /// Continue/step operation.
        action: ResumeAction,
    },
    /// Read scopes of a retained call frame, with lexical scope order preserved.
    Scopes {
        /// Current pause generation.
        pause: u64,
        /// Native stack index from the paused event.
        frame: u32,
    },
    /// Read a bounded page of own data descriptors; getters and proxies stay unavailable.
    Properties {
        /// Current pause generation, or zero for an idle console value.
        pause: u64,
        /// Native value handle returned in this attachment and scope.
        handle: u32,
        /// Zero-based property offset, without running code between pages.
        offset: u32,
    },
    /// Release value handles early without changing execution.
    Release {
        /// Current pause generation, or zero for idle Console values.
        pause: u64,
        /// Handles to release, bounded by the per-pause handle ceiling.
        handles: Vec<u32>,
    },
    /// Stop execution. Session and worker owners independently kill unresponsive processes.
    Stop,
    /// Detach from an idle worker after normal completion, preserving its actual heap.
    Detach,
}

/// An exact engine-registered module. Inspector-reported sourceURL labels are not authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugSource {
    /// Exact verified module identity.
    pub source: String,
    /// Byte length of immutable executing code, when retained by this attachment.
    pub bytes: Option<u32>,
}
/// Resolution of one source request; missing locations remain visibly unbound.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BreakpointResolution {
    /// Original client identity.
    pub id: u32,
    /// Requested generated coordinates.
    pub requested: SourceLocation,
    /// Actual executable position; absent while pending or unbound.
    pub resolved: Option<SourceLocation>,
    /// Host-owned explanation for an unresolved request.
    pub issue: Option<String>,
}
/// Bounded native frame; internal Inspector call-frame IDs never leave the runtime.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugFrame {
    /// Pause-local native stack index.
    pub index: u32,
    /// Bounded engine-owned name.
    pub function: String,
    /// Generated position, attributed through authoritative engine script IDs.
    pub location: Option<SourceLocation>,
    /// Original position verified by the native supervisor against the executing map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<SourceLocation>,
    /// Whether some scopes or frame metadata were unavailable.
    pub unavailable: bool,
}
/// A lexical environment. Reading it never evaluates a watch expression.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugScope {
    /// Local/closure/module/global/etc., as inert text.
    pub kind: String,
    /// Optional bounded engine name.
    pub name: String,
    /// Native pause-local handle, absent when unavailable or at the handle limit.
    pub handle: Option<u32>,
}
/// Inert read-only value preview; objects are never stringified or expanded implicitly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugValue {
    /// Host-owned primitive/object/array/function/proxy/unavailable classification.
    pub kind: String,
    /// Bounded primitive text or inert label.
    pub display: String,
    /// Native scope-local value identity, with no Inspector object ID or pointer.
    pub handle: Option<u32>,
    /// Some primitive text was shortened.
    pub truncated: bool,
}
/// Own property descriptor. Accessors are represented without invoking them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugProperty {
    /// Bounded primitive key text; symbols retain an explicit label.
    pub name: String,
    /// Inert value or unavailable/accessor marker.
    pub value: DebugValue,
}
/// Bounded event/reply stream, separate from ordinary callback results and logs.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugEvent {
    /// Supervisor-stamped value lifetime; never accepted from an isolated host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_generation: Option<u64>,
    /// Authenticated session identity, independent of display labels.
    pub session: String,
    /// Active callback, zero for module initialization.
    pub invocation: u64,
    /// Correlated controller request; zero denotes an unsolicited state transition.
    pub request: u64,
    /// Native typed result.
    pub event: DebugEventKind,
}
impl std::fmt::Debug for DebugEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.event {
            DebugEventKind::Control { .. } => "control",
            DebugEventKind::Logpoint { .. } => "logpoint",
            DebugEventKind::LogpointStarted => "logpoint-started",
            DebugEventKind::Output { .. } => "output",
            DebugEventKind::Evaluating => "evaluating",
            DebugEventKind::Evaluated { .. } => "evaluated",
            DebugEventKind::Completions { .. } => "completions",
            DebugEventKind::Attached => "attached",
            DebugEventKind::Detached => "detached",
            DebugEventKind::Sources { .. } => "sources",
            DebugEventKind::Paused { .. } => "paused",
            DebugEventKind::Resumed => "resumed",
            DebugEventKind::Scopes { .. } => "scopes",
            DebugEventKind::Properties { .. } => "properties",
            DebugEventKind::Acknowledged => "acknowledged",
            DebugEventKind::Rejected { .. } => "rejected",
            DebugEventKind::Stopped { .. } => "stopped",
        };
        f.debug_struct("DebugEvent")
            .field("invocation", &self.invocation)
            .field("request", &self.request)
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}
/// Typed event payloads; source text, locals and exceptions remain ephemeral local evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DebugEventKind {
    /// Engine-only automatic logpoint inspection began; suspend dependent budgets.
    LogpointStarted,
    /// Native execution-control state, independent of saved breakpoint intent.
    Control {
        /// Whether future pauses and logpoints are suppressed.
        skip_pauses: bool,
        /// A pause is scheduled for the next verified user callback.
        pause_next: bool,
    },
    /// Bounded output from a logpoint; no value handles or Inspector IDs escape.
    Logpoint {
        /// Actual verified generated position; absent for an omission notice.
        location: Option<SourceLocation>,
        /// Original position verified by the application.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original: Option<SourceLocation>,
        /// Inert primitive text or object label.
        text: String,
        /// Evaluating the log expression failed.
        thrown: bool,
        /// Additional messages omitted by the rate budget.
        omitted: u64,
        /// Native observation time for merging with Console output.
        time_unix_ms: u64,
    },
    /// Bounded data-property names from the selected lexical or global scope.
    Completions {
        /// Requested scope epoch; zero selects idle globals.
        pause: u64,
        /// Full property names matching the final prefix.
        matches: Vec<String>,
        /// Additional names were omitted by the inspection budget.
        omitted: bool,
    },
    /// Console calls made by an explicit Immediate command, separate from callback logs.
    Output {
        /// Native console level.
        level: crate::LogLevel,
        /// Bounded inert snapshot text.
        text: String,
    },
    /// An explicit console command began running inside the selected isolate.
    Evaluating,
    /// Bounded console result, including an exception outcome.
    Evaluated {
        /// Requested frame generation, or zero for globals.
        pause: u64,
        /// Inert native preview, with handles scoped to this attachment.
        value: DebugValue,
        /// The command threw instead of returning.
        thrown: bool,
    },
    /// Session is attached; ordinary script activation remains independent.
    Attached,
    /// Idle debugger detached; script activation and worker globals are unchanged.
    Detached,
    /// Exact compiled modules or breakpoint resolution changed.
    Sources {
        /// Finite module inventory.
        sources: Vec<DebugSource>,
        /// Truthful current resolution, including unbound breakpoints.
        breakpoints: Vec<BreakpointResolution>,
    },
    /// Engine is paused in one exact invocation and epoch.
    Paused {
        /// Monotonically increasing pause generation.
        pause: u64,
        /// Host-controlled pause reason.
        reason: String,
        /// Bounded engine-owned stack.
        frames: Vec<DebugFrame>,
        /// Safely captured thrown value when this is an exception pause.
        exception: Option<Box<ExceptionDiagnostic>>,
        /// Frames were omitted by an evidence limit.
        truncated: bool,
    },
    /// Previous stack and value handles are invalid from this point onward.
    Resumed,
    /// Read-only lexical scope reply.
    Scopes {
        /// Current pause generation.
        pause: u64,
        /// Selected stack index.
        frame: u32,
        /// In lexical order, preserving shadowing between scopes.
        scopes: Vec<DebugScope>,
    },
    /// Read-only own-property page.
    Properties {
        /// Current pause generation, or zero for idle Console values.
        pause: u64,
        /// Requested native value handle.
        handle: u32,
        /// Requested property offset.
        offset: u32,
        /// Finite own descriptors.
        properties: Vec<DebugProperty>,
        /// Another explicit page is available; no data was expanded implicitly.
        next: Option<u32>,
    },
    /// Non-state command completed without exposing Inspector protocol internals.
    Acknowledged,
    /// Safe host-controlled rejection; this never includes authored command text.
    Rejected {
        /// Fixed explanation of stale, unsupported or bounded work.
        message: String,
    },
    /// Stop or native lifetime expiry; this session cannot resume.
    Stopped {
        /// Fixed host-controlled reason.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    #[test]
    fn configuration_rejects_ambiguous_breakpoint_identities_and_invalid_coordinates() {
        let mut config = super::DebugOptions {
            skip_pauses: false,
            pause_on_start: false,
            persistent: false,
            session: "a".repeat(32),
            exceptions: super::ExceptionPolicy::None,
            breakpoints: vec![super::SourceBreakpoint {
                log_expression: None,
                condition: None,
                id: 1,
                location: crate::SourceLocation {
                    source: "file:///main.js".into(),
                    line: 1,
                    column: 1,
                },
            }],
        };
        assert!(config.is_bounded());
        config.breakpoints.push(config.breakpoints[0].clone());
        assert!(!config.is_bounded());
        config.breakpoints.pop();
        config.breakpoints[0].location.column = 0;
        assert!(!config.is_bounded());
        config.breakpoints.clear();
        config.session = "untrusted label".into();
        assert!(!config.is_bounded());
    }
}
