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
    ScriptRunner, compile_typescript, registration_with_assets, source_hash,
};
use transmog_script_supervisor::{
    IsolationPolicy, MAX_ACTIVE_SCRIPT_HOSTS, ScriptHostConfig, SupervisedScriptRunner,
};

use crate::{AppError, ErrorCategory};

const SCRIPT_WORKSPACE_SCHEMA_VERSION: u32 = 1;
const MAX_SCRIPT_WORKSPACE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_CANDIDATES: usize = 16;
const MAX_HISTORY: usize = 32;
const MAX_SAVED_DRAFTS: usize = 64;

/// Editable source and explicit authority for one script revision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptDraft {
    /// Stable script identity.
    pub id: String,
    /// Monotonic caller-owned revision.
    pub revision: u64,
    /// TypeScript ES module source.
    pub source: String,
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
    /// Exact TypeScript source.
    pub source: String,
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
}

#[derive(Clone)]
struct ActiveScript {
    registration: InterceptorRegistration,
}

struct ActiveSnapshot {
    workspace: ScriptWorkspace,
    scripts: Vec<ActiveScript>,
}

struct CandidateEntry {
    id: String,
    revision: ScriptRevision,
}

/// Crash-safe validate/start/activate registry and registration provider.
#[derive(Clone)]
pub struct ScriptRegistry {
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
        let workspace = path
            .as_deref()
            .map(load_latest)
            .transpose()?
            .flatten()
            .unwrap_or(ScriptWorkspace {
                schema_version: SCRIPT_WORKSPACE_SCHEMA_VERSION,
                ..ScriptWorkspace::default()
            });
        validate_workspace(&workspace)?;
        let registry = Self {
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
        validate_draft_shape(&draft)?;
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
        let revision = ScriptRevision {
            manifest: manifest.clone(),
            source: draft.source,
        };
        compile_typescript(manifest, &revision.source).map_err(script_compile_error)?;
        let encoded = serde_json::to_vec(&revision).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "script candidate serialization failed",
                false,
            )
        })?;
        let id = hex_sha256(&encoded);
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
        candidates.push_back(CandidateEntry { id, revision });
        while candidates.len() > MAX_CANDIDATES {
            candidates.pop_front();
        }
        Ok(result)
    }

    pub(crate) fn save(&self, draft: ScriptDraft) -> Result<ScriptStatus, AppError> {
        validate_draft_shape(&draft)?;
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workspace = active.workspace.clone();
        let scripts = active.scripts.clone();
        drop(active);
        workspace
            .saved
            .retain(|saved| saved.id != draft.id || saved.revision != draft.revision);
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
        let compiled = compile_typescript(revision.manifest, &revision.source)
            .map_err(script_compile_error)?;
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
        let candidate = {
            let mut candidates = self
                .candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let index = candidates
                .iter()
                .position(|candidate| candidate.id == candidate_id)
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::Conflict,
                        "script candidate is unknown or expired",
                        false,
                    )
                })?;
            candidates
                .remove(index)
                .expect("located script candidate exists")
        };
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.workspace.active.iter().any(|revision| {
            revision.manifest.id == candidate.revision.manifest.id
                && revision.manifest.revision >= candidate.revision.manifest.revision
        }) {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "script revision must increase before activation",
                false,
            ));
        }
        let mut workspace = current.workspace.clone();
        drop(current);
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
        workspace
            .active
            .retain(|revision| revision.manifest.id != id);
        workspace.generation = workspace.generation.saturating_add(1);
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
        self.history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(previous);
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

    fn build_snapshot(
        &self,
        workspace: ScriptWorkspace,
        strict: bool,
    ) -> Result<ActiveSnapshot, AppError> {
        let mut scripts = Vec::with_capacity(workspace.active.len());
        for revision in &workspace.active {
            let compiled = compile_typescript(revision.manifest.clone(), &revision.source)
                .map_err(script_compile_error)?;
            let runner: Arc<dyn ScriptRunner> = match self.start_runner(compiled) {
                Ok(runner) => runner,
                Err(error) if !strict => Arc::new(UnavailableRunner(ScriptFailure {
                    category: transmog_script::ScriptFailureCategory::Unavailable,
                    message: error.message,
                    line: None,
                    column: None,
                })),
                Err(error) => return Err(error),
            };
            let registration = registration_with_assets(
                revision.manifest.clone(),
                runner,
                Some(self.assets.clone()),
            )
            .map_err(script_compile_error)?;
            scripts.push(ActiveScript { registration });
        }
        Ok(ActiveSnapshot { workspace, scripts })
    }

    fn start_runner(&self, compiled: CompiledScript) -> Result<Arc<dyn ScriptRunner>, AppError> {
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
        .map(|runner| runner as Arc<dyn ScriptRunner>)
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

struct UnavailableRunner(ScriptFailure);

impl ScriptRunner for UnavailableRunner {
    fn invoke(&self, _invocation: ScriptInvocation) -> BoxScriptFuture<'_> {
        let failure = self.0.clone();
        Box::pin(async move { Err(failure) })
    }
}

fn validate_workspace(workspace: &ScriptWorkspace) -> Result<(), AppError> {
    if workspace.schema_version != SCRIPT_WORKSPACE_SCHEMA_VERSION
        || workspace.active.len() > MAX_ACTIVE_SCRIPT_HOSTS
        || workspace.saved.len() > MAX_SAVED_DRAFTS
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "script workspace is unsupported",
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
    if draft.source.len() > transmog_script::MAX_SCRIPT_SOURCE_BYTES {
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

fn load_latest(path: &Path) -> Result<Option<ScriptWorkspace>, AppError> {
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
        if let Ok(workspace) = serde_json::from_slice::<ScriptWorkspace>(&bytes) {
            if validate_workspace(&workspace).is_ok() {
                workspaces.push(workspace);
            }
        }
    }
    workspaces.sort_by_key(|workspace| workspace.generation);
    Ok(workspaces.pop())
}

fn persist(path: &Path, workspace: &ScriptWorkspace) -> Result<(), AppError> {
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
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn script_compile_error(error: transmog_script::ScriptCompileError) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, error.to_string(), false)
}
