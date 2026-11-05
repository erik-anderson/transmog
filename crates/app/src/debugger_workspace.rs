//! Native ownership and source inventory for the dockable debugger.
mod projections;
use crate::{AppError, Application, DebugConfiguration, DebugSessionInfo, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use transmog_script_diagnostics::debugger::{ExceptionPolicy, SourceBreakpoint};

pub(crate) const OWNER: &str = "debugger-workspace";
#[derive(Clone, Default)]
pub(crate) struct Workspace(Arc<Mutex<State>>);
#[derive(Default)]
struct State {
    policies: BTreeMap<String, ExceptionPolicy>,
    maps: Vec<projections::Map>,
    focus: Option<String>,
    preferences: Option<Workspace>,
    bindings: BTreeMap<String, u64>,
    open: bool,
    targets: Vec<DebugSessionInfo>,
    breakpoints: Vec<WorkspaceBreakpoint>,
    path: Option<PathBuf>,
    generation: u64,
    catalog_key: String,
    catalog: Vec<(DebuggerFile, String)>,
    catalog_checked: Option<std::time::Instant>,
    epoch: u64,
}
/// Persisted logical breakpoint, independent of any running worker.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBreakpoint {
    /// Exact displayed artifact for canonicalizing an edit; rebinding uses logical intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// Logical verified file identity.
    pub file: String,
    /// Engine or original-source coordinates.
    pub breakpoint: SourceBreakpoint,
    /// Disabled points retain their intent without creating engine bindings.
    pub enabled: bool,
    /// Exact statement used to rebind after a source update.
    pub anchor: String,
}
/// One verified source. Reading it does not activate its contribution.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebuggerFile {
    /// Stable script/package and module path.
    pub id: String,
    /// User scripts or Extensions.
    pub group: String,
    /// Script or extension display name.
    pub package: String,
    /// Relative path within this contribution.
    pub path: String,
    /// Exact immutable executable identity.
    pub artifact: String,
    /// Source coordinate namespace used by the engine and source map.
    pub source: String,
    /// Source language for Monaco.
    pub language: String,
    /// Other representation of this module, when a verified source map connects them.
    pub counterpart: Option<String>,
    /// Loaded, unloaded, or saved draft.
    pub state: String,
}
/// Current catalog and attached worker instances of one authorized application.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebuggerWorkspaceView {
    /// Most recent explicit fixture/attachment selection.
    pub focus: Option<String>,
    /// Current product palette, shared by docked and detached surfaces.
    pub theme: crate::ThemePreference,
    /// Active workspace lifetime.
    pub open: bool,
    /// Verified sources, including contributions which have never loaded.
    pub files: Vec<DebuggerFile>,
    /// Actual worker attachments. Several instances can share one artifact.
    pub targets: Vec<DebugSessionInfo>,
    /// Stored breakpoint intent.
    pub breakpoints: Vec<WorkspaceBreakpoint>,
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, false)
}
fn script_info(compiled: &transmog_script::CompiledScript) -> DebugSessionInfo {
    DebugSessionInfo {
        session: String::new(),
        mode: "Workspace script".into(),
        label: format!(
            "{} · revision {}",
            compiled.manifest.id, compiled.manifest.revision
        ),
        artifact: format!(
            "{}:{}:{}:{}",
            compiled.manifest.id,
            compiled.manifest.revision,
            transmog_script::source_hash(compiled.javascript.as_bytes()),
            transmog_script::source_hash(&compiled.source_map)
        ),
        source: "file:///transmog-script/main.js".into(),
    }
}
impl Workspace {
    fn preference_owner(&self) -> Self {
        let parent = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .preferences
            .clone();
        parent.map_or_else(|| self.clone(), |parent| parent.preference_owner())
    }
    fn sync_preferences(&self) -> Result<(), AppError> {
        let owner = self.preference_owner();
        if Arc::ptr_eq(&owner.0, &self.0) {
            return Ok(());
        }
        let (generation, points) = {
            let state = owner
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            (
                state.generation,
                state
                    .breakpoints
                    .iter()
                    .filter(|point| point.file.starts_with("extension/"))
                    .cloned()
                    .collect(),
            )
        };
        let mut state = self.0.lock().map_err(|_| invalid("Debugger unavailable"))?;
        state.generation = generation;
        state.breakpoints = points;
        Ok(())
    }
    pub(crate) fn load(path: Option<&Path>) -> Self {
        let mut state = State {
            path: path.map(|p| p.with_extension("debugger.json")),
            ..State::default()
        };
        if let Some(path) = &state.path {
            for slot in 0..2 {
                let candidate = read_preferences(&path.with_extension(format!("{slot}.json")));
                if let Some(stored) = candidate.filter(|stored| {
                    stored.generation >= state.generation && valid_points(&stored.points)
                }) {
                    state.generation = stored.generation;
                    state.breakpoints = stored.points;
                }
            }
        }
        Self(Arc::new(Mutex::new(state)))
    }
    pub(crate) fn is_open(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open
    }
    pub(crate) fn configuration(&self, prefix: &str) -> DebugConfiguration {
        let _ = self.sync_preferences();
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        DebugConfiguration {
            pause_on_start: false,
            persistent: true,
            exceptions: state.policies.get(prefix).copied().unwrap_or_default(),
            breakpoints: state
                .breakpoints
                .iter()
                .filter(|point| point.enabled && point.file.starts_with(prefix))
                .map(|point| point.breakpoint.clone())
                .collect(),
        }
    }
    fn script_configuration(
        &self,
        compiled: &transmog_script::CompiledScript,
    ) -> DebugConfiguration {
        let mut configuration = self.configuration(&format!("script/{}/", compiled.manifest.id));
        let mut sources = BTreeMap::from([(
            "file:///transmog-script/main.js".to_owned(),
            compiled.javascript.clone(),
        )]);
        if let Ok(sourcemap::DecodedMap::Regular(map)) =
            sourcemap::decode_slice(&compiled.source_map)
            && let Some((source, text)) = map.get_source(0).zip(map.get_source_contents(0))
        {
            sources.insert(source.into(), text.into());
        }
        self.rebind_configuration(&mut configuration, &sources);
        configuration
    }
    fn rebind_configuration(
        &self,
        configuration: &mut DebugConfiguration,
        sources: &BTreeMap<String, String>,
    ) {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        configuration.breakpoints = configuration
            .breakpoints
            .iter()
            .filter_map(|breakpoint| {
                let Some(point) = state
                    .breakpoints
                    .iter()
                    .find(|point| point.breakpoint.id == breakpoint.id)
                else {
                    return Some(breakpoint.clone());
                };
                sources
                    .get(&breakpoint.location.source)
                    .map_or_else(|| Some(breakpoint.clone()), |text| rebind(point, text))
            })
            .collect();
    }
    pub(crate) fn extension(
        &self,
        app: &Application,
        snapshot: transmog_extensions::Snapshot,
        entry: &str,
    ) -> Result<Option<Arc<crate::debugging::Run>>, AppError> {
        let epoch = {
            let state = self.0.lock().map_err(|_| invalid("Debugger unavailable"))?;
            if !state.open {
                return Ok(None);
            }
            state.epoch
        };
        let info = DebugSessionInfo {
            session: String::new(),
            mode: "Workspace visualizer".into(),
            label: snapshot.manifest.id.clone(),
            artifact: snapshot.digest.clone(),
            source: format!("file:///transmog-extension/{entry}"),
        };
        let mut config = self.configuration(&format!("extension/{}/", snapshot.manifest.id));
        let mut catalog = Sources::new();
        extension_files(&snapshot, &mut catalog);
        extension_maps(&snapshot, &mut catalog);
        self.rebind_configuration(
            &mut config,
            &catalog
                .values()
                .map(|(file, text)| (file.source.clone(), text.clone()))
                .collect(),
        );
        let run = app
            .debuggers
            .create_package(OWNER, &config, info, snapshot)?;
        let mut state = self.0.lock().map_err(|_| invalid("Debugger unavailable"))?;
        if !state.open || state.epoch != epoch {
            run.session.stop("Debugger closed before attachment");
            return Ok(None);
        }
        state.targets.push(run.info.clone());
        let generation = state.generation;
        state.bindings.insert(run.info.session.clone(), generation);
        for (key, (file, text)) in catalog {
            if !state
                .catalog
                .iter()
                .any(|(previous, _)| previous.id == key.0 && previous.artifact == key.1)
            {
                state.catalog.push((file, text));
            }
        }
        state.catalog_key.clear();
        Ok(Some(run))
    }
}
impl Application {
    /// Starts a fixture under the open workspace before any user code executes.
    /// # Errors
    /// Rejects a closed workspace, stale builds, invalid inputs or unavailable workers.
    pub fn debug_workspace_test(
        &self,
        mut request: crate::DebugTestRequest,
    ) -> Result<DebugSessionInfo, AppError> {
        if !self.debugger_workspace.is_open() {
            return Err(invalid("Open the debugger before starting a debug fixture"));
        }
        let (revision, _) = self.scripts.debug_candidate(&request.candidate_id)?;
        let compiled = transmog_script::CompiledScript {
            manifest: revision.manifest.clone(),
            javascript: revision.javascript.clone(),
            source_map: revision.source_map.clone(),
        };
        let configuration = self.debugger_workspace.script_configuration(&compiled);
        {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            let mut sources = Sources::new();
            script_sources(std::slice::from_ref(&revision), &[], &mut sources);
            state.catalog.extend(sources.into_values());
            if let Some(map) = projections::Map::read(
                &script_info(&compiled).artifact,
                "file:///transmog-script/main.js",
                "file:///transmog-script/main.js",
                &compiled.source_map,
            ) {
                state.maps.push(map);
            }
        }
        request.configuration.breakpoints = configuration.breakpoints;
        request.configuration.persistent = true;
        request.configuration.pause_on_start = true;
        let info = self.debug_script_test(OWNER, request)?;
        self.register_workspace_target(info.clone())?;
        Ok(info)
    }
    /// Publishes an explicitly started fixture without creating another debugger owner.
    /// # Errors
    /// Rejects stale native ownership or unavailable workspace state.
    pub fn register_workspace_target(&self, info: DebugSessionInfo) -> Result<(), AppError> {
        let run = self.debuggers.owned(OWNER, &info.session)?;
        let mut state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        if !state.open {
            drop(state);
            run.session.request_detach().map_err(invalid)?;
            return Ok(());
        }
        if !state
            .targets
            .iter()
            .any(|target| target.session == info.session)
        {
            state.targets.push(info.clone());
        }
        let generation = state.generation;
        state.bindings.insert(info.session.clone(), generation);
        state.focus = Some(info.session);
        Ok(())
    }
    /// Restarts the currently applied script in a fresh worker.
    /// # Errors
    /// Rejects non-script targets, stale activation or unavailable sandbox startup.
    pub fn restart_workspace_script(&self, id: &str) -> Result<(), AppError> {
        let run = self.debuggers.owned(OWNER, id)?;
        if run.info.mode != "Workspace script" {
            return Err(invalid("This target is not an applied script"));
        }
        let script = run
            .info
            .artifact
            .split(':')
            .next()
            .ok_or_else(|| invalid("Script identity is unavailable"))?;
        run.session.stop("Restarted by its owner");
        for (revision, runner) in self.scripts.debug_runners() {
            if revision.manifest.id == script {
                runner.end_debugger();
            }
        }
        self.retry_script(script, self.scripts.debug_generation())?;
        let mut state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        state.focus = state
            .targets
            .iter()
            .rev()
            .find(|target| {
                target.mode == "Workspace script"
                    && target.artifact.split(':').next() == Some(script)
            })
            .map(|target| target.session.clone());
        Ok(())
    }
    /// Shares durable extension breakpoint preferences while retaining independent worker ownership.
    /// # Errors
    /// Rejects changing the preference owner while this workspace is open.
    pub fn inherit_debugger_preferences(&self, source: &Self) -> Result<(), AppError> {
        let owner = source.debugger_workspace.preference_owner();
        if Arc::ptr_eq(&owner.0, &self.debugger_workspace.0) {
            return Ok(());
        }
        let mut state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        if state.open {
            return Err(invalid(
                "Close the debugger before changing its preference owner",
            ));
        }
        state.preferences = Some(owner);
        state.path = None;
        drop(state);
        self.debugger_workspace.sync_preferences()
    }
    /// Reads an attached target through this application's debugger owner.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn workspace_debug_session(
        &self,
        id: &str,
        after: u64,
    ) -> Result<crate::DebugSessionView, AppError> {
        self.debug_session(OWNER, id, after)
    }
    /// Runs one typed command against an attached target.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn workspace_debug_command(
        &self,
        id: &str,
        action: transmog_script_diagnostics::debugger::DebugAction,
    ) -> Result<u64, AppError> {
        let policy =
            if let transmog_script_diagnostics::debugger::DebugAction::Exceptions { policy } =
                &action
            {
                Some(*policy)
            } else {
                None
            };
        let run = self.debuggers.owned(OWNER, id)?;
        let request = self.debug_command(OWNER, id, action)?;
        if let Some(policy) = policy {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            let prefix = target_prefix(&run.info, &state.catalog);
            state.policies.insert(prefix, policy);
            while state.policies.len() > 128 {
                state.policies.pop_first();
            }
        }
        Ok(request)
    }
    /// Opens the application's debugger and subscribes to future traffic workers.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn open_debugger_workspace(&self) -> Result<DebuggerWorkspaceView, AppError> {
        self.debugger_catalog()?;
        let epoch = {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            if state.open {
                drop(state);
                return self.debugger_workspace();
            }
            state.open = true;
            state.epoch = state.epoch.wrapping_add(1);
            state.epoch
        };
        let state = self.debugger_workspace.clone();
        let debuggers = self.debuggers.clone();
        let factory: crate::scripts::DebugFactory = Arc::new(move |compiled| {
            if !state
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?
                .open
            {
                return Ok(None);
            }
            let info = script_info(compiled);
            let configuration = state.script_configuration(compiled);
            let run = debuggers.create(
                OWNER,
                &configuration,
                info,
                Some(compiled.source_map.clone()),
            )?;
            let mut workspace = state
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            if !workspace.open || workspace.epoch != epoch {
                run.session.stop("Debugger closed before attachment");
                return Ok(None);
            }
            workspace.targets.push(run.info.clone());
            let generation = workspace.generation;
            workspace
                .bindings
                .insert(run.info.session.clone(), generation);
            Ok(Some(Arc::clone(&run.session)))
        });
        self.scripts.debug_factory(Some(Arc::clone(&factory)));
        for (revision, runner) in self.scripts.debug_runners() {
            if runner.debugger_attached() {
                continue;
            }
            let compiled = transmog_script::CompiledScript {
                manifest: revision.manifest,
                javascript: revision.javascript,
                source_map: revision.source_map,
            };
            if let Some(session) = factory(&compiled)?
                && let Err(error) = runner.attach_debugger(Arc::clone(&session))
            {
                session.stop("Debugger could not attach");
                let _ = self.close_debugger_workspace();
                return Err(invalid(&error.message));
            }
        }
        self.debugger_workspace()
    }
    /// Retires the workspace and resumes healthy workers before detaching.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn close_debugger_workspace(&self) -> Result<(), AppError> {
        self.scripts.debug_factory(None);
        let targets = {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            state.open = false;
            state.focus = None;
            state.bindings.clear();
            state.epoch = state.epoch.wrapping_add(1);
            std::mem::take(&mut state.targets)
        };
        for target in targets {
            if let Ok(run) = self.debuggers.owned(OWNER, &target.session)
                && run.session.request_detach().is_err()
                && !run.session.is_closed()
            {
                run.session.stop("Debugger could not detach safely");
            }
        }
        Ok(())
    }
    /// Snapshot of sources and worker targets, without activating inactive code.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn debugger_workspace(&self) -> Result<DebuggerWorkspaceView, AppError> {
        self.debugger_workspace.sync_preferences()?;
        self.attach_workspace_visualizers()?;
        let files = self.debugger_catalog()?;
        self.apply_workspace_breakpoints()?;
        let mut state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        let recent: std::collections::BTreeSet<_> = state
            .targets
            .iter()
            .rev()
            .filter(|target| {
                self.debuggers
                    .owned(OWNER, &target.session)
                    .is_ok_and(|run| run.session.is_closed())
            })
            .take(8)
            .map(|target| target.session.clone())
            .collect();
        state.targets.retain(|target| {
            self.debuggers
                .owned(OWNER, &target.session)
                .is_ok_and(|run| !run.session.is_closed() || recent.contains(&target.session))
        });
        let loaded: std::collections::BTreeSet<_> = state
            .targets
            .iter()
            .filter(|target| {
                self.debuggers
                    .owned(OWNER, &target.session)
                    .is_ok_and(|run| !run.session.is_closed())
            })
            .map(|target| target.artifact.clone())
            .collect();
        let retained: std::collections::BTreeSet<_> = state
            .targets
            .iter()
            .map(|target| target.session.clone())
            .collect();
        state
            .bindings
            .retain(|session, _| retained.contains(session));
        Ok(DebuggerWorkspaceView {
            focus: state.focus.clone(),
            theme: self.product_state().preferences.theme,
            open: state.open,
            files: files
                .into_iter()
                .map(|mut file| {
                    if loaded.contains(&file.artifact) {
                        file.state = "Loaded".into();
                    }
                    file
                })
                .collect(),
            targets: state.targets.clone(),
            breakpoints: state.breakpoints.clone(),
        })
    }
    /// Replaces this workspace's breakpoint intent and updates its live workers.
    /// # Errors
    /// Rejects excessive points, unavailable storage or native control failures.
    pub fn set_workspace_breakpoints(
        &self,
        points: &[WorkspaceBreakpoint],
    ) -> Result<(), AppError> {
        if !valid_points(points) {
            return Err(invalid("Invalid debugger breakpoints"));
        }
        self.debugger_workspace.sync_preferences()?;
        {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            persist(&mut state, points)?;
        }
        self.apply_workspace_breakpoints()
    }
    /// Merges explicit edits so another window's unrelated breakpoints are preserved.
    /// # Errors
    /// Rejects invalid scope, excessive preferences or failed native persistence/control.
    pub fn edit_workspace_breakpoints(
        &self,
        mut points: Vec<WorkspaceBreakpoint>,
        removed: &[WorkspaceBreakpoint],
    ) -> Result<Vec<WorkspaceBreakpoint>, AppError> {
        if !valid_points(&points) || !valid_points(removed) {
            return Err(invalid("Invalid debugger breakpoints"));
        }
        self.refresh_debugger_sources()?;
        {
            let state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            for point in &mut points {
                projections::canonicalize(point, &state.catalog, &state.maps);
            }
        }
        let viewer = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?
            .preferences
            .is_some();
        if viewer
            && points
                .iter()
                .chain(removed)
                .any(|point| !point.file.starts_with("extension/"))
        {
            return Err(invalid(
                "This capture window can edit only extension breakpoints",
            ));
        }
        let owner = self.debugger_workspace.preference_owner();
        {
            let mut state = owner
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            let merged = merge_points(&state.breakpoints, &points, removed)?;
            persist(&mut state, &merged)?;
        }
        self.debugger_workspace.sync_preferences()?;
        self.apply_workspace_breakpoints()?;
        Ok(self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?
            .breakpoints
            .clone())
    }
    fn apply_workspace_breakpoints(&self) -> Result<(), AppError> {
        self.debugger_workspace.sync_preferences()?;
        let (targets, generation) = {
            let state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            (state.targets.clone(), state.generation)
        };
        for target in targets {
            if self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?
                .bindings
                .get(&target.session)
                == Some(&generation)
            {
                continue;
            }
            if !self
                .debuggers
                .owned(OWNER, &target.session)
                .is_ok_and(|run| run.session.ready_for_commands())
            {
                continue;
            }
            let prefix = {
                let state = self
                    .debugger_workspace
                    .0
                    .lock()
                    .map_err(|_| invalid("Debugger unavailable"))?;
                target_prefix(&target, &state.catalog)
            };
            let mut config = self.debugger_workspace.configuration(&prefix);
            let sources = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?
                .catalog
                .iter()
                .filter(|(file, _)| file.artifact == target.artifact)
                .map(|(file, text)| (file.source.clone(), text.clone()))
                .collect();
            self.debugger_workspace
                .rebind_configuration(&mut config, &sources);
            self.debug_command(
                OWNER,
                &target.session,
                transmog_script_diagnostics::debugger::DebugAction::Breakpoints {
                    values: config.breakpoints,
                },
            )?;
            self.debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?
                .bindings
                .insert(target.session, generation);
        }
        Ok(())
    }
    /// Returns complete verified source text for an explicit file selection.
    /// # Errors
    /// Rejects unavailable ownership, invalid input or a failed native operation.
    pub fn debugger_file(&self, id: &str, artifact: &str) -> Result<String, AppError> {
        self.refresh_debugger_sources()?;
        self.debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?
            .catalog
            .iter()
            .find(|(file, _)| file.id == id && file.artifact == artifact)
            .map(|(_, text)| text.clone())
            .ok_or_else(|| invalid("Source is no longer available"))
    }
    /// Projects saved intent into either representation of an exact verified source.
    /// # Errors
    /// Rejects unavailable file identities or native catalog failures.
    pub fn debugger_breakpoint_positions(
        &self,
        id: &str,
        artifact: &str,
    ) -> Result<Vec<(u32, transmog_script_diagnostics::SourceLocation)>, AppError> {
        self.refresh_debugger_sources()?;
        let state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        let file = state
            .catalog
            .iter()
            .find(|(file, _)| file.id == id && file.artifact == artifact)
            .map(|(file, _)| file)
            .ok_or_else(|| invalid("Source is no longer available"))?;
        Ok(state
            .breakpoints
            .iter()
            .filter_map(|point| {
                projections::project(point, file, &state.catalog, &state.maps)
                    .map(|location| (point.breakpoint.id, location))
            })
            .collect())
    }
    fn debugger_catalog(&self) -> Result<Vec<DebuggerFile>, AppError> {
        self.refresh_debugger_sources()?;
        Ok(self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?
            .catalog
            .iter()
            .map(|(file, _)| file.clone())
            .collect())
    }
    fn extension_sources(
        &self,
        entries: &[transmog_extension_registry::ExtensionEntry],
        output: &mut Sources,
        maps: &mut Vec<projections::Map>,
    ) -> std::collections::BTreeSet<String> {
        let mut verified = std::collections::BTreeSet::new();
        for entry in entries {
            let Ok(snapshot) = self
                .visualizers
                .registry
                .debugger_snapshot(&entry.catalog.manifest.id)
            else {
                continue;
            };
            verified.insert(snapshot.manifest.id.clone());
            extension_files(&snapshot, output);
            extension_maps(&snapshot, output);
            collect_extension_maps(&snapshot, maps);
        }
        verified
    }
    fn refresh_debugger_sources(&self) -> Result<(), AppError> {
        let registry = self.visualizers.registry.status();
        let authoritative = !registry.management_blocked;
        let entries = registry.entries;
        let artifacts = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?
            .targets
            .iter()
            .map(|target| {
                (
                    target.artifact.clone(),
                    self.debuggers
                        .owned(OWNER, &target.session)
                        .is_ok_and(|run| !run.session.is_closed()),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        let script_generation = self.scripts.debug_generation();
        let key = format!(
            "{}:{}:{artifacts:?}:{authoritative}",
            script_generation,
            serde_json::to_string(&entries).unwrap_or_default()
        );
        {
            let state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            if state.catalog_key == key
                || state.catalog_key.is_empty()
                    && state.catalog_checked.is_some_and(|checked| {
                        checked.elapsed() < std::time::Duration::from_secs(2)
                    })
            {
                return Ok(());
            }
        }
        let (revisions, drafts) = self.scripts.debug_sources();
        let mut output = BTreeMap::new();
        script_sources(&revisions, &drafts, &mut output);
        let mut maps = script_maps(&revisions);
        let verified_extensions = self.extension_sources(&entries, &mut output, &mut maps);
        let mut state = self
            .debugger_workspace
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        retain_sources(&state, &mut output, &artifacts);
        // A source save can finish while package bytes are being read. Do not
        // prune intent added against that newer revision using this older snapshot.
        if self.scripts.debug_generation() != script_generation {
            return Ok(());
        }
        let points = reconcile_points(
            &state.breakpoints,
            &revisions,
            &drafts,
            &entries,
            &verified_extensions,
            &output,
            authoritative,
        );
        if points != state.breakpoints {
            persist(&mut state, &points)?;
        }
        state.catalog_key = if verified_extensions.len() == entries.len() {
            key
        } else {
            String::new()
        };
        state.catalog_checked = Some(std::time::Instant::now());
        for map in &state.maps {
            if state
                .targets
                .iter()
                .any(|target| target.artifact == map.artifact())
            {
                maps.push(map.clone());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        maps.retain(|map| seen.insert(map.key()));
        state.maps = maps;
        state.catalog = output.into_values().collect();
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Stored {
    generation: u64,
    points: Vec<WorkspaceBreakpoint>,
}
fn read_preferences(path: &Path) -> Option<Stored> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 1024 * 1024)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}
fn valid_points(points: &[WorkspaceBreakpoint]) -> bool {
    points.len() <= 128
        && points
            .iter()
            .all(|point| point.file.len() <= 2048 && point.anchor.len() <= 2048)
        && transmog_script_diagnostics::debugger::valid_breakpoints(
            &points
                .iter()
                .map(|point| point.breakpoint.clone())
                .collect::<Vec<_>>(),
        )
}
fn rebind(point: &WorkspaceBreakpoint, text: &str) -> Option<SourceBreakpoint> {
    let line = usize::try_from(point.breakpoint.location.line)
        .ok()?
        .checked_sub(1)?;
    if text.lines().nth(line) == Some(point.anchor.as_str()) {
        return Some(point.breakpoint.clone());
    }
    let mut matches = text
        .lines()
        .enumerate()
        .filter(|(_, line)| *line == point.anchor);
    let (line, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut breakpoint = point.breakpoint.clone();
    breakpoint.location.line = u32::try_from(line + 1).ok()?;
    Some(breakpoint)
}
fn persist(state: &mut State, points: &[WorkspaceBreakpoint]) -> Result<(), AppError> {
    use std::io::Write;
    if let Some(owner) = state.preferences.clone() {
        let changed: Vec<_> = points
            .iter()
            .filter(|point| !state.breakpoints.contains(point))
            .cloned()
            .collect();
        let removed: Vec<_> = state
            .breakpoints
            .iter()
            .filter(|point| {
                !points
                    .iter()
                    .any(|next| next.breakpoint.id == point.breakpoint.id)
            })
            .cloned()
            .collect();
        let mut parent = owner
            .0
            .lock()
            .map_err(|_| invalid("Debugger unavailable"))?;
        let merged = merge_points(&parent.breakpoints, &changed, &removed)?;
        persist(&mut parent, &merged)?;
        state.generation = parent.generation;
        state.breakpoints = parent
            .breakpoints
            .iter()
            .filter(|point| point.file.starts_with("extension/"))
            .cloned()
            .collect();
        return Ok(());
    }
    let generation = state
        .generation
        .checked_add(1)
        .ok_or_else(|| invalid("Debugger preference generation exhausted"))?;
    if let Some(path) = &state.path {
        let bytes = serde_json::to_vec(&Stored {
            generation,
            points: points.to_vec(),
        })
        .map_err(|_| invalid("Debugger preferences could not be encoded"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("Debugger preferences exceed their storage limit"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| invalid("Debugger preferences could not be saved"))?;
        }
        let mut file =
            std::fs::File::create(path.with_extension(format!("{}.json", generation % 2)))
                .map_err(|_| invalid("Debugger preferences could not be saved"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| invalid("Debugger preferences could not be saved"))?;
    }
    state.generation = generation;
    state.breakpoints = points.to_vec();
    Ok(())
}
fn merge_points(
    current: &[WorkspaceBreakpoint],
    changed: &[WorkspaceBreakpoint],
    removed: &[WorkspaceBreakpoint],
) -> Result<Vec<WorkspaceBreakpoint>, AppError> {
    let mut result = current.to_vec();
    result.retain(|point| {
        !removed.iter().any(|removed| {
            removed.breakpoint.id == point.breakpoint.id
                && removed.file == point.file
                && removed.anchor == point.anchor
        })
    });
    for change in changed {
        if let Some(previous) = result.iter_mut().find(|previous| {
            previous.breakpoint.id == change.breakpoint.id
                && previous.file == change.file
                && previous.anchor == change.anchor
        }) {
            previous.enabled = change.enabled;
            previous
                .breakpoint
                .condition
                .clone_from(&change.breakpoint.condition);
        } else {
            let mut addition = change.clone();
            if result
                .iter()
                .any(|point| point.breakpoint.id == addition.breakpoint.id)
            {
                addition.breakpoint.id = result
                    .iter()
                    .map(|point| point.breakpoint.id)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| invalid("Breakpoint identity limit reached"))?;
            }
            result.push(addition);
        }
    }
    if !valid_points(&result) {
        return Err(invalid("Invalid debugger breakpoints"));
    }
    Ok(result)
}

type Sources = BTreeMap<(String, String), (DebuggerFile, String)>;
fn script_sources(
    revisions: &[crate::ScriptRevision],
    drafts: &[crate::ScriptDraft],
    output: &mut Sources,
) {
    for revision in revisions {
        let artifact = format!(
            "{}:{}:{}:{}",
            revision.manifest.id,
            revision.manifest.revision,
            revision.javascript_hash,
            revision.source_map_hash
        );
        let generated = DebuggerFile {
            id: format!("script/{}/main.js", revision.manifest.id),
            group: "User scripts".into(),
            package: revision.manifest.id.clone(),
            path: "main.js".into(),
            artifact: artifact.clone(),
            source: "file:///transmog-script/main.js".into(),
            language: "javascript".into(),
            counterpart: (revision.language == crate::ScriptLanguage::TypeScript)
                .then(|| format!("script/{}/main.ts", revision.manifest.id)),
            state: "Unloaded".into(),
        };
        output.insert(
            (generated.id.clone(), artifact.clone()),
            (generated, revision.javascript.clone()),
        );
        if revision.language == crate::ScriptLanguage::TypeScript {
            let source = sourcemap::decode_slice(&revision.source_map)
                .ok()
                .and_then(|map| {
                    if let sourcemap::DecodedMap::Regular(map) = map {
                        map.get_source(0).map(str::to_owned)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    format!("file:///transmog-scripts/{}/main.ts", revision.manifest.id)
                });
            let file = DebuggerFile {
                id: format!("script/{}/main.ts", revision.manifest.id),
                group: "User scripts".into(),
                package: revision.manifest.id.clone(),
                path: "main.ts".into(),
                artifact: artifact.clone(),
                source,
                language: "typescript".into(),
                counterpart: Some(format!("script/{}/main.js", revision.manifest.id)),
                state: "Unloaded".into(),
            };
            output.insert((file.id.clone(), artifact), (file, revision.source.clone()));
        }
    }
    for draft in drafts {
        let suffix = if draft.language == crate::ScriptLanguage::TypeScript {
            "ts"
        } else {
            "js"
        };
        let id = format!("script/{}/main.{suffix}", draft.id);
        if output
            .values()
            .any(|(file, text)| file.id == id && text == &draft.source)
        {
            continue;
        }
        let artifact = format!(
            "draft:{}",
            transmog_script::source_hash(draft.source.as_bytes())
        );
        let source = output.values().find(|(file, _)| file.id == id).map_or_else(
            || format!("file:///transmog-scripts/{}/main.{suffix}", draft.id),
            |(file, _)| file.source.clone(),
        );
        let file = DebuggerFile {
            id: id.clone(),
            group: "User scripts".into(),
            package: draft.id.clone(),
            path: format!("main.{suffix}"),
            artifact: artifact.clone(),
            source,
            language: if suffix == "ts" {
                "typescript"
            } else {
                "javascript"
            }
            .into(),
            counterpart: None,
            state: "Saved draft".into(),
        };
        output.insert((id, artifact), (file, draft.source.clone()));
    }
}
fn extension_files(snapshot: &transmog_extensions::Snapshot, output: &mut Sources) {
    for (path, bytes) in &snapshot.files {
        if !extension(path, "js") && !extension(path, "ts") {
            continue;
        }
        if bytes.len() > 4 * 1024 * 1024 {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let Some((id, source)) = extension_identity(&snapshot.manifest.id, path) else {
            continue;
        };
        let file = DebuggerFile {
            id,
            group: "Extensions".into(),
            package: snapshot.manifest.id.clone(),
            path: path.clone(),
            artifact: snapshot.digest.clone(),
            source,
            language: if extension(path, "ts") {
                "typescript"
            } else {
                "javascript"
            }
            .into(),
            state: "Unloaded".into(),
            counterpart: None,
        };
        output.insert(
            (file.id.clone(), file.artifact.clone()),
            (file, text.to_owned()),
        );
    }
}
fn extension_identity(package: &str, path: &str) -> Option<(String, String)> {
    let root = url::Url::parse("file:///transmog-extension/").ok()?;
    let source = root.join(path).ok()?;
    let relative = source.as_str().strip_prefix(root.as_str())?;
    if source.query().is_some() || source.fragment().is_some() {
        return None;
    }
    Some((
        format!("extension/{package}/{relative}"),
        source.to_string(),
    ))
}
fn target_prefix(target: &DebugSessionInfo, catalog: &[(DebuggerFile, String)]) -> String {
    if target.mode == "Workspace script" || target.mode.starts_with("Sandbox") {
        format!(
            "script/{}/",
            target.artifact.split(':').next().unwrap_or_default()
        )
    } else {
        let package = catalog
            .iter()
            .find(|(file, _)| file.artifact == target.artifact && file.group == "Extensions")
            .map_or(target.label.as_str(), |(file, _)| file.package.as_str());
        format!("extension/{package}/")
    }
}
fn retain_sources(
    state: &State,
    output: &mut Sources,
    artifacts: &std::collections::BTreeSet<(String, bool)>,
) {
    for (file, text) in &state.catalog {
        if artifacts
            .iter()
            .any(|(artifact, _)| artifact == &file.artifact)
        {
            let mut retained = file.clone();
            retained.state = if artifacts.contains(&(file.artifact.clone(), true)) {
                "Unloaded"
            } else {
                "Retired"
            }
            .into();
            output
                .entry((file.id.clone(), file.artifact.clone()))
                .or_insert_with(|| (retained, text.clone()));
        }
    }
}
fn script_maps(revisions: &[crate::ScriptRevision]) -> Vec<projections::Map> {
    let mut maps = Vec::new();
    for revision in revisions {
        let compiled = transmog_script::CompiledScript {
            manifest: revision.manifest.clone(),
            javascript: revision.javascript.clone(),
            source_map: revision.source_map.clone(),
        };
        if let Some(map) = projections::Map::read(
            &script_info(&compiled).artifact,
            "file:///transmog-script/main.js",
            "file:///transmog-script/main.js",
            &revision.source_map,
        ) {
            maps.push(map);
        }
    }

    maps
}
fn collect_extension_maps(
    snapshot: &transmog_extensions::Snapshot,
    maps: &mut Vec<projections::Map>,
) {
    let Some(declaration) = snapshot
        .manifest
        .source_maps
        .as_ref()
        .filter(|value| value.version == 1 && value.modules.len() <= 64)
    else {
        return;
    };
    let root = url::Url::parse("file:///transmog-extension/").expect("constant root");
    let mut budget = 1024 * 1024usize;
    for (module, path) in &declaration.modules {
        if let Some(bytes) = snapshot.files.get(path)
            && let Some(remaining) = budget.checked_sub(bytes.len())
            && let Ok(base) = root.join(path)
            && let Some((_, generated)) = extension_identity(&snapshot.manifest.id, module)
            && let Some(map) =
                projections::Map::read(&snapshot.digest, &generated, base.as_str(), bytes)
        {
            budget = remaining;
            maps.push(map);
        }
    }
}
fn extension_maps(snapshot: &transmog_extensions::Snapshot, output: &mut Sources) {
    let Some(declaration) = snapshot
        .manifest
        .source_maps
        .as_ref()
        .filter(|declaration| declaration.version == 1 && declaration.modules.len() <= 64)
    else {
        return;
    };
    for (generated, path) in &declaration.modules {
        let Some(bytes) = snapshot.files.get(path) else {
            continue;
        };
        if bytes.len() > 256 * 1024 {
            continue;
        }
        let Ok(sourcemap::DecodedMap::Regular(map)) = sourcemap::decode_slice(bytes) else {
            continue;
        };
        let Some((generated_id, _)) = extension_identity(&snapshot.manifest.id, generated) else {
            continue;
        };
        if !output.contains_key(&(generated_id.clone(), snapshot.digest.clone())) {
            continue;
        }
        let root = url::Url::parse("file:///transmog-extension/").expect("constant source root");
        let Ok(base) = root.join(path) else {
            continue;
        };
        for index in 0..map.get_source_count().min(128) {
            let Some(name) = map.get_source(index) else {
                continue;
            };
            let Ok(source) = base.join(name) else {
                continue;
            };
            let Some(relative) = source
                .as_str()
                .strip_prefix(root.as_str())
                .filter(|_| source.query().is_none() && source.fragment().is_none())
            else {
                continue;
            };
            let id = format!("extension/{}/{relative}", snapshot.manifest.id);
            if let std::collections::btree_map::Entry::Vacant(entry) =
                output.entry((id.clone(), snapshot.digest.clone()))
            {
                let Some(text) = map
                    .get_source_contents(index)
                    .filter(|text| text.len() <= 4 * 1024 * 1024)
                else {
                    continue;
                };
                let file = DebuggerFile {
                    id: id.clone(),
                    group: "Extensions".into(),
                    package: snapshot.manifest.id.clone(),
                    path: relative.into(),
                    artifact: snapshot.digest.clone(),
                    source: source.to_string(),
                    language: if extension(relative, "ts") {
                        "typescript"
                    } else {
                        "javascript"
                    }
                    .into(),
                    counterpart: Some(generated_id.clone()),
                    state: "Unloaded".into(),
                };
                entry.insert((file, text.into()));
            }
            if let Some((original, _)) = output.get_mut(&(id.clone(), snapshot.digest.clone())) {
                original.counterpart = Some(generated_id.clone());
            }
            if let Some((generated, _)) =
                output.get_mut(&(generated_id.clone(), snapshot.digest.clone()))
            {
                generated.counterpart.get_or_insert(id);
            }
        }
    }
}
fn extension(path: &str, suffix: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(suffix))
}

fn reconcile_points(
    points: &[WorkspaceBreakpoint],
    revisions: &[crate::ScriptRevision],
    drafts: &[crate::ScriptDraft],
    entries: &[transmog_extension_registry::ExtensionEntry],
    verified_extensions: &std::collections::BTreeSet<String>,
    output: &Sources,
    authoritative: bool,
) -> Vec<WorkspaceBreakpoint> {
    points
        .iter()
        .filter_map(|point| {
            if !authoritative && point.file.starts_with("extension/") {
                return Some(point.clone());
            }
            let prefix = point.file.split('/').take(2).collect::<Vec<_>>().join("/");
            // A developer instance in another capture window may still run an
            // older snapshot after a failed reload. Its absence here proves no deletion.
            let active_development = entries.iter().any(|entry| {
                entry.hot_reload && format!("extension/{}", entry.catalog.manifest.id) == prefix
            });
            let exists = if point.file.starts_with("script/") {
                revisions
                    .iter()
                    .any(|r| format!("script/{}", r.manifest.id) == prefix)
                    || drafts.iter().any(|d| format!("script/{}", d.id) == prefix)
            } else {
                entries
                    .iter()
                    .any(|e| format!("extension/{}", e.catalog.manifest.id) == prefix)
            };
            if !exists {
                return None;
            }
            let matching: Vec<_> = output
                .values()
                .filter(|(file, _)| {
                    file.id == point.file && file.state != "Saved draft" && file.state != "Retired"
                })
                .collect();
            if matching.is_empty() {
                return (!point.file.starts_with("extension/")
                    || active_development
                    || !verified_extensions
                        .contains(point.file.split('/').nth(1).unwrap_or_default()))
                .then(|| point.clone());
            }
            // Keep a point that resolves in any retained immutable revision. Rebind only
            // when its statement moved to one unambiguous line across known revisions.
            if matching.iter().any(|(_, text)| {
                text.lines()
                    .nth(point.breakpoint.location.line.saturating_sub(1) as usize)
                    == Some(point.anchor.as_str())
            }) {
                return Some(point.clone());
            }
            let lines: std::collections::BTreeSet<_> = matching
                .iter()
                .flat_map(|(_, text)| {
                    text.lines()
                        .enumerate()
                        .filter(|(_, line)| *line == point.anchor)
                        .map(|(index, _)| index + 1)
                })
                .collect();
            if lines.len() == 1 {
                let mut rebound = point.clone();
                rebound.breakpoint.location.line = u32::try_from(*lines.first().unwrap()).ok()?;
                Some(rebound)
            } else {
                active_development.then(|| point.clone())
            }
        })
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point() -> WorkspaceBreakpoint {
        WorkspaceBreakpoint {
            artifact: None,
            file: "script/sample/main.ts".into(),
            breakpoint: SourceBreakpoint {
                id: 1,
                condition: Some("value === 7".into()),
                location: transmog_script_diagnostics::SourceLocation {
                    source: "file:///sample.ts".into(),
                    line: 2,
                    column: 1,
                },
            },
            enabled: true,
            anchor: " const value=7;".into(),
        }
    }
    #[test]
    fn independent_window_edits_merge_and_preserve_rebound_coordinates() {
        let mut original = point();
        original.breakpoint.location.line = 7;
        let mut edit = point();
        edit.enabled = false;
        let merged = merge_points(std::slice::from_ref(&original), &[edit], &[]).unwrap();
        assert!(!merged[0].enabled);
        assert_eq!(merged[0].breakpoint.location.line, 7);
        let mut other = point();
        other.file = "extension/another/main.ts".into();
        let merged = merge_points(&merged, &[other], &[]).unwrap();
        assert_eq!(merged.len(), 2);
        assert_ne!(merged[0].breakpoint.id, merged[1].breakpoint.id);
        let removed = merge_points(&merged, &[], &[original]).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].file.starts_with("extension/"));
    }
    #[test]
    fn capture_preferences_share_storage_and_keep_worker_ownership_independent() {
        let owner = Workspace::default();
        let capture = Workspace::default();
        let script = point();
        let mut extension = point();
        extension.file = "extension/example/main.ts".into();
        extension.breakpoint.id = 2;
        {
            let mut state = owner.0.lock().unwrap();
            persist(&mut state, &[script.clone(), extension.clone()]).unwrap();
        }
        capture.0.lock().unwrap().preferences = Some(owner.clone());
        capture.sync_preferences().unwrap();
        assert_eq!(capture.0.lock().unwrap().breakpoints.len(), 1);
        extension.enabled = false;
        {
            let mut state = capture.0.lock().unwrap();
            state.open = true;
            persist(&mut state, &[extension]).unwrap();
        }
        let state = owner.0.lock().unwrap();
        assert!(!state.open);
        assert!(state.targets.is_empty());
        assert!(state.breakpoints.contains(&script));
        assert!(!state.breakpoints[1].enabled);
    }
    #[test]
    fn an_unavailable_catalog_does_not_prove_extension_breakpoints_stale() {
        let mut extension = point();
        extension.file = "extension/example/main.ts".into();
        assert_eq!(
            reconcile_points(
                std::slice::from_ref(&extension),
                &[],
                &[],
                &[],
                &std::collections::BTreeSet::new(),
                &Sources::new(),
                false
            )
            .len(),
            1
        );
        assert!(
            reconcile_points(
                &[extension],
                &[],
                &[],
                &[],
                &std::collections::BTreeSet::new(),
                &Sources::new(),
                true
            )
            .is_empty()
        );
    }
    #[test]
    fn active_development_in_another_window_does_not_prove_old_intent_stale() {
        let mut manifest: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../extensions/binary/manifest.json"))
                .unwrap();
        manifest["files"] = serde_json::json!([]);
        manifest["id"] = serde_json::json!("example");
        let mut entry = transmog_extension_registry::ExtensionEntry {
            catalog: transmog_extensions::CatalogEntry {
                manifest: serde_json::from_value(manifest).unwrap(),
                digest: "a".repeat(64),
                enabled: true,
                origin: transmog_extensions::Origin::Development,
            },
            directory: Some("example".into()),
            hot_reload: true,
        };
        let mut point = point();
        point.file = "extension/example/main.ts".into();
        let verified = std::collections::BTreeSet::from(["example".into()]);
        let sources = Sources::new();
        assert_eq!(
            reconcile_points(
                std::slice::from_ref(&point),
                &[],
                &[],
                std::slice::from_ref(&entry),
                &verified,
                &sources,
                true
            )
            .len(),
            1
        );
        entry.hot_reload = false;
        assert!(
            reconcile_points(&[point], &[], &[], &[entry], &verified, &sources, true).is_empty()
        );
    }
    #[test]
    fn source_updates_rebind_unique_statements_and_drop_removed_or_ambiguous_ones() {
        let point = point();
        assert_eq!(
            rebind(&point, "function run(){\n const value=7;\n}")
                .unwrap()
                .location
                .line,
            2
        );
        assert_eq!(
            rebind(&point, "// inserted\nfunction run(){\n const value=7;\n}")
                .unwrap()
                .location
                .line,
            3
        );
        assert!(rebind(&point, "function run(){\n return 7;\n}").is_none());
        assert!(
            rebind(
                &point,
                "// inserted\nfunction run(){\n const value=7;\n const value=7;\n}"
            )
            .is_none()
        );
    }
    #[test]
    fn breakpoint_preferences_survive_restart_and_an_incomplete_latest_write() {
        let prefix = std::env::temp_dir().join(format!(
            "transmog-debugger-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace = Workspace::load(Some(&prefix));
        let points = vec![point()];
        {
            let mut state = workspace.0.lock().unwrap();
            persist(&mut state, &points).unwrap();
        }
        let path = workspace.0.lock().unwrap().path.clone().unwrap();
        std::fs::write(path.with_extension("0.json"), b"{partial").unwrap();
        let reopened = Workspace::load(Some(&prefix));
        assert!(reopened.0.lock().unwrap().breakpoints == points);
        std::fs::remove_file(path.with_extension("0.json")).unwrap();
        std::fs::remove_file(path.with_extension("1.json")).unwrap();
    }
}
