//! One editable collection with independently retained named, applied and Undo snapshots.
use super::{
    AutomationRegistry, AutomationRuleSet, AutomationStatus, ExperimentSelection, compile_error,
};
use crate::{AppError, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use transmog_automation::{Rule, compile_with_assets};

pub(crate) const MAX_RESPONSES: usize = 1_000;

/// Immutable content retained by a named version, live application or Undo.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseSetSnapshot {
    /// Current collection revision from which this snapshot was made.
    pub revision: u64,
    /// Complete ordered saved response definitions.
    pub rules: Vec<Rule>,
}

/// Exact generation-scoped restoration after applying or stopping a set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseSetApplyUndo {
    /// Automation generation at which restoration is valid.
    pub generation: u64,
    /// Previous independently retained applied content, or no live effects.
    pub applied: Option<ResponseSetSnapshot>,
    /// Previous per-set pause state.
    pub paused: bool,
}

/// An explicitly retained named version, independent of current and applied content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseSetVersion {
    /// Stable identity within this set.
    pub id: String,
    /// Explicit version name.
    pub name: String,
    /// Exact current revision captured when saved; also its immutable selector token.
    pub revision: u64,
    /// Frozen ordered response definitions.
    pub rules: Vec<Rule>,
}

/// Saved debugging workspace with a single current editable collection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseSet {
    /// Stable set identity.
    pub id: String,
    /// Immutable creation nonce; distinguishes deletion and recreation of an owner ID.
    pub incarnation: String,
    /// Set name.
    pub name: String,
    /// Investigation notes, at most 16 KiB UTF-8.
    pub notes: String,
    /// Monotonic compare-and-set token for current content and notes.
    pub revision: u64,
    /// Last explicit content save, Unix milliseconds.
    pub modified_at: u64,
    /// Current ordered response definitions; may be empty.
    pub rules: Vec<Rule>,
    /// Explicit milestones and alternatives, at most 64.
    pub versions: Vec<ResponseSetVersion>,
    /// Independently frozen live configuration.
    pub applied: Option<ResponseSetSnapshot>,
    /// Exact available restoration; intervening automation generations invalidate it.
    pub undo_apply: Option<ResponseSetApplyUndo>,
    /// Participation gate independent of authored rule inclusion and global pause.
    pub paused: bool,
    /// Explicit captured evidence association; does not retain source traffic.
    pub preview_page: Option<crate::ExperimentPreviewPage>,
    /// Independent compare-and-set token for page association.
    pub preview_page_revision: u64,
}

impl ResponseSet {
    pub(crate) fn owns(&self, id: &str) -> bool {
        self.snapshots()
            .any(|rules| rules.iter().any(|rule| rule.id == id))
    }

    pub(crate) fn snapshots(&self) -> impl Iterator<Item = &[Rule]> {
        std::iter::once(self.rules.as_slice())
            .chain(self.versions.iter().map(|version| version.rules.as_slice()))
            .chain(
                self.applied
                    .iter()
                    .map(|snapshot| snapshot.rules.as_slice()),
            )
            .chain(self.undo_apply.iter().flat_map(|undo| {
                undo.applied
                    .iter()
                    .map(|snapshot| snapshot.rules.as_slice())
            }))
    }

    /// Resolves an exact current or named-version selector; stale selectors never alias newer content.
    pub fn selected_rules(&self, selection: &ExperimentSelection) -> Option<&[Rule]> {
        if selection.experiment_id != self.id || selection.incarnation != self.incarnation {
            return None;
        }
        if selection.variant_id.is_empty() {
            return (selection.revision == self.revision).then_some(self.rules.as_slice());
        }
        self.versions
            .iter()
            .find(|version| {
                version.id == selection.variant_id && version.revision == selection.revision
            })
            .map(|version| version.rules.as_slice())
    }

    /// Exact selector for this saved current collection.
    pub fn current_selection(&self) -> ExperimentSelection {
        ExperimentSelection {
            experiment_id: self.id.clone(),
            incarnation: self.incarnation.clone(),
            variant_id: String::new(),
            revision: self.revision,
        }
    }

    pub(crate) fn new(
        id: String,
        name: String,
        notes: String,
        rules: Vec<Rule>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            id,
            incarnation: crate::rule_interchange::fresh_id("owner")?,
            name,
            notes,
            revision: 1,
            modified_at: now()?,
            rules,
            versions: Vec::new(),
            applied: None,
            undo_apply: None,
            paused: false,
            preview_page: None,
            preview_page_revision: 0,
        })
    }
}

/// Generation-checked atomic workspace mutation. Passive saves never activate content.
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ResponseSetChange {
    /// Create an empty or populated inactive workspace.
    Create {
        /// New stable set identity.
        id: String,
        /// User-facing set name.
        name: String,
        /// Optional investigation notes.
        notes: String,
        /// Complete ordered initial collection; empty is allowed.
        rules: Vec<Rule>,
    },
    /// Replace the current collection, preserving versions and the live snapshot.
    SaveCurrent {
        /// Set to edit.
        id: String,
        /// Current revision edited by the caller.
        expected_revision: u64,
        /// Complete replacement display name.
        name: String,
        /// Complete replacement notes.
        notes: String,
        /// Complete ordered replacement collection.
        rules: Vec<Rule>,
    },
    /// Retain the saved current collection under an explicit name.
    SaveVersion {
        /// Owning set.
        id: String,
        /// Current revision to freeze.
        expected_revision: u64,
        /// New stable named version identity.
        version_id: String,
        /// Explicit version name.
        name: String,
    },
    /// Replace current with a retained version, without changing live traffic.
    RestoreVersion {
        /// Owning set.
        id: String,
        /// Current revision to replace.
        expected_revision: u64,
        /// Retained version to restore.
        version_id: String,
    },
    /// Delete a named version without changing current or applied content.
    DeleteVersion {
        /// Owning set.
        id: String,
        /// Retained version to delete.
        version_id: String,
    },
    /// Freeze current and atomically replace only this set's live rules.
    Apply {
        /// Owning set.
        id: String,
        /// Exact current revision to apply.
        expected_revision: u64,
        /// Explicit acknowledgement after reviewing overlap warnings.
        acknowledge_overlaps: bool,
        /// Explicitly resume this set with the applied content, preserving global pause.
        #[serde(default)]
        resume: bool,
    },
    /// Remove only this set's live rules; retain its current collection and versions.
    Stop {
        /// Set whose live effects should stop.
        id: String,
    },
    /// Restore only the prior applied snapshot and per-set pause at its valid generation.
    UndoApply {
        /// Owning set.
        id: String,
    },
    /// Preserve content and global pause while changing this set's participation.
    Pause {
        /// Owning set.
        id: String,
        /// Desired per-set participation gate.
        paused: bool,
    },
    /// Stop effects and delete this set and all its owned metadata atomically.
    Delete {
        /// Set to delete, including its versions and metadata.
        id: String,
    },
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn conflict(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, true)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn next(value: u64) -> Result<u64, AppError> {
    value
        .checked_add(1)
        .ok_or_else(|| invalid("Response set revision exhausted."))
}
fn now() -> Result<u64, AppError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or_else(|| invalid("The current time is unavailable."))
}

pub(super) fn views(document: &AutomationRuleSet) -> Vec<ResponseSet> {
    document
        .response_sets
        .iter()
        .map(|set| {
            let mut view = set.clone();
            if view
                .undo_apply
                .as_ref()
                .is_some_and(|undo| undo.generation != document.generation)
            {
                view.undo_apply = None;
            }
            view
        })
        .collect()
}

pub(super) fn validate(document: &AutomationRuleSet) -> Result<(), AppError> {
    if document.response_sets.len() > 32 {
        return Err(invalid("At most 32 response sets can be retained."));
    }
    let mut ids = HashSet::new();
    let mut owners = HashMap::new();
    let mut definitions = HashMap::new();
    for set in &document.response_sets {
        if !valid_id(&set.id)
            || !valid_id(&set.incarnation)
            || !valid_name(&set.name)
            || !ids.insert(&set.id)
            || set.revision == 0
            || set.notes.len() > 16 * 1024
            || set.versions.len() > 64
            || set.preview_page.as_ref().is_some_and(|page| !page.valid())
        {
            return Err(invalid(
                "Response set identity, metadata or limit is invalid.",
            ));
        }
        let mut versions = HashSet::new();
        for version in &set.versions {
            if !valid_id(&version.id)
                || !valid_name(&version.name)
                || !versions.insert(&version.id)
                || version.revision == 0
                || version.revision > set.revision
            {
                return Err(invalid(
                    "Response set version identity, name or snapshot token is invalid.",
                ));
            }
        }
        if set
            .applied
            .as_ref()
            .is_some_and(|snapshot| snapshot.revision == 0 || snapshot.revision > set.revision)
            || set.undo_apply.as_ref().is_some_and(|undo| {
                undo.generation == 0
                    || undo.applied.as_ref().is_some_and(|snapshot| {
                        snapshot.revision == 0 || snapshot.revision > set.revision
                    })
            })
        {
            return Err(invalid("Applied response set snapshot is invalid."));
        }
        for rules in set.snapshots() {
            if rules.len() > MAX_RESPONSES
                || rules
                    .windows(2)
                    .any(|pair| pair[0].priority >= pair[1].priority)
            {
                return Err(invalid(
                    "Response sets require up to 1,000 rules in increasing priority order.",
                ));
            }
            let mut rule_ids = HashSet::new();
            for rule in rules {
                if !rule_ids.insert(&rule.id) || rule.request.response_asset.is_none() {
                    return Err(invalid(
                        "Response sets require unique saved response rules.",
                    ));
                }
                if definitions
                    .insert((&rule.id, rule.revision), rule)
                    .is_some_and(|previous| previous != rule)
                {
                    return Err(invalid(
                        "A saved rule identity and revision cannot describe different definitions.",
                    ));
                }
                if owners
                    .insert(&rule.id, &set.id)
                    .is_some_and(|owner| owner != &set.id)
                {
                    return Err(invalid(
                        "A rule cannot belong to more than one response set.",
                    ));
                }
            }
        }
        let actual = document
            .rules
            .iter()
            .filter(|rule| set.owns(&rule.id))
            .collect::<Vec<_>>();
        let expected = set
            .applied
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.rules.as_slice());
        if actual != expected.iter().collect::<Vec<_>>() {
            return Err(invalid(
                "Live response set rules must match its independent applied snapshot.",
            ));
        }
    }
    Ok(())
}

fn find_mut<'a>(
    document: &'a mut AutomationRuleSet,
    id: &str,
) -> Result<&'a mut ResponseSet, AppError> {
    document
        .response_sets
        .iter_mut()
        .find(|set| set.id == id)
        .ok_or_else(|| invalid("Response set is unavailable. Refresh the collection."))
}
fn check_revision(set: &ResponseSet, revision: u64) -> Result<(), AppError> {
    if set.revision != revision {
        return Err(conflict(
            "The response set changed. Reopen its current collection before saving.",
        ));
    }
    Ok(())
}
fn check_rules(document: &AutomationRuleSet, id: &str, rules: &[Rule]) -> Result<(), AppError> {
    for rule in rules {
        if document
            .response_sets
            .iter()
            .any(|set| set.id != id && set.owns(&rule.id))
            || document.rules.iter().any(|other| {
                other.id == rule.id
                    && !document
                        .response_sets
                        .iter()
                        .any(|set| set.id == id && set.owns(&rule.id))
            })
        {
            return Err(invalid(
                "Copy rules with new identities before adding them to a response set.",
            ));
        }
        if let Some(set) = document.response_sets.iter().find(|set| set.id == id) {
            let previous = set
                .snapshots()
                .flatten()
                .filter(|old| old.id == rule.id)
                .collect::<Vec<_>>();
            if !previous.contains(&rule) && previous.iter().any(|old| old.revision >= rule.revision)
            {
                return Err(conflict(
                    "Changed response rules require a new rule revision.",
                ));
            }
        }
    }
    Ok(())
}

impl AutomationRegistry {
    /// Overlap review for the current saved collection without applying it.
    pub fn response_set_overlaps(&self, id: &str, revision: u64) -> Result<Vec<String>, AppError> {
        let document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        let set = document
            .response_sets
            .iter()
            .find(|set| set.id == id)
            .ok_or_else(|| invalid("Response set is unavailable."))?;
        let mut selection = set.current_selection();
        selection.revision = revision;
        overlaps(&document, &selection)
    }

    /// Validates, persists and publishes one current-set transaction.
    #[allow(clippy::too_many_lines)]
    pub fn change_response_set(
        &self,
        generation: u64,
        change: ResponseSetChange,
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
            ResponseSetChange::Create {
                id,
                name,
                notes,
                rules,
            } => {
                if document.response_sets.iter().any(|set| set.id == id) {
                    return Err(invalid("Choose an unused response set identity."));
                }
                check_rules(&document, &id, &rules)?;
                document
                    .response_sets
                    .push(ResponseSet::new(id, name, notes, rules)?);
            }
            ResponseSetChange::SaveCurrent {
                id,
                expected_revision,
                name,
                notes,
                rules,
            } => {
                check_rules(&document, &id, &rules)?;
                let set = find_mut(&mut document, &id)?;
                check_revision(set, expected_revision)?;
                set.revision = next(set.revision)?;
                set.modified_at = now()?;
                set.name = name;
                set.notes = notes;
                set.rules = rules;
            }
            ResponseSetChange::SaveVersion {
                id,
                expected_revision,
                version_id,
                name,
            } => {
                let set = find_mut(&mut document, &id)?;
                check_revision(set, expected_revision)?;
                if set.versions.iter().any(|version| version.id == version_id) {
                    return Err(conflict("That version identity is already retained."));
                }
                set.versions.push(ResponseSetVersion {
                    id: version_id,
                    name,
                    revision: set.revision,
                    rules: set.rules.clone(),
                });
            }
            ResponseSetChange::RestoreVersion {
                id,
                expected_revision,
                version_id,
            } => {
                let set = find_mut(&mut document, &id)?;
                check_revision(set, expected_revision)?;
                let rules = set
                    .versions
                    .iter()
                    .find(|version| version.id == version_id)
                    .ok_or_else(|| invalid("Named version is unavailable."))?
                    .rules
                    .clone();
                set.rules = rules;
                set.revision = next(set.revision)?;
                set.modified_at = now()?;
            }
            ResponseSetChange::DeleteVersion { id, version_id } => {
                let set = find_mut(&mut document, &id)?;
                let index = set
                    .versions
                    .iter()
                    .position(|version| version.id == version_id)
                    .ok_or_else(|| invalid("Named version is unavailable."))?;
                set.versions.remove(index);
            }
            ResponseSetChange::Apply {
                id,
                expected_revision,
                acknowledge_overlaps,
                resume,
            } => {
                check_revision(find_mut(&mut document, &id)?, expected_revision)?;
                let selection = find_mut(&mut document, &id)?.current_selection();
                let warnings = overlaps(&document, &selection)?;
                if !warnings.is_empty() && !acknowledge_overlaps {
                    return Err(conflict(&format!(
                        "Review overlapping live rules before applying: {}",
                        warnings.join("; ")
                    )));
                }
                let set = find_mut(&mut document, &id)?;
                record_undo(set, next(generation)?);
                let snapshot = ResponseSetSnapshot {
                    revision: set.revision,
                    rules: set.rules.clone(),
                };
                replace_applied(&mut document, &id, Some(snapshot), resume.then_some(false))?;
            }
            ResponseSetChange::Stop { id } => {
                record_undo(find_mut(&mut document, &id)?, next(generation)?);
                replace_applied(&mut document, &id, None, None)?;
            }
            ResponseSetChange::UndoApply { id } => {
                let set = find_mut(&mut document, &id)?;
                let undo = set
                    .undo_apply
                    .clone()
                    .ok_or_else(|| conflict("No live response set change is available to undo."))?;
                if undo.generation != generation {
                    return Err(conflict(
                        "Automation changed after this live change. Current work has been preserved.",
                    ));
                }
                replace_applied(&mut document, &id, undo.applied, Some(undo.paused))?;
                find_mut(&mut document, &id)?.undo_apply = None;
            }
            ResponseSetChange::Pause { id, paused } => {
                find_mut(&mut document, &id)?.paused = paused;
            }
            ResponseSetChange::Delete { id } => {
                replace_applied(&mut document, &id, None, None)?;
                document.response_sets.retain(|set| set.id != id);
            }
        }
        for set in &mut document.response_sets {
            if set
                .undo_apply
                .as_ref()
                .is_some_and(|undo| undo.generation != generation.saturating_add(1))
            {
                set.undo_apply = None;
            }
        }
        super::validate_document(&document)?;
        for rules in document
            .response_sets
            .iter()
            .flat_map(ResponseSet::snapshots)
        {
            compile_with_assets(rules.to_vec(), self.limits, Some(self.assets.clone()))
                .map_err(compile_error)?;
        }
        let candidate = self.validate_candidate(document)?;
        self.activate(&candidate.candidate_id)
    }
}

fn record_undo(set: &mut ResponseSet, generation: u64) {
    set.undo_apply = Some(ResponseSetApplyUndo {
        generation,
        applied: set.applied.clone(),
        paused: set.paused,
    });
}
fn replace_applied(
    document: &mut AutomationRuleSet,
    id: &str,
    applied: Option<ResponseSetSnapshot>,
    paused: Option<bool>,
) -> Result<(), AppError> {
    let set = document
        .response_sets
        .iter()
        .find(|set| set.id == id)
        .ok_or_else(|| invalid("Response set is unavailable."))?;
    let owned = document
        .rules
        .iter()
        .filter(|rule| set.owns(&rule.id))
        .map(|rule| rule.id.clone())
        .collect::<HashSet<_>>();
    document.rules.retain(|rule| !owned.contains(&rule.id));
    let rules = applied
        .as_ref()
        .map_or_else(Vec::new, |snapshot| snapshot.rules.clone());
    let set = find_mut(document, id)?;
    set.applied = applied;
    if let Some(paused) = paused {
        set.paused = paused;
    }
    document.rules.extend(rules);
    Ok(())
}

pub(super) fn overlaps(
    document: &AutomationRuleSet,
    selection: &ExperimentSelection,
) -> Result<Vec<String>, AppError> {
    let set = document
        .response_sets
        .iter()
        .find(|set| set.id == selection.experiment_id)
        .ok_or_else(|| invalid("Response set is unavailable."))?;
    let rules = set
        .selected_rules(selection)
        .ok_or_else(|| invalid("Saved response collection is unavailable."))?;
    let mut warnings = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        for other in document.rules.iter().filter(|other| {
            other.enabled
                && !set.owns(&other.id)
                && !document
                    .response_sets
                    .iter()
                    .any(|set| set.paused && set.owns(&other.id))
        }) {
            if rule.matcher.may_overlap_request(&other.matcher) && warnings.len() < 64 {
                let order = if (other.priority, &other.id) < (rule.priority, &rule.id) {
                    "earlier"
                } else {
                    "later"
                };
                warnings.push(format!(
                    "{} may overlap {} ({order} live rule)",
                    rule.display_name.as_deref().unwrap_or(&rule.id),
                    other.display_name.as_deref().unwrap_or(&other.id)
                ));
            }
        }
    }
    Ok(warnings)
}

#[cfg(test)]
#[path = "response_sets_tests.rs"]
mod tests;
