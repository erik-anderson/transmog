//! Bounded batch preparation and one atomic rule activation.

use serde::{Deserialize, Serialize};
use transmog_automation::{RequestActions, ResponseActions, Rule, RuleMatcher, UrlCondition};

use crate::{
    AppError, Application, AutomationRuleSet, AutomationStatus, ErrorCategory, SessionResponseAsset,
};

const PRIORITY_BASE: i32 = -1_000_000;

/// Explicitly reviewed Traffic responses to turn into rules.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoResponseBatchInput {
    /// IDs in the desired first-match order, at most 256.
    pub ids: Vec<String>,
    /// Rule generation seen by the review pane.
    pub generation: u64,
}

/// One committed batch, with IDs to select in the rule manager.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResponseBatchResult {
    /// New atomic rule snapshot.
    pub status: AutomationStatus,
    /// Rules created in the reviewed order.
    pub created_ids: Vec<String>,
}

impl Application {
    /// Copies complete selected responses and activates their rules together.
    ///
    /// # Errors
    /// Rejects invalid selection, eviction, stale state or storage failures.
    /// All accepted source bodies are leased before copying; failed activation
    /// removes newly created asset references and leaves current rules intact.
    pub async fn create_autoresponse_batch(
        &self,
        input: AutoResponseBatchInput,
    ) -> Result<AutoResponseBatchResult, AppError> {
        if input.ids.is_empty() || input.ids.len() > 256 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Select 1–256 completed responses per batch.",
                false,
            ));
        }
        let current = self.automation_status();
        if current.rules.len() + input.ids.len()
            > transmog_automation::AutomationLimits::default().max_rules
        {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "This batch would exceed the 1024-rule limit. Remove rules or select fewer responses.",
                false,
            ));
        }
        if current.generation != input.generation {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "Rules changed during batch review. Review again before creating rules.",
                true,
            ));
        }
        let mut sources = Vec::new();
        let mut leases = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in &input.ids {
            if !seen.insert(id) {
                return Err(AppError::new(
                    ErrorCategory::InvalidInput,
                    "A traffic entry was selected twice.",
                    false,
                ));
            }
            leases.push(self.prepare_response_file(id, "client-response")?);
            let detail = self.session_detail(id)?;
            let request = detail
                .requests
                .iter()
                .find(|head| head.boundary == "client-request")
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCategory::Unavailable,
                        "Original request is unavailable.",
                        false,
                    )
                })?;
            let method = request.method.clone().ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "Request method is unavailable.",
                    false,
                )
            })?;
            let url = request.target.clone().ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "Request URL is unavailable.",
                    false,
                )
            })?;
            sources.push((id.clone(), method, url));
        }
        let mut created_assets = Vec::new();
        let result = self
            .copy_autoresponse_sources(sources, &current, &mut created_assets)
            .await;
        if result.is_err()
            && !created_assets.is_empty()
            && let Err(error) = self.response_assets.discard_created(&created_assets)
        {
            self.record_diagnostic(
                crate::DiagnosticLevel::Warning,
                "autoresponse",
                "batch-asset-cleanup",
                &error.message,
            );
        }
        drop(leases);
        result
    }

    async fn copy_autoresponse_sources(
        &self,
        sources: Vec<(String, String, String)>,
        current: &AutomationStatus,
        created_assets: &mut Vec<String>,
    ) -> Result<AutoResponseBatchResult, AppError> {
        let mut rules = Vec::new();
        let mut created_ids = Vec::new();
        for (source, method, url) in sources {
            let suffix = crate::response_assets::random_hex()?;
            let asset = self
                .create_response_asset_from_session(SessionResponseAsset {
                    id: format!("autoresponse-{suffix}"),
                    revision: 1,
                    exchange_id: source,
                    boundary: "client-response".into(),
                    decoded_body: None,
                    preserve_content_encoding: true,
                })
                .await?;
            created_assets.push(asset.asset_ref());
            let id = format!("autoresponse-rule-{suffix}");
            let name = format!("{method} {url}");
            rules.push(Rule {
                id: id.clone(),
                display_name: Some(name.chars().take(128).collect()),
                enabled: true,
                revision: 1,
                priority: PRIORITY_BASE,
                matcher: RuleMatcher {
                    method: Some(method),
                    url: Some(UrlCondition::Exact(url)),
                    ..RuleMatcher::default()
                },
                request: RequestActions {
                    response_asset: Some(asset.asset_ref()),
                    ..RequestActions::default()
                },
                response: ResponseActions::default(),
            });
            created_ids.push(id);
        }
        let mut existing = current
            .rules
            .iter()
            .filter(|rule| rule.request.response_asset.is_some())
            .cloned()
            .collect::<Vec<_>>();
        existing.sort_by_key(|rule| (rule.priority, rule.id.clone()));
        rules.extend(existing);
        for (index, rule) in rules.iter_mut().enumerate() {
            let priority = PRIORITY_BASE
                + i32::try_from(index)
                    .map_err(|_| AppError::new(ErrorCategory::Limit, "Too many rules.", false))?;
            if rule.priority != priority {
                rule.priority = priority;
                rule.revision += 1;
            }
        }
        rules.extend(
            current
                .rules
                .iter()
                .filter(|rule| rule.request.response_asset.is_none())
                .cloned(),
        );
        let candidate = self.validate_automation(AutomationRuleSet {
            schema_version: 1,
            generation: current.generation,
            autoresponses_enabled: current.autoresponses_enabled,
            rules,
        })?;
        let status = self.activate_automation(&candidate.candidate_id)?;
        Ok(AutoResponseBatchResult {
            status,
            created_ids,
        })
    }
}
