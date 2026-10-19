//! User-chosen preview targets. References never own capture bytes or extend retention.
use crate::{AppError, Application, BodyAvailability, ErrorCategory, inspector::parse_session_id};
use serde::{Deserialize, Serialize};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};

/// Exact captured response identity plus a small display snapshot for unavailable captures.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentPreviewPage {
    /// Application capture lifetime; live exchange IDs alone restart at one.
    pub capture: String,
    /// Exact retained exchange, never a URL alias.
    pub entry_id: String,
    /// Human-readable workspace request number at association time.
    pub request_number: String,
    /// Imported trace namespace, or live traffic.
    pub trace_id: Option<String>,
    /// Original URL, retained as inert text when the capture expires.
    pub url: String,
    /// Original method.
    pub method: String,
    /// Original recorded time, Unix milliseconds.
    pub started_at: u64,
    /// Human-readable capture source.
    pub source: String,
}

impl ExperimentPreviewPage {
    pub(crate) fn valid(&self) -> bool {
        self.capture.len() == 32
            && self.capture.bytes().all(|b| b.is_ascii_hexdigit())
            && self.entry_id.len() == 32
            && parse_session_id(&self.entry_id).is_ok()
            && !self.request_number.is_empty()
            && self.request_number.len() <= 20
            && self
                .request_number
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            && self.trace_id.as_ref().is_none_or(|id| id.len() <= 128)
            && !self.url.is_empty()
            && self.url.len() <= 64 * 1024
            && !self.method.is_empty()
            && self.method.len() <= 32
            && self.source.len() <= 4096
    }
}

/// Bounded choices and the current association's availability in this capture lifetime.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentPreviewPages {
    /// Up to 100 complete HTML responses, plus an explicitly selected older response.
    pub pages: Vec<ExperimentPreviewPage>,
    /// Further choices can be selected directly in Traffic.
    pub more: bool,
    /// Current set metadata; missing on older saved sets.
    pub page: Option<ExperimentPreviewPage>,
    /// Independent optimistic concurrency version, unrelated to live activation.
    pub revision: u64,
    /// The exact capture is currently available.
    pub available: bool,
    /// Native adapter may supply an existing window-owned frozen scene.
    pub source_label: Option<String>,
}

impl Application {
    fn preview_page(&self, id: &str) -> Option<ExperimentPreviewPage> {
        let numeric = ExchangeId(parse_session_id(id).ok()?);
        let snapshot = self.service.catalog().get(numeric)?;
        let store = self.body_store.as_ref()?;
        if !store.metadata(numeric).iter().any(|body| {
            body.boundary == "client-response"
                && body.availability == BodyAvailability::Complete
                && matches!(
                    body.media_type.as_deref(),
                    Some("text/html" | "application/xhtml+xml")
                )
        }) {
            return None;
        }
        let response = snapshot
            .response_heads
            .iter()
            .find(|head| head.boundary == ExchangeBoundary::ClientResponse)?;
        if !(200..=599).contains(&response.head.status) || response.head.status == 304 {
            return None;
        }
        let request = snapshot
            .request_heads
            .iter()
            .find(|head| head.boundary == ExchangeBoundary::ClientRequest)?;
        let target = &request.head.target;
        let trace_id = self.traces.source_id(id);
        let source = trace_id
            .as_ref()
            .and_then(|id| self.traces.metadata(id))
            .map_or_else(|| "Live traffic".into(), |meta| meta.name);
        let page = ExperimentPreviewPage {
            capture: self.preview_capture.clone(),
            entry_id: id.into(),
            request_number: snapshot.request_number.to_string(),
            trace_id,
            url: format!(
                "{}://{}{}{}",
                target.scheme,
                target.authority,
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}"))
            ),
            method: request.head.method.as_str().into(),
            started_at: crate::captured_page::millis(snapshot.metadata.started_at),
            source,
        };
        page.valid().then_some(page)
    }

    /// Reads complete HTML candidates without choosing a page or acquiring body leases.
    /// # Errors
    /// Returns unavailable storage or a missing response set.
    pub fn experiment_preview_pages(
        &self,
        id: &str,
        selected_id: Option<&str>,
        include_pages: bool,
    ) -> Result<ExperimentPreviewPages, AppError> {
        let status = self.automation.status();
        let set = status
            .response_sets
            .iter()
            .find(|set| set.id == id)
            .ok_or_else(|| invalid("This response set is unavailable"))?;
        if let Some(store) = &self.body_store {
            store
                .flush()
                .map_err(|_| invalid("Captured response metadata is unavailable"))?;
        }
        let mut pages = if include_pages {
            self.service
                .catalog()
                .project_retained(|snapshot| format!("{:032x}", snapshot.exchange_id.0))
                .iter()
                .filter_map(|id| self.preview_page(id))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        pages.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.entry_id.cmp(&b.entry_id))
        });
        let more = pages.len() > 100;
        pages.truncate(100);
        if let Some(selected) = selected_id.and_then(|id| self.preview_page(id))
            && !pages.iter().any(|page| page.entry_id == selected.entry_id)
        {
            pages.push(selected);
        }
        Ok(ExperimentPreviewPages {
            available: set
                .preview_page
                .as_ref()
                .is_some_and(|page| self.experiment_page_available(page)),
            page: set.preview_page.clone(),
            revision: set.preview_page_revision,
            pages,
            more,
            source_label: None,
        })
    }

    /// Tests the exact capture reference; no same-URL or restarted exchange-ID rebinding.
    pub fn experiment_page_available(&self, page: &ExperimentPreviewPage) -> bool {
        self.preview_page(&page.entry_id).as_ref() == Some(page)
    }

    /// Whether a native frozen scene can belong to this capture lifetime.
    pub fn experiment_page_capture_current(&self, page: &ExperimentPreviewPage) -> bool {
        page.valid() && page.capture == self.preview_capture
    }

    /// Remembers a deliberately chosen page without saving or activating response revisions.
    /// # Errors
    /// Rejects expired capture metadata, concurrent choices, or durable write failures.
    pub fn set_experiment_preview_page(
        &self,
        id: &str,
        revision: u64,
        page: ExperimentPreviewPage,
    ) -> Result<crate::AutomationStatus, AppError> {
        if !self.experiment_page_available(&page) {
            return Err(invalid(
                "That captured page is unavailable. Choose another captured response.",
            ));
        }
        self.automation
            .set_experiment_preview_page(id, revision, page)?;
        Ok(self.automation_status())
    }
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
