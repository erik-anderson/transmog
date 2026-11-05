//! Explicit local desktop debugging. None of these operations are registered with MCP.
use crate::{AppError, Application, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_script::{ScriptInvocation, ScriptRunner, ScriptScenario, SourceLocation};
use transmog_script_diagnostics::{
    debugger::{DebugAction, DebugEventKind, DebugOptions, ExceptionPolicy, SourceBreakpoint},
    supervision::{Session, SessionPage},
};

static OWNERS: Mutex<Vec<Weak<Run>>> = Mutex::new(Vec::new());
mod live;
pub use live::DebugLiveRequest;
#[cfg(test)]
mod tests;
/// Explicit local debugger configuration, independent of normal script capabilities.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugConfiguration {
    /// Temporarily suppress breakpoints, logpoints, exceptions and debugger statements.
    #[serde(default)]
    pub skip_pauses: bool,
    /// Stop a fresh fixture at its first executable user statement.
    #[serde(default)]
    pub pause_on_start: bool,
    /// Keep a workspace attachment alive until its surface closes.
    #[serde(default)]
    pub persistent: bool,
    /// Pause on no, uncaught, or all exceptions.
    pub exceptions: ExceptionPolicy,
    /// Source positions requested by this local author.
    pub breakpoints: Vec<SourceBreakpoint>,
}
/// One exact built candidate and one synthetic test mode.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugTestRequest {
    /// Exact immutable authoring build token.
    pub candidate_id: String,
    /// Single callback, mutually exclusive with scenario.
    pub invocation: Option<ScriptInvocation>,
    /// Complete production-pipeline fixture, mutually exclusive with invocation.
    pub scenario: Option<ScriptScenario>,
    /// Explicit native debugger settings.
    pub configuration: DebugConfiguration,
}
/// Immutable identity of a local debugger session.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugSessionInfo {
    /// Native unpredictable session identity.
    pub session: String,
    /// Sandbox, visualizer fixture, existing visualizer, or live traffic.
    pub mode: String,
    /// Human-readable exact contribution/revision label.
    pub label: String,
    /// Immutable executing code and map identity.
    pub artifact: String,
    /// Initial read-only source identity.
    pub source: String,
}
/// Finite test result retained separately from execution state and logs.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugOutcome {
    /// Completed, failed or stopped; Stop never manufactures success.
    pub status: String,
    /// Host-owned completion summary.
    pub summary: String,
    /// Bounded explicit local result, absent when too large or unavailable.
    pub result: Option<serde_json::Value>,
    /// Full result exceeded the local debug-result bound.
    pub truncated: bool,
    /// Exact structured exception evidence when available.
    pub exception: Option<Box<transmog_script::ExceptionDiagnostic>>,
    /// Independent bounded console streams.
    pub logs: Vec<transmog_script::LogRun>,
}
/// Cursor-qualified local evidence for one owning desktop window.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugSessionView {
    /// Immutable executing identity.
    pub info: DebugSessionInfo,
    /// Native ordered state and value replies.
    pub stream: SessionPage,
    /// Test completion, independent of the last paused frame.
    pub outcome: Option<DebugOutcome>,
}
#[derive(Clone, Default)]
pub(crate) struct Debuggers(Arc<Mutex<BTreeMap<String, Arc<Run>>>>);
pub(crate) struct Run {
    persistent: bool,
    owner: String,
    pub(crate) session: Arc<Session>,
    pub(crate) info: DebugSessionInfo,
    outcome: Mutex<Option<DebugOutcome>>,
    canceled: AtomicBool,
    expired: AtomicBool,
    map: Mutex<Option<Vec<u8>>>,
    package: Mutex<Option<transmog_extensions::Snapshot>>,
    pub(crate) visualizer_result:
        Mutex<Option<tokio::sync::oneshot::Receiver<Result<crate::VisualizerView, AppError>>>>,
}
impl Debuggers {
    pub(crate) fn create(
        &self,
        owner: &str,
        configuration: &DebugConfiguration,
        mut info: DebugSessionInfo,
        map: Option<Vec<u8>>,
    ) -> Result<Arc<Run>, AppError> {
        if owner.is_empty() || owner.len() > 128 {
            return Err(invalid("Debugger owner unavailable"));
        }
        if !transmog_script_diagnostics::debugger::valid_breakpoints(&configuration.breakpoints) {
            return Err(invalid("Invalid source breakpoints"));
        }
        let mut owners = OWNERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|run| run.strong_count() > 0);
        if owners
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|run| run.persistent == configuration.persistent && !run.session.is_closed())
            .count()
            >= if configuration.persistent { 64 } else { 4 }
        {
            return Err(invalid(
                "The debugger worker limit has been reached. Close a debugger before starting another.",
            ));
        }
        while owners
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|run| run.persistent == configuration.persistent)
            .count()
            >= if configuration.persistent { 80 } else { 16 }
        {
            let index = owners
                .iter()
                .position(|run| {
                    run.upgrade().is_none_or(|run| {
                        run.persistent == configuration.persistent && run.session.is_closed()
                    })
                })
                .ok_or_else(|| invalid("Debugger history is full"))?;
            if let Some(run) = owners.remove(index).upgrade() {
                run.expire();
            }
        }
        let map = map.filter(|bytes| bytes.len() <= 4 * 1024 * 1024);
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| invalid("Debugger identity unavailable"))?;
        info.session = format!("{:032x}", u128::from_le_bytes(random));
        let options = DebugOptions {
            skip_pauses: configuration.skip_pauses,
            pause_on_start: configuration.pause_on_start,
            persistent: configuration.persistent,
            session: info.session.clone(),
            exceptions: configuration.exceptions,
            breakpoints: translate(&configuration.breakpoints, map.as_deref()),
        };
        // Validate the original requests too; translation cannot hide a client flood.
        if !transmog_script_diagnostics::debugger::valid_breakpoints(&configuration.breakpoints) {
            return Err(invalid("Invalid source breakpoints"));
        }
        let session = Session::new(options).map_err(invalid)?;
        let run = Arc::new(Run {
            persistent: configuration.persistent,
            owner: owner.into(),
            session,
            info,
            outcome: Mutex::new(None),
            canceled: AtomicBool::new(false),
            expired: AtomicBool::new(false),
            map: Mutex::new(map),
            package: Mutex::new(None),
            visualizer_result: Mutex::new(None),
        });
        let closing = Arc::downgrade(&run);
        run.session.on_closed(move || {
            if let Some(run) = closing.upgrade() {
                run.clear_mappings();
                run.visualizer_result
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
        });
        let mut runs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // At most eight closed histories plus the four active owners, per application.
        while runs
            .values()
            .filter(|run| run.persistent == configuration.persistent)
            .count()
            >= if configuration.persistent { 76 } else { 12 }
        {
            let key = runs
                .iter()
                .find(|(_, run)| {
                    run.persistent == configuration.persistent && run.session.is_closed()
                })
                .map(|(key, _)| key.clone())
                .ok_or_else(|| invalid("Debugger history is full"))?;
            runs.remove(&key);
        }
        runs.insert(run.info.session.clone(), Arc::clone(&run));
        owners.push(Arc::downgrade(&run));
        Ok(run)
    }
    pub(crate) fn owned(&self, owner: &str, id: &str) -> Result<Arc<Run>, AppError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .filter(|run| run.owner == owner)
            .cloned()
            .ok_or_else(|| invalid("Debugger session is unavailable in this window"))
    }
    pub(crate) fn create_package(
        &self,
        owner: &str,
        configuration: &DebugConfiguration,
        info: DebugSessionInfo,
        snapshot: transmog_extensions::Snapshot,
    ) -> Result<Arc<Run>, AppError> {
        if !transmog_script_diagnostics::debugger::valid_breakpoints(&configuration.breakpoints) {
            return Err(invalid("Invalid source breakpoints"));
        }
        let configuration = DebugConfiguration {
            skip_pauses: configuration.skip_pauses,
            pause_on_start: configuration.pause_on_start,
            persistent: configuration.persistent,
            exceptions: configuration.exceptions,
            breakpoints: transmog_extension_supervisor::map_debug_breakpoints(
                &snapshot,
                &configuration.breakpoints,
            ),
        };
        let run = self.create(owner, &configuration, info, None)?;
        *run.package
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
        Ok(run)
    }
    pub(crate) fn release(&self, owner: &str) {
        let mut runs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runs.retain(|_, run| {
            if run.owner == owner {
                run.canceled.store(true, Ordering::Release);
                run.session.stop("Owning window closed");
                false
            } else {
                true
            }
        });
    }
}
impl Run {
    fn clear_mappings(&self) {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.package
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    fn expire(&self) {
        self.expired.store(true, Ordering::Release);
        self.clear_mappings();
        self.session.clear_evidence();
        *self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome(
            "expired",
            "Older debugger evidence expired from the shared history limit",
            None,
            None,
            vec![],
        ));
    }
    pub(crate) fn record(&self, mut outcome: DebugOutcome) {
        if self.expired.load(Ordering::Acquire) {
            return;
        }
        if self.canceled.load(Ordering::Acquire)
            || (self.session.is_closed()
                && !self.session.detached()
                && outcome.status == "completed")
        {
            outcome.status = "stopped".into();
            outcome.summary = "Debug run stopped before completion".into();
            outcome.result = None;
        }
        let mut slot = self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.expired.load(Ordering::Acquire) {
            *slot = Some(outcome);
        }
    }
    pub(crate) fn finish(&self, mut outcome: DebugOutcome) {
        if self.expired.load(Ordering::Acquire) {
            return;
        }
        if self.canceled.load(Ordering::Acquire)
            || (self.session.is_closed()
                && !self.session.detached()
                && outcome.status == "completed")
        {
            outcome.status = "stopped".into();
            outcome.summary = "Debug run stopped before completion".into();
            outcome.result = None;
        }
        {
            let mut slot = self
                .outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !self.expired.load(Ordering::Acquire) {
                *slot = Some(outcome);
            }
        }
        self.session.stop("Debug run completed; worker released");
        self.clear_mappings();
    }
}
impl Application {
    /// Starts an isolated test and returns its controller before module initialization.
    /// # Errors
    /// Rejects stale candidates, invalid ownership/configuration, or excessive sessions.
    #[allow(clippy::too_many_lines)] // Exact build, controller publication and background worker cleanup share one transaction.
    pub fn debug_script_test(
        &self,
        owner: &str,
        request: DebugTestRequest,
    ) -> Result<DebugSessionInfo, AppError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| invalid("Debugger executor is unavailable"))?;
        if request.invocation.is_some() == request.scenario.is_some() {
            return Err(invalid("Choose one callback or scenario fixture"));
        }
        let (revision, config) = self.scripts.debug_candidate(&request.candidate_id)?;
        let compiled = transmog_script::CompiledScript {
            manifest: revision.manifest.clone(),
            javascript: revision.javascript.clone(),
            source_map: revision.source_map.clone(),
        };
        let artifact = format!(
            "{}:{}:{}:{}",
            revision.manifest.id,
            revision.manifest.revision,
            revision.javascript_hash,
            revision.source_map_hash
        );
        let source = if matches!(revision.language, crate::ScriptLanguage::TypeScript) {
            format!("file:///transmog-scripts/{}/main.ts", revision.manifest.id)
        } else {
            "file:///transmog-script/main.js".into()
        };
        let run = self.debuggers.create(
            owner,
            &request.configuration,
            DebugSessionInfo {
                session: String::new(),
                mode: if request.scenario.is_some() {
                    "Sandbox scenario"
                } else {
                    "Sandbox callback"
                }
                .into(),
                label: format!(
                    "{} · revision {}",
                    revision.manifest.id, revision.manifest.revision
                ),
                artifact,
                source,
            },
            Some(revision.source_map),
        )?;
        let info = run.info.clone();
        runtime.spawn(async move {
            let session = Arc::clone(&run.session);
            let manifest = compiled.manifest.clone();
            let started = tokio::task::spawn_blocking(move || {
                transmog_script_supervisor::SupervisedScriptRunner::start_debug(
                    &config,
                    compiled,
                    Some(session),
                )
            })
            .await;
            // Keep the worker alive until the result is committed and its session is closed.
            let mut held_runner = None;
            let outcome = match started {
                Ok(Ok(runner)) => {
                    held_runner = Some(Arc::clone(&runner));
                    let result = if let Some(mut invocation) = request.invocation {
                        invocation.context.source = transmog_script::ScriptSource::Sandbox;
                        runner
                            .invoke(invocation)
                            .await
                            .map(|result| ("Callback completed", limited_json(&result)))
                    } else if let Some(scenario) = request.scenario {
                        transmog_script::test_scenario(manifest, runner.clone(), scenario)
                            .await
                            .map(|result| {
                                (
                                    match result.verdict {
                                        transmog_script::ScriptScenarioVerdict::Passed => {
                                            "Scenario passed"
                                        }
                                        transmog_script::ScriptScenarioVerdict::Failed => {
                                            "Scenario failed"
                                        }
                                        transmog_script::ScriptScenarioVerdict::Incomplete => {
                                            "Scenario incomplete"
                                        }
                                    },
                                    limited_json(&result),
                                )
                            })
                    } else {
                        run.finish(outcome(
                            "failed",
                            "Choose a callback or scenario fixture",
                            None,
                            None,
                            vec![],
                        ));
                        return;
                    };
                    let logs = runner.logs();
                    match result {
                        Ok((summary, (result, truncated))) => {
                            let mut value = outcome(
                                if summary == "Scenario failed" {
                                    "failed"
                                } else {
                                    "completed"
                                },
                                summary,
                                result,
                                None,
                                logs,
                            );
                            value.truncated = truncated;
                            value
                        }
                        Err(failure) => {
                            outcome("failed", "Debug test failed", None, failure.exception, logs)
                        }
                    }
                }
                Ok(Err(failure)) => outcome(
                    "failed",
                    "Module initialization failed",
                    None,
                    failure.exception,
                    failure.logs,
                ),
                Err(_) => outcome("failed", "Debug worker could not start", None, None, vec![]),
            };
            run.finish(outcome);
            drop(held_runner);
        });
        Ok(info)
    }
    /// Reads ephemeral state only for its owning local desktop window.
    /// # Errors
    /// Rejects expired sessions and cross-window ownership.
    pub fn debug_session(
        &self,
        owner: &str,
        id: &str,
        after: u64,
    ) -> Result<DebugSessionView, AppError> {
        let run = self.debuggers.owned(owner, id)?;
        let mut stream = run.session.page(after);
        for event in stream.events.iter_mut().chain(stream.paused.iter_mut()) {
            if let DebugEventKind::Paused {
                exception: Some(exception),
                ..
            } = &mut event.event
            {
                exception.artifact = Some(run.info.artifact.clone());
            }
        }
        if let Some(map) = run
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
        {
            map_events(&mut stream, map);
        }
        if let Some(snapshot) = run
            .package
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            for event in stream.events.iter_mut().chain(stream.paused.iter_mut()) {
                transmog_extension_supervisor::map_debug_event(snapshot, event);
            }
        }
        let outcome = run
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if stream.closed {
            run.clear_mappings();
        }
        Ok(DebugSessionView {
            info: run.info.clone(),
            stream,
            outcome,
        })
    }
    /// Submits one typed local action with native-generated request correlation.
    /// # Errors
    /// Rejects cross-window control, stale handles, ended sessions and invalid work.
    pub fn debug_command(
        &self,
        owner: &str,
        id: &str,
        mut action: DebugAction,
    ) -> Result<u64, AppError> {
        let run = self.debuggers.owned(owner, id)?;
        if let DebugAction::Breakpoints { values } = &mut action {
            if !transmog_script_diagnostics::debugger::valid_breakpoints(values) {
                return Err(invalid("Invalid source breakpoints"));
            }
            *values = translate_for_run(&run, values);
        } else if let DebugAction::ContinueTo { location, .. } = &mut action {
            let point = SourceBreakpoint {
                id: 0,
                condition: None,
                log_expression: None,
                location: location.clone(),
            };
            if !transmog_script_diagnostics::debugger::valid_breakpoints(std::slice::from_ref(
                &point,
            )) {
                return Err(invalid("Invalid source position"));
            }
            if let Some(mapped) = translate_for_run(&run, &[point]).first() {
                *location = mapped.location.clone();
            }
        }
        if matches!(action, DebugAction::Stop) {
            run.canceled.store(true, Ordering::Release);
        }
        run.session.command(action).map_err(invalid)
    }
    /// Releases a session and its ephemeral local evidence.
    /// # Errors
    /// Rejects cross-window ownership.
    pub fn close_debug_session(&self, owner: &str, id: &str) -> Result<(), AppError> {
        let run = self.debuggers.owned(owner, id)?;
        run.canceled.store(true, Ordering::Release);
        run.session.stop("Debugger closed by its owner");
        self.debuggers
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        Ok(())
    }
    /// Stops every session owned by a destroyed desktop window.
    pub fn release_debuggers(&self, owner: &str) {
        self.debuggers.release(owner);
    }
}
pub(crate) fn outcome(
    status: &str,
    summary: &str,
    result: Option<serde_json::Value>,
    exception: Option<Box<transmog_script::ExceptionDiagnostic>>,
    logs: Vec<transmog_script::LogRun>,
) -> DebugOutcome {
    DebugOutcome {
        status: status.into(),
        summary: summary.into(),
        result,
        truncated: false,
        exception,
        logs,
    }
}
fn limited_json(value: &impl Serialize) -> (Option<serde_json::Value>, bool) {
    struct Buffer(Vec<u8>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > 64 * 1024 {
                return Err(std::io::Error::other("Debug result limit reached"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer(Vec::new());
    if serde_json::to_writer(&mut buffer, value).is_err() {
        return (None, true);
    }
    match serde_json::from_slice(&buffer.0) {
        Ok(value) => (Some(value), false),
        Err(_) => (None, true),
    }
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, false)
}
fn decode(bytes: &[u8]) -> Option<sourcemap::SourceMap> {
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let sourcemap::DecodedMap::Regular(map) = sourcemap::decode_slice(bytes).ok()? else {
        return None;
    };
    Some(map)
}
fn translate_for_run(run: &Run, values: &[SourceBreakpoint]) -> Vec<SourceBreakpoint> {
    if let Some(snapshot) = run
        .package
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        transmog_extension_supervisor::map_debug_breakpoints(snapshot, values)
    } else {
        translate(
            values,
            run.map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
        )
    }
}
fn translate(values: &[SourceBreakpoint], bytes: Option<&[u8]>) -> Vec<SourceBreakpoint> {
    let map = bytes.and_then(decode);
    values
        .iter()
        .map(|value| {
            let mut result = value.clone();
            if let Some(map) = &map
                && map.get_token_count() <= 100_000
                && let Some(token) = map
                    .tokens()
                    .filter(|token| {
                        token.get_source() == Some(value.location.source.as_str())
                            && token.get_src_line().checked_add(1) == Some(value.location.line)
                    })
                    .min_by_key(|token| {
                        (
                            token
                                .get_src_col()
                                .abs_diff(value.location.column.saturating_sub(1)),
                            token.get_dst_line(),
                            token.get_dst_col(),
                        )
                    })
            {
                result.location = SourceLocation {
                    source: "file:///transmog-script/main.js".into(),
                    line: token.get_dst_line() + 1,
                    column: token.get_dst_col() + 1,
                };
            }
            result
        })
        .collect()
}
fn map_events(page: &mut SessionPage, bytes: &[u8]) {
    if !page.events.iter().chain(page.paused.iter()).any(|event| {
        matches!(
            event.event,
            DebugEventKind::Paused { .. } | DebugEventKind::Logpoint { .. }
        )
    }) {
        return;
    }
    let Some(map) = decode(bytes) else {
        return;
    };
    for event in page.events.iter_mut().chain(page.paused.iter_mut()) {
        match &mut event.event {
            DebugEventKind::Paused { frames, .. } => {
                for frame in frames {
                    frame.original = frame
                        .location
                        .as_ref()
                        .and_then(|location| original_location(&map, location));
                }
            }
            DebugEventKind::Logpoint {
                location, original, ..
            } => {
                *original = location
                    .as_ref()
                    .and_then(|location| original_location(&map, location));
            }
            _ => {}
        }
    }
}
fn original_location(
    map: &sourcemap::SourceMap,
    location: &SourceLocation,
) -> Option<SourceLocation> {
    if location.source != "file:///transmog-script/main.js"
        || location.line == 0
        || location.column == 0
    {
        return None;
    }
    let token = same_line_token(map, location.line - 1, location.column - 1)?;
    Some(SourceLocation {
        source: token
            .get_source()
            .filter(|source| source.len() <= 2048)?
            .into(),
        line: token.get_src_line().checked_add(1)?,
        column: token
            .get_src_col()
            .checked_add(if token.is_range() {
                location.column - 1 - token.get_dst_col()
            } else {
                0
            })?
            .checked_add(1)?,
    })
}
fn same_line_token(
    map: &sourcemap::SourceMap,
    line: u32,
    column: u32,
) -> Option<sourcemap::Token<'_>> {
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
