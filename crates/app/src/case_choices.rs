//! Metadata-only suggestions; a match is not evidence that a rule ran.
use super::{AppError, Application, invalid};
use crate::{ExperimentSelection, comparison::ComparisonScope};
use serde::Serialize;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use transmog_automation::{CompiledRequestMatcher, HeaderCondition, HeaderPredicate, Rule};
use transmog_core::{HeaderBlock, RequestHead, ResponseHead, observe::ExchangeBoundary};

/// One indivisible saved rule or exact experiment version.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseExportChoice {
    /// Stable picker identity, independent of relevance or label.
    pub id: String,
    /// Human rule name or experiment/variant/revision label.
    pub label: String,
    /// Standalone rule identity, mutually exclusive with selection.
    pub rule_id: Option<String>,
    /// Whole saved experiment version, including all its rules.
    pub selection: Option<ExperimentSelection>,
    /// matched, unmatched, unknown, or not-assessed (no evidence chosen).
    pub relevance: &'static str,
}
/// Suggestions cover the complete selected scope, before UI windowing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseExportChoices {
    /// Standalone rules and whole experiment versions in one collection.
    pub choices: Vec<CaseExportChoice>,
    /// Unique request membership at assessment time.
    pub request_count: usize,
    /// False means rules-and-notes only, rather than a measured empty capture.
    pub evidence_selected: bool,
}
struct Samples {
    heads: Vec<(Option<RequestHead>, Option<ResponseHead>)>,
    complete: bool,
    remaining: usize,
    deadline: Instant,
}
impl Application {
    /// Includes all requests required by explicitly selected comparisons.
    /// # Errors
    /// Rejects expired comparisons, invalid scopes, or excessive selections.
    pub fn case_export_scope(
        &self,
        scope: Option<ComparisonScope>,
        comparison_ids: &[String],
    ) -> Result<Option<ComparisonScope>, AppError> {
        if comparison_ids.is_empty() {
            return Ok(scope);
        }
        if comparison_ids.len() > 16 {
            return Err(invalid("Select up to 16 comparisons"));
        }
        let mut ids: std::collections::BTreeSet<String> = scope
            .as_ref()
            .map(|s| crate::export_review::scope_ids(self, s))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .collect();
        let choices = self.case_comparison_choices();
        for id in comparison_ids {
            let (_, _, required) =
                choices
                    .iter()
                    .find(|(key, _, _)| key == id)
                    .ok_or_else(|| {
                        invalid("A selected comparison expired. Compare the requests again.")
                    })?;
            ids.extend(required.iter().cloned());
        }
        Ok(Some(ComparisonScope::Selected {
            ids: ids.into_iter().collect(),
        }))
    }
}
impl Samples {
    fn relevance(&mut self, rules: &[Rule]) -> &'static str {
        if self.heads.is_empty() {
            return if self.complete {
                "unmatched"
            } else {
                "unknown"
            };
        }
        let mut unknown = !self.complete;
        for rule in rules {
            if self.remaining == 0 || Instant::now() >= self.deadline {
                return "unknown";
            }
            let Ok(compiled) = CompiledRequestMatcher::new(&rule.matcher) else {
                unknown = true;
                continue;
            };
            let response_conditions = rule.matcher.response_status.is_some()
                || rule.matcher.response_status_class.is_some()
                || !rule.matcher.response_headers.is_empty();
            for (request, response) in &self.heads {
                if self.remaining == 0 || Instant::now() >= self.deadline {
                    return "unknown";
                }
                self.remaining -= 1;
                let Some(request) = request else {
                    unknown = true;
                    continue;
                };
                if redacted(&rule.matcher.request_headers, &request.headers) {
                    unknown = true;
                    continue;
                }
                if !compiled.matches_request(request) {
                    continue;
                }
                if response_conditions {
                    let Some(response) = response else {
                        unknown = true;
                        continue;
                    };
                    if redacted(&rule.matcher.response_headers, &response.headers) {
                        unknown = true;
                        continue;
                    }
                    if !compiled.matches_response(request, response) {
                        continue;
                    }
                }
                return "matched";
            }
        }
        if unknown { "unknown" } else { "unmatched" }
    }
}
fn redacted(predicates: &[HeaderPredicate], headers: &HeaderBlock) -> bool {
    predicates.iter().any(|p| {
        !matches!(p.condition, HeaderCondition::Exists)
            && headers
                .iter()
                .any(|h| h.is_redacted() && h.name().eq_ignore_ascii_case(p.name.as_bytes()))
    })
}
fn request_bytes(head: &RequestHead) -> usize {
    head.method.len()
        + head.target.scheme.len()
        + head.target.authority.len()
        + head.target.path.len()
        + head.target.query.as_ref().map_or(0, String::len)
        + header_bytes(&head.headers)
}
fn header_bytes(headers: &HeaderBlock) -> usize {
    headers
        .iter()
        .map(|h| h.name().len() + h.value().len() + 64)
        .sum()
}
impl Application {
    /// Suggests matching saved definitions without running actions or reading bodies.
    /// Disabled rules and paused experiments are assessed independently of activation.
    /// # Errors
    /// Rejects unavailable/invalid evidence scopes or more than 100,000 requests.
    #[allow(clippy::too_many_lines)] // Keep one bounded metadata-only assessment transaction together.
    pub fn case_export_choices(
        &self,
        scope: Option<&ComparisonScope>,
    ) -> Result<CaseExportChoices, AppError> {
        let ids = scope
            .map(|s| crate::export_review::scope_ids(self, s))
            .transpose()?
            .unwrap_or_default();
        if ids.len() > 100_000 {
            return Err(invalid(
                "Choose up to 100,000 requests to find matching rules",
            ));
        }
        let ids = ids.into_iter().collect::<HashSet<_>>();
        let mut samples = Samples {
            heads: vec![],
            complete: true,
            remaining: 1_000_000,
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let mut charge = 0usize;
        // Snapshot only heads. No bodies, assets, scripts, or export review slots are acquired.
        for id in &ids {
            if charge >= 32 * 1024 * 1024 || Instant::now() >= samples.deadline {
                samples.complete = false;
                break;
            }
            let numeric = crate::inspector::parse_session_id(id)?;
            let snapshot = self
                .service
                .catalog()
                .get(transmog_core::intercept::ExchangeId(numeric))
                .ok_or_else(|| {
                    invalid("An included request was removed; choose the evidence again")
                })?;
            let request = snapshot
                .request_heads
                .iter()
                .find(|h| h.boundary == ExchangeBoundary::ClientRequest)
                .filter(|_| self.traces.entry(id).is_none_or(|e| e.target_known))
                .map(|h| &h.head);
            let response = snapshot
                .response_heads
                .iter()
                .find(|h| h.boundary == ExchangeBoundary::ClientResponse)
                .map(|h| &h.head);
            charge = charge.saturating_add(
                256 + request.map_or(0, request_bytes)
                    + response.map_or(0, |h| header_bytes(&h.headers)),
            );
            if charge > 32 * 1024 * 1024 {
                samples.complete = false;
                break;
            }
            samples.heads.push((request.cloned(), response.cloned()));
        }
        let status = self.automation.status();
        let owned = status
            .experiments
            .iter()
            .flat_map(|e| &e.variants)
            .flat_map(|v| &v.revisions)
            .flat_map(|r| &r.rules)
            .map(|r| &r.id)
            .collect::<HashSet<_>>();
        let mut result = CaseExportChoices {
            choices: vec![],
            request_count: ids.len(),
            evidence_selected: scope.is_some(),
        };
        for rule in &status.rules {
            if owned.contains(&rule.id) {
                continue;
            }
            result.choices.push(CaseExportChoice {
                id: format!("rule:{}", rule.id),
                label: rule.display_name.clone().unwrap_or_else(|| rule.id.clone()),
                rule_id: Some(rule.id.clone()),
                selection: None,
                relevance: if scope.is_some() {
                    samples.relevance(std::slice::from_ref(rule))
                } else {
                    "not-assessed"
                },
            });
        }
        for experiment in &status.experiments {
            for variant in &experiment.variants {
                for revision in &variant.revisions {
                    if revision.rules.is_empty() {
                        continue;
                    }
                    result.choices.push(CaseExportChoice {
                        id: format!(
                            "version:{}",
                            serde_json::to_string(&(
                                &experiment.id,
                                &variant.id,
                                revision.revision
                            ))
                            .map_err(|_| invalid("Invalid experiment identity"))?
                        ),
                        label: format!(
                            "{} / {} / revision {}",
                            experiment.name, variant.name, revision.revision
                        ),
                        rule_id: None,
                        selection: Some(ExperimentSelection {
                            experiment_id: experiment.id.clone(),
                            variant_id: variant.id.clone(),
                            revision: revision.revision,
                        }),
                        relevance: if scope.is_some() {
                            samples.relevance(&revision.rules)
                        } else {
                            "not-assessed"
                        },
                    });
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "case_choices_tests.rs"]
mod tests;
