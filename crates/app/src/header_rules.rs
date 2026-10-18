//! Scoped header authoring uses the existing automation generation and compiler.
use super::{AutomationRegistry, AutomationRuleSet, AutomationStatus};
use crate::{AppError, ErrorCategory};
use serde::Deserialize;
use transmog_automation::{HeaderRuleTest, Rule, header_rule_phase};
use transmog_core::{HeaderBlock, HttpLegVersion, ResponseHead};

/// Friendly position choices; arbitrary native priority integers stay out of authoring.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HeaderRulePlacement {
    /// Keep an existing rule position, or append a new header rule after the current group.
    #[default]
    Keep,
    /// Save this rule before other native rules, without changing their priorities.
    Before,
    /// Save this rule after other native rules, without changing their priorities.
    After,
}

/// A bounded generation-checked edit to header-only rules.
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HeaderRuleChange {
    /// Validate and persist one complete header rule; the enabled choice is explicit.
    Save {
        /// Complete draft. The registry assigns the next revision and selected position.
        rule: Rule,
        /// Revision opened in the editor; none requires a new identity.
        expected_revision: Option<u64>,
        /// Position relative to other native rules.
        #[serde(default)]
        placement: HeaderRulePlacement,
    },
    /// Remove one exact saved revision; the client may retain it for generation-safe Undo.
    Remove {
        /// Stable rule ID.
        id: String,
        /// Revision the user chose to remove.
        expected_revision: u64,
    },
    /// Move one step among header rules, preserving unrelated automation.
    Move {
        /// Stable rule ID.
        id: String,
        /// Revision chosen by the user.
        expected_revision: u64,
        /// True moves earlier; false moves later.
        earlier: bool,
    },
    /// Restore an exact removed rule at its previous position after a generation check.
    Restore {
        /// Removed definition, retained only for the current session's Undo.
        rule: Rule,
    },
}

/// Explicit network-free sample input. It never acquires traffic or executes scripts.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderRuleTestInput {
    /// Complete header-only draft, tested independently of its enabled state.
    pub rule: Rule,
    /// Sample method.
    pub method: String,
    /// Sample absolute URL.
    pub url: String,
    /// Sample request fields, preserving duplicate order and value bytes.
    pub request_headers: HeaderBlock,
    /// Sample response status (100–599).
    pub response_status: u16,
    /// Sample response fields, preserving duplicate order and value bytes.
    pub response_headers: HeaderBlock,
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn conflict(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, true)
}
fn validate_header_rule(rule: &Rule) -> Result<(), AppError> {
    if serde_json::to_vec(rule)
        .map_err(|_| invalid("Header rule could not be serialized."))?
        .len()
        > 256 * 1024
    {
        return Err(invalid(
            "A header rule exceeds its 256 KiB definition limit.",
        ));
    }
    if header_rule_phase(rule).is_none() {
        return Err(invalid(
            "Use exactly one request or response phase with header operations. Body actions, local responses, User-Agent policies and request-phase response conditions belong in their own editors.",
        ));
    }
    for field in rule.request.headers.iter().chain(&rule.response.headers) {
        let length = match field {
            transmog_automation::HeaderOperation::Set(field)
            | transmog_automation::HeaderOperation::Append(field) => field.value().len(),
            transmog_automation::HeaderOperation::Remove(_) => 0,
        };
        if length > 64 * 1024 {
            return Err(invalid("A header operation value exceeds 64 KiB."));
        }
    }
    for predicate in rule
        .matcher
        .request_headers
        .iter()
        .chain(&rule.matcher.response_headers)
    {
        let length = match &predicate.condition {
            transmog_automation::HeaderCondition::Exists => 0,
            transmog_automation::HeaderCondition::Exact(bytes)
            | transmog_automation::HeaderCondition::Prefix(bytes)
            | transmog_automation::HeaderCondition::Suffix(bytes) => bytes.len(),
            transmog_automation::HeaderCondition::Regex(pattern) => pattern.len(),
        };
        if length > 64 * 1024 {
            return Err(invalid("A header condition value exceeds 64 KiB."));
        }
    }
    Ok(())
}
fn owned(document: &AutomationRuleSet, id: &str) -> bool {
    document.experiments.iter().any(|e| e.owns(id))
}
fn eligible(rule: &Rule, text: bool) -> bool {
    if text {
        transmog_automation::is_text_transform_rule(rule)
    } else {
        header_rule_phase(rule).is_some()
    }
}
fn validate_scope(rule: &Rule, text: bool) -> Result<(), AppError> {
    if !text {
        return validate_header_rule(rule);
    }
    if !eligible(rule, true)
        || serde_json::to_vec(rule)
            .map_err(|_| invalid("Invalid rule"))?
            .len()
            > 256 * 1024
    {
        return Err(invalid("Use one standalone bounded literal response edit."));
    }
    Ok(())
}
fn existing<'a>(
    document: &'a AutomationRuleSet,
    id: &str,
    revision: u64,
    text: bool,
) -> Result<&'a Rule, AppError> {
    let rule = document.rules.iter().find(|r| r.id == id).ok_or_else(|| {
        conflict("This rule was removed. Your draft is preserved; save it with a new identity.")
    })?;
    if owned(document, id) || !eligible(rule, text) {
        return Err(invalid(
            "This rule belongs to another automation editor. Its definition was preserved.",
        ));
    }
    if rule.revision != revision {
        return Err(conflict(
            "This header rule changed while you were editing. Your draft is preserved. Reopen the saved rule or duplicate the draft as a new rule.",
        ));
    }
    Ok(rule)
}

fn move_header_rule(
    document: &mut AutomationRuleSet,
    id: &str,
    expected_revision: u64,
    earlier: bool,
    text: bool,
) -> Result<bool, AppError> {
    existing(document, id, expected_revision, text)?;
    let mut ordered = document
        .rules
        .iter()
        .filter(|r| eligible(r, text) && !owned(document, &r.id))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|r| (r.priority, &r.id));
    let index = ordered
        .iter()
        .position(|r| r.id == id)
        .expect("located rule");
    let target = if earlier {
        index.checked_sub(1)
    } else {
        index.checked_add(1).filter(|i| *i < ordered.len())
    };
    let Some(target) = target else {
        return Ok(false);
    };
    let other_id = ordered[target].id.clone();
    let priority = ordered[index].priority;
    let other_priority = ordered[target].priority;
    if priority == other_priority {
        return Err(conflict(
            "These rules share a position. Save one Before or After other native rules to establish a distinct order.",
        ));
    }
    for rule in &mut document.rules {
        if rule.id == id || rule.id == other_id {
            rule.priority = if rule.id == id {
                other_priority
            } else {
                priority
            };
            rule.revision = rule
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("Rule revision exhausted."))?;
        }
    }
    Ok(true)
}

fn restore_header_rule(
    document: &mut AutomationRuleSet,
    mut rule: Rule,
    text: bool,
) -> Result<(), AppError> {
    validate_scope(&rule, text)?;
    if owned(document, &rule.id) || document.rules.iter().any(|r| r.id == rule.id) {
        return Err(conflict(
            "The removed rule identity is already in use. Current automation was preserved.",
        ));
    }
    rule.revision = rule
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("Rule revision exhausted."))?;
    document.rules.push(rule);
    Ok(())
}

impl AutomationRegistry {
    pub(crate) fn change_response_transform(
        &self,
        generation: u64,
        change: HeaderRuleChange,
    ) -> Result<AutomationStatus, AppError> {
        self.change_scoped_rule(generation, change, true)
    }
    pub(crate) fn change_header_rule(
        &self,
        generation: u64,
        change: HeaderRuleChange,
    ) -> Result<AutomationStatus, AppError> {
        self.change_scoped_rule(generation, change, false)
    }
    fn change_scoped_rule(
        &self,
        generation: u64,
        change: HeaderRuleChange,
        text: bool,
    ) -> Result<AutomationStatus, AppError> {
        let mut document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        if generation != document.generation {
            return Err(conflict(
                "Automation changed. Refresh the rules and retry; your draft is preserved.",
            ));
        }
        match change {
            HeaderRuleChange::Save {
                mut rule,
                expected_revision,
                placement,
            } => {
                validate_scope(&rule, text)?;
                if owned(&document, &rule.id) {
                    return Err(invalid(
                        "Edit rules owned by a response set through their saved variant.",
                    ));
                }
                let old = expected_revision
                    .map(|revision| existing(&document, &rule.id, revision, text))
                    .transpose()?;
                if old.is_none() && document.rules.iter().any(|r| r.id == rule.id) {
                    return Err(conflict(
                        "This rule identity already exists. Save the draft as a new rule.",
                    ));
                }
                rule.revision = old.map_or(Ok(1), |r| {
                    r.revision
                        .checked_add(1)
                        .ok_or_else(|| invalid("Rule revision exhausted."))
                })?;
                rule.priority = match placement {
                    HeaderRulePlacement::Keep => {
                        if let Some(old) = old {
                            old.priority
                        } else {
                            document.rules.iter().filter(|r|eligible(r,text)).map(|r|r.priority).max().map_or(Ok(0),|p|p.checked_add(1).ok_or_else(||invalid("No later position is available. Choose Before other native rules.")))?
                        }
                    }
                    HeaderRulePlacement::Before => document
                        .rules
                        .iter()
                        .filter(|r| r.id != rule.id)
                        .map(|r| r.priority)
                        .min()
                        .unwrap_or(0)
                        .checked_sub(1)
                        .ok_or_else(|| invalid("No earlier position is available."))?,
                    HeaderRulePlacement::After => document
                        .rules
                        .iter()
                        .filter(|r| r.id != rule.id)
                        .map(|r| r.priority)
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(|| invalid("No later position is available."))?,
                };
                if let Some(index) = document.rules.iter().position(|r| r.id == rule.id) {
                    document.rules[index] = rule;
                } else {
                    document.rules.push(rule);
                }
            }
            HeaderRuleChange::Remove {
                id,
                expected_revision,
            } => {
                existing(&document, &id, expected_revision, text)?;
                document.rules.retain(|r| r.id != id);
            }
            HeaderRuleChange::Restore { rule } => restore_header_rule(&mut document, rule, text)?,
            HeaderRuleChange::Move {
                id,
                expected_revision,
                earlier,
            } => {
                if !move_header_rule(&mut document, &id, expected_revision, earlier, text)? {
                    return Ok(self.status());
                }
            }
        }
        let candidate = self.validate_candidate(document)?;
        self.activate(&candidate.candidate_id)
    }
}

pub(crate) fn test(input: &HeaderRuleTestInput) -> Result<HeaderRuleTest, AppError> {
    validate_header_rule(&input.rule)?;
    let (request, response) = sample(input)?;
    transmog_automation::test_header_rule(&input.rule, &request, Some(&response))
        .map_err(|error| invalid(&error))
}
fn sample(
    input: &HeaderRuleTestInput,
) -> Result<(transmog_core::RequestHead, ResponseHead), AppError> {
    for headers in [&input.request_headers, &input.response_headers] {
        if headers.iter().count() > 256
            || headers
                .iter()
                .map(|h| h.name().len() + h.value().len())
                .sum::<usize>()
                > 64 * 1024
        {
            return Err(invalid(
                "A test sample allows at most 256 fields and 64 KiB per header block.",
            ));
        }
    }
    for field in input
        .request_headers
        .iter()
        .chain(input.response_headers.iter())
    {
        transmog_core::HeaderField::try_new(field.name().to_vec(), field.value().to_vec())
            .map_err(|_| invalid("A sample header has an invalid name or value."))?;
    }
    if !(100..=599).contains(&input.response_status) {
        return Err(invalid("Enter a sample response status from 100 to 599."));
    }
    let mut request = transmog_automation::request_for_test(&input.method, &input.url, &[])
        .map_err(|error| invalid(&error))?;
    request.headers = input.request_headers.clone();
    let response = ResponseHead {
        status: input.response_status,
        headers: input.response_headers.clone(),
        source_version: HttpLegVersion::Http1,
    };
    Ok((request, response))
}

/// A bounded explicit decoded UTF-8 draft sample.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextTransformTestInput {
    /// Rule, request and response head sample.
    pub sample: HeaderRuleTestInput,
    /// Complete decoded UTF-8 sample, at most 1 MiB.
    pub body: String,
}
pub(crate) fn test_transform(
    input: &TextTransformTestInput,
) -> Result<transmog_automation::TextTransformTest, AppError> {
    validate_scope(&input.sample.rule, true)?;
    if input.body.len() > 1024 * 1024 {
        return Err(invalid("Sample exceeds 1 MiB."));
    }
    let (request, response) = sample(&input.sample)?;
    transmog_automation::test_text_transform(
        &input.sample.rule,
        &request,
        &response,
        input.body.as_bytes(),
    )
    .map_err(|e| invalid(&e))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use transmog_automation::{HeaderOperation, RequestActions, ResponseActions, RuleMatcher};
    fn setup(path: Option<std::path::PathBuf>) -> AutomationRegistry {
        AutomationRegistry::load(
            path,
            Arc::new(crate::response_assets::ResponseAssetStore::load(None).unwrap()),
        )
        .unwrap()
    }
    #[test]
    fn text_transform_scope_preserves_headers_pause_and_checks_stale_revisions() {
        let registry = setup(None);
        let headers = save(&registry, rule("headers"), None).unwrap();
        let mut text = rule("text");
        text.response.headers.clear();
        text.response.text_transform = Some(transmog_automation::LiteralTransform {
            find: "old".into(),
            replacement: "new".into(),
            occurrences: transmog_automation::Occurrences::All,
            expected_matches: Some(1),
            media_type: "text/plain".into(),
            encoding: "utf-8".into(),
            max_bytes: 1024,
        });
        assert!(save(&registry, text.clone(), None).is_err());
        let saved = registry
            .change_response_transform(
                headers.generation,
                HeaderRuleChange::Save {
                    rule: text.clone(),
                    expected_revision: None,
                    placement: HeaderRulePlacement::Keep,
                },
            )
            .unwrap();
        assert_eq!(saved.rules[0], headers.rules[0]);
        assert!(!saved.rules[1].enabled);
        assert!(
            registry
                .change_response_transform(
                    saved.generation,
                    HeaderRuleChange::Remove {
                        id: "headers".into(),
                        expected_revision: 1
                    }
                )
                .is_err()
        );
        registry
            .set_autoresponses_enabled(false, saved.generation)
            .unwrap();
        assert!(
            registry
                .change_response_transform(
                    saved.generation,
                    HeaderRuleChange::Save {
                        rule: text.clone(),
                        expected_revision: Some(1),
                        placement: HeaderRulePlacement::Keep
                    }
                )
                .is_err()
        );
        text.enabled = true;
        let next = registry
            .change_response_transform(
                registry.status().generation,
                HeaderRuleChange::Save {
                    rule: text,
                    expected_revision: Some(1),
                    placement: HeaderRulePlacement::Keep,
                },
            )
            .unwrap();
        assert!(!next.autoresponses_enabled);
        assert_eq!(next.rules[1].revision, 2);
        assert_eq!(next.rules[0], headers.rules[0]);
    }
    fn rule(id: &str) -> Rule {
        Rule {
            id: id.into(),
            display_name: Some("Remove CSP".into()),
            enabled: false,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher {
                url: Some(transmog_automation::UrlCondition::Exact(
                    "https://example.test/".into(),
                )),
                ..RuleMatcher::default()
            },
            request: RequestActions::default(),
            response: ResponseActions {
                headers: vec![HeaderOperation::remove("Content-Security-Policy").unwrap()],
                ..ResponseActions::default()
            },
        }
    }
    fn save(
        registry: &AutomationRegistry,
        rule: Rule,
        expected: Option<u64>,
    ) -> Result<AutomationStatus, AppError> {
        registry.change_header_rule(
            registry.status().generation,
            HeaderRuleChange::Save {
                rule,
                expected_revision: expected,
                placement: HeaderRulePlacement::Keep,
            },
        )
    }
    #[test]
    fn scoped_saves_preserve_other_rules_pause_and_revisions() {
        let registry = setup(None);
        let mut unrelated = rule("ua");
        unrelated.revision = 17;
        unrelated.priority = -500_000;
        unrelated.enabled = true;
        unrelated.response = ResponseActions::default();
        unrelated.request.user_agent = Some(transmog_automation::UserAgentOverride {
            template: Some("Different UA".into()),
            ..Default::default()
        });
        let candidate = registry
            .validate(AutomationRuleSet {
                rules: vec![unrelated.clone()],
                autoresponses_enabled: false,
                ..Default::default()
            })
            .unwrap();
        registry.activate(&candidate.candidate_id).unwrap();
        let first = save(&registry, rule("headers"), None).unwrap();
        assert_eq!(first.rules[0], unrelated);
        assert!(!first.rules[1].enabled);
        assert!(!first.autoresponses_enabled);
        let mut updated = first.rules[1].clone();
        updated.enabled = true;
        updated
            .response
            .headers
            .push(HeaderOperation::set("Cache-Control", "no-cache").unwrap());
        let second = save(&registry, updated, Some(1)).unwrap();
        assert_eq!(second.rules[0], unrelated);
        assert_eq!(second.rules[1].revision, 2);
        assert!(!second.autoresponses_enabled);
        let registrations = registry.active.read().unwrap().compiled.registrations();
        assert_eq!(
            registrations.len(),
            2,
            "Auto-response pause must not gate header rules or UA policies"
        );
    }
    #[test]
    fn stale_edits_removed_targets_and_identity_collisions_preserve_current_work() {
        let registry = setup(None);
        let before = save(&registry, rule("headers"), None).unwrap();
        let saved = save(&registry, rule("headers"), Some(1)).unwrap();
        assert!(
            registry
                .change_header_rule(
                    before.generation,
                    HeaderRuleChange::Remove {
                        id: "headers".into(),
                        expected_revision: 1
                    }
                )
                .is_err()
        );
        assert!(save(&registry, rule("headers"), Some(1)).is_err());
        assert!(save(&registry, rule("headers"), None).is_err());
        assert_eq!(registry.status().rules, saved.rules);
        registry
            .change_header_rule(
                saved.generation,
                HeaderRuleChange::Remove {
                    id: "headers".into(),
                    expected_revision: 2,
                },
            )
            .unwrap();
        assert!(save(&registry, rule("headers"), Some(2)).is_err());
        assert!(registry.status().rules.is_empty());
    }
    #[test]
    fn order_changes_and_generation_checked_undo_do_not_rewrite_unrelated_rules() {
        let registry = setup(None);
        save(&registry, rule("first"), None).unwrap();
        let second = save(&registry, rule("second"), None).unwrap();
        let moved = registry
            .change_header_rule(
                second.generation,
                HeaderRuleChange::Move {
                    id: "second".into(),
                    expected_revision: 1,
                    earlier: true,
                },
            )
            .unwrap();
        assert_eq!(moved.rules[0].priority, 1);
        assert_eq!(moved.rules[1].priority, 0);
        assert!(moved.rules.iter().all(|r| r.revision == 2));
        let removed = moved.rules[1].clone();
        let after = registry
            .change_header_rule(
                moved.generation,
                HeaderRuleChange::Remove {
                    id: removed.id.clone(),
                    expected_revision: 2,
                },
            )
            .unwrap();
        registry
            .set_autoresponses_enabled(false, after.generation)
            .unwrap();
        assert!(
            registry
                .change_header_rule(
                    after.generation,
                    HeaderRuleChange::Restore {
                        rule: removed.clone()
                    }
                )
                .is_err()
        );
        let restored = registry
            .change_header_rule(
                registry.status().generation,
                HeaderRuleChange::Restore { rule: removed },
            )
            .unwrap();
        assert_eq!(restored.rules[1].revision, 3);
        assert!(!restored.autoresponses_enabled);
    }
    #[test]
    fn protected_invalid_and_advanced_actions_are_rejected_before_persistence() {
        let registry = setup(None);
        let mut draft = rule("bad");
        draft.response.headers = vec![HeaderOperation::set("Content-Encoding", "gzip").unwrap()];
        assert!(save(&registry, draft, None).is_err());
        let mut draft = rule("bad");
        draft.request.replace_body = Some(vec![1]);
        assert!(save(&registry, draft, None).is_err());
        let mut draft = rule("bad");
        draft.response.headers = vec![HeaderOperation::Remove("x\r\nbad".into())];
        assert!(save(&registry, draft, None).is_err());
        assert_eq!(registry.status().generation, 0);
    }
    #[test]
    fn saved_position_repairs_an_equal_priority_conflict_atomically() {
        let registry = setup(None);
        let mut advanced = rule("advanced");
        advanced.enabled = true;
        advanced.request.abort_reason = Some("Deliberate advanced rule".into());
        let candidate = registry
            .validate(AutomationRuleSet {
                rules: vec![advanced.clone()],
                ..Default::default()
            })
            .unwrap();
        registry.activate(&candidate.candidate_id).unwrap();
        let mut draft = rule("headers");
        draft.enabled = true;
        assert!(save(&registry, draft.clone(), None).is_err());
        assert_eq!(registry.status().rules, vec![advanced.clone()]);
        let result = registry
            .change_header_rule(
                1,
                HeaderRuleChange::Save {
                    rule: draft,
                    expected_revision: None,
                    placement: HeaderRulePlacement::After,
                },
            )
            .unwrap();
        assert_eq!(result.rules[0], advanced);
        assert_eq!(result.rules[1].priority, 1);
    }
    #[test]
    fn persisted_header_definitions_survive_restart_and_failed_writes_keep_prior_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rules");
        let registry = setup(Some(path.clone()));
        let saved = save(&registry, rule("headers"), None).unwrap();
        let recovered = setup(Some(path.clone()));
        assert_eq!(recovered.status().rules, saved.rules);
        std::fs::create_dir(super::super::slot_path(&path, 0)).unwrap();
        let mut updated = rule("headers");
        updated.enabled = true;
        assert!(save(&registry, updated, Some(1)).is_err());
        assert_eq!(registry.status().rules, saved.rules);
    }
}
