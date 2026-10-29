//! Window-local marker revisions; exported metadata contains current text only.
use super::{AppError, State, TraceMetadataView, TraceRegistry, invalid, trace_metadata_bytes};
use crate::incidents::{MarkerChange, MarkerReview, MarkerReviewState, MarkerSnapshot, markers};

fn snapshot(source: &TraceMetadataView) -> Result<MarkerSnapshot, AppError> {
    Ok(MarkerSnapshot {
        links: source
            .context
            .pointer("/markerReview/links")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| invalid("Invalid marker links"))?
            .unwrap_or_default(),
        markers: markers(&source.context)?,
        edited: source
            .context
            .pointer("/markerReview/editedMarkerIds")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    })
}
fn view(state: &State, id: &str) -> Result<MarkerReview, AppError> {
    let source = state
        .metadata
        .get(id)
        .ok_or_else(|| invalid("Trace is no longer available"))?;
    let current = snapshot(source)?;
    let review = state.marker_reviews.get(id);
    let baseline = review.map_or(&current, |r| &r.baseline);
    let mut anchors = std::collections::HashMap::new();
    for (exchange, entry) in &state.entries {
        if entry.trace_id == id
            && let Some(anchor) = &entry.marker_anchor
        {
            anchors
                .entry(anchor)
                .and_modify(|entry| *entry = None)
                .or_insert(Some(exchange));
        }
    }
    Ok(MarkerReview {
        trace_id: id.into(),
        name: source.name.clone(),
        revision: review.map_or(0, |r| r.revision),
        dirty: review.is_some_and(|r| current != r.saved),
        can_undo: state.marker_undo.iter().any(|(source, _)| source == id),
        edited_notes: current
            .markers
            .iter()
            .filter(|m| {
                baseline
                    .markers
                    .iter()
                    .any(|b| b.id == m.id && b.note != m.note)
            })
            .count(),
        deleted_markers: baseline
            .markers
            .iter()
            .filter(|b| !current.markers.iter().any(|m| m.id == b.id))
            .count(),
        added_markers: current
            .markers
            .iter()
            .filter(|m| !baseline.markers.iter().any(|b| b.id == m.id))
            .count(),
        marker_entries: current
            .links
            .iter()
            .filter_map(|link| {
                anchors
                    .get(&link.anchor_id)
                    .and_then(|id| id.as_ref())
                    .map(|id| (link.marker_id.clone(), (*id).clone()))
            })
            .collect(),
        links: current.links,
        markers: current.markers,
        edited_marker_ids: current.edited,
    })
}
fn install(source: &mut TraceMetadataView, snapshot: &MarkerSnapshot) -> Result<(), AppError> {
    if !source.context.is_object() {
        source.context = serde_json::json!({});
    }
    source.context["incidentMarkers"] = serde_json::to_value(&snapshot.markers)
        .map_err(|_| invalid("Marker could not be updated"))?;
    source.context["markerReview"] =
        serde_json::json!({"version":1,"editedMarkerIds":snapshot.edited,"links":snapshot.links});
    Ok(())
}
fn edited_source(
    source: &TraceMetadataView,
    next: &MarkerSnapshot,
) -> Result<(TraceMetadataView, usize, usize), AppError> {
    let mut updated = source.clone();
    install(&mut updated, next)?;
    transmog_capture::support::validate_context(&updated.context)
        .map_err(|e| invalid(&e.to_string()))?;
    let old = trace_metadata_bytes(source).map_err(|_| invalid("Trace metadata unavailable"))?;
    let bytes =
        trace_metadata_bytes(&updated).map_err(|_| invalid("Trace metadata unavailable"))?;
    Ok((updated, old, bytes))
}
impl TraceRegistry {
    pub(crate) fn ensure_marker_source(&self, id: &str) -> Result<(), AppError> {
        if self.metadata(id).is_some() {
            return Ok(());
        }
        if id != "live" {
            return Err(invalid("Trace is no longer available"));
        }
        let mut state = self.lock();
        state
            .metadata
            .entry("live".into())
            .or_insert_with(|| TraceMetadataView {
                id: "live".into(),
                name: "Live traffic".into(),
                format: "live",
                path: std::path::PathBuf::new(),
                sessions: 0,
                imported_at: 0,
                context: serde_json::json!({}),
                notes: vec![],
            });
        Ok(())
    }
    pub(crate) fn add_linked_marker(
        &self,
        id: &str,
        mut entry: super::TraceEntry,
        anchor: String,
        revision: u64,
        marker: transmog_capture::support::IncidentMarker,
        link: crate::incidents::MarkerLink,
    ) -> Result<MarkerReview, AppError> {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let source = entry.trace_id.clone();
        let mut state = self.lock();
        let previous = state.entries.get(id).cloned();
        let mut link = link;
        let anchor = previous
            .as_ref()
            .and_then(|entry| entry.marker_anchor.clone())
            .unwrap_or(anchor);
        link.anchor_id.clone_from(&anchor);
        entry.marker_anchor = Some(anchor);
        state.entries.insert(id.into(), entry);
        let result = Self::change_marker_locked(
            &mut state,
            &source,
            revision,
            MarkerChange::Add { marker, link },
        );
        if result.is_err() {
            if let Some(previous) = previous {
                state.entries.insert(id.into(), previous);
            } else {
                state.entries.remove(id);
            }
        }
        result
    }

    pub(crate) fn marker_review(&self, id: &str) -> Result<MarkerReview, AppError> {
        view(&self.lock(), id)
    }
    pub(crate) fn marker_reviews(&self) -> Result<Vec<MarkerReview>, AppError> {
        let state = self.lock();
        state.metadata.keys().map(|id| view(&state, id)).collect()
    }
    pub(crate) fn change_marker(
        &self,
        id: &str,
        revision: u64,
        change: MarkerChange,
    ) -> Result<MarkerReview, AppError> {
        let _publication = self.lock_publication();
        Self::change_marker_locked(&mut self.lock(), id, revision, change)
    }
    fn change_marker_locked(
        state: &mut State,
        id: &str,
        revision: u64,
        change: MarkerChange,
    ) -> Result<MarkerReview, AppError> {
        if state.marker_reviews.get(id).map_or(0, |r| r.revision) != revision {
            return Err(invalid("Markers changed. Reload them and try again."));
        }
        let source = state
            .metadata
            .get(id)
            .ok_or_else(|| invalid("Trace is no longer available"))?;
        let previous = snapshot(source)?;
        let mut next = previous.clone();
        let undo_index = if matches!(change, MarkerChange::Undo) {
            Some(
                state
                    .marker_undo
                    .iter()
                    .rposition(|(source, _)| source == id)
                    .ok_or_else(|| invalid("Nothing to undo"))?,
            )
        } else {
            None
        };
        match change {
            MarkerChange::Add { marker, link } => {
                marker.validate().map_err(|e| invalid(&e.to_string()))?;
                if next.markers.len() >= transmog_capture::support::MAX_MARKERS {
                    return Err(invalid(
                        "This trace already has 256 markers. Delete a marker before adding another.",
                    ));
                }
                next.markers.push(marker);
                next.links.push(link);
            }
            MarkerChange::Edit {
                id: marker_id,
                note,
            } => {
                let marker = next
                    .markers
                    .iter_mut()
                    .find(|m| m.id == marker_id)
                    .ok_or_else(|| invalid("Marker is no longer available"))?;
                marker.note = note;
                marker.validate().map_err(|e| invalid(&e.to_string()))?;
                if marker.note
                    != previous
                        .markers
                        .iter()
                        .find(|m| m.id == marker_id)
                        .expect("existing marker")
                        .note
                    && !next.edited.contains(&marker_id)
                {
                    next.edited.push(marker_id);
                }
            }
            MarkerChange::Delete { id: marker_id } => {
                if !next.markers.iter().any(|m| m.id == marker_id) {
                    return Err(invalid("Marker is no longer available"));
                }
                next.markers.retain(|m| m.id != marker_id);
                next.edited.retain(|id| id != &marker_id);
                next.links.retain(|link| link.marker_id != marker_id);
            }
            MarkerChange::Undo => {
                next = state.marker_undo[undo_index.expect("undo index")].1.clone();
            }
        }
        if next == previous {
            return view(state, id);
        }
        let (updated, old_bytes, bytes) = edited_source(source, &next)?;
        let metadata_bytes = state
            .metadata_bytes
            .saturating_sub(old_bytes)
            .saturating_add(bytes);
        if metadata_bytes > super::MAX_HEAD_BYTES {
            return Err(invalid("Trace metadata limit reached"));
        }
        let review = state
            .marker_reviews
            .entry(id.into())
            .or_insert_with(|| MarkerReviewState {
                revision: 0,
                baseline: previous.clone(),
                saved: previous.clone(),
            });
        review.revision += 1;
        if let Some(index) = undo_index {
            state.marker_undo.remove(index);
        } else {
            state.marker_undo.push_back((id.into(), previous));
        }
        // A workspace retains at most 32 bounded marker snapshots for Undo.
        while state.marker_undo.len() > 32 {
            state.marker_undo.pop_front();
        }
        state.metadata_bytes = metadata_bytes;
        state.metadata.insert(id.into(), updated);
        view(state, id)
    }
    pub(crate) fn markers_saved(&self, sources: &[TraceMetadataView]) -> Result<(), AppError> {
        let mut state = self.lock();
        for source in sources {
            if let Some(review) = state.marker_reviews.get_mut(&source.id) {
                review.saved = snapshot(source)?;
            }
        }
        Ok(())
    }
    pub(crate) fn accept_marker_changes(&self) -> Result<(), AppError> {
        let mut state = self.lock();
        let saved = state
            .marker_reviews
            .iter()
            .map(|(id, r)| (id.clone(), r.saved.clone()))
            .collect::<Vec<_>>();
        for (id, saved) in saved {
            if let Some(source) = state.metadata.get_mut(&id) {
                install(source, &saved)?;
            }
        }
        state.marker_reviews.clear();
        state.marker_undo.clear();
        state.marker_draft_dirty = false;
        state.metadata_bytes = state
            .metadata
            .values()
            .map(trace_metadata_bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid("Trace metadata unavailable"))?
            .into_iter()
            .sum();
        Ok(())
    }
    pub(crate) fn set_marker_draft_dirty(&self, dirty: bool) {
        let _publication = self.lock_publication();
        self.lock().marker_draft_dirty = dirty;
    }
    pub(crate) fn has_marker_changes(&self) -> bool {
        let state = self.lock();
        state.marker_draft_dirty
            || state
                .marker_reviews
                .keys()
                .any(|id| view(&state, id).is_ok_and(|r| r.dirty))
    }
    pub(crate) fn marker_export_sources(
        &self,
        review: Option<&crate::export_review::ReviewedScope>,
    ) -> Result<Vec<TraceMetadataView>, AppError> {
        let state = self.lock();
        if review.is_some_and(|review| {
            review.marker_revisions.iter().any(|(id, revision)| {
                state.marker_reviews.get(id).map_or(0, |r| r.revision) != *revision
            })
        }) {
            return Err(invalid(
                "Notes changed. Refresh the inventory before saving.",
            ));
        }
        Ok(state
            .metadata
            .values()
            .filter(|source| review.is_none_or(|r| r.sources.contains(&source.id)))
            .cloned()
            .collect())
    }
}
