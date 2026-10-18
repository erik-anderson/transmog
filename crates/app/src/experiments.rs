//! Revisioned experiment definitions share the automation transaction and persistence boundary.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use transmog_automation::{Rule, compile_with_assets};

use super::{AutomationRegistry, AutomationRuleSet, AutomationStatus, compile_error};
use crate::{AppError, ErrorCategory};

/// An exact immutable saved variant, also used by preview selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentSelection {
    /// Stable experiment ID.
    pub experiment_id: String,
    /// Stable variant ID within the experiment.
    pub variant_id: String,
    /// Exact saved revision, never a mutable alias for latest.
    pub revision: u64,
}

/// An immutable ordered rule snapshot. Linked assets still read current files on a new run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentRevision {
    /// Monotonic revision within the variant.
    pub revision: u64,
    /// Ordered complete definitions with exact asset revision references.
    pub rules: Vec<Rule>,
}

/// A named alternative with bounded persistent revision history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentVariant {
    /// Stable ID, independent of display name.
    pub id: String,
    /// User-facing label.
    pub name: String,
    /// Archived variants remain inspectable and can be restored.
    pub archived: bool,
    /// Oldest to newest; at most 32 immutable revisions.
    pub revisions: Vec<ExperimentRevision>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivationUndo {
    selection: Option<ExperimentSelection>,
    generation: u64,
}

/// A lightweight collection whose live participation is explicit and scoped.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Experiment {
    /// Stable experiment ID.
    pub id: String,
    /// User-facing hypothesis or task.
    pub name: String,
    /// Gates live participation without rewriting rules or revisions.
    pub paused: bool,
    /// Archived experiments remain inspectable and can be restored.
    pub archived: bool,
    /// Exact active revision; none means Baseline (no owned rules).
    pub active: Option<ExperimentSelection>,
    /// Named alternatives, including the immutable empty Baseline.
    pub variants: Vec<ExperimentVariant>,
    /// Chosen exact captured page; metadata only, independent of variant history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_page: Option<crate::ExperimentPreviewPage>,
    /// Independent compare-and-set revision for page choices.
    #[serde(default)]
    pub preview_page_revision: u64,
    undo: Option<ActivationUndo>,
}

impl Experiment {
    pub(super) fn imported(id: String, name: String, variants: Vec<ExperimentVariant>) -> Self {
        Self {
            id,
            name,
            paused: true,
            archived: false,
            active: None,
            variants,
            preview_page: None,
            preview_page_revision: 0,
            undo: None,
        }
    }
    pub(crate) fn owns(&self, id: &str) -> bool {
        self.variants
            .iter()
            .flat_map(|v| &v.revisions)
            .flat_map(|r| &r.rules)
            .any(|r| r.id == id)
    }

    /// Returns an exact saved revision, including archived history.
    pub fn revision(&self, selection: &ExperimentSelection) -> Option<&ExperimentRevision> {
        (self.id == selection.experiment_id).then_some(())?;
        self.variants
            .iter()
            .find(|v| v.id == selection.variant_id)?
            .revisions
            .iter()
            .find(|r| r.revision == selection.revision)
    }
}

/// One generation-checked edit. Saving definitions never activates them.
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExperimentChange {
    /// Create a collection with empty Baseline and one authored variant.
    Create {
        /// New experiment ID.
        id: String,
        /// Hypothesis name.
        name: String,
        /// New first variant ID (cannot be baseline).
        variant_id: String,
        /// First alternative's name.
        variant_name: String,
        /// Ordered copied rule definitions; ownership transfers to the new experiment.
        rules: Vec<Rule>,
    },
    /// Append a saved revision, checking the previous revision to protect drafts.
    Save {
        /// Owner.
        id: String,
        /// Existing variant.
        variant_id: String,
        /// Revision edited by the caller.
        expected_revision: u64,
        /// Complete ordered replacement, with immutable asset references.
        rules: Vec<Rule>,
    },
    /// Duplicate any saved revision into a new named variant.
    Duplicate {
        /// Exact source.
        selection: ExperimentSelection,
        /// New stable variant ID.
        variant_id: String,
        /// Display name.
        name: String,
    },
    /// Change only an experiment or variant display name.
    Rename {
        /// Owner.
        id: String,
        /// None renames the experiment.
        variant_id: Option<String>,
        /// Display name.
        name: String,
    },
    /// Archive or restore; activation must be cleared before archiving active content.
    Archive {
        /// Owner.
        id: String,
        /// None archives the experiment.
        variant_id: Option<String>,
        /// False restores the same identities and revisions (Undo archive).
        archived: bool,
    },
    /// Gate the active revision without modifying any authored rule.
    Pause {
        /// Owner.
        id: String,
        /// Desired participation gate.
        paused: bool,
    },
    /// Atomically replace only the experiment's live subset after overlap review.
    Activate {
        /// Exact target revision, including Baseline.
        selection: ExperimentSelection,
        /// Explicit acknowledgement of conservative overlap diagnostics.
        acknowledge_overlaps: bool,
    },
    /// Restore only the last activation, rejecting intervening generations.
    UndoActivation {
        /// Owner.
        id: String,
    },
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn conflict(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, true)
}
fn name_valid(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}
fn id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(super) fn validate_owned_rules(
    current: &AutomationRuleSet,
    rules: &[Rule],
) -> Result<(), AppError> {
    let owned = |rule: &&Rule| current.experiments.iter().any(|e| e.owns(&rule.id));
    let before = current.rules.iter().filter(owned).collect::<Vec<_>>();
    let after = rules.iter().filter(owned).collect::<Vec<_>>();
    if before != after {
        return Err(conflict(
            "Response set rules changed outside their variant. Edit the variant or use its activation controls.",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // One pass checks bounded history and live projection consistency.
pub(super) fn validate_experiments(document: &AutomationRuleSet) -> Result<(), AppError> {
    if document.experiments.len() > 32 {
        return Err(invalid("At most 32 response sets can be retained."));
    }
    let mut experiments = HashSet::new();
    let mut owners = std::collections::HashMap::new();
    let mut identities = std::collections::HashMap::new();
    for experiment in &document.experiments {
        if experiment
            .preview_page
            .as_ref()
            .is_some_and(|page| !page.valid())
        {
            return Err(invalid("Response set preview page metadata is invalid."));
        }
        if !id_valid(&experiment.id)
            || !name_valid(&experiment.name)
            || !experiments.insert(&experiment.id)
            || experiment.variants.len() > 16
        {
            return Err(invalid(
                "Response set identity, name or variant limit is invalid.",
            ));
        }
        let mut variants = HashSet::new();
        for variant in &experiment.variants {
            if !id_valid(&variant.id)
                || !name_valid(&variant.name)
                || !variants.insert(&variant.id)
                || variant.revisions.is_empty()
                || variant.revisions.len() > 32
            {
                return Err(invalid(
                    "Variant identity, name or history limit is invalid (16 variants, 32 revisions).",
                ));
            }
            for (index, revision) in variant.revisions.iter().enumerate() {
                if revision.revision != index as u64 + 1
                    || revision.rules.len() > 64
                    || revision
                        .rules
                        .windows(2)
                        .any(|pair| pair[0].priority >= pair[1].priority)
                    || (variant.id == "baseline"
                        && (!revision.rules.is_empty() || index != 0 || variant.archived))
                {
                    return Err(invalid(
                        "Variant revisions require increasing rule priorities and at most 64 rules. Baseline always has no rules.",
                    ));
                }
                let mut ids = HashSet::new();
                for rule in &revision.rules {
                    if identities
                        .insert((&rule.id, rule.revision), rule)
                        .is_some_and(|previous| previous != rule)
                    {
                        return Err(invalid(
                            "A saved rule identity and revision cannot describe different definitions.",
                        ));
                    }
                    if !ids.insert(&rule.id) || rule.request.response_asset.is_none() {
                        return Err(invalid(
                            "A response variant requires unique saved response rules.",
                        ));
                    }
                    if owners
                        .insert(&rule.id, &experiment.id)
                        .is_some_and(|owner| owner != &experiment.id)
                    {
                        return Err(invalid(
                            "A rule cannot belong to more than one response set.",
                        ));
                    }
                }
            }
        }
        if !variants.contains(&"baseline".to_owned()) {
            return Err(invalid("A response set must retain its empty Baseline."));
        }
        let expected = experiment
            .active
            .as_ref()
            .map(|s| {
                experiment
                    .revision(s)
                    .ok_or_else(|| invalid("Active variant revision is missing."))
            })
            .transpose()?
            .map_or(&[][..], |r| r.rules.as_slice());
        if experiment.archived && !expected.is_empty() {
            return Err(invalid(
                "Activate Baseline before archiving this response set.",
            ));
        }
        let actual = document
            .rules
            .iter()
            .filter(|r| experiment.owns(&r.id))
            .collect::<Vec<_>>();
        if actual != expected.iter().collect::<Vec<_>>() {
            return Err(invalid(
                "Live response set rules must match its exact active revision.",
            ));
        }
    }
    Ok(())
}

impl AutomationRegistry {
    pub(crate) fn set_experiment_preview_page(
        &self,
        id: &str,
        revision: u64,
        page: crate::ExperimentPreviewPage,
    ) -> Result<(), AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut document = current.document.clone();
        let experiment = find_mut(&mut document, id)?;
        if experiment.preview_page_revision != revision {
            return Err(conflict(
                "The page association changed. Review its current page before choosing again.",
            ));
        }
        experiment.preview_page = Some(page);
        experiment.preview_page_revision = revision
            .checked_add(1)
            .ok_or_else(|| invalid("Page association revision exhausted."))?;
        super::validate_document(&document)?;
        super::persistence_bytes(&document)?;
        if let Some(path) = self.path.as_deref() {
            super::persist(path, &document)?;
        }
        // Preserve the exact compiled graph, candidates, history, generation and Undo.
        let mut replacement = (*current).clone();
        replacement.document = document;
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::sync::Arc::new(replacement);
        Ok(())
    }

    /// Reports conservative overlaps before activating an exact saved revision.
    pub fn experiment_overlaps(
        &self,
        selection: &ExperimentSelection,
    ) -> Result<Vec<String>, AppError> {
        let document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        overlaps(&document, selection)
    }

    /// Saves and publishes a single generation-checked experiment transaction.
    #[allow(clippy::too_many_lines)] // One checked transaction for the closed edit vocabulary.
    pub fn change_experiment(
        &self,
        generation: u64,
        change: ExperimentChange,
    ) -> Result<AutomationStatus, AppError> {
        let mut document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        if document.generation != generation {
            return Err(conflict(
                "Automation changed. Refresh before retrying; your draft has been preserved.",
            ));
        }
        match change {
            ExperimentChange::Create {
                id,
                name,
                variant_id,
                variant_name,
                rules,
            } => {
                if document.experiments.iter().any(|e| e.id == id) || variant_id == "baseline" {
                    return Err(invalid(
                        "Choose unused response set and variant identities.",
                    ));
                }
                check_new_rule_ids(&document, &rules, None)?;
                document.experiments.push(Experiment {
                    preview_page: None,
                    preview_page_revision: 0,
                    id,
                    name,
                    paused: false,
                    archived: false,
                    active: None,
                    undo: None,
                    variants: vec![
                        ExperimentVariant {
                            id: "baseline".into(),
                            name: "Baseline".into(),
                            archived: false,
                            revisions: vec![ExperimentRevision {
                                revision: 1,
                                rules: Vec::new(),
                            }],
                        },
                        ExperimentVariant {
                            id: variant_id,
                            name: variant_name,
                            archived: false,
                            revisions: vec![ExperimentRevision { revision: 1, rules }],
                        },
                    ],
                });
            }
            ExperimentChange::Save {
                id,
                variant_id,
                expected_revision,
                rules,
            } => {
                check_new_rule_ids(&document, &rules, Some((&id, &variant_id)))?;
                let experiment = find_mut(&mut document, &id)?;
                let variant = experiment
                    .variants
                    .iter_mut()
                    .find(|v| v.id == variant_id)
                    .ok_or_else(|| invalid("Variant is unavailable."))?;
                if variant.id == "baseline"
                    || variant.archived
                    || experiment.archived
                    || variant.revisions.last().map(|r| r.revision) != Some(expected_revision)
                {
                    return Err(conflict(
                        "Variant changed or is archived. Reopen its latest revision before saving.",
                    ));
                }
                variant.revisions.push(ExperimentRevision {
                    revision: expected_revision
                        .checked_add(1)
                        .ok_or_else(|| invalid("Variant revision exhausted."))?,
                    rules,
                });
            }
            ExperimentChange::Duplicate {
                selection,
                variant_id,
                name,
            } => {
                let experiment = find_mut(&mut document, &selection.experiment_id)?;
                let mut rules = experiment
                    .revision(&selection)
                    .ok_or_else(|| invalid("Source revision is unavailable."))?
                    .rules
                    .clone();
                if variant_id == "baseline"
                    || experiment.variants.iter().any(|v| v.id == variant_id)
                {
                    return Err(invalid("Choose an unused variant identity."));
                }
                for (index, rule) in rules.iter_mut().enumerate() {
                    rule.id = format!("variant-{variant_id}-{index}");
                    rule.revision = 1;
                }
                experiment.variants.push(ExperimentVariant {
                    id: variant_id,
                    name,
                    archived: false,
                    revisions: vec![ExperimentRevision { revision: 1, rules }],
                });
            }
            ExperimentChange::Rename {
                id,
                variant_id,
                name,
            } => {
                let experiment = find_mut(&mut document, &id)?;
                if let Some(id) = variant_id {
                    let variant = experiment
                        .variants
                        .iter_mut()
                        .find(|v| v.id == id)
                        .ok_or_else(|| invalid("Variant unavailable."))?;
                    if id == "baseline" {
                        return Err(invalid("Baseline keeps its name."));
                    }
                    variant.name = name;
                } else {
                    experiment.name = name;
                }
            }
            ExperimentChange::Archive {
                id,
                variant_id,
                archived,
            } => {
                let experiment = find_mut(&mut document, &id)?;
                if archived
                    && experiment.active.as_ref().is_some_and(|s| {
                        s.variant_id != "baseline"
                            && variant_id.as_ref().is_none_or(|id| id == &s.variant_id)
                    })
                {
                    return Err(conflict(
                        "Activate Baseline or another variant before archiving active content.",
                    ));
                }
                if let Some(id) = variant_id {
                    let variant = experiment
                        .variants
                        .iter_mut()
                        .find(|v| v.id == id)
                        .ok_or_else(|| invalid("Variant unavailable."))?;
                    if id == "baseline" {
                        return Err(invalid("Baseline cannot be archived."));
                    }
                    variant.archived = archived;
                } else {
                    experiment.archived = archived;
                }
            }
            ExperimentChange::Pause { id, paused } => {
                find_mut(&mut document, &id)?.paused = paused;
            }
            ExperimentChange::Activate {
                selection,
                acknowledge_overlaps,
            } => {
                let warnings = overlaps(&document, &selection)?;
                if !warnings.is_empty() && !acknowledge_overlaps {
                    return Err(conflict(&format!(
                        "Review overlapping live rules before activation: {}",
                        warnings.join("; ")
                    )));
                }
                let experiment = find_mut(&mut document, &selection.experiment_id)?;
                let variant = experiment
                    .variants
                    .iter()
                    .find(|v| v.id == selection.variant_id)
                    .ok_or_else(|| invalid("Variant unavailable."))?;
                if experiment.archived || variant.archived {
                    return Err(invalid(
                        "Restore this response set and variant before activation.",
                    ));
                }
                experiment.undo = Some(ActivationUndo {
                    selection: experiment.active.clone(),
                    generation: generation
                        .checked_add(1)
                        .ok_or_else(|| invalid("Automation generation exhausted."))?,
                });
                project(&mut document, &selection)?;
            }
            ExperimentChange::UndoActivation { id } => {
                let experiment = find_mut(&mut document, &id)?;
                let undo = experiment
                    .undo
                    .clone()
                    .ok_or_else(|| conflict("No activation is available to undo."))?;
                if undo.generation != generation {
                    return Err(conflict(
                        "Automation changed after activation. Current work was preserved; select and activate a saved revision explicitly.",
                    ));
                }
                let selection = undo.selection.unwrap_or(ExperimentSelection {
                    experiment_id: id,
                    variant_id: "baseline".into(),
                    revision: 1,
                });
                project(&mut document, &selection)?;
                find_mut(&mut document, &selection.experiment_id)?.undo = None;
            }
        }
        // Validate every retained snapshot, including disabled/archived alternatives, before persistence.
        super::validate_document(&document)?;
        for experiment in &document.experiments {
            for revision in experiment.variants.iter().flat_map(|v| &v.revisions) {
                compile_with_assets(
                    revision.rules.clone(),
                    self.limits,
                    Some(self.assets.clone()),
                )
                .map_err(compile_error)?;
            }
        }
        let candidate = self.validate_candidate(document)?;
        self.activate(&candidate.candidate_id)
    }
}

fn find_mut<'a>(
    document: &'a mut AutomationRuleSet,
    id: &str,
) -> Result<&'a mut Experiment, AppError> {
    document
        .experiments
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| invalid("Response set is unavailable. Refresh the collection."))
}

fn check_new_rule_ids(
    document: &AutomationRuleSet,
    rules: &[Rule],
    owner: Option<(&str, &str)>,
) -> Result<(), AppError> {
    for rule in rules {
        let existing_owner = document.experiments.iter().find(|e| e.owns(&rule.id));
        let allowed = owner.is_some_and(|(eid, vid)| {
            existing_owner.is_some_and(|e| {
                e.id == eid
                    && e.variants.iter().any(|v| {
                        v.id == vid
                            && v.revisions
                                .iter()
                                .any(|r| r.rules.iter().any(|r| r.id == rule.id))
                    })
            })
        });
        if !allowed && (existing_owner.is_some() || document.rules.iter().any(|r| r.id == rule.id))
        {
            return Err(invalid(
                "Copy rules with new identities before adding them to a response set.",
            ));
        }
        if let Some((eid, vid)) = owner {
            let previous = document
                .experiments
                .iter()
                .find(|e| e.id == eid)
                .and_then(|e| e.variants.iter().find(|v| v.id == vid))
                .and_then(|v| v.revisions.last())
                .and_then(|r| r.rules.iter().find(|r| r.id == rule.id));
            if previous.is_some_and(|old| old != rule && rule.revision <= old.revision) {
                return Err(conflict(
                    "Changed variant rules require a new rule revision.",
                ));
            }
        }
    }
    Ok(())
}

fn project(
    document: &mut AutomationRuleSet,
    selection: &ExperimentSelection,
) -> Result<(), AppError> {
    let experiment = document
        .experiments
        .iter()
        .find(|e| e.id == selection.experiment_id)
        .ok_or_else(|| invalid("Response set unavailable."))?;
    let rules = experiment
        .revision(selection)
        .ok_or_else(|| invalid("Saved revision unavailable."))?
        .rules
        .clone();
    document.rules.retain(|r| !experiment.owns(&r.id));
    document.rules.extend(rules);
    find_mut(document, &selection.experiment_id)?.active = Some(selection.clone());
    Ok(())
}

fn overlaps(
    document: &AutomationRuleSet,
    selection: &ExperimentSelection,
) -> Result<Vec<String>, AppError> {
    let experiment = document
        .experiments
        .iter()
        .find(|e| e.id == selection.experiment_id)
        .ok_or_else(|| invalid("Response set unavailable."))?;
    let revision = experiment
        .revision(selection)
        .ok_or_else(|| invalid("Saved revision unavailable."))?;
    let mut warnings = Vec::new();
    for rule in revision.rules.iter().filter(|r| r.enabled) {
        for other in document.rules.iter().filter(|r| {
            r.enabled
                && !experiment.owns(&r.id)
                && !document
                    .experiments
                    .iter()
                    .any(|e| e.paused && e.owns(&r.id))
        }) {
            if rule.matcher.may_overlap_request(&other.matcher) && warnings.len() < 64 {
                let first = if (other.priority, &other.id) < (rule.priority, &rule.id) {
                    "earlier"
                } else {
                    "later"
                };
                warnings.push(format!(
                    "{} may overlap {} ({first} live rule)",
                    rule.display_name.as_deref().unwrap_or(&rule.id),
                    other.display_name.as_deref().unwrap_or(&other.id)
                ));
            }
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use transmog_automation::{RequestActions, ResponseActions, RuleMatcher};

    fn setup(path: Option<std::path::PathBuf>) -> (AutomationRegistry, Rule, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let assets =
            crate::response_assets::ResponseAssetStore::load(Some(root.path().join("assets")))
                .unwrap();
        let asset = assets
            .create_authored(crate::AuthoredResponseAsset {
                id: "body".into(),
                revision: 1,
                status: 200,
                headers: transmog_core::HeaderBlock::new(),
                body: b"first".to_vec(),
                media_type: None,
            })
            .unwrap();
        let rule = Rule {
            id: "owned".into(),
            display_name: Some("Edited response".into()),
            enabled: true,
            revision: 1,
            priority: -100,
            matcher: RuleMatcher {
                host: Some("example.test".into()),
                ..RuleMatcher::default()
            },
            request: RequestActions {
                response_asset: Some(asset.asset_ref()),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        (
            AutomationRegistry::load(path, Arc::new(assets)).unwrap(),
            rule,
            root,
        )
    }
    fn create(registry: &AutomationRegistry, rule: Rule) -> AutomationStatus {
        registry
            .change_experiment(
                registry.status().generation,
                ExperimentChange::Create {
                    id: "experiment".into(),
                    name: "Checkout works".into(),
                    variant_id: "edited".into(),
                    variant_name: "Patched script".into(),
                    rules: vec![rule],
                },
            )
            .unwrap()
    }
    fn selection(variant: &str, revision: u64) -> ExperimentSelection {
        ExperimentSelection {
            experiment_id: "experiment".into(),
            variant_id: variant.into(),
            revision,
        }
    }

    fn page_reference() -> crate::ExperimentPreviewPage {
        crate::ExperimentPreviewPage {
            capture: "a".repeat(32),
            entry_id: format!("{:032x}", 1),
            request_number: "1".into(),
            trace_id: None,
            url: "https://example.test/page".into(),
            method: "GET".into(),
            started_at: 1,
            source: "Live traffic".into(),
        }
    }

    #[test]
    fn page_metadata_is_durable_and_does_not_invalidate_activation_undo_or_candidates() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("automation");
        let (registry, rule, _assets_root) = setup(Some(path.clone()));
        create(&registry, rule);
        let before = activate(&registry, "edited", 1);
        let document = registry.active.read().unwrap().document.clone();
        let candidate = registry.validate(document).unwrap();
        assert_eq!(candidate.rule_count, before.rules.len());
        let page = page_reference();
        registry
            .set_experiment_preview_page("experiment", 0, page.clone())
            .unwrap();
        let after = registry.status();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.rules, before.rules);
        assert_eq!(
            after.experiments[0].variants,
            before.experiments[0].variants
        );
        assert_eq!(after.experiments[0].undo, before.experiments[0].undo);
        assert_eq!(after.candidate_count, 1);
        assert_eq!(after.history_count, before.history_count);
        assert!(
            registry
                .set_experiment_preview_page("experiment", 0, page.clone())
                .is_err()
        );
        // Restart reads metadata from the same generation's atomically replaced slot.
        let reopened = AutomationRegistry::load(Some(path), registry.assets.clone()).unwrap();
        assert_eq!(
            reopened.status().experiments[0].preview_page,
            Some(page.clone())
        );
        assert_eq!(reopened.status().generation, before.generation);
        registry
            .change_experiment(
                after.generation,
                ExperimentChange::UndoActivation {
                    id: "experiment".into(),
                },
            )
            .unwrap();
        assert_eq!(
            registry.status().experiments[0].preview_page,
            Some(page.clone())
        );
        // Another registry demonstrates an already-validated candidate merging new metadata.
        let (registry, rule, _root) = setup(None);
        create(&registry, rule);
        let candidate = registry
            .validate(registry.active.read().unwrap().document.clone())
            .unwrap();
        registry
            .set_experiment_preview_page("experiment", 0, page.clone())
            .unwrap();
        registry.activate(&candidate.candidate_id).unwrap();
        assert_eq!(registry.status().experiments[0].preview_page, Some(page));
        assert_eq!(registry.status().experiments[0].preview_page_revision, 1);
        let _ = candidate;
    }
    fn activate(registry: &AutomationRegistry, variant: &str, revision: u64) -> AutomationStatus {
        registry
            .change_experiment(
                registry.status().generation,
                ExperimentChange::Activate {
                    selection: selection(variant, revision),
                    acknowledge_overlaps: true,
                },
            )
            .unwrap()
    }
    fn save(
        registry: &AutomationRegistry,
        revision: u64,
        rules: Vec<Rule>,
    ) -> Result<AutomationStatus, AppError> {
        registry.change_experiment(
            registry.status().generation,
            ExperimentChange::Save {
                id: "experiment".into(),
                variant_id: "edited".into(),
                expected_revision: revision,
                rules,
            },
        )
    }

    #[test]
    fn saved_revisions_never_rewrite_the_live_revision_and_baseline_is_scoped() {
        let (registry, rule, _assets_root) = setup(None);
        let mut unrelated = rule.clone();
        unrelated.id = "unrelated".into();
        unrelated.priority = 20;
        unrelated.revision = 42;
        let mut document = AutomationRuleSet::default();
        document.rules.push(unrelated.clone());
        let candidate = registry.validate(document).unwrap();
        registry.activate(&candidate.candidate_id).unwrap();
        let initial = create(&registry, rule.clone());
        assert_eq!(initial.rules, vec![unrelated.clone()]);
        let first = activate(&registry, "edited", 1);
        assert_eq!(first.rules, vec![unrelated.clone(), rule.clone()]);
        let mut changed = rule.clone();
        changed.revision = 2;
        changed.matcher.path_prefix = Some("/changed".into());
        let saved = save(&registry, 1, vec![changed.clone()]).unwrap();
        assert_eq!(saved.rules, first.rules);
        assert_eq!(saved.experiments[0].active, Some(selection("edited", 1)));
        assert_eq!(
            saved.experiments[0].variants[1].revisions[0].rules,
            vec![rule.clone()]
        );
        let second = activate(&registry, "edited", 2);
        assert_eq!(second.rules, vec![unrelated.clone(), changed]);
        let baseline = activate(&registry, "baseline", 1);
        assert_eq!(baseline.rules, vec![unrelated]);
        assert_eq!(
            baseline.experiments[0].variants[1].revisions[0].rules,
            vec![rule]
        );
    }

    #[test]
    fn pause_preserves_definitions_and_global_pause_through_activation_and_undo() {
        let (registry, rule, _assets_root) = setup(None);
        create(&registry, rule);
        let active = activate(&registry, "edited", 1);
        let admission = registry.active.read().unwrap().clone();
        assert_eq!(admission.compiled.registrations().len(), 1);
        let paused = registry
            .change_experiment(
                active.generation,
                ExperimentChange::Pause {
                    id: "experiment".into(),
                    paused: true,
                },
            )
            .unwrap();
        assert_eq!(paused.rules, active.rules);
        assert_eq!(
            paused.experiments[0].variants,
            active.experiments[0].variants
        );
        assert!(
            registry
                .active
                .read()
                .unwrap()
                .compiled
                .registrations()
                .is_empty()
        );
        assert_eq!(admission.compiled.registrations().len(), 1);
        registry
            .set_autoresponses_enabled(false, paused.generation)
            .unwrap();
        activate(&registry, "baseline", 1);
        let before = activate(&registry, "edited", 1);
        assert!(!before.autoresponses_enabled);
        assert!(before.experiments[0].paused);
        let undone = registry
            .change_experiment(
                before.generation,
                ExperimentChange::UndoActivation {
                    id: "experiment".into(),
                },
            )
            .unwrap();
        assert!(undone.rules.is_empty());
        assert!(!undone.autoresponses_enabled);
        assert!(undone.experiments[0].paused);
        registry
            .change_experiment(
                undone.generation,
                ExperimentChange::Pause {
                    id: "experiment".into(),
                    paused: false,
                },
            )
            .unwrap();
        assert!(
            registry
                .active
                .read()
                .unwrap()
                .compiled
                .registrations()
                .is_empty()
        );
    }

    #[test]
    fn stale_saves_and_undo_preserve_newer_unrelated_work() {
        let (registry, rule, _assets_root) = setup(None);
        let initial = create(&registry, rule.clone());
        activate(&registry, "edited", 1);
        assert!(
            registry
                .change_experiment(
                    initial.generation,
                    ExperimentChange::Pause {
                        id: "experiment".into(),
                        paused: true
                    }
                )
                .is_err()
        );
        save(&registry, 1, vec![rule.clone()]).unwrap();
        assert!(save(&registry, 1, vec![rule.clone()]).is_err());
        let current = registry.active.read().unwrap().document.clone();
        let mut document = current.clone();
        let mut unrelated = rule;
        unrelated.id = "new-work".into();
        unrelated.priority = 50;
        document.rules.push(unrelated.clone());
        let candidate = registry.validate(document).unwrap();
        let changed = registry.activate(&candidate.candidate_id).unwrap();
        assert!(
            registry
                .change_experiment(
                    changed.generation,
                    ExperimentChange::UndoActivation {
                        id: "experiment".into()
                    }
                )
                .is_err()
        );
        assert!(registry.status().rules.contains(&unrelated));
        let mut tampered = registry.active.read().unwrap().document.clone();
        tampered.rules.retain(|r| r.id != "owned");
        assert!(registry.validate(tampered).is_err());
        assert!(registry.status().rules.contains(&unrelated));
    }

    #[test]
    fn overlap_acknowledgement_and_compile_failure_are_non_mutating() {
        let (registry, rule, _assets_root) = setup(None);
        let mut other = rule.clone();
        other.id = "existing".into();
        other.priority = -100;
        let mut document = AutomationRuleSet::default();
        document.rules.push(other);
        let candidate = registry.validate(document).unwrap();
        registry.activate(&candidate.candidate_id).unwrap();
        create(&registry, rule.clone());
        assert_eq!(
            registry
                .experiment_overlaps(&selection("edited", 1))
                .unwrap()
                .len(),
            1
        );
        let before = registry.status();
        for acknowledged in [false, true] {
            assert!(
                registry
                    .change_experiment(
                        before.generation,
                        ExperimentChange::Activate {
                            selection: selection("edited", 1),
                            acknowledge_overlaps: acknowledged
                        }
                    )
                    .is_err()
            );
            assert_eq!(registry.status().generation, before.generation);
            assert_eq!(registry.status().rules, before.rules);
        }
        let mut invalid = rule;
        invalid.request.response_asset = Some("missing@999".into());
        invalid.revision = 2;
        assert!(save(&registry, 1, vec![invalid]).is_err());
        assert_eq!(registry.status().experiments, before.experiments);
    }

    #[test]
    fn duplicate_rename_archive_restore_and_history_keep_stable_identities() {
        let (registry, rule, _assets_root) = setup(None);
        create(&registry, rule.clone());
        let duplicated = registry
            .change_experiment(
                1,
                ExperimentChange::Duplicate {
                    selection: selection("edited", 1),
                    variant_id: "another".into(),
                    name: "Remove CSP".into(),
                },
            )
            .unwrap();
        assert_ne!(
            duplicated.experiments[0].variants[2].revisions[0].rules[0].id,
            rule.id
        );
        let renamed = registry
            .change_experiment(
                2,
                ExperimentChange::Rename {
                    id: "experiment".into(),
                    variant_id: Some("another".into()),
                    name: "Policy test".into(),
                },
            )
            .unwrap();
        assert_eq!(renamed.experiments[0].variants[2].id, "another");
        for archived in [true, false] {
            let status = registry
                .change_experiment(
                    registry.status().generation,
                    ExperimentChange::Archive {
                        id: "experiment".into(),
                        variant_id: Some("another".into()),
                        archived,
                    },
                )
                .unwrap();
            assert_eq!(status.experiments[0].variants[2].archived, archived);
        }
        activate(&registry, "another", 1);
        assert!(
            registry
                .change_experiment(
                    registry.status().generation,
                    ExperimentChange::Archive {
                        id: "experiment".into(),
                        variant_id: Some("another".into()),
                        archived: true
                    }
                )
                .is_err()
        );
        assert!(
            registry
                .change_experiment(
                    registry.status().generation,
                    ExperimentChange::Save {
                        id: "experiment".into(),
                        variant_id: "baseline".into(),
                        expected_revision: 1,
                        rules: vec![rule]
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn persistence_failure_preserves_live_state_and_restart_retains_saved_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rules");
        let (registry, rule, _assets_root) = setup(Some(path.clone()));
        create(&registry, rule.clone());
        save(&registry, 1, vec![rule]).unwrap();
        let recovered =
            AutomationRegistry::load(Some(path.clone()), registry.assets.clone()).unwrap();
        assert_eq!(
            recovered.status().experiments,
            registry.status().experiments
        );
        std::fs::remove_file(super::super::slot_path(&path, 1)).unwrap();
        std::fs::create_dir(super::super::slot_path(&path, 1)).unwrap();
        let before = registry.status();
        assert!(
            registry
                .change_experiment(
                    before.generation,
                    ExperimentChange::Activate {
                        selection: selection("edited", 1),
                        acknowledge_overlaps: true
                    }
                )
                .is_err()
        );
        assert_eq!(registry.status().rules, before.rules);
        assert_eq!(registry.status().experiments, before.experiments);
    }

    #[test]
    fn simultaneous_saves_accept_only_one_without_losing_history() {
        let (registry, rule, _assets_root) = setup(None);
        create(&registry, rule.clone());
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            let rule = rule.clone();
            tasks.push(std::thread::spawn(move || {
                barrier.wait();
                registry.change_experiment(
                    1,
                    ExperimentChange::Save {
                        id: "experiment".into(),
                        variant_id: "edited".into(),
                        expected_revision: 1,
                        rules: vec![rule],
                    },
                )
            }));
        }
        barrier.wait();
        let successes = tasks
            .into_iter()
            .flat_map(|task| task.join().unwrap())
            .count();
        assert_eq!(successes, 1);
        assert_eq!(
            registry.status().experiments[0].variants[1].revisions.len(),
            2
        );
    }

    #[test]
    fn removed_rule_identity_cannot_be_reused_for_changed_historical_meaning() {
        let (registry, mut rule, _assets_root) = setup(None);
        create(&registry, rule.clone());
        save(&registry, 1, Vec::new()).unwrap();
        rule.display_name = Some("Changed definition".into());
        assert!(save(&registry, 2, vec![rule.clone()]).is_err());
        rule.revision += 1;
        save(&registry, 2, vec![rule]).unwrap();
        assert_eq!(
            registry.status().experiments[0].variants[1].revisions[0].rules[0]
                .display_name
                .as_deref(),
            Some("Edited response")
        );
    }

    #[test]
    fn history_and_order_limits_reject_without_dropping_revisions() {
        let (registry, rule, _assets_root) = setup(None);
        create(&registry, rule.clone());
        let mut second = rule.clone();
        second.id = "second".into();
        assert!(save(&registry, 1, vec![rule.clone(), second.clone()]).is_err());
        second.priority += 1;
        save(&registry, 1, vec![rule.clone(), second]).unwrap();
        for revision in 2..32 {
            save(&registry, revision, vec![rule.clone()]).unwrap();
        }
        let before = registry.status();
        assert!(save(&registry, 32, vec![rule]).is_err());
        assert_eq!(registry.status().experiments, before.experiments);
        assert_eq!(registry.status().generation, before.generation);
    }
}
