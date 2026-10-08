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

/// A network-free draft matcher evaluation.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoResponseTestInput {
    /// Draft matching conditions, never activated by testing.
    pub matcher: transmog_automation::RuleMatcher,
    /// Method of the synthetic request.
    pub method: String,
    /// Absolute URL of the synthetic request.
    pub url: String,
    /// Optional request headers for testing additional conditions.
    #[serde(default)]
    pub headers: Vec<crate::ComposerHeader>,
    /// Existing rule position, or null for a new highest-priority rule.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// Draft rule's enabled state.
    #[serde(default = "default_autoresponses_enabled")]
    pub enabled: bool,
}

/// One saved regression expectation after evaluating the draft matcher.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleResult {
    /// URL being checked.
    pub url: String,
    /// Whether the actual outcome agrees with its expectation.
    pub passed: bool,
}

/// Draft matcher outcome and actual rule-group eligibility.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResponseTestResult {
    /// Independent match checks and captures from the runtime matcher.
    pub test: transmog_automation::MatchTest,
    /// Whether this draft would serve the synthetic request if saved.
    pub would_serve: bool,
    /// Concise explanation including pause and first-match ordering.
    pub explanation: String,
    /// Earlier matching rule, when applicable.
    pub winning_rule: Option<String>,
    /// Saved examples evaluated against the current draft.
    pub examples: Vec<ExampleResult>,
}

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
    /// Global hook gate; individual rule enabled states remain unchanged.
    #[serde(default = "default_autoresponses_enabled")]
    pub autoresponses_enabled: bool,
}

fn default_autoresponses_enabled() -> bool {
    true
}

impl Default for AutomationRuleSet {
    fn default() -> Self {
        Self {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            generation: 0,
            rules: Vec::new(),
            autoresponses_enabled: true,
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
    /// Whether autoresponse hook registrations are active for new requests.
    pub autoresponses_enabled: bool,
    /// Guaranteed identical-match shadowing in priority order.
    pub diagnostics: Vec<RuleDiagnostic>,
    /// Optional usage from the currently retained Traffic entries.
    pub usage: Vec<RuleUsage>,
    /// Number of validated candidates awaiting explicit activation.
    pub candidate_count: usize,
    /// Number of retained prior in-memory revisions available for rollback.
    pub history_count: usize,
}

/// An autoresponse which is always superseded by an earlier enabled equivalent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleDiagnostic {
    /// Lower-priority rule identifier.
    pub rule_id: String,
    /// Earlier enabled rule with the same matching behavior.
    pub superseded_by: String,
    /// Both rules refer to the same response asset.
    pub duplicate_response: bool,
}

/// Observed usage within retained Traffic, independent of rule revisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleUsage {
    /// Rule identity retained by winning exchanges.
    pub rule_id: String,
    /// Count of retained requests served by this rule.
    pub matches: usize,
    /// Latest retained matching request start time, Unix milliseconds.
    pub last_matched_at: u64,
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
    mutation: Arc<Mutex<()>>,
    active: Arc<RwLock<Arc<ActiveAutomation>>>,
    candidates: Arc<Mutex<VecDeque<CandidateEntry>>>,
    history: Arc<Mutex<VecDeque<Arc<ActiveAutomation>>>>,
    path: Option<Arc<PathBuf>>,
    limits: AutomationLimits,
    assets: Arc<dyn ResponseAssetResolver>,
}

impl AutomationRegistry {
    pub(crate) fn test_match(
        &self,
        input: &AutoResponseTestInput,
    ) -> Result<AutoResponseTestResult, AppError> {
        let headers = input
            .headers
            .iter()
            .map(|header| (header.name.clone(), header.value.clone()))
            .collect::<Vec<_>>();
        let request = transmog_automation::request_for_test(&input.method, &input.url, &headers)
            .map_err(|message| AppError::new(ErrorCategory::InvalidInput, message, false))?;
        let test = transmog_automation::test_matcher(&input.matcher, &request)
            .map_err(|message| AppError::new(ErrorCategory::InvalidInput, message, false))?;
        let status = self.status();
        let mut ordered = status
            .rules
            .iter()
            .filter(|rule| rule.request.response_asset.is_some())
            .collect::<Vec<_>>();
        ordered.sort_by_key(|rule| (rule.priority, &rule.id));
        let position = input
            .rule_id
            .as_ref()
            .and_then(|id| ordered.iter().position(|rule| &rule.id == id))
            .unwrap_or(0);
        let winner = ordered[..position].iter().find(|rule| {
            rule.enabled
                && transmog_automation::test_matcher(&rule.matcher, &request)
                    .is_ok_and(|test| test.matched)
        });
        let would_serve =
            test.matched && input.enabled && status.autoresponses_enabled && winner.is_none();
        let explanation = if !test.matched {
            "This request does not match the draft rule.".to_owned()
        } else if !input.enabled {
            "The request matches, but this rule is disabled.".to_owned()
        } else if !status.autoresponses_enabled {
            "The request matches, but autoresponses are paused.".to_owned()
        } else if let Some(rule) = winner {
            format!(
                "The request matches, but “{}” wins earlier.",
                rule.display_name.as_deref().unwrap_or(&rule.id)
            )
        } else {
            "This rule would serve the saved response.".to_owned()
        };
        let mut examples = Vec::new();
        for example in &input.matcher.examples {
            let request =
                transmog_automation::request_for_test(&example.method, &example.url, &headers)
                    .map_err(|message| {
                        AppError::new(ErrorCategory::InvalidInput, message, false)
                    })?;
            let actual = transmog_automation::test_matcher(&input.matcher, &request)
                .map_err(|message| AppError::new(ErrorCategory::InvalidInput, message, false))?;
            examples.push(ExampleResult {
                url: example.url.clone(),
                passed: actual.matched == example.expected,
            });
        }
        Ok(AutoResponseTestResult {
            test,
            would_serve,
            explanation,
            winning_rule: winner.map(|rule| rule.id.clone()),
            examples,
        })
    }
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
        let compiled = compile_document(&document, limits, assets.clone())?;
        Ok(Self {
            mutation: Arc::new(Mutex::new(())),
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
        let generation = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .generation;
        if document.generation != generation {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "Rules changed while you were editing. Refresh before saving.",
                true,
            ));
        }
        document.generation = generation.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "automation generation is exhausted",
                false,
            )
        })?;
        let compiled = compile_document(&document, self.limits, self.assets.clone())?;
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
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        if candidate.document.generation
            != self
                .active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .document
                .generation
                .saturating_add(1)
        {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "Rules changed before activation. Refresh before saving.",
                true,
            ));
        }
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
            autoresponses_enabled: active.document.autoresponses_enabled,
            diagnostics: rule_diagnostics(&active.document.rules),
            usage: Vec::new(),
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

    /// Changes only the global autoresponse gate in one atomic persisted generation.
    ///
    /// # Errors
    /// Returns a stale-generation, validation or persistence failure.
    pub fn set_autoresponses_enabled(
        &self,
        enabled: bool,
        generation: u64,
    ) -> Result<AutomationStatus, AppError> {
        let mut document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        if document.generation != generation {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "Autoresponse state changed. Refresh and try again.",
                true,
            ));
        }
        if document.autoresponses_enabled == enabled {
            return Ok(self.status());
        }
        document.autoresponses_enabled = enabled;
        let candidate = self.validate(document)?;
        self.activate(&candidate.candidate_id)
    }
}

fn compile_document(
    document: &AutomationRuleSet,
    limits: AutomationLimits,
    assets: Arc<dyn ResponseAssetResolver>,
) -> Result<CompiledAutomation, AppError> {
    let compiled = compile_with_assets(document.rules.clone(), limits, Some(assets.clone()))
        .map_err(compile_error)?;
    if document.autoresponses_enabled {
        return Ok(compiled);
    }
    compile_with_assets(
        document
            .rules
            .iter()
            .filter(|rule| rule.request.response_asset.is_none())
            .cloned()
            .collect(),
        limits,
        Some(assets),
    )
    .map_err(compile_error)
}

fn rule_diagnostics(rules: &[Rule]) -> Vec<RuleDiagnostic> {
    let mut ordered = rules
        .iter()
        .filter(|rule| rule.request.response_asset.is_some())
        .collect::<Vec<_>>();
    ordered.sort_by_key(|rule| (rule.priority, &rule.id));
    let mut earlier = std::collections::HashMap::<String, &Rule>::new();
    let mut diagnostics = Vec::new();
    for rule in ordered {
        let Ok(key) = transmog_automation::matcher_key(&rule.matcher) else {
            continue;
        };
        if let Some(winner) = earlier.get(&key) {
            diagnostics.push(RuleDiagnostic {
                rule_id: rule.id.clone(),
                superseded_by: winner.id.clone(),
                duplicate_response: rule.request.response_asset == winner.request.response_asset,
            });
        } else if rule.enabled {
            earlier.insert(key, rule);
        }
    }
    diagnostics
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
            generation: 0,
            rules: vec![Rule {
                id: "conditional-ua".to_owned(),
                display_name: None,
                enabled: true,
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
            autoresponses_enabled: true,
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
        let mut second_document = document("two");
        second_document.generation = 1;
        let second = registry.validate(second_document).unwrap();
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

    #[test]
    fn pause_preserves_rule_state_snapshots_and_persistence() {
        let root = tempfile::tempdir().unwrap();
        let assets =
            crate::response_assets::ResponseAssetStore::load(Some(root.path().join("assets")))
                .unwrap();
        let asset = assets
            .create_authored(crate::AuthoredResponseAsset {
                id: "saved".into(),
                revision: 1,
                status: 200,
                headers: transmog_core::HeaderBlock::new(),
                body: b"saved".to_vec(),
                media_type: None,
            })
            .unwrap();
        let path = root.path().join("rules");
        let registry =
            AutomationRegistry::load(Some(path.clone()), Arc::new(assets.clone())).unwrap();
        let mut document = document("agent");
        let mut response = document.rules[0].clone();
        response.id = "response".into();
        response.priority = -10;
        response.request = RequestActions {
            response_asset: Some(asset.asset_ref()),
            ..RequestActions::default()
        };
        document.rules.push(response.clone());
        let mut duplicate = response;
        duplicate.id = "duplicate".into();
        duplicate.priority = -9;
        document.rules.push(duplicate);
        let candidate = registry.validate(document).unwrap();
        let initial = registry.activate(&candidate.candidate_id).unwrap();
        assert_eq!(initial.diagnostics[0].superseded_by, "response");
        assert!(initial.diagnostics[0].duplicate_response);
        let snapshot = registry.active.read().unwrap().clone();
        assert_eq!(snapshot.compiled.registrations().len(), 3);
        let paused = registry
            .set_autoresponses_enabled(false, initial.generation)
            .unwrap();
        assert_eq!(paused.rules, initial.rules);
        assert!(!paused.autoresponses_enabled);
        assert_eq!(
            registry
                .active
                .read()
                .unwrap()
                .compiled
                .registrations()
                .len(),
            1
        );
        assert_eq!(snapshot.compiled.registrations().len(), 3);
        assert!(
            registry
                .set_autoresponses_enabled(true, initial.generation)
                .is_err()
        );
        let loaded = AutomationRegistry::load(Some(path), Arc::new(assets)).unwrap();
        assert!(!loaded.status().autoresponses_enabled);
        assert_eq!(loaded.status().rules, initial.rules);
        let resumed = loaded
            .set_autoresponses_enabled(true, paused.generation)
            .unwrap();
        assert_eq!(resumed.rules, initial.rules);
        let old: AutomationRuleSet =
            serde_json::from_str(r#"{"schemaVersion":1,"generation":0,"rules":[]}"#).unwrap();
        assert!(old.autoresponses_enabled);
    }
}
