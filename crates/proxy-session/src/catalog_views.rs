//! Independent memberships over canonical live exchanges; no observer or hook is duplicated.
use super::{
    Arc, CatalogCursor, CatalogError, CatalogPage, CatalogQuery, ExchangeId, NonZeroUsize,
    SessionCatalog, SessionSnapshot, forget_dismissed,
};

static REFERENCE_ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) struct Reference {
    source: SessionCatalog,
    id: ExchangeId,
    admission: u64,
    release: Arc<dyn Fn(Vec<ExchangeId>) + Send + Sync>,
}
impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reference")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl Reference {
    pub(super) fn snapshot(&self, content: bool) -> Option<SessionSnapshot> {
        let state = self.source.lock_state();
        let mut snapshot = state.by_id.get(&self.id)?.snapshot_with_content(
            content,
            std::time::Instant::now(),
            transmog_core::stream::observed_millis(),
        );
        snapshot.request_number = self.admission;
        Some(snapshot)
    }
}
impl Drop for Reference {
    fn drop(&mut self) {
        let mut state = self.source.lock_state();
        if let Some(count) = state.pins.get_mut(&self.id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.pins.remove(&self.id);
            }
        }
        let released = forget_dismissed(&mut state, &[self.id]);
        drop(state);
        if !released.is_empty() {
            (self.release)(released);
        }
    }
}

impl SessionCatalog {
    /// Count of shared live memberships; owned imported entries are excluded.
    pub fn reference_count(&self) -> usize {
        self.lock_state().borrowed.len()
    }
    /// Adds references to another catalog's canonical exchanges with independent view ordering.
    /// The release callback disposes body sources no longer retained by any view.
    /// # Errors
    /// Rejects cycles, reference limits or exhausted view admission numbers.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "New memberships retain ownership of this callback."
    )]
    pub fn reference_exchanges(
        &self,
        source: &Self,
        ids: &[ExchangeId],
        release: Arc<dyn Fn(Vec<ExchangeId>) + Send + Sync>,
    ) -> Result<usize, CatalogError> {
        let _admission = REFERENCE_ADMISSION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if Arc::ptr_eq(&self.inner, &source.inner) || source.has_references() {
            return Err(CatalogError::ImportLimit);
        }
        let mut state = self.lock_state();
        let mut parent = source.lock_state();
        let mut seen = std::collections::HashSet::new();
        let candidates = ids
            .iter()
            .copied()
            .filter(|id| {
                seen.insert(*id)
                    && !state.by_id.contains_key(id)
                    && !state.borrowed.contains_key(id)
                    && parent.by_id.contains_key(id)
            })
            .collect::<Vec<_>>();
        if state.borrowed.len().saturating_add(candidates.len()) > 200_000 {
            return Err(CatalogError::ImportLimit);
        }
        let new_pins = candidates
            .iter()
            .filter(|id| !parent.pins.contains_key(id))
            .count();
        // Keep space for the bounded main catalog to rotate newer traffic.
        if parent
            .max_live_entries
            .is_some_and(|limit| parent.pins.len().saturating_add(new_pins) > limit.get() / 2)
        {
            return Err(CatalogError::ImportLimit);
        }
        state
            .next_admission
            .checked_add(
                u64::try_from(candidates.len())
                    .map_err(|_| CatalogError::RequestNumberExhausted)?,
            )
            .ok_or(CatalogError::RequestNumberExhausted)?;
        let added = candidates.len();
        for id in candidates {
            *parent.pins.entry(id).or_default() += 1;
            state.next_admission += 1;
            let admission = state.next_admission;
            state.order.insert(admission, id);
            state.borrowed.insert(
                id,
                Reference {
                    source: source.clone(),
                    id,
                    admission,
                    release: release.clone(),
                },
            );
        }
        drop(parent);
        drop(state);
        self.notify_view_change(ids);
        Ok(added)
    }
    /// Whether an entry belongs to this view through a shared live reference.
    pub fn references(&self, id: ExchangeId) -> bool {
        self.lock_state().borrowed.contains_key(&id)
    }
    /// Sends a bounded refresh hint after canonical referenced evidence changes.
    pub fn refresh_references(&self, id: ExchangeId) {
        if self.references(id) {
            self.notify_view_change(&[id]);
        }
    }
    pub(super) fn has_references(&self) -> bool {
        !self.lock_state().borrowed.is_empty()
    }
    pub(super) fn view_metadata(&self) -> Vec<SessionSnapshot> {
        let state = self.lock_state();
        let now = std::time::Instant::now();
        let cutoff = transmog_core::stream::observed_millis();
        state
            .order
            .values()
            .filter(|id| !state.dismissed.contains(id))
            .filter_map(|id| {
                state
                    .by_id
                    .get(id)
                    .map(|entry| entry.snapshot_with_content(false, now, cutoff))
                    .or_else(|| state.borrowed.get(id)?.snapshot(false))
            })
            .collect()
    }
    pub(super) fn query_view(&self, query: &CatalogQuery, latest: bool) -> CatalogPage {
        let limit = query
            .limit
            .map_or(self.page_size_limit(), NonZeroUsize::get)
            .min(self.page_size_limit());
        let mut rows = self
            .view_metadata()
            .into_iter()
            .filter(|snapshot| {
                let active = snapshot
                    .stream
                    .as_ref()
                    .map_or(snapshot.terminal.is_none(), |stream| {
                        stream.status.is_active()
                    });
                query
                    .filter
                    .terminal
                    .is_none_or(|terminal| terminal != active)
                    && query.filter.host.as_ref().is_none_or(|host| {
                        snapshot
                            .metadata
                            .original_target
                            .as_target()
                            .host
                            .eq_ignore_ascii_case(host)
                    })
                    && query.filter.method.as_ref().is_none_or(|method| {
                        snapshot
                            .request_heads
                            .first()
                            .is_some_and(|head| head.head.method.eq_ignore_ascii_case(method))
                    })
                    && (latest
                        || query
                            .after
                            .is_none_or(|cursor| snapshot.request_number > cursor.admission))
            })
            .collect::<Vec<_>>();
        let more = rows.len() > limit;
        if latest {
            rows = rows.into_iter().rev().take(limit).collect();
            rows.reverse();
        } else {
            rows.truncate(limit);
        }
        let next = if more && !latest {
            rows.last().map(|snapshot| CatalogCursor {
                admission: snapshot.request_number,
                exchange_id: snapshot.exchange_id,
            })
        } else {
            None
        };
        CatalogPage {
            sessions: rows
                .into_iter()
                .filter_map(|snapshot| self.get(snapshot.exchange_id))
                .collect(),
            next,
        }
    }
}
