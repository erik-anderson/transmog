//! Explicit collection of retained Traffic and ordinary saved responses.

use super::{AutomationRegistry, AutomationRuleSet, AutomationStatus, ResponseSet};
use crate::{AppError, Application, ErrorCategory, ResponseBodyFileMode, SessionResponseAsset};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use transmog_automation::{RequestActions, ResponseActions, Rule, RuleMatcher, UrlCondition};

const PRIORITY_BASE: i32 = -1_000_000;

/// Explicit source identities. Owned response-set content is never an ordinary rule.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResponseSetIntakeSource {
    /// Complete retained client responses, in the reviewed order.
    Traffic {
        /// Explicit Traffic identities, at most 1,000.
        ids: Vec<String>,
    },
    /// Ordinary saved-response rules; collection preserves their evaluation order.
    Rules {
        /// Explicit ordinary saved-response identities, at most 1,000.
        ids: Vec<String>,
    },
}

/// A saved destination selected by identity, or a newly named inactive workspace.
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ResponseSetIntakeDestination {
    /// Exact current collection reviewed before appending.
    Existing {
        /// Destination owner identity.
        id: String,
        /// Immutable owner creation nonce.
        incarnation: String,
        /// Exact current collection revision.
        revision: u64,
    },
    /// Create and populate one workspace in the same durable transaction.
    New {
        /// New owner identity.
        id: String,
        /// User-authored collection name.
        name: String,
        /// Investigation notes.
        notes: String,
    },
}

/// Collection leaves originals in place unless Move is explicitly selected.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseSetIntakeMode {
    /// Preserve original rules and their live behavior.
    #[default]
    Copy,
    /// Remove ordinary originals only after the destination is durably saved.
    Move,
}

/// Generation and destination checks bind review and completion to exact saved work.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseSetIntakeInput {
    /// Automation generation seen during review.
    pub generation: u64,
    /// Explicit source identities.
    pub source: ResponseSetIntakeSource,
    /// Exact existing or newly named destination.
    pub destination: ResponseSetIntakeDestination,
    /// Copy by default; Move is only available for ordinary saved rules.
    #[serde(default)]
    pub mode: ResponseSetIntakeMode,
    /// Explicit acknowledgement of the authoritative overlap review.
    #[serde(default)]
    pub acknowledge_overlaps: bool,
}

/// A readable source definition; bodies stay in native storage.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSetIntakeRow {
    /// Original source identity.
    pub id: String,
    /// Original friendly name.
    pub name: String,
    /// Authored method, or ANY.
    pub method: String,
    /// Readable URL condition; the complete matcher is preserved in storage.
    pub url: String,
    /// Response status and body size summary.
    pub response: String,
    /// Explicit copied or linked body-source description.
    pub body_source: String,
    /// Preserved authored inclusion state.
    pub included: bool,
}

/// Authoritative review of appending content, without changing or activating it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSetIntakeReview {
    /// Selected responses in the order they will be appended.
    pub rows: Vec<ResponseSetIntakeRow>,
    /// Potential overlaps; appending never silently replaces a response.
    pub warnings: Vec<String>,
    /// Selected ordinary rules currently enabled and not globally paused.
    pub active_count: usize,
    /// Existing response count before append.
    pub destination_count: usize,
}

/// Successful atomic collection. Opening the destination remains an explicit UI action.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSetIntakeResult {
    /// Complete committed automation state.
    pub status: AutomationStatus,
    /// Destination identity.
    pub set_id: String,
    /// Saved destination name.
    pub set_name: String,
    /// Number of definitions appended.
    pub added_count: usize,
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

fn conflict(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, true)
}

fn source_ids(source: &ResponseSetIntakeSource) -> &[String] {
    match source {
        ResponseSetIntakeSource::Traffic { ids } | ResponseSetIntakeSource::Rules { ids } => ids,
    }
}

fn check_input(input: &ResponseSetIntakeInput) -> Result<(), AppError> {
    let ids = source_ids(&input.source);
    if ids.is_empty() || ids.len() > crate::automation::MAX_RESPONSES {
        return Err(invalid(
            "Select 1–1,000 responses to add to a response set.",
        ));
    }
    let mut unique = HashSet::new();
    if ids.iter().any(|id| !unique.insert(id)) {
        return Err(invalid("A response was selected more than once."));
    }
    if input.mode == ResponseSetIntakeMode::Move
        && matches!(input.source, ResponseSetIntakeSource::Traffic { .. })
    {
        return Err(invalid("Captured Traffic is copied into a response set."));
    }
    Ok(())
}

fn destination_rules<'a>(
    document: &'a AutomationRuleSet,
    input: &ResponseSetIntakeInput,
) -> Result<&'a [Rule], AppError> {
    if document.generation != input.generation {
        return Err(conflict(
            "Automation changed. Refresh the collection review before retrying.",
        ));
    }
    let rules = match &input.destination {
        ResponseSetIntakeDestination::Existing {
            id,
            incarnation,
            revision,
        } => {
            let set = document
                .response_sets
                .iter()
                .find(|set| &set.id == id && &set.incarnation == incarnation)
                .ok_or_else(|| conflict("The selected response set is no longer available."))?;
            if set.revision != *revision {
                return Err(conflict(
                    "The destination changed. Review its saved collection before adding responses.",
                ));
            }
            set.rules.as_slice()
        }
        ResponseSetIntakeDestination::New { id, name, notes } => {
            if document.response_sets.iter().any(|set| &set.id == id) {
                return Err(conflict("This response set identity is already in use."));
            }
            if name.trim().is_empty()
                || name.len() > 128
                || name.chars().any(char::is_control)
                || notes.len() > 16 * 1024
            {
                return Err(invalid(
                    "Enter a response set name of at most 128 UTF-8 bytes and notes of at most 16,384 UTF-8 bytes.",
                ));
            }
            if document.response_sets.len() >= 32 {
                return Err(invalid("At most 32 response sets can be retained."));
            }
            &[]
        }
    };
    if rules.len() + source_ids(&input.source).len() > crate::automation::MAX_RESPONSES {
        return Err(invalid(
            "A response set can contain at most 1,000 responses.",
        ));
    }
    Ok(rules)
}

fn ordinary_sources(
    document: &AutomationRuleSet,
    ids: &[String],
) -> Result<Vec<(String, Rule)>, AppError> {
    let mut sources = Vec::with_capacity(ids.len());
    for id in ids {
        let rule = document
            .rules
            .iter()
            .find(|rule| &rule.id == id)
            .filter(|rule| {
                rule.request.response_asset.is_some()
                    && !document.response_sets.iter().any(|set| set.owns(id))
            })
            .ok_or_else(|| {
                invalid("Select only ordinary saved-response rules. Response-set content and other rule kinds cannot be collected here.")
            })?;
        sources.push((id.clone(), rule.clone()));
    }
    sources.sort_by(|(_, left), (_, right)| {
        (left.priority, &left.id).cmp(&(right.priority, &right.id))
    });
    Ok(sources)
}

fn source_url(rule: &Rule) -> String {
    match &rule.matcher.url {
        Some(UrlCondition::Exact(url)) => url.clone(),
        Some(UrlCondition::Pattern(pattern)) => pattern.address.clone(),
        Some(UrlCondition::Regex(pattern)) => pattern.pattern.clone(),
        None => rule
            .matcher
            .host
            .clone()
            .unwrap_or_else(|| "Any URL".into()),
    }
}

fn warnings(
    document: &AutomationRuleSet,
    destination: &[Rule],
    sources: &[(String, Rule)],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut count = 0_usize;
    let selected = sources.iter().map(|(id, _)| id).collect::<HashSet<_>>();
    for (index, (_, rule)) in sources.iter().enumerate() {
        let peers = destination
            .iter()
            .chain(sources[..index].iter().map(|(_, rule)| rule));
        for other in peers {
            if rule.matcher.may_overlap_request(&other.matcher) {
                count += 1;
                if warnings.len() < 64 {
                    warnings.push(format!(
                        "{} may overlap {} in this collection. Both responses will be kept; the first included match wins.",
                        rule.display_name.as_deref().unwrap_or(&rule.id),
                        other.display_name.as_deref().unwrap_or(&other.id)
                    ));
                }
            }
        }
        for other in document.rules.iter().filter(|other| {
            other.enabled
                && !selected.contains(&other.id)
                && !document
                    .response_sets
                    .iter()
                    .any(|set| set.paused && set.owns(&other.id))
        }) {
            if document.autoresponses_enabled
                && other.request.response_asset.is_some()
                && rule.matcher.may_overlap_request(&other.matcher)
            {
                count += 1;
                if warnings.len() < 64 {
                    warnings.push(format!(
                        "{} may overlap live rule {} when this set is applied. Live traffic will not change when these responses are added.",
                        rule.display_name.as_deref().unwrap_or(&rule.id),
                        other.display_name.as_deref().unwrap_or(&other.id)
                    ));
                }
            }
        }
    }
    if count > warnings.len() {
        warnings.push(format!(
            "{} more possible overlaps.",
            count - warnings.len()
        ));
    }
    warnings
}

impl Application {
    fn intake_sources(
        &self,
        document: &AutomationRuleSet,
        source: &ResponseSetIntakeSource,
    ) -> Result<Vec<(String, Rule)>, AppError> {
        let ResponseSetIntakeSource::Traffic { ids } = source else {
            return ordinary_sources(document, source_ids(source));
        };
        let mut sources = Vec::with_capacity(ids.len());
        let mut leases = Vec::with_capacity(ids.len());
        for id in ids {
            leases.push(self.prepare_response_file(id, "client-response")?);
            let detail = self.session_detail(id)?;
            let request = detail
                .requests
                .iter()
                .find(|head| head.boundary == "client-request")
                .ok_or_else(|| invalid("The captured request is unavailable."))?;
            let method = request
                .method
                .clone()
                .ok_or_else(|| invalid("The captured request method is unavailable."))?;
            let url = request
                .target
                .as_deref()
                .ok_or_else(|| invalid("The captured request URL is unavailable."))?;
            let url = transmog_automation::normalize_url(url).map_err(|error| invalid(&error))?;
            let mut name = format!("{method} {url}");
            while name.len() > 128 {
                name.pop();
            }
            sources.push((
                id.clone(),
                Rule {
                    id: id.clone(),
                    display_name: Some(name),
                    enabled: true,
                    revision: 1,
                    priority: PRIORITY_BASE,
                    matcher: RuleMatcher {
                        method: Some(method),
                        url: Some(UrlCondition::Exact(url)),
                        ..RuleMatcher::default()
                    },
                    request: RequestActions::default(),
                    response: ResponseActions::default(),
                },
            ));
        }
        Ok(sources)
    }

    /// Reviews exact selected definitions and potential overlap without saving anything.
    ///
    /// # Errors
    /// Rejects stale destinations, unavailable captured bytes, and ineligible rule kinds.
    pub fn review_response_set_intake(
        &self,
        input: &ResponseSetIntakeInput,
    ) -> Result<ResponseSetIntakeReview, AppError> {
        check_input(input)?;
        let document = self.automation.intake_document();
        let destination = destination_rules(&document, input)?;
        let sources = self.intake_sources(&document, &input.source)?;
        let assets = self.response_assets.list();
        if sources.iter().any(|(_, rule)| {
            rule.request
                .response_asset
                .as_ref()
                .is_some_and(|reference| {
                    !assets.iter().any(|asset| &asset.asset_ref() == reference)
                })
        }) {
            return Err(invalid("A selected saved response asset is unavailable."));
        }
        let rows = sources
            .iter()
            .map(|(id, rule)| {
                let asset = assets.iter().find(|asset| {
                    Some(asset.asset_ref()).as_ref() == rule.request.response_asset.as_ref()
                });
                ResponseSetIntakeRow {
                    id: id.clone(),
                    name: rule.display_name.clone().unwrap_or_else(|| rule.id.clone()),
                    method: rule.matcher.method.clone().unwrap_or_else(|| "ANY".into()),
                    url: source_url(rule),
                    response: asset.map_or_else(
                        || "Complete captured response".into(),
                        |asset| format!("{} · {} bytes", asset.status, asset.body_bytes),
                    ),
                    body_source: asset.map_or_else(
                        || "Copied from retained Traffic".into(),
                        |asset| {
                            if let Some(link) = &asset.linked_body {
                                format!("Linked file: {}", link.path.display())
                            } else if let Some(path) = &asset.copied_from {
                                format!("Copied file: {}", path.display())
                            } else {
                                "Saved response bytes".into()
                            }
                        },
                    ),
                    included: rule.enabled,
                }
            })
            .collect();
        Ok(ResponseSetIntakeReview {
            rows,
            warnings: warnings(&document, destination, &sources),
            active_count: if document.autoresponses_enabled
                && matches!(input.source, ResponseSetIntakeSource::Rules { .. })
            {
                sources.iter().filter(|(_, rule)| rule.enabled).count()
            } else {
                0
            },
            destination_count: destination.len(),
        })
    }

    /// Appends exact definitions and optionally removes ordinary originals atomically.
    ///
    /// # Errors
    /// Any source, CAS, validation or durable-write failure leaves original rules and
    /// the destination unchanged. New Traffic assets are discarded on failure.
    pub async fn collect_response_set(
        &self,
        input: ResponseSetIntakeInput,
    ) -> Result<ResponseSetIntakeResult, AppError> {
        check_input(&input)?;
        let document = self.automation.intake_document();
        let destination = destination_rules(&document, &input)?;
        let mut sources = self.intake_sources(&document, &input.source)?;
        if !input.acknowledge_overlaps && !warnings(&document, destination, &sources).is_empty() {
            return Err(conflict(
                "Review and acknowledge overlapping matches before adding responses.",
            ));
        }
        let mut created_assets = Vec::new();
        let mut leases = Vec::new();
        if let ResponseSetIntakeSource::Traffic { ids } = &input.source {
            for id in ids {
                leases.push(self.prepare_response_file(id, "client-response")?);
            }
        }
        let result = async {
            for (source_id, rule) in &mut sources {
                let suffix = crate::response_assets::random_hex()?;
                rule.id = format!("autoresponse-rule-{suffix}");
                rule.revision = 1;
                if matches!(input.source, ResponseSetIntakeSource::Traffic { .. }) {
                    let asset = self
                        .create_response_asset_from_session(SessionResponseAsset {
                            id: format!("autoresponse-{suffix}"),
                            revision: 1,
                            exchange_id: source_id.clone(),
                            boundary: "client-response".into(),
                            decoded_body: None,
                            body_path: None,
                            body_file_mode: ResponseBodyFileMode::Copy,
                            preserve_content_encoding: true,
                        })
                        .await?;
                    let reference = asset.asset_ref();
                    created_assets.push(reference.clone());
                    rule.request.response_asset = Some(reference);
                }
            }
            let _operation = self
                .response_set_operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.automation
                .commit_response_set_intake(&input, sources)?;
            let status = self.automation_status();
            let id = match &input.destination {
                ResponseSetIntakeDestination::Existing { id, .. }
                | ResponseSetIntakeDestination::New { id, .. } => id,
            };
            let set = status
                .response_sets
                .iter()
                .find(|set| &set.id == id)
                .ok_or_else(|| conflict("The response set is no longer available."))?;
            Ok(ResponseSetIntakeResult {
                set_id: set.id.clone(),
                set_name: set.name.clone(),
                added_count: source_ids(&input.source).len(),
                status,
            })
        }
        .await;
        if result.is_err()
            && !created_assets.is_empty()
            && let Err(error) = self.response_assets.discard_created(&created_assets)
        {
            self.record_diagnostic(
                crate::DiagnosticLevel::Warning,
                "response-sets",
                "intake-asset-cleanup",
                &error.message,
            );
        }
        drop(leases);
        result
    }
}

impl AutomationRegistry {
    fn intake_document(&self) -> AutomationRuleSet {
        self.active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone()
    }

    fn commit_response_set_intake(
        &self,
        input: &ResponseSetIntakeInput,
        sources: Vec<(String, Rule)>,
    ) -> Result<(), AppError> {
        let mut document = self.intake_document();
        destination_rules(&document, input)?;
        if let ResponseSetIntakeSource::Rules { ids } = &input.source {
            ordinary_sources(&document, ids)?;
        }
        if input.mode == ResponseSetIntakeMode::Move {
            let ids = source_ids(&input.source);
            document.rules.retain(|rule| !ids.contains(&rule.id));
        }
        let id = match &input.destination {
            ResponseSetIntakeDestination::Existing { id, .. } => id,
            ResponseSetIntakeDestination::New { id, name, notes } => {
                document.response_sets.push(ResponseSet::new(
                    id.clone(),
                    name.trim().into(),
                    notes.clone(),
                    Vec::new(),
                )?);
                id
            }
        };
        let set = document
            .response_sets
            .iter_mut()
            .find(|set| &set.id == id)
            .ok_or_else(|| conflict("The selected response set is no longer available."))?;
        let retained = set.snapshots().flatten().cloned().collect::<Vec<_>>();
        let old_count = set.rules.len();
        set.rules.extend(sources.into_iter().map(|(_, rule)| rule));
        for (index, rule) in set.rules.iter_mut().enumerate() {
            let priority =
                PRIORITY_BASE + i32::try_from(index).map_err(|_| invalid("Too many responses."))?;
            if rule.priority != priority {
                rule.priority = priority;
                if index < old_count {
                    let revision = retained
                        .iter()
                        .filter(|old| old.id == rule.id)
                        .map(|old| old.revision)
                        .max()
                        .unwrap_or(rule.revision);
                    rule.revision = revision
                        .checked_add(1)
                        .ok_or_else(|| invalid("Response revision exhausted."))?;
                }
            }
        }
        if matches!(
            input.destination,
            ResponseSetIntakeDestination::Existing { .. }
        ) {
            set.revision = set
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("Response set revision exhausted."))?;
        }
        set.modified_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .ok_or_else(|| invalid("The current time is unavailable."))?;
        for set in &mut document.response_sets {
            set.undo_apply = None;
        }
        super::validate_document(&document)?;
        for rules in document
            .response_sets
            .iter()
            .flat_map(ResponseSet::snapshots)
        {
            transmog_automation::compile_with_assets(
                rules.to_vec(),
                self.limits,
                Some(self.assets.clone()),
            )
            .map_err(super::compile_error)?;
        }
        let candidate = self.validate_candidate(document)?;
        self.activate(&candidate.candidate_id)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "response_set_intake_tests.rs"]
mod tests;
