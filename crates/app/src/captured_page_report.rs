//! Bounded local diagnostics for a frozen preview, without credential values.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Which captured requests may supply resources.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapturedPageScope {
    /// The selected entry's original trace, or the current live capture.
    #[default]
    OriginalTrace,
    /// Any captured traffic currently loaded in this window.
    AllLoaded,
}
/// Options snapshotted before preparing an isolated preview.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedPageOptions {
    /// Exact organizational page choice, validated before a named-set preview.
    #[serde(default)]
    pub page_reference: Option<crate::ExperimentPreviewPage>,
    /// Window-owned package selection; only the native case adapter resolves it.
    #[serde(default)]
    pub case_preview: Option<crate::CasePreviewOptions>,
    /// Exact saved experiment revision; independent of live participation and pause.
    #[serde(default)]
    pub variant: Option<crate::ExperimentSelection>,
    /// Resource source scope; mixing captures is always explicit.
    #[serde(default)]
    pub scope: CapturedPageScope,
    /// Saved rule IDs participating only in this preview. None selects captured responses.
    #[serde(default)]
    pub rule_ids: Option<Vec<String>>,
    /// Reject a stale picker instead of silently using changed saved rules.
    #[serde(default)]
    pub generation: Option<u64>,
}
/// Immutable rule and byte identity; authored header values are never reported.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRuleSnapshot {
    /// Stable saved rule identity.
    pub id: String,
    /// Friendly saved name.
    pub name: String,
    /// Saved rule revision.
    pub revision: u64,
    /// Exact asset revision reference.
    pub asset: String,
    /// SHA-256 of the decoded response bytes.
    pub sha256: String,
    /// Frozen decoded byte count.
    pub bytes: u64,
    /// Earlier participating rule with equivalent conditions, if any.
    pub shadowed_by: Option<String>,
}
/// A retained response variant and why it is available or omitted.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedResourceDecision {
    /// Workspace entry from which these bytes came.
    pub entry_id: String,
    /// Friendly source filename, or Live traffic.
    pub source: String,
    /// True while the originating entry remains in the owner window.
    pub source_available: bool,
    /// Original method.
    pub method: String,
    /// Canonical resource URL, bounded for diagnostics.
    pub url: String,
    /// Captured request UTC time, when recorded.
    pub unix_millis: Option<u64>,
    /// Decoded response bytes, if prepared.
    pub bytes: Option<u64>,
    /// SHA-256 of the exact decoded response, when available.
    pub sha256: Option<String>,
    /// Matching/selection rule or omission reason.
    pub decision: String,
}
/// One actual intercepted browser request; header values and bodies are absent.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedPreviewRequest {
    /// Local monotonically increasing observation identity.
    pub id: u64,
    /// Requested method.
    pub method: String,
    /// Requested URL, bounded for diagnostics.
    pub url: String,
    /// served or missing (a network failure).
    pub outcome: String,
    /// Selected source traffic entry, when matched.
    pub entry_id: Option<String>,
    /// Winning frozen rule identity, independent of retained Traffic.
    pub rule_id: Option<String>,
    /// Exact delivered body identity, when served.
    pub sha256: Option<String>,
    /// Selection or miss reason, without credential values.
    pub reason: String,
}
/// Current resource decisions and local browser hit/miss counters.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedPageReport {
    /// Selected HTML URL.
    pub url: String,
    /// Chosen captured source scope.
    pub scope: CapturedPageScope,
    /// Friendly selected trace filename or Live traffic.
    pub source: String,
    /// Immutable original trace identity; None denotes current live capture.
    pub source_trace_id: Option<String>,
    /// Native script choice, if a browser has been opened.
    pub scripts_enabled: Option<bool>,
    /// Frozen selected document identity, even after Traffic eviction.
    pub selected_entry_id: String,
    /// Engine used for rendering; does not qualify another browser engine.
    pub runtime: String,
    /// Whether retained scene bytes can still open a related preview.
    pub bytes_available: bool,
    /// Participating rules in effective first-match order.
    pub rules: Vec<PreviewRuleSnapshot>,
    /// Exact saved experiment selection, independent of mutable names and live state.
    pub variant: Option<crate::ExperimentSelection>,
    /// Portable case/revision provenance; never interpreted as a local saved selection.
    pub case_origin: Option<String>,
    /// Response variants successfully frozen.
    pub available: usize,
    /// Captured variants omitted during preparation.
    pub skipped: usize,
    /// Decoded bytes frozen for all variants together.
    pub bytes: u64,
    /// Actual browser requests served from captured evidence.
    pub hits: u64,
    /// Actual unmatched browser requests answered with a network failure.
    pub misses: u64,
    /// Unsupported or unavailable document responses that could not be rendered.
    pub failures: u64,
    /// Up to 256 preparation decisions; counters include every candidate.
    pub resources: Vec<CapturedResourceDecision>,
    /// Last 256 intercepted requests; counters include older requests too.
    pub requests: Vec<CapturedPreviewRequest>,
}
/// Shared diagnostics survive the preview's close while its owner shows them.
#[derive(Clone)]
pub struct CapturedPageDiagnostics(
    Arc<Mutex<CapturedPageReport>>,
    Arc<Mutex<std::collections::VecDeque<(u64, crate::CapturedResource)>>>,
);
impl CapturedPageDiagnostics {
    pub(super) fn new(report: CapturedPageReport) -> Self {
        Self(Arc::new(Mutex::new(report)), Arc::default())
    }
    /// Records the script choice made in the trusted warning before navigation.
    pub fn script_choice(&self, enabled: bool) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scripts_enabled = Some(enabled);
    }
    /// Records the native engine version without reading captured content.
    pub fn runtime(&self, runtime: String) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .runtime = runtime;
    }
    /// Copies bounded diagnostics without accessing browser or traffic state.
    pub fn snapshot(&self) -> CapturedPageReport {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub(super) fn request(&self, method: &str, url: &str, entry_id: Option<String>, reason: &str) {
        self.outcome(method, url, entry_id, None, None, reason);
    }
    pub(super) fn failed_response(&self, method: &str, url: &str, rule_id: String, reason: &str) {
        self.record(CapturedPreviewRequest {
            id: 0,
            method: method.into(),
            url: url.into(),
            entry_id: None,
            rule_id: Some(rule_id),
            sha256: None,
            reason: reason.into(),
            outcome: "failed/unavailable".into(),
        });
    }
    pub(super) fn outcome(
        &self,
        method: &str,
        url: &str,
        entry_id: Option<String>,
        rule_id: Option<String>,
        sha256: Option<String>,
        reason: &str,
    ) -> u64 {
        let outcome = if rule_id.is_some() {
            "auto-response"
        } else if entry_id.is_some() {
            "captured"
        } else {
            "missing"
        };
        self.record(CapturedPreviewRequest {
            id: 0,
            method: method.chars().take(32).collect(),
            url: url.chars().take(2048).collect(),
            outcome: outcome.into(),
            entry_id,
            rule_id,
            sha256,
            reason: reason.into(),
        })
    }
    /// Read-only lookup; callers must retain the owning scene while reading its file.
    pub fn supplied_response(&self, observation: u64) -> Option<crate::CapturedResource> {
        let report = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !report.requests.iter().any(|r| r.id == observation) {
            return None;
        }
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(id, _)| *id == observation)
            .map(|(_, resource)| resource.clone())
    }
    pub(super) fn supplied(
        &self,
        method: &str,
        url: &str,
        entry: Option<String>,
        rule: Option<String>,
        resource: &crate::CapturedResource,
        reason: &str,
    ) {
        let id = self.outcome(
            method,
            url,
            entry,
            rule,
            Some(resource.sha256.clone()),
            reason,
        );
        let mut responses = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if responses.len() == 256 {
            responses.pop_front();
        }
        responses.push_back((id, resource.clone()));
    }
    fn record(&self, mut request: CapturedPreviewRequest) -> u64 {
        let mut report = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match request.outcome.as_str() {
            "missing" => report.misses = report.misses.saturating_add(1),
            "failed/unavailable" => report.failures = report.failures.saturating_add(1),
            _ => report.hits = report.hits.saturating_add(1),
        }
        request.id = report
            .hits
            .saturating_add(report.misses)
            .saturating_add(report.failures);
        request.method = request.method.chars().take(32).collect();
        request.url = request.url.chars().take(2048).collect();
        if report.requests.len() == 256 {
            report.requests.remove(0);
        }
        let id = request.id;
        report.requests.push(request);
        id
    }
}
