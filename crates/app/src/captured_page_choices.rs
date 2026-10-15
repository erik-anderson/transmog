//! Source-local document choices for previewing a response experiment.
use crate::{AppError, Application, BodyAvailability, ErrorCategory, inspector::parse_session_id};
use serde::Serialize;
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};

/// A retained document candidate; association is advisory and explicitly chosen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedPageChoice {
    /// Stable exchange identity.
    pub id: String,
    /// Original document URL.
    pub url: String,
    /// Original start in the source clock.
    pub started_at: u64,
    /// The resource's recorded Referer matches this document URL.
    pub recorded_referrer: bool,
}

/// Bounded document choices from the selected resource's original source.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedPageChoices {
    /// Up to 100 complete retained HTML responses.
    pub pages: Vec<CapturedPageChoice>,
    /// More eligible documents exist; the user can select one directly in Traffic.
    pub more: bool,
}

impl Application {
    /// Finds documents without changing Traffic selection or loading response bodies.
    /// # Errors
    /// Returns invalid/removed source or unavailable body metadata.
    pub fn captured_page_choices(&self, source_id: &str) -> Result<CapturedPageChoices, AppError> {
        let original = self.service.catalog().get(ExchangeId(parse_session_id(source_id)?))
            .ok_or_else(|| error("The experiment's source request is no longer retained. Select an HTML response in Traffic."))?;
        let source = self.traces.source_id(source_id);
        let store = self
            .body_store
            .as_ref()
            .ok_or_else(|| error("Body retention is unavailable"))?;
        store
            .flush()
            .map_err(|_| error("Response metadata is unavailable"))?;
        let referrer = original
            .request_heads
            .iter()
            .find(|h| h.boundary == ExchangeBoundary::ClientRequest)
            .and_then(|h| h.head.headers.values("referer").next())
            .and_then(|v| std::str::from_utf8(v).ok());
        let anchor = crate::captured_page::millis(original.metadata.started_at);
        let mut pages = self
            .service
            .catalog()
            .project_retained(|snapshot| {
                let id = format!("{:032x}", snapshot.exchange_id.0);
                if self.traces.source_id(&id) != source
                    || !snapshot
                        .response_heads
                        .iter()
                        .any(|h| h.boundary == ExchangeBoundary::ClientResponse)
                    || !store.metadata(snapshot.exchange_id).iter().any(|b| {
                        b.boundary == "client-response"
                            && b.availability == BodyAvailability::Complete
                            && matches!(
                                b.media_type.as_deref(),
                                Some("text/html" | "application/xhtml+xml")
                            )
                    })
                {
                    return None;
                }
                let request = snapshot
                    .request_heads
                    .iter()
                    .find(|h| h.boundary == ExchangeBoundary::ClientRequest)?;
                let target = &request.head.target;
                let url = format!(
                    "{}://{}{}{}",
                    target.scheme,
                    target.authority,
                    target.path,
                    target
                        .query
                        .as_ref()
                        .map_or_else(String::new, |q| format!("?{q}"))
                );
                Some(CapturedPageChoice {
                    id,
                    recorded_referrer: referrer.is_some_and(|v| v == url),
                    url,
                    started_at: crate::captured_page::millis(snapshot.metadata.started_at),
                })
            })
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        pages.sort_by_key(|p| {
            (
                !p.recorded_referrer,
                p.started_at > anchor,
                p.started_at.abs_diff(anchor),
                p.id.clone(),
            )
        });
        let more = pages.len() > 100;
        pages.truncate(100);
        Ok(CapturedPageChoices { pages, more })
    }
}
fn error(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
