//! Cancellation and completion scoped to an individual upgraded connection.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use transmog_core::intercept::{ExchangeCancellation, ExchangeId};

#[derive(Clone, Default)]
struct Signals {
    cancel: ExchangeCancellation,
    finished: ExchangeCancellation,
}

#[derive(Default)]
pub(super) struct WebSocketSessions(Mutex<HashMap<ExchangeId, Signals>>);

impl WebSocketSessions {
    pub(super) fn register(self: &Arc<Self>, id: ExchangeId) -> WebSocketLease {
        let signals = Signals::default();
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, signals.clone());
        WebSocketLease {
            owner: self.clone(),
            id,
            signals,
        }
    }

    pub(super) async fn terminate(&self, ids: &[ExchangeId]) -> Vec<ExchangeId> {
        let selected = {
            let sessions = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ids.iter()
                .filter_map(|id| sessions.get(id).cloned().map(|signals| (*id, signals)))
                .collect::<Vec<_>>()
        };
        for (_, signals) in &selected {
            signals.cancel.cancel();
        }
        for (_, signals) in &selected {
            signals.finished.cancelled().await;
        }
        selected.into_iter().map(|(id, _)| id).collect()
    }
}

pub(super) struct WebSocketLease {
    owner: Arc<WebSocketSessions>,
    id: ExchangeId,
    signals: Signals,
}

impl WebSocketLease {
    pub(super) async fn cancelled(&self) {
        self.signals.cancel.cancelled().await;
    }
}

impl Drop for WebSocketLease {
    fn drop(&mut self) {
        self.owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
        self.signals.finished.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn termination_waits_for_selected_relay_cleanup_and_preserves_other_sessions() {
        let registry = Arc::new(WebSocketSessions::default());
        let first = registry.register(ExchangeId(1));
        let other = registry.register(ExchangeId(2));
        let terminating = {
            let registry = registry.clone();
            tokio::spawn(async move { registry.terminate(&[ExchangeId(1), ExchangeId(99)]).await })
        };
        first.cancelled().await;
        assert!(!terminating.is_finished());
        assert!(!other.signals.cancel.is_cancelled());
        drop(first);
        assert_eq!(terminating.await.unwrap(), vec![ExchangeId(1)]);
        assert!(registry.terminate(&[ExchangeId(1)]).await.is_empty());
    }
}
