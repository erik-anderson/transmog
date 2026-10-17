//! Bounded export inventories freeze entry membership without pinning body payloads.
#![allow(missing_docs)]
use crate::{AppError, Application, ErrorCategory, comparison::ComparisonScope};
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    sync::{Arc, Mutex},
};
#[derive(Clone, Default)]
pub(crate) struct ReviewRegistry(Arc<Mutex<VecDeque<Arc<ReviewedScope>>>>);
pub(crate) struct ReviewedScope {
    pub id: String,
    pub ids: HashSet<String>,
    pub sources: HashSet<String>,
    pub marker_revisions: std::collections::BTreeMap<String, u64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportInventory {
    pub id: String,
    pub entries: usize,
    pub retained_entries: usize,
    pub sources: Vec<String>,
    pub source_count: usize,
    pub hosts: Vec<String>,
    pub first_millis: Option<u64>,
    pub last_millis: Option<u64>,
    pub incomplete_bodies: usize,
    pub retained_body_bytes: u64,
    pub stream_records: u64,
    pub retained_stream_bytes: u64,
    pub incident_markers: usize,
    pub edited_notes: usize,
    pub added_markers: usize,
    pub deleted_markers: usize,
    pub note: String,
}
impl ReviewRegistry {
    pub(crate) fn get(&self, id: &str) -> Result<Arc<ReviewedScope>, AppError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| error("Export review expired; review the selection again"))
    }
}
pub(crate) fn scope_ids(
    app: &Application,
    scope: &ComparisonScope,
) -> Result<Vec<String>, AppError> {
    match scope {
        ComparisonScope::Selected { ids } => Ok(ids.clone()),
        ComparisonScope::Filtered { query } => app.matching_traffic_ids(query),
        ComparisonScope::Source { trace_id } => {
            if trace_id
                .as_ref()
                .is_some_and(|id| app.traces.metadata(id).is_none())
            {
                return Err(error("Export source expired"));
            }
            Ok(app
                .service
                .catalog()
                .project_retained(|s| {
                    let id = format!("{:032x}", s.exchange_id.0);
                    (app.traces.source_id(&id) == *trace_id
                        || trace_id.as_deref() == Some("live") && !s.imported)
                        .then_some(id)
                })
                .into_iter()
                .flatten()
                .collect())
        }
    }
}
impl Application {
    /// Inventories and freezes all, selected, filtered or original-source entry membership.
    /// # Errors
    /// Returns missing entries/source, invalid filters or a finite review limit.
    #[allow(clippy::too_many_lines)] // Keep each bounded review/save transaction auditable together.
    pub fn review_traffic_export(
        &self,
        selection: Option<ComparisonScope>,
    ) -> Result<ExportInventory, AppError> {
        let all_sources = selection.is_none();
        let (ids, source) = match selection {
            None => (
                self.service
                    .catalog()
                    .project_retained(|s| format!("{:032x}", s.exchange_id.0)),
                None,
            ),
            Some(scope) => {
                let ids = scope_ids(self, &scope)?;
                let source = if let ComparisonScope::Source { trace_id } = scope {
                    Some(trace_id)
                } else {
                    None
                };
                (ids, source)
            }
        };
        if ids.len() > 100_000 {
            return Err(error(
                "Review up to 100,000 entries at once; narrow the export scope",
            ));
        }
        let ids = ids.into_iter().collect::<HashSet<_>>();
        let mut sources = HashSet::new();
        let mut hosts = BTreeSet::new();
        let mut first = None;
        let mut last = None;
        let mut incomplete = 0;
        let mut body_bytes = 0u64;
        let mut streams = 0u64;
        let mut stream_bytes = 0u64;
        let mut found = 0;
        let total =
            self.service
                .catalog()
                .project_retained(|s| {
                    let id = format!("{:032x}", s.exchange_id.0);
                    if !ids.contains(&id) {
                        return;
                    }
                    found += 1;
                    if let Some(source) = self.traces.source_id(&id) {
                        sources.insert(source);
                    } else if !s.imported && self.traces.metadata("live").is_some() {
                        sources.insert("live".into());
                    }
                    if let Some(head) = s.request_heads.iter().find(|h| {
                        h.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest
                    }) && hosts.len() < 100
                    {
                        hosts.insert(head.head.target.host.clone());
                    }
                    if let Ok(time) = s.metadata.started_at.duration_since(std::time::UNIX_EPOCH)
                        && !time.is_zero()
                        && let Ok(ms) = u64::try_from(time.as_millis())
                    {
                        first = Some(first.map_or(ms, |v: u64| v.min(ms)));
                        last = Some(last.map_or(ms, |v: u64| v.max(ms)));
                    }
                    if let Some(store) = &self.body_store {
                        for body in store.metadata(s.exchange_id) {
                            incomplete +=
                                usize::from(body.availability != crate::BodyAvailability::Complete);
                            body_bytes = body_bytes.saturating_add(body.retained_bytes);
                        }
                    }
                    streams = streams.saturating_add(s.stream_history.observed);
                    stream_bytes =
                        stream_bytes.saturating_add(s.stream_history.retained_bytes as u64);
                })
                .len();
        if found != ids.len() {
            return Err(error(
                "An export entry was removed; review the selection again",
            ));
        }
        if all_sources {
            sources.extend(self.traces.list().into_iter().map(|s| s.id));
        }
        if let Some(Some(source)) = source {
            sources.insert(source);
        }
        let metadata = sources
            .iter()
            .filter_map(|id| self.traces.metadata(id))
            .collect::<Vec<_>>();
        let id = transmog_capture::support::new_id()
            .map_err(|_| error("Export review identity unavailable"))?;
        let reviews = self.trace_marker_reviews()?;
        let markers = reviews
            .iter()
            .filter(|r| sources.contains(&r.trace_id))
            .map(|r| r.markers.len())
            .sum();
        let marker_revisions = reviews
            .iter()
            .filter(|r| sources.contains(&r.trace_id))
            .map(|r| (r.trace_id.clone(), r.revision))
            .collect();
        let edited_notes = reviews
            .iter()
            .filter(|r| sources.contains(&r.trace_id))
            .map(|r| r.edited_notes)
            .sum();
        let deleted_markers = reviews
            .iter()
            .filter(|r| sources.contains(&r.trace_id))
            .map(|r| r.deleted_markers)
            .sum();
        let added_markers = reviews
            .iter()
            .filter(|r| sources.contains(&r.trace_id))
            .map(|r| r.added_markers)
            .sum();
        let result=ExportInventory{added_markers,edited_notes,deleted_markers,id:id.clone(),entries:found,retained_entries:total,sources:metadata.iter().take(100).map(|s|s.name.clone()).collect(),source_count:metadata.len(),hosts:hosts.into_iter().collect(),first_millis:first,last_millis:last,incomplete_bodies:incomplete,retained_body_bytes:body_bytes,stream_records:streams,retained_stream_bytes:stream_bytes,incident_markers:markers,note:"Entry membership is frozen for this review; new traffic is excluded. Body/stream availability can change before save and is disclosed in the resulting file. Host inventory shows at most 100 distinct hosts. Source notes, incident context, URLs, bodies and stream content may contain private data. Header redaction is not complete anonymization. User-entered client versions and observed client evidence stay distinct.".into()};
        let mut reviews = self
            .export_reviews
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reviews.len() == 4 {
            reviews.pop_front();
        }
        reviews.push_back(Arc::new(ReviewedScope {
            id,
            ids,
            sources,
            marker_revisions,
        }));
        Ok(result)
    }
    pub(crate) fn export_scope(
        &self,
        id: Option<&str>,
    ) -> Result<Option<Arc<ReviewedScope>>, AppError> {
        id.map(|id| {
            let review = self.export_reviews.get(id)?;
            let revisions = self.marker_revisions()?;
            if review
                .marker_revisions
                .iter()
                .any(|(id, revision)| revisions.get(id) != Some(revision))
            {
                return Err(error("Notes changed. Refresh the inventory before saving."));
            }
            let available = self
                .service
                .catalog()
                .project_retained(|s| review.ids.contains(&format!("{:032x}", s.exchange_id.0)))
                .into_iter()
                .filter(|v| *v)
                .count();
            if available != review.ids.len()
                || review
                    .sources
                    .iter()
                    .any(|id| self.traces.metadata(id).is_none())
            {
                return Err(error(
                    "Export evidence changed or expired; review the selection again",
                ));
            }
            Ok(review)
        })
        .transpose()
    }
}
fn error(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
