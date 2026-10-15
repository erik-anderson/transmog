//! Incident intake and source-scoped recorded-time navigation; captured text stays inert.
#![allow(missing_docs)]
use crate::{AppError, Application, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};
pub use transmog_capture::support::IncidentMarker;
use transmog_capture::support::SupportProfile;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncidentWindow {
    pub trace_id: String,
    pub marker_id: String,
    pub before_seconds: u32,
    pub after_seconds: u32,
    #[serde(default)]
    pub relevant_hosts_only: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentIntake {
    pub trace_id: String,
    pub source: String,
    pub profile: Option<SupportProfile>,
    pub markers: Vec<IncidentMarker>,
    pub expected_behavior: String,
    pub actual_behavior: String,
    pub entered_client_version: String,
    pub collector: String,
    pub observed_clients: Vec<String>,
    pub observed_user_agents: Vec<String>,
    pub entries: usize,
    pub incomplete_bodies: usize,
    pub note: String,
}
fn markers(context: &serde_json::Value) -> Result<Vec<IncidentMarker>, AppError> {
    context
        .get("incidentMarkers")
        .and_then(serde_json::Value::as_array)
        .map_or(Ok(Vec::new()), |values| {
            values
                .iter()
                .map(|v| {
                    transmog_capture::support::parse_marker(v.clone())
                        .map_err(|e| error(&e.to_string()))
                })
                .collect()
        })
}
fn profile(context: &serde_json::Value) -> Result<Option<SupportProfile>, AppError> {
    context
        .pointer("/supportCapture/profile")
        .map(|v| {
            serde_json::to_vec(v)
                .map_err(|_| error("Profile unavailable"))
                .and_then(|bytes| SupportProfile::parse(&bytes).map_err(|e| error(&e.to_string())))
        })
        .transpose()
}
impl Application {
    /// Summarizes original support context and currently retained source evidence.
    /// # Errors
    /// Returns missing source or invalid support metadata.
    pub fn incident_intake(&self, trace_id: &str) -> Result<IncidentIntake, AppError> {
        let trace = self
            .traces
            .metadata(trace_id)
            .ok_or_else(|| error("Incident source is no longer retained"))?;
        transmog_capture::support::validate_context(&trace.context)
            .map_err(|e| error(&e.to_string()))?;
        let mut clients = BTreeSet::new();
        let mut agents = BTreeSet::new();
        let mut incomplete = 0;
        let mut entries = 0;
        self.service.catalog().project_retained(|s| {
            let id = format!("{:032x}", s.exchange_id.0);
            if self.traces.source_id(&id).as_deref() != Some(trace_id) {
                return;
            }
            entries += 1;
            if clients.len() < 8 {
                clients.insert(
                    format!("{:?}", s.metadata.client_identity)
                        .chars()
                        .take(512)
                        .collect::<String>(),
                );
            }
            for head in &s.request_heads {
                if head.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest {
                    for f in head
                        .head
                        .headers
                        .iter()
                        .filter(|f| f.name().eq_ignore_ascii_case(b"user-agent"))
                    {
                        if agents.len() < 8 {
                            agents.insert(if f.is_redacted() {
                                "Redacted".into()
                            } else {
                                String::from_utf8_lossy(f.value())
                                    .chars()
                                    .take(512)
                                    .collect()
                            });
                        }
                    }
                }
            }
            incomplete += self.body_store.as_ref().map_or(0, |store| {
                store
                    .metadata(s.exchange_id)
                    .iter()
                    .filter(|b| b.availability != crate::BodyAvailability::Complete)
                    .count()
            });
        });
        let context = &trace.context;
        let field = |path: &str| {
            context
                .pointer(path)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .chars()
                .take(2048)
                .collect::<String>()
        };
        let retention=context.get("circularRetention").map_or_else(String::new,|r|format!(" Circular retention: {} exchanges evicted; surrounding incident traffic may be missing.",r["evictedExchanges"]));
        Ok(IncidentIntake {
            trace_id: trace_id.into(),
            source: trace.name,
            profile: profile(context)?,
            markers: markers(context)?,
            expected_behavior: field("/supportCapture/expectedBehavior"),
            actual_behavior: field("/supportCapture/actualBehavior"),
            entered_client_version: field("/supportCapture/clientVersionEntered"),
            collector: format!("{} {}", field("/application"), field("/version")),
            observed_clients: clients.into_iter().collect(),
            observed_user_agents: agents.into_iter().collect(),
            entries,
            incomplete_bodies: incomplete,
            note: format!(
                "Markers identify recorded user observations. Their windows use this source's clock; clock changes or other sources do not establish chronology. No surrounding traffic can mean no collection, an excluded interval or eviction. User-entered browser/version, recorded User-Agent/client identity and collector version are separate evidence.{retention} {}",
                trace
                    .notes
                    .iter()
                    .flat_map(|note| note.chars().chain(std::iter::once(' ')))
                    .take(8192)
                    .collect::<String>()
            ),
        })
    }
    /// Marks an active file recording; it does not insert a fake Traffic exchange.
    /// # Errors
    /// Returns inactive recording, marker/queue limits or durable writer failure.
    pub async fn mark_incident(&self, note: String) -> Result<IncidentMarker, AppError> {
        self.service
            .capture()
            .mark(note, transmog_capture::support::MarkerOrigin::Desktop)
            .await
            .map_err(|e| error(&e.to_string()))
    }
}
pub(crate) fn restrict(
    app: &Application,
    window: Option<&IncidentWindow>,
    matching: Option<Arc<HashSet<String>>>,
) -> Result<Option<Arc<HashSet<String>>>, AppError> {
    let Some(window) = window else {
        return Ok(matching);
    };
    if window.before_seconds > 86400 || window.after_seconds > 86400 {
        return Err(error(
            "Incident window accepts at most 86400 seconds before/after",
        ));
    }
    let trace = app
        .traces
        .metadata(&window.trace_id)
        .ok_or_else(|| error("Incident source expired; reset to all traffic"))?;
    let marker = if window.marker_id.is_empty() {
        None
    } else {
        Some(
            markers(&trace.context)?
                .into_iter()
                .find(|m| m.id == window.marker_id)
                .ok_or_else(|| error("Incident marker is no longer available"))?,
        )
    };
    let profile = profile(&trace.context)?;
    if window.relevant_hosts_only && profile.as_ref().is_none_or(|p| p.relevant_hosts.is_empty()) {
        return Err(error(
            "This incident has no relevant-host hints; use the time window without that restriction",
        ));
    }
    let start = marker.as_ref().map_or(0, |m| {
        m.unix_millis
            .saturating_sub(u64::from(window.before_seconds) * 1000)
    });
    let end = marker.as_ref().map_or(u64::MAX, |m| {
        m.unix_millis
            .saturating_add(u64::from(window.after_seconds) * 1000)
    });
    let ids = app
        .service
        .catalog()
        .project_retained(|s| {
            let id = format!("{:032x}", s.exchange_id.0);
            if app.traces.source_id(&id).as_deref() != Some(&window.trace_id)
                || matching.as_ref().is_some_and(|ids| !ids.contains(&id))
            {
                return None;
            }
            if marker.is_some() {
                let millis = s
                    .metadata
                    .started_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| u64::try_from(d.as_millis()).ok())?;
                if millis == 0 || millis < start || millis > end {
                    return None;
                }
            }
            if window.relevant_hosts_only
                && !s.request_heads.iter().any(|h| {
                    h.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest
                        && profile
                            .as_ref()
                            .is_some_and(|p| p.relevant(&h.head.target.host))
                })
            {
                return None;
            }
            Some(id)
        })
        .into_iter()
        .flatten()
        .collect();
    Ok(Some(Arc::new(ids)))
}
fn error(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

#[cfg(test)]
mod tests;
