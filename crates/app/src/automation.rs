use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use transmog_automation::{
    AutomationLimits, CompiledAutomation, ResponseAssetResolver, Rule, compile_with_assets,
};
use transmog_core::intercept::{
    ExchangeMetadata, HookInitError, InterceptorRegistration, InterceptorRegistrationProvider,
};

use crate::{AppError, ErrorCategory};

const AUTOMATION_SCHEMA_VERSION: u32 = 1;
const MAX_AUTOMATION_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CANDIDATES: usize = 16;
const MAX_HISTORY: usize = 32;

/// Persisted built-in automation rules.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRuleSet {
    /// Workspace schema revision. The initial supported value is one.
    pub schema_version: u32,
    /// Monotonic activation generation.
    pub generation: u64,
    /// Ordered native automation sources.
    pub rules: Vec<Rule>,
}

impl Default for AutomationRuleSet {
    fn default() -> Self {
        Self {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            generation: 0,
            rules: Vec::new(),
        }
    }
}

/// Result of validation without changing active traffic behavior.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCandidate {
    /// Content-addressed candidate token required for activation.
    pub candidate_id: String,
    /// Number of rules in the validated candidate.
    pub rule_count: usize,
    /// Number of hook registrations the rules produce.
    pub registration_count: usize,
}

/// Current atomic automation registry state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationStatus {
    /// Active activation generation.
    pub generation: u64,
    /// Active rules, including stable IDs and revisions.
    pub rules: Vec<Rule>,
    /// Number of validated candidates awaiting explicit activation.
    pub candidate_count: usize,
    /// Number of retained prior in-memory revisions available for rollback.
    pub history_count: usize,
}

#[derive(Clone, Debug)]
struct ActiveAutomation {
    document: AutomationRuleSet,
    compiled: CompiledAutomation,
}

#[derive(Clone, Debug)]
struct CandidateEntry {
    id: String,
    document: AutomationRuleSet,
    compiled: CompiledAutomation,
}

/// Thread-safe validate-then-activate registry and core registration provider.
#[derive(Clone)]
pub struct AutomationRegistry {
    active: Arc<RwLock<Arc<ActiveAutomation>>>,
    candidates: Arc<Mutex<VecDeque<CandidateEntry>>>,
    history: Arc<Mutex<VecDeque<Arc<ActiveAutomation>>>>,
    path: Option<Arc<PathBuf>>,
    limits: AutomationLimits,
    assets: Arc<dyn ResponseAssetResolver>,
}

impl AutomationRegistry {
    pub(crate) fn load(
        path: Option<PathBuf>,
        assets: Arc<dyn ResponseAssetResolver>,
    ) -> Result<Self, AppError> {
        let limits = AutomationLimits::default();
        let document = path
            .as_deref()
            .and_then(|path| load_latest(path, &assets))
            .unwrap_or_default();
        validate_document(&document)?;
        let compiled = compile_with_assets(document.rules.clone(), limits, Some(assets.clone()))
            .map_err(compile_error)?;
        Ok(Self {
            active: Arc::new(RwLock::new(Arc::new(ActiveAutomation {
                document,
                compiled,
            }))),
            candidates: Arc::new(Mutex::new(VecDeque::new())),
            history: Arc::new(Mutex::new(VecDeque::new())),
            path: path.map(Arc::new),
            limits,
            assets,
        })
    }

    /// Validates and compiles a candidate without changing active exchanges.
    ///
    /// # Errors
    /// Returns a bounded schema, syntax, conflict, or resource-limit error.
    pub fn validate(
        &self,
        mut document: AutomationRuleSet,
    ) -> Result<AutomationCandidate, AppError> {
        validate_document(&document)?;
        document.generation = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .generation
            .checked_add(1)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Limit,
                    "automation generation is exhausted",
                    false,
                )
            })?;
        let compiled = compile_with_assets(
            document.rules.clone(),
            self.limits,
            Some(self.assets.clone()),
        )
        .map_err(compile_error)?;
        let encoded = serde_json::to_vec(&document).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "automation candidate serialization failed",
                false,
            )
        })?;
        if encoded.len() as u64 > MAX_AUTOMATION_FILE_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "automation workspace exceeds its four MiB limit",
                false,
            ));
        }
        let id = hex_sha256(&encoded);
        let result = AutomationCandidate {
            candidate_id: id.clone(),
            rule_count: document.rules.len(),
            registration_count: compiled.registrations().len(),
        };
        let mut candidates = self
            .candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidates.retain(|candidate| candidate.id != id);
        candidates.push_back(CandidateEntry {
            id,
            document,
            compiled,
        });
        while candidates.len() > MAX_CANDIDATES {
            candidates.pop_front();
        }
        Ok(result)
    }

    /// Atomically activates a previously validated candidate for new exchanges.
    ///
    /// Existing exchanges retain the registrations captured at admission.
    ///
    /// # Errors
    /// Returns a conflict for an unknown/stale token or a durable-write error.
    pub fn activate(&self, candidate_id: &str) -> Result<AutomationStatus, AppError> {
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
                        "automation candidate is unknown or expired",
                        false,
                    )
                })?;
            candidates.remove(index).expect("located candidate exists")
        };
        if let Some(path) = self.path.as_deref() {
            persist(path, &candidate.document)?;
        }
        let replacement = Arc::new(ActiveAutomation {
            document: candidate.document,
            compiled: candidate.compiled,
        });
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

    /// Returns active rules and finite candidate/history counts.
    pub fn status(&self) -> AutomationStatus {
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        AutomationStatus {
            generation: active.document.generation,
            rules: active.document.rules.clone(),
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
}

impl InterceptorRegistrationProvider for AutomationRegistry {
    fn registrations(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Vec<InterceptorRegistration>, HookInitError> {
        Ok(self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .compiled
            .registrations())
    }
}

fn validate_document(document: &AutomationRuleSet) -> Result<(), AppError> {
    if document.schema_version != AUTOMATION_SCHEMA_VERSION {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "automation workspace schema is unsupported",
            false,
        ));
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn compile_error(error: transmog_automation::CompileError) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, error.to_string(), false)
}

fn slot_path(path: &Path, slot: u64) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".{slot}.json"));
    PathBuf::from(value)
}

fn load_latest(path: &Path, assets: &Arc<dyn ResponseAssetResolver>) -> Option<AutomationRuleSet> {
    let mut candidates = Vec::new();
    for slot in 0..=1 {
        let candidate = slot_path(path, slot);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.len() > MAX_AUTOMATION_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(candidate) else {
            continue;
        };
        if let Ok(document) = serde_json::from_slice::<AutomationRuleSet>(&bytes)
            && validate_document(&document).is_ok()
            && compile_with_assets(
                document.rules.clone(),
                AutomationLimits::default(),
                Some(Arc::clone(assets)),
            )
            .is_ok()
        {
            candidates.push(document);
        }
    }
    candidates.sort_by_key(|candidate| candidate.generation);
    candidates.pop()
}

fn persist(path: &Path, document: &AutomationRuleSet) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(document).map_err(|_| {
        AppError::new(
            ErrorCategory::Internal,
            "automation persistence serialization failed",
            false,
        )
    })?;
    if bytes.len() as u64 > MAX_AUTOMATION_FILE_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "automation workspace exceeds its four MiB limit",
            false,
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "automation workspace directory is unavailable",
                true,
            )
        })?;
    }
    let destination = slot_path(path, document.generation % 2);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "automation workspace cannot be opened",
                true,
            )
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "automation workspace could not be persisted",
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

#[cfg(test)]
mod tests {
    use transmog_automation::{HeaderOperation, RequestActions, ResponseActions, RuleMatcher};

    use super::*;

    fn document(value: &str) -> AutomationRuleSet {
        AutomationRuleSet {
            schema_version: 1,
            generation: 999,
            rules: vec![Rule {
                id: "conditional-ua".to_owned(),
                revision: 7,
                priority: 10,
                matcher: RuleMatcher {
                    host: Some("example.test".to_owned()),
                    path_prefix: Some("/api".to_owned()),
                    ..RuleMatcher::default()
                },
                request: RequestActions {
                    headers: vec![HeaderOperation::set("user-agent", value).unwrap()],
                    ..RequestActions::default()
                },
                response: ResponseActions::default(),
            }],
        }
    }

    #[test]
    fn validation_is_non_mutating_and_activation_is_explicit() {
        let assets = crate::response_assets::ResponseAssetStore::load(None).unwrap();
        let registry = AutomationRegistry::load(None, Arc::new(assets)).unwrap();
        let candidate = registry.validate(document("Transmog/1")).unwrap();
        assert_eq!(registry.status().generation, 0);
        assert_eq!(registry.status().candidate_count, 1);
        let active = registry.activate(&candidate.candidate_id).unwrap();
        assert_eq!(active.generation, 1);
        assert_eq!(active.rules[0].revision, 7);
        assert_eq!(active.history_count, 1);
    }

    #[test]
    fn persisted_generations_recover_from_a_corrupt_newest_slot() {
        let root = std::env::temp_dir().join(format!(
            "transmog-automation-{}",
            u128::from_le_bytes(rand_bytes())
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rules");
        let assets = crate::response_assets::ResponseAssetStore::load(None).unwrap();
        let registry =
            AutomationRegistry::load(Some(path.clone()), Arc::new(assets.clone())).unwrap();
        let first = registry.validate(document("one")).unwrap();
        registry.activate(&first.candidate_id).unwrap();
        let second = registry.validate(document("two")).unwrap();
        registry.activate(&second.candidate_id).unwrap();
        std::fs::write(slot_path(&path, 0), b"not-json").unwrap();

        let recovered = AutomationRegistry::load(Some(path), Arc::new(assets)).unwrap();
        assert_eq!(recovered.status().generation, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    fn rand_bytes() -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).unwrap();
        bytes
    }
}
