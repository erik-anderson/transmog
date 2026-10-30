use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use transmog_core::intercept::{
    ExchangeMetadata, HookInitError, InterceptorRegistration, InterceptorRegistrationProvider,
};
use transmog_script::{
    BoxScriptFuture, CompiledScript, ScriptCapabilities, ScriptFailure, ScriptHandler,
    ScriptInvocation, ScriptLimits, ScriptManifest, ScriptPrefilter, ScriptResponseAssetResolver,
    ScriptRunner, compile_typescript, registration_with_assets, source_hash, validate_javascript,
};
use transmog_script_supervisor::{
    IsolationPolicy, MAX_ACTIVE_SCRIPT_HOSTS, ScriptHostConfig, SupervisedScriptRunner,
};

use crate::{AppError, ErrorCategory};

const SCRIPT_WORKSPACE_SCHEMA_VERSION: u32 = 2;
const MAX_SCRIPT_WORKSPACE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_CANDIDATES: usize = 16;
const MAX_HISTORY: usize = 32;
const MAX_SAVED_DRAFTS: usize = 64;

/// Preferred authoring language; execution always uses saved JavaScript.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    /// Default typed authoring experience.
    #[default]
    TypeScript,
    /// Direct JavaScript authoring.
    JavaScript,
}

/// Editable source and explicit authority for one script revision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptDraft {
    /// Stable script identity.
    pub id: String,
    /// Monotonic caller-owned revision.
    pub revision: u64,
    /// Editable ES module source.
    pub source: String,
    /// Current authoring language.
    #[serde(default)]
    pub language: ScriptLanguage,
    /// Preserved source from the other authoring language.
    #[serde(default)]
    pub retained_source: Option<String>,
    /// Explicitly recoverable earlier JavaScript drafts, newest first.
    #[serde(default)]
    pub javascript_history: Vec<String>,
    /// Named synchronous handlers.
    pub handlers: std::collections::BTreeSet<ScriptHandler>,
    /// Native prefilter.
    pub prefilter: ScriptPrefilter,
    /// Explicit read/write authority.
    pub capabilities: ScriptCapabilities,
    /// Finite runtime limits.
    pub limits: ScriptLimits,
    /// Hook ordering priority.
    pub priority: i32,
}

/// Durable immutable script source and generated manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptRevision {
    /// Validated manifest including exact source hash.
    pub manifest: ScriptManifest,
    /// Exact source used for this build.
    pub source: String,
    /// Source authoring language.
    pub language: ScriptLanguage,
    /// Other-language draft retained through activation.
    pub retained_source: Option<String>,
    /// Explicitly recoverable earlier JavaScript drafts, newest first.
    #[serde(default)]
    pub javascript_history: Vec<String>,
    /// Executable bytes, never regenerated during loading or activation.
    pub javascript: String,
    /// Map belonging to these exact executable bytes and source.
    pub source_map: Vec<u8>,
    /// SHA-256 of executable bytes.
    pub javascript_hash: String,
    /// SHA-256 of the separate map.
    pub source_map_hash: String,
    /// Versioned emission recipe for provenance, not a load requirement.
    pub compiler: String,
    /// API declaration identity at build time.
    pub declarations_hash: String,
}

/// Non-active validated candidate token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptCandidate {
    /// Content-addressed activation token.
    pub candidate_id: String,
    /// Stable script ID.
    pub script_id: String,
    /// Immutable revision.
    pub revision: u64,
    /// Exact source hash.
    pub source_hash: String,
}

/// Availability of one exact applied JavaScript build.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRuntimeStatus {
    /// Stable script identity.
    pub script_id: String,
    /// Applied revision, independently of newer drafts or builds.
    pub revision: u64,
    /// Whether the initialized sandbox is available for invocations.
    pub available: bool,
    /// Bounded failure and recovery detail when unavailable.
    pub error: Option<String>,
}

/// Current atomic script registry status.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptStatus {
    /// Monotonic activation generation.
    pub generation: u64,
    /// Exact active revisions.
    pub active: Vec<ScriptRevision>,
    /// Saved drafts, including drafts that have not validated yet.
    pub saved: Vec<ScriptDraft>,
    /// Last successful authoring build per script, independently of activation.
    pub built: Vec<ScriptRevision>,
    /// Last applied versions retained while paused.
    pub paused: Vec<ScriptRevision>,
    /// Actual sandbox availability, separate from configured activation.
    pub runtimes: Vec<ScriptRuntimeStatus>,
    /// Validated candidates awaiting activation.
    pub candidate_count: usize,
    /// Retained prior active snapshots.
    pub history_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScriptWorkspace {
    schema_version: u32,
    generation: u64,
    active: Vec<ScriptRevision>,
    #[serde(default)]
    saved: Vec<ScriptDraft>,
    #[serde(default)]
    built: Vec<ScriptRevision>,
    #[serde(default)]
    paused: Vec<ScriptRevision>,
}

#[derive(Clone)]
struct ActiveScript {
    registration: InterceptorRegistration,
    runner: Option<Arc<SupervisedScriptRunner>>,
    startup_error: Option<String>,
}

struct ActiveSnapshot {
    workspace: ScriptWorkspace,
    scripts: Vec<ActiveScript>,
}

#[derive(Clone)]
struct CandidateEntry {
    id: String,
    revision: ScriptRevision,
    generation: u64,
}

/// Crash-safe validate/start/activate registry and registration provider.
#[derive(Clone)]
pub struct ScriptRegistry {
    mutation: Arc<Mutex<()>>,
    active: Arc<RwLock<Arc<ActiveSnapshot>>>,
    candidates: Arc<Mutex<VecDeque<CandidateEntry>>>,
    history: Arc<Mutex<VecDeque<Arc<ActiveSnapshot>>>>,
    path: Option<Arc<PathBuf>>,
    host: Option<Arc<PathBuf>>,
    assets: Arc<dyn ScriptResponseAssetResolver>,
}

impl ScriptRegistry {
    pub(crate) fn load(
        path: Option<PathBuf>,
        host: Option<PathBuf>,
        assets: Arc<dyn ScriptResponseAssetResolver>,
    ) -> Result<Self, AppError> {
        let loaded = path.as_deref().and_then(load_latest);
        if loaded.is_none()
            && path
                .as_deref()
                .is_some_and(|path| (0..=1).any(|slot| slot_path(path, slot).exists()))
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "saved script workspace is unsupported or damaged; original files have been preserved",
                false,
            ));
        }
        let workspace = loaded.unwrap_or(ScriptWorkspace {
            schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
            ..ScriptWorkspace::default()
        });
        validate_workspace(&workspace)?;
        let registry = Self {
            mutation: Arc::new(Mutex::new(())),
            active: Arc::new(RwLock::new(Arc::new(ActiveSnapshot {
                workspace: ScriptWorkspace {
                    schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                    ..ScriptWorkspace::default()
                },
                scripts: Vec::new(),
            }))),
            candidates: Arc::new(Mutex::new(VecDeque::new())),
            history: Arc::new(Mutex::new(VecDeque::new())),
            path: path.map(Arc::new),
            host: host.map(Arc::new),
            assets,
        };
        let snapshot = registry.build_snapshot(workspace, false)?;
        *registry
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(snapshot);
        Ok(registry)
    }

    pub(crate) fn validate(&self, draft: ScriptDraft) -> Result<ScriptCandidate, AppError> {
        self.validate_checked(draft, None)
    }

    pub(crate) fn validate_checked(
        &self,
        draft: ScriptDraft,
        expected_generation: Option<u64>,
    ) -> Result<ScriptCandidate, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = {
            let active = self
                .active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            check_generation(&active.workspace, expected_generation)?;
            active.workspace.generation
        };
        validate_draft_shape(&draft)?;
        let revision = build_revision(draft)?;
        // Authoring builds are durable, but never alter live registrations or drafts.
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workspace = active.workspace.clone();
        let scripts = active.scripts.clone();
        drop(active);
        workspace
            .built
            .retain(|item| item.manifest.id != revision.manifest.id);
        workspace.built.push(revision.clone());
        if workspace.built.len() > MAX_SAVED_DRAFTS {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "script build limit exceeded",
                false,
            ));
        }
        workspace.generation = generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "script generation is exhausted",
                false,
            )
        })?;
        if let Some(path) = self.path.as_deref() {
            persist(path, &workspace)?;
        }
        let generation = workspace.generation;
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Arc::new(ActiveSnapshot { workspace, scripts });
        let encoded = serde_json::to_vec(&revision).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "script candidate serialization failed",
                false,
            )
        })?;
        // A token belongs to the exact workspace against which it was checked.
        let id = format!("{generation}-{}", hex_sha256(&encoded));
        let result = ScriptCandidate {
            candidate_id: id.clone(),
            script_id: revision.manifest.id.clone(),
            revision: revision.manifest.revision,
            source_hash: revision.manifest.source_hash.clone(),
        };
        let mut candidates = self
            .candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidates.retain(|candidate| candidate.id != id);
        candidates.push_back(CandidateEntry {
            id,
            revision,
            generation,
        });
        while candidates.len() > MAX_CANDIDATES {
            candidates.pop_front();
        }
        Ok(result)
    }

    pub(crate) fn save(&self, draft: ScriptDraft) -> Result<ScriptStatus, AppError> {
        self.save_checked(draft, None)
    }

    pub(crate) fn save_checked(
        &self,
        draft: ScriptDraft,
        expected_generation: Option<u64>,
    ) -> Result<ScriptStatus, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_draft_shape(&draft)?;
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workspace = active.workspace.clone();
        let scripts = active.scripts.clone();
        drop(active);
        check_generation(&workspace, expected_generation)?;
        if workspace
            .active
            .iter()
            .chain(&workspace.paused)
            .any(|active| {
                active.manifest.id == draft.id && active.manifest.revision >= draft.revision
            })
        {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "script draft revision must increase after activation",
                false,
            ));
        }
        workspace.saved.retain(|saved| saved.id != draft.id);
        workspace.saved.push(draft);
        workspace.saved.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.revision.cmp(&right.revision))
        });
        if workspace.saved.len() > MAX_SAVED_DRAFTS {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "saved script draft limit exceeded",
                false,
            ));
        }
        workspace.generation = workspace.generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "script generation is exhausted",
                false,
            )
        })?;
        if let Some(path) = self.path.as_deref() {
            persist(path, &workspace)?;
        }
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Arc::new(ActiveSnapshot { workspace, scripts });
        Ok(self.status())
    }

    pub(crate) async fn test(
        &self,
        candidate_id: &str,
        invocation: ScriptInvocation,
    ) -> Result<transmog_script::ScriptAction, AppError> {
        let revision = self
            .candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .map(|candidate| candidate.revision.clone())
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Conflict,
                    "script candidate is unknown or expired",
                    false,
                )
            })?;
        let compiled = executable(&revision)?;
        let runner = self.start_runner(compiled)?;
        runner.invoke(invocation).await.map_err(|failure| {
            let location = failure
                .line
                .map(|line| format!(":{line}:{}", failure.column.unwrap_or(1)))
                .unwrap_or_default();
            AppError::new(
                match failure.category {
                    transmog_script::ScriptFailureCategory::InvalidAction
                    | transmog_script::ScriptFailureCategory::Exception
                    | transmog_script::ScriptFailureCategory::Protocol => {
                        ErrorCategory::InvalidInput
                    }
                    _ => ErrorCategory::Unavailable,
                },
                format!("script test failed{location}: {}", failure.message),
                matches!(
                    failure.category,
                    transmog_script::ScriptFailureCategory::Crash
                        | transmog_script::ScriptFailureCategory::Unavailable
                ),
            )
        })
    }

    pub(crate) fn activate(&self, candidate_id: &str) -> Result<ScriptStatus, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let candidate = {
            let candidates = self
                .candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            candidates
                .iter()
                .find(|candidate| candidate.id == candidate_id)
                .cloned()
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::Conflict,
                        "script candidate is unknown or expired",
                        false,
                    )
                })?
        };
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current
            .workspace
            .active
            .iter()
            .chain(&current.workspace.paused)
            .any(|revision| {
                revision.manifest.id == candidate.revision.manifest.id
                    && revision.manifest.revision >= candidate.revision.manifest.revision
            })
        {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "script revision must increase before activation",
                false,
            ));
        }
        let mut workspace = current.workspace.clone();
        drop(current);
        check_generation(&workspace, Some(candidate.generation))?;
        workspace.generation = workspace.generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "script generation is exhausted",
                false,
            )
        })?;
        workspace
            .active
            .retain(|revision| revision.manifest.id != candidate.revision.manifest.id);
        save_activated_draft(&mut workspace, &candidate.revision)?;
        workspace
            .paused
            .retain(|item| item.manifest.id != candidate.revision.manifest.id);
        workspace.active.push(candidate.revision);
        workspace.active.sort_by(|left, right| {
            left.manifest
                .priority
                .cmp(&right.manifest.priority)
                .then_with(|| left.manifest.id.cmp(&right.manifest.id))
        });
        if workspace.active.len() > MAX_ACTIVE_SCRIPT_HOSTS {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "active script process limit exceeded",
                false,
            ));
        }
        // Start and authenticate every requested revision before persistence or
        // atomic replacement, so a failed edit leaves the old snapshot intact.
        let snapshot = self.build_snapshot(workspace, true)?;
        if let Some(path) = self.path.as_deref() {
            persist(path, &snapshot.workspace)?;
        }
        let replacement = Arc::new(snapshot);
        let previous = {
            let mut active = self
                .active
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *active, replacement)
        };
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        history.push_back(previous);
        while history.len() > MAX_HISTORY {
            history.pop_front();
        }
        drop(history);
        self.candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(self.status())
    }

    pub(crate) fn disable(&self, id: &str) -> Result<ScriptStatus, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workspace = current.workspace.clone();
        if !workspace
            .active
            .iter()
            .any(|revision| revision.manifest.id == id)
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "active script was not found",
                false,
            ));
        }
        // Older workspaces could contain an active revision without a saved
        // editing baseline. Pausing must not make that source disappear.
        if let Some(revision) = workspace
            .active
            .iter()
            .find(|revision| revision.manifest.id == id)
            .cloned()
            && !workspace
                .saved
                .iter()
                .any(|draft| draft.id == id && draft.revision > revision.manifest.revision)
        {
            save_activated_draft(&mut workspace, &revision)?;
        }
        if let Some(revision) = workspace
            .active
            .iter()
            .find(|item| item.manifest.id == id)
            .cloned()
        {
            workspace.paused.retain(|item| item.manifest.id != id);
            workspace.paused.push(revision);
        }
        workspace
            .active
            .retain(|revision| revision.manifest.id != id);
        workspace.generation = workspace.generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "script generation is exhausted",
                false,
            )
        })?;
        drop(current);
        let snapshot = self.build_snapshot(workspace, true)?;
        if let Some(path) = self.path.as_deref() {
            persist(path, &snapshot.workspace)?;
        }
        let replacement = Arc::new(snapshot);
        let previous = {
            let mut active = self
                .active
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *active, replacement)
        };
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        history.push_back(previous);
        while history.len() > MAX_HISTORY {
            history.pop_front();
        }
        drop(history);
        Ok(self.status())
    }

    pub(crate) fn resume(
        &self,
        id: &str,
        expected_generation: u64,
    ) -> Result<ScriptStatus, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workspace = current.workspace.clone();
        drop(current);
        check_generation(&workspace, Some(expected_generation))?;
        let index = workspace
            .paused
            .iter()
            .position(|item| item.manifest.id == id)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Conflict,
                    "paused script is unavailable",
                    false,
                )
            })?;
        let revision = workspace.paused.remove(index);
        workspace.active.push(revision);
        workspace
            .active
            .sort_by_key(|item| (item.manifest.priority, item.manifest.id.clone()));
        if workspace.active.len() > MAX_ACTIVE_SCRIPT_HOSTS {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "active script process limit exceeded",
                false,
            ));
        }
        workspace.generation = workspace.generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "script generation is exhausted",
                false,
            )
        })?;
        let snapshot = self.build_snapshot(workspace, true)?;
        if let Some(path) = self.path.as_deref() {
            persist(path, &snapshot.workspace)?;
        }
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(snapshot);
        self.candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(self.status())
    }

    pub(crate) fn status(&self) -> ScriptStatus {
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ScriptStatus {
            generation: active.workspace.generation,
            active: active.workspace.active.clone(),
            saved: active.workspace.saved.clone(),
            built: active.workspace.built.clone(),
            paused: active.workspace.paused.clone(),
            runtimes: snapshot_runtimes(&active),
            candidate_count: self
                .candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            history_count: self
                .history
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
        }
    }

    pub(crate) fn activity_hint(&self) -> (u64, usize) {
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (active.workspace.generation, active.workspace.active.len())
    }

    fn build_snapshot(
        &self,
        workspace: ScriptWorkspace,
        strict: bool,
    ) -> Result<ActiveSnapshot, AppError> {
        let scripts = workspace
            .active
            .iter()
            .map(|revision| self.build_active(revision, strict))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ActiveSnapshot { workspace, scripts })
    }

    fn build_active(
        &self,
        revision: &ScriptRevision,
        strict: bool,
    ) -> Result<ActiveScript, AppError> {
        let (runner, startup_error) =
            match executable(revision).and_then(|compiled| self.start_runner(compiled)) {
                Ok(runner) => (Some(runner), None),
                Err(error) if !strict => (None, Some(error.message)),
                Err(error) => return Err(error),
            };
        let invocation_runner: Arc<dyn ScriptRunner> = match &runner {
            Some(runner) => runner.clone(),
            None => Arc::new(UnavailableRunner(ScriptFailure {
                category: transmog_script::ScriptFailureCategory::Unavailable,
                message: startup_error.clone().unwrap_or_default(),
                line: None,
                column: None,
            })),
        };
        let registration = registration_with_assets(
            revision.manifest.clone(),
            invocation_runner,
            Some(self.assets.clone()),
        )
        .map_err(script_compile_error)?;
        Ok(ActiveScript {
            registration,
            runner,
            startup_error,
        })
    }

    pub(crate) fn runtime_status(&self) -> Vec<ScriptRuntimeStatus> {
        snapshot_runtimes(
            &self
                .active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn retry(
        &self,
        id: &str,
        expected_generation: u64,
    ) -> Result<ScriptStatus, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        check_generation(&current.workspace, Some(expected_generation))?;
        let index = current
            .workspace
            .active
            .iter()
            .position(|item| item.manifest.id == id)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Conflict,
                    "applied script changed; refresh before retrying",
                    false,
                )
            })?;
        let replacement = self.build_active(&current.workspace.active[index], true)?;
        let mut scripts = current.scripts.clone();
        scripts[index] = replacement;
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(ActiveSnapshot {
            workspace: current.workspace.clone(),
            scripts,
        });
        Ok(self.status())
    }

    fn start_runner(
        &self,
        compiled: CompiledScript,
    ) -> Result<Arc<SupervisedScriptRunner>, AppError> {
        let executable = self.host.as_deref().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "packaged script host is unavailable",
                true,
            )
        })?;
        SupervisedScriptRunner::start(
            &ScriptHostConfig {
                executable: executable.clone(),
                isolation: IsolationPolicy::RequireSandbox,
            },
            compiled,
        )
        .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.message, true))
    }
}

impl InterceptorRegistrationProvider for ScriptRegistry {
    fn registrations(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Vec<InterceptorRegistration>, HookInitError> {
        Ok(self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scripts
            .iter()
            .map(|script| script.registration.clone())
            .collect())
    }
}

fn snapshot_runtimes(snapshot: &ActiveSnapshot) -> Vec<ScriptRuntimeStatus> {
    snapshot.workspace.active.iter().zip(&snapshot.scripts).map(|(revision, script)| {
        let available = script.runner.as_ref().is_some_and(|runner| !runner.has_failed());
        ScriptRuntimeStatus { script_id: revision.manifest.id.clone(), revision: revision.manifest.revision,
            available, error: if available { None } else { Some(script.startup_error.clone().unwrap_or_else(|| "The script host stopped after a runtime failure. Retry the applied build or pause this script.".to_owned())) } }
    }).collect()
}

struct UnavailableRunner(ScriptFailure);

impl ScriptRunner for UnavailableRunner {
    fn invoke(&self, _invocation: ScriptInvocation) -> BoxScriptFuture<'_> {
        let failure = self.0.clone();
        Box::pin(async move { Err(failure) })
    }
}

fn build_revision(draft: ScriptDraft) -> Result<ScriptRevision, AppError> {
    let manifest = ScriptManifest {
        id: draft.id,
        revision: draft.revision,
        api_revision: transmog_script::SCRIPT_API_REVISION,
        source_hash: source_hash(draft.source.as_bytes()),
        handlers: draft.handlers,
        prefilter: draft.prefilter,
        capabilities: draft.capabilities,
        limits: draft.limits,
        priority: draft.priority,
    };
    let compiled = match draft.language {
        ScriptLanguage::TypeScript => compile_typescript(manifest.clone(), &draft.source),
        ScriptLanguage::JavaScript => validate_javascript(manifest.clone(), &draft.source),
    }
    .map_err(script_compile_error)?;
    let mut javascript_manifest = manifest.clone();
    javascript_manifest.source_hash = source_hash(compiled.javascript.as_bytes());
    validate_javascript(javascript_manifest, &compiled.javascript).map_err(script_compile_error)?;
    let revision = ScriptRevision {
        manifest,
        source: draft.source,
        language: draft.language,
        retained_source: draft.retained_source,
        javascript_history: draft.javascript_history,
        javascript_hash: source_hash(compiled.javascript.as_bytes()),
        source_map_hash: source_hash(&compiled.source_map),
        javascript: compiled.javascript,
        source_map: compiled.source_map,
        compiler: format!(
            "swc;recipe=1;lock={}",
            source_hash(include_bytes!("../../../Cargo.lock"))
        ),
        declarations_hash: source_hash(transmog_script::typescript_declarations().as_bytes()),
    };
    Ok(revision)
}

// This path validates stored JavaScript only; TypeScript is inert provenance.
fn executable(revision: &ScriptRevision) -> Result<CompiledScript, AppError> {
    if source_hash(revision.source.as_bytes()) != revision.manifest.source_hash
        || source_hash(revision.javascript.as_bytes()) != revision.javascript_hash
        || source_hash(&revision.source_map) != revision.source_map_hash
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "saved script build integrity check failed",
            false,
        ));
    }
    let mut manifest = revision.manifest.clone();
    manifest.source_hash.clone_from(&revision.javascript_hash);
    let mut compiled =
        validate_javascript(manifest, &revision.javascript).map_err(script_compile_error)?;
    compiled.manifest = revision.manifest.clone();
    compiled.source_map.clone_from(&revision.source_map);
    Ok(compiled)
}

fn validate_workspace(workspace: &ScriptWorkspace) -> Result<(), AppError> {
    if workspace.schema_version != SCRIPT_WORKSPACE_SCHEMA_VERSION
        || workspace.active.len() > MAX_ACTIVE_SCRIPT_HOSTS
        || workspace.saved.len() > MAX_SAVED_DRAFTS
        || workspace.built.len() > MAX_SAVED_DRAFTS
        || workspace.paused.len() > MAX_SAVED_DRAFTS
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "script workspace is unsupported",
            false,
        ));
    }
    for draft in &workspace.saved {
        validate_draft_shape(draft)?;
    }
    for revision in workspace
        .active
        .iter()
        .chain(&workspace.paused)
        .chain(&workspace.built)
    {
        validate_javascript_history(&revision.javascript_history)?;
        if revision.source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES
            || revision.javascript.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES
            || revision.source_map.len() > 8 * transmog_script::MAX_SCRIPT_SOURCE_BYTES
            || revision
                .retained_source
                .as_ref()
                .is_some_and(|source| source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES)
        {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "saved script build exceeds limits",
                false,
            ));
        }
    }
    Ok(())
}

fn check_generation(workspace: &ScriptWorkspace, expected: Option<u64>) -> Result<(), AppError> {
    if expected.is_some_and(|expected| expected != workspace.generation) {
        return Err(AppError::new(
            ErrorCategory::Conflict,
            "script workspace changed; retry with the current draft",
            false,
        ));
    }
    Ok(())
}

fn save_activated_draft(
    workspace: &mut ScriptWorkspace,
    revision: &ScriptRevision,
) -> Result<(), AppError> {
    let manifest = &revision.manifest;
    if workspace
        .saved
        .iter()
        .any(|draft| draft.id == manifest.id && draft.revision > manifest.revision)
    {
        return Err(AppError::new(
            ErrorCategory::Conflict,
            "a newer script draft is saved; activate the current draft",
            false,
        ));
    }
    workspace.saved.retain(|draft| draft.id != manifest.id);
    workspace.saved.push(ScriptDraft {
        id: manifest.id.clone(),
        revision: manifest.revision,
        source: revision.source.clone(),
        language: revision.language,
        retained_source: revision.retained_source.clone(),
        javascript_history: revision.javascript_history.clone(),
        handlers: manifest.handlers.clone(),
        prefilter: manifest.prefilter.clone(),
        capabilities: manifest.capabilities.clone(),
        limits: manifest.limits,
        priority: manifest.priority,
    });
    if workspace.saved.len() > MAX_SAVED_DRAFTS {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "saved script draft limit exceeded",
            false,
        ));
    }
    Ok(())
}

fn validate_javascript_history(history: &[String]) -> Result<(), AppError> {
    if history.len() > 8
        || history
            .iter()
            .any(|source| source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES)
    {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "previous JavaScript drafts exceed limits; remove a draft before retaining another",
            false,
        ));
    }
    Ok(())
}

fn validate_draft_shape(draft: &ScriptDraft) -> Result<(), AppError> {
    if draft.id.is_empty()
        || draft.id.len() > 128
        || !draft
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || draft.revision == 0
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "script draft identity or revision is invalid",
            false,
        ));
    }
    validate_javascript_history(&draft.javascript_history)?;
    if draft.source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES
        || draft
            .retained_source
            .as_ref()
            .is_some_and(|source| source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES)
    {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "script source exceeds one MiB",
            false,
        ));
    }
    Ok(())
}

fn slot_path(path: &Path, slot: u64) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".{slot}.json"));
    PathBuf::from(value)
}

fn load_latest(path: &Path) -> Option<ScriptWorkspace> {
    let mut workspaces = Vec::new();
    for slot in 0..=1 {
        let candidate = slot_path(path, slot);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.len() > MAX_SCRIPT_WORKSPACE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(candidate) else {
            continue;
        };
        if let Ok(workspace) = serde_json::from_slice::<ScriptWorkspace>(&bytes)
            && validate_workspace(&workspace).is_ok()
        {
            workspaces.push(workspace);
        }
    }
    workspaces.sort_by_key(|workspace| workspace.generation);
    workspaces.pop()
}

fn persist(path: &Path, workspace: &ScriptWorkspace) -> Result<(), AppError> {
    validate_workspace(workspace)?;
    let bytes = serde_json::to_vec_pretty(workspace).map_err(|_| {
        AppError::new(
            ErrorCategory::Internal,
            "script workspace serialization failed",
            false,
        )
    })?;
    if bytes.len() as u64 > MAX_SCRIPT_WORKSPACE_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "script workspace exceeds forty MiB",
            false,
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "script workspace directory is unavailable",
                true,
            )
        })?;
    }
    let destination = slot_path(path, workspace.generation % 2);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "script workspace cannot be opened",
                true,
            )
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "script workspace could not be persisted",
                true,
            )
        })
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"));
            output
        })
}

#[allow(clippy::needless_pass_by_value)]
fn script_compile_error(error: transmog_script::ScriptCompileError) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, error.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response_assets::ResponseAssetStore;

    fn draft(revision: u64) -> ScriptDraft {
        ScriptDraft {
            id: "user-script".to_owned(),
            revision,
            source: "export function onRequestHead() { return { action: 'continue' }; }".to_owned(),
            language: ScriptLanguage::TypeScript,
            retained_source: None,
            javascript_history: Vec::new(),
            handlers: [ScriptHandler::RequestHead].into_iter().collect(),
            prefilter: ScriptPrefilter::default(),
            capabilities: ScriptCapabilities::default(),
            limits: ScriptLimits::default(),
            priority: 0,
        }
    }

    fn registry(path: Option<PathBuf>) -> ScriptRegistry {
        ScriptRegistry::load(
            path,
            None,
            Arc::new(ResponseAssetStore::load(None).unwrap()),
        )
        .unwrap()
    }

    fn revision(registry: &ScriptRegistry, draft: ScriptDraft) -> ScriptRevision {
        let candidate = registry.validate(draft).unwrap();
        registry
            .candidates
            .lock()
            .unwrap()
            .iter()
            .find(|item| item.id == candidate.candidate_id)
            .unwrap()
            .revision
            .clone()
    }

    #[test]
    fn stored_execution_never_recompiles_typescript_and_detects_modified_artifacts() {
        let registry = registry(None);
        let mut built = revision(&registry, draft(1));
        // Simulate a future compiler that cannot parse the retained authored source.
        built.source = "future TypeScript syntax not understood by this version".to_owned();
        built.manifest.source_hash = source_hash(built.source.as_bytes());
        assert!(executable(&built).is_ok());
        built.javascript.push_str("// changed outside authoring");
        assert!(
            executable(&built)
                .unwrap_err()
                .message
                .contains("integrity")
        );
    }

    #[test]
    fn successful_build_survives_restart_and_failed_authoring_with_retained_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(Some(path.clone()));
        let mut javascript = draft(1);
        javascript.language = ScriptLanguage::JavaScript;
        javascript.retained_source = Some("const bad: number = 'draft';".to_owned());
        let built = revision(&registry, javascript.clone());
        assert_eq!(built.javascript, javascript.source);
        assert_eq!(built.source_map, [] as [u8; 0]);
        let mut broken = javascript;
        broken.source = "export function {".to_owned();
        assert!(registry.validate(broken).is_err());
        let reopened = registry_with_path(path);
        assert_eq!(reopened.status().built[0].javascript, built.javascript);
        assert_eq!(
            reopened.status().built[0].retained_source,
            built.retained_source
        );
        assert!(reopened.status().active.is_empty());
    }

    #[test]
    fn paused_applied_revision_cannot_be_reused_for_new_edits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let applied = revision(&registry, draft(4));
        persist(
            &path,
            &ScriptWorkspace {
                schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                generation: 1,
                paused: vec![applied],
                saved: vec![draft(4)],
                ..ScriptWorkspace::default()
            },
        )
        .unwrap();
        let reopened = registry_with_path(path);
        assert!(
            reopened
                .save(draft(4))
                .unwrap_err()
                .message
                .contains("revision must increase")
        );
        let candidate = reopened.validate(draft(4)).unwrap();
        assert!(
            reopened
                .activate(&candidate.candidate_id)
                .unwrap_err()
                .message
                .contains("revision must increase")
        );
        reopened.save(draft(5)).unwrap();
        assert_eq!(reopened.status().saved[0].revision, 5);
        assert_eq!(reopened.status().paused[0].manifest.revision, 4);
    }

    #[test]
    fn javascript_recovery_history_survives_build_save_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(Some(path.clone()));
        let mut input = draft(1);
        input.javascript_history = vec!["// earlier authored JavaScript".to_owned()];
        registry.validate(input.clone()).unwrap();
        registry.save(input.clone()).unwrap();
        let reopened = registry_with_path(path);
        assert_eq!(
            reopened.status().saved[0].javascript_history,
            input.javascript_history
        );
        assert_eq!(
            reopened.status().built[0].javascript_history,
            input.javascript_history
        );
        let mut over_limit = draft(2);
        over_limit.javascript_history = vec![String::new(); 9];
        assert!(reopened.save(over_limit).is_err());
        assert_eq!(
            reopened.status().saved[0].javascript_history,
            input.javascript_history
        );
    }

    #[test]
    fn unavailable_applied_build_exposes_health_and_failed_retry_preserves_drafts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let active = revision(&registry, draft(1));
        persist(
            &path,
            &ScriptWorkspace {
                schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                generation: 1,
                active: vec![active],
                saved: vec![draft(2)],
                ..ScriptWorkspace::default()
            },
        )
        .unwrap();
        let reopened = registry_with_path(path);
        let before = reopened.status();
        assert!(!before.runtimes[0].available);
        assert_eq!(before.runtimes[0].revision, 1);
        assert!(
            before.runtimes[0]
                .error
                .as_ref()
                .unwrap()
                .contains("unavailable")
        );
        assert!(reopened.retry("user-script", before.generation).is_err());
        assert_eq!(reopened.status().generation, before.generation);
        assert_eq!(reopened.status().saved[0].revision, 2);
        assert_eq!(reopened.status().active[0].manifest.revision, 1);
    }

    #[test]
    fn activation_baseline_round_trips_exact_source_and_authority() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let mut input = draft(7);
        input.prefilter.host = Some("example.test".to_owned());
        input
            .capabilities
            .write_headers
            .insert("user-agent".to_owned());
        let active = revision(&registry, input.clone());
        let mut workspace = ScriptWorkspace {
            schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
            generation: 1,
            active: vec![active.clone()],
            saved: vec![draft(6)],
            ..ScriptWorkspace::default()
        };
        save_activated_draft(&mut workspace, &active).unwrap();
        persist(&path, &workspace).unwrap();
        let reopened = registry_with_path(path);
        let status = reopened.status();
        assert_eq!(status.saved.len(), 1);
        assert_eq!(
            serde_json::to_value(&status.saved[0]).unwrap(),
            serde_json::to_value(input).unwrap()
        );
        assert_eq!(status.active[0].source, status.saved[0].source);
    }

    fn registry_with_path(path: PathBuf) -> ScriptRegistry {
        registry(Some(path))
    }

    #[test]
    fn an_intervening_save_invalidates_activation_and_old_save_requests() {
        let registry = registry(None);
        let candidate = registry.validate(draft(1)).unwrap();
        let mut newer = draft(2);
        newer.source.push_str("\n// newer saved work");
        registry
            .save_checked(newer.clone(), Some(registry.status().generation))
            .unwrap();
        let error = registry.activate(&candidate.candidate_id).unwrap_err();
        assert!(error.message.contains("workspace changed"));
        assert!(
            registry
                .save_checked(draft(3), Some(0))
                .unwrap_err()
                .message
                .contains("workspace changed")
        );
        assert_eq!(registry.status().saved[0].source, newer.source);
        assert!(registry.status().active.is_empty());
    }

    #[test]
    fn validation_rejects_a_save_since_the_editor_read_the_workspace() {
        let registry = registry(None);
        let observed_generation = registry.status().generation;
        let stale = draft(1);
        let mut replacement = stale.clone();
        replacement
            .source
            .push_str("\n// another window saved this revision");
        registry.save(replacement.clone()).unwrap();
        let error = registry
            .validate_checked(stale, Some(observed_generation))
            .unwrap_err();
        assert!(error.message.contains("workspace changed"));
        assert_eq!(registry.status().candidate_count, 0);
        assert_eq!(registry.status().saved[0].source, replacement.source);
    }

    #[test]
    fn concurrent_saves_do_not_overwrite_unrelated_drafts() {
        let registry = registry(None);
        std::thread::scope(|scope| {
            for index in 0..8 {
                let registry = registry.clone();
                scope.spawn(move || {
                    let mut input = draft(1);
                    input.id = format!("script-{index}");
                    registry.save(input).unwrap();
                });
            }
        });
        assert_eq!(registry.status().saved.len(), 8);
        assert_eq!(registry.status().generation, 8);
    }

    #[test]
    fn failed_host_start_preserves_saved_and_active_versions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let active = revision(&registry, draft(1));
        let workspace = ScriptWorkspace {
            schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
            generation: 1,
            active: vec![active.clone()],
            saved: vec![draft(1)],
            ..ScriptWorkspace::default()
        };
        persist(&path, &workspace).unwrap();
        let reopened = registry_with_path(path.clone());
        let mut changed = draft(2);
        changed.source.push_str("\n// unapplied edit");
        let candidate = reopened.validate(changed).unwrap();
        assert!(
            reopened
                .activate(&candidate.candidate_id)
                .unwrap_err()
                .message
                .contains("host is unavailable")
        );
        assert_eq!(reopened.status().active[0].source, active.source);
        assert_eq!(reopened.status().saved[0].revision, 1);
        assert_eq!(
            load_latest(&path).unwrap().generation,
            reopened.status().generation
        );
        assert_eq!(
            reopened.status().candidate_count,
            1,
            "a failed start should remain retryable"
        );
    }

    #[test]
    fn pausing_legacy_active_source_keeps_it_for_reopening() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let active = revision(&registry, draft(4));
        persist(
            &path,
            &ScriptWorkspace {
                schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                generation: 1,
                active: vec![active],
                saved: Vec::new(),
                ..ScriptWorkspace::default()
            },
        )
        .unwrap();
        let reopened = registry_with_path(path.clone());
        reopened.disable("user-script").unwrap();
        let status = registry_with_path(path).status();
        assert!(status.active.is_empty());
        assert_eq!(status.saved[0].revision, 4);
    }

    #[test]
    fn pausing_keeps_a_newer_saved_draft_and_persistence_failure_keeps_active() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts");
        let registry = registry(None);
        let active = revision(&registry, draft(1));
        persist(
            &path,
            &ScriptWorkspace {
                schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                generation: 1,
                active: vec![active],
                saved: vec![draft(2)],
                ..ScriptWorkspace::default()
            },
        )
        .unwrap();
        let reopened = registry_with_path(path.clone());
        std::fs::create_dir(slot_path(&path, 0)).unwrap();
        assert!(reopened.disable("user-script").is_err());
        assert_eq!(reopened.status().active.len(), 1);
        assert_eq!(reopened.status().generation, 1);
        std::fs::remove_dir(slot_path(&path, 0)).unwrap();
        assert_eq!(
            reopened.disable("user-script").unwrap().saved[0].revision,
            2
        );
    }
}
