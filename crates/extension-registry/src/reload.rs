//! Transient package-wide reload signals; no source reads, capture callbacks or access history.
use super::{Error, Origin, Registry, invalid};
use serde::Serialize;
use std::sync::Arc;

/// A settings action schedules existing instances, without starting inactive contributions.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadRequest {
    /// Existing instances across all owners; individual replacement may still fail or be canceled.
    pub active_instances: usize,
}
impl Registry {
    /// Requests replacement of every active instance of one enabled development package.
    /// # Errors
    /// Rejects missing, disabled and non-development entries. Reads no source files.
    pub fn request_reload(&self, id: &str) -> Result<ReloadRequest, Error> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        if !state.entries.iter().any(|entry| {
            entry.catalog.manifest.id == id
                && entry.catalog.enabled
                && entry.catalog.origin == Origin::Development
        }) {
            return Err(invalid(
                "Reload requires an enabled unpacked development extension",
            ));
        }
        let installed: std::collections::BTreeSet<_> = state
            .entries
            .iter()
            .map(|entry| entry.catalog.manifest.id.clone())
            .collect();
        state.reload_requests.retain(|id, _| installed.contains(id));
        state.reload_epoch = state.reload_epoch.checked_add(1).ok_or(Error::Limit)?;
        let epoch = state.reload_epoch;
        state.reload_requests.insert(id.into(), epoch);
        Ok(ReloadRequest {
            active_instances: state
                .active
                .values()
                .filter(|active| active.info.extension_id == id)
                .count(),
        })
    }
    pub(super) fn reload_epoch(&self, id: &str) -> Result<u64, Error> {
        Ok(self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?
            .reload_requests
            .get(id)
            .copied()
            .unwrap_or(0))
    }
    /// Shares one candidate admission gate across windows, preserving capacity for the current workers.
    /// Ordinary parsing/live delivery does not acquire it. Callers must recheck cancellation after waiting.
    pub fn reload_gate(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.inner.reload_gate)
    }
}
