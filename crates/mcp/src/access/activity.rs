//! Presentation-only presence. It never authenticates calls or changes a grant.
use super::{AccessManager, Grant, Store, now};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

const QUIET_PERIOD: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(super) struct Activity {
    identity: Arc<()>,
    revision: u64,
    calls: usize,
    last_finished: Instant,
}

pub(super) fn prune(state: &mut Store) {
    let enabled = state.enabled;
    let records = &state.records;
    state.activity.retain(|id, activity| {
        enabled
            && (activity.calls > 0 || activity.last_finished.elapsed() < QUIET_PERIOD)
            && records.iter().any(|record| {
                record.grant.id == *id
                    && record.grant.revision == activity.revision
                    && !record.grant.revoked
                    && record.grant.expires_at.is_none_or(|expiry| expiry > now())
            })
    });
}

/// Keeps admitted calls visible until completion, including cancellation and errors.
pub(crate) struct ActiveCall {
    manager: AccessManager,
    id: String,
    identity: Arc<()>,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut state = self.manager.lock();
        if let Some(activity) = state.activity.get_mut(&self.id)
            && Arc::ptr_eq(&activity.identity, &self.identity)
        {
            activity.calls = activity.calls.saturating_sub(1);
            activity.last_finished = Instant::now();
        }
    }
}

impl AccessManager {
    /// Separate presentation hints avoid triggering permission refreshes for ordinary calls.
    pub fn subscribe_activity(&self) -> watch::Receiver<u64> {
        self.activity_changed.subscribe()
    }

    pub(crate) fn begin_call(&self, grant: &Grant) -> ActiveCall {
        let mut state = self.lock();
        prune(&mut state);
        let newly_visible = !state.activity.contains_key(&grant.id);
        let activity = state.activity.entry(grant.id.clone()).or_insert(Activity {
            identity: Arc::new(()),
            revision: grant.revision,
            calls: 0,
            last_finished: Instant::now(),
        });
        activity.calls += 1;
        let identity = Arc::clone(&activity.identity);
        drop(state);
        if newly_visible {
            self.activity_changed
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        ActiveCall {
            manager: self.clone(),
            id: grant.id.clone(),
            identity,
        }
    }

    /// Names of keys with authenticated calls running or completed in the last three seconds.
    /// Only the trusted desktop consumes this state; no secrets or traffic are included.
    pub fn active_agents(&self) -> Vec<Value> {
        let mut state = self.lock();
        prune(&mut state);
        state
            .records
            .iter()
            .filter(|record| state.activity.contains_key(&record.grant.id))
            .map(|record| json!({"id":record.grant.id,"name":record.grant.name}))
            .collect()
    }

    /// Clears presentation state when its listener is stopped.
    pub fn clear_activity(&self) {
        self.lock().activity.clear();
        self.activity_changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

#[cfg(test)]
mod tests;
