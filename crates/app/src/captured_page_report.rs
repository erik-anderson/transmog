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
    /// Resource source scope; mixing captures is always explicit.
    #[serde(default)]
    pub scope: CapturedPageScope,
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
    /// served or missing (an empty 404).
    pub outcome: String,
    /// Selected source traffic entry, when matched.
    pub entry_id: Option<String>,
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
    /// Native script choice, if a browser has been opened.
    pub scripts_enabled: Option<bool>,
    /// Response variants successfully frozen.
    pub available: usize,
    /// Captured variants omitted during preparation.
    pub skipped: usize,
    /// Decoded bytes frozen for all variants together.
    pub bytes: u64,
    /// Actual browser requests served from captured evidence.
    pub hits: u64,
    /// Actual unmatched browser requests answered with an empty 404.
    pub misses: u64,
    /// Up to 256 preparation decisions; counters include every candidate.
    pub resources: Vec<CapturedResourceDecision>,
    /// Last 256 intercepted requests; counters include older requests too.
    pub requests: Vec<CapturedPreviewRequest>,
}
/// Shared diagnostics survive the preview's close while its owner shows them.
#[derive(Clone)]
pub struct CapturedPageDiagnostics(Arc<Mutex<CapturedPageReport>>);
impl CapturedPageDiagnostics {
    pub(super) fn new(report: CapturedPageReport) -> Self {
        Self(Arc::new(Mutex::new(report)))
    }
    /// Records the script choice made in the trusted warning before navigation.
    pub fn script_choice(&self, enabled: bool) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scripts_enabled = Some(enabled);
    }
    /// Copies bounded diagnostics without accessing browser or traffic state.
    pub fn snapshot(&self) -> CapturedPageReport {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub(super) fn request(&self, method: &str, url: &str, entry_id: Option<String>, reason: &str) {
        let mut report = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entry_id.is_some() {
            report.hits = report.hits.saturating_add(1);
        } else {
            report.misses = report.misses.saturating_add(1);
        }
        let id = report.hits.saturating_add(report.misses);
        if report.requests.len() == 256 {
            report.requests.remove(0);
        }
        report.requests.push(CapturedPreviewRequest {
            id,
            method: method.chars().take(32).collect(),
            url: url.chars().take(2048).collect(),
            outcome: if entry_id.is_some() {
                "served"
            } else {
                "missing"
            }
            .into(),
            entry_id,
            reason: reason.into(),
        });
    }
}
