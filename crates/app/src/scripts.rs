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
        let workspace = path
            .as_deref()
            .and_then(load_latest)
            .unwrap_or(ScriptWorkspace {
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
        let generation = {
            let active = self
                .active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            check_generation(&active.workspace, expected_generation)?;
            active.workspace.generation
        };
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
        if workspace.active.iter().any(|active| {
            active.manifest.id == draft.id && active.manifest.revision >= draft.revision
        }) {
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
        registry.save_checked(newer.clone(), Some(0)).unwrap();
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
        assert_eq!(load_latest(&path).unwrap().generation, 1);
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
