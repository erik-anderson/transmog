//! Admission and activity, separate from idle physical connection ownership.
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Current measurable activity for a listener; idle clients do not delay drain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProxyActivity {
    /// New requests and connections can be admitted.
    pub accepting: bool,
    /// Requests whose response bodies have not finished.
    pub requests: usize,
    /// Active TLS negotiations and upgraded relays.
    pub transports: usize,
    /// Connected clients, including idle keep-alive clients.
    pub clients: usize,
}
struct State {
    generation: u64,
    accepting: bool,
    finalizing: bool,
    requests: usize,
    transports: usize,
    clients: usize,
}
pub(crate) struct ActivityTracker {
    state: Mutex<State>,
    changed: Notify,
}
impl Default for ActivityTracker {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                generation: 0,
                accepting: true,
                finalizing: false,
                requests: 0,
                transports: 0,
                clients: 0,
            }),
            changed: Notify::new(),
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) enum ActivityKind {
    Request,
    Transport,
    Client,
}
pub(crate) struct ActivityLease {
    owner: Arc<ActivityTracker>,
    kind: ActivityKind,
}
impl ActivityTracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn snapshot(&self) -> ProxyActivity {
        let state = self.lock();
        ProxyActivity {
            accepting: state.accepting,
            requests: state.requests,
            transports: state.transports,
            clients: state.clients,
        }
    }
    pub(crate) fn finalized(&self) -> bool {
        self.lock().finalizing
    }
    pub(crate) fn acquire(self: &Arc<Self>, kind: ActivityKind) -> Option<ActivityLease> {
        let mut state = self.lock();
        if !state.accepting && !matches!(kind, ActivityKind::Client) {
            return None;
        }
        match kind {
            ActivityKind::Request => state.requests += 1,
            ActivityKind::Transport => state.transports += 1,
            ActivityKind::Client => state.clients += 1,
        }
        Some(ActivityLease {
            owner: self.clone(),
            kind,
        })
    }
    /// Extends an admitted exchange into an upgraded relay even if Off arrived
    /// between its admission and its successful upgrade.
    pub(crate) fn continue_transport(self: &Arc<Self>) -> ActivityLease {
        self.lock().transports += 1;
        ActivityLease {
            owner: self.clone(),
            kind: ActivityKind::Transport,
        }
    }
    pub(crate) fn drain(&self) -> u64 {
        let mut state = self.lock();
        state.generation = state.generation.saturating_add(1);
        state.accepting = false;
        let generation = state.generation;
        drop(state);
        self.changed.notify_waiters();
        generation
    }
    pub(crate) fn resume(&self, generation: u64) -> bool {
        let mut state = self.lock();
        if state.generation != generation || state.finalizing {
            return false;
        }
        state.generation = state.generation.saturating_add(1);
        state.accepting = true;
        drop(state);
        self.changed.notify_waiters();
        true
    }
    pub(crate) fn seal(&self, generation: u64) -> bool {
        let mut state = self.lock();
        if state.generation != generation
            || state.accepting
            || state.requests != 0
            || state.transports != 0
        {
            return false;
        }
        state.finalizing = true;
        true
    }
    pub(crate) async fn idle(&self, generation: u64) -> bool {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.lock();
                if state.generation != generation || state.accepting {
                    return false;
                }
                if state.requests == 0 && state.transports == 0 {
                    return true;
                }
            }
            changed.await;
        }
    }
}
impl Drop for ActivityLease {
    fn drop(&mut self) {
        let mut state = self.owner.lock();
        match self.kind {
            ActivityKind::Request => state.requests = state.requests.saturating_sub(1),
            ActivityKind::Transport => state.transports = state.transports.saturating_sub(1),
            ActivityKind::Client => state.clients = state.clients.saturating_sub(1),
        }
        drop(state);
        self.owner.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn idle_clients_do_not_block_and_stale_drains_cannot_seal_after_resume() {
        let activity = Arc::new(ActivityTracker::default());
        let _client = activity.acquire(ActivityKind::Client).unwrap();
        let request = activity.acquire(ActivityKind::Request).unwrap();
        let first = activity.drain();
        assert!(activity.acquire(ActivityKind::Request).is_none());
        assert!(activity.resume(first));
        assert!(!activity.seal(first));
        assert!(!activity.idle(first).await);
        let second = activity.drain();
        assert!(!activity.seal(second));
        drop(request);
        assert!(activity.idle(second).await);
        assert!(activity.seal(second));
        assert!(!activity.resume(second));
        assert_eq!(activity.snapshot().clients, 1);
    }
}
