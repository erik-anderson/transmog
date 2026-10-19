//! Terminal evidence for a service future dropped before returning its response.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::task::JoinSet;
use transmog_core::{
    intercept::{
        ExchangeCancellation, ExchangeChain, ExchangeFailure, ExchangeFailureKind, ExchangeId,
        ExchangeMetadata, ExchangeStage,
    },
    observe::{ExchangeObserver, ObserverEventKind},
    performance::{Milestone, PerformanceRecorder},
};

use super::{ActivityLease, observe_hook_effects};

#[derive(Default)]
pub(super) struct RequestControls(Mutex<HashMap<ExchangeId, ExchangeCancellation>>);
impl RequestControls {
    pub(super) fn register(
        self: &Arc<Self>,
        id: ExchangeId,
        signal: ExchangeCancellation,
    ) -> RequestLease {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, signal);
        RequestLease {
            owner: self.clone(),
            id,
        }
    }
    pub(super) fn cancel(&self, ids: &[ExchangeId]) -> Vec<ExchangeId> {
        let controls = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ids.iter()
            .filter_map(|id| {
                controls.get(id).map(|signal| {
                    signal.cancel();
                    *id
                })
            })
            .collect()
    }
}
pub(super) struct RequestLease {
    owner: Arc<RequestControls>,
    id: ExchangeId,
}
impl Drop for RequestLease {
    fn drop(&mut self) {
        self.owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

#[derive(Clone)]
pub(super) struct ManualCancellation(pub(super) ExchangeCancellation);

pub(super) struct RequestCancellation {
    pub(super) manual: ExchangeCancellation,
    activity: Option<ActivityLease>,
    shutdown: ExchangeCancellation,
    cleanup: Arc<Mutex<JoinSet<()>>>,
    evidence: Option<(
        Arc<ExchangeObserver>,
        Arc<ExchangeMetadata>,
        PerformanceRecorder,
    )>,
    pub(super) initializing_chain: Option<ExchangeChain>,
    chain: Option<Arc<ExchangeChain>>,
}

impl RequestCancellation {
    pub(super) fn new(
        activity: ActivityLease,
        shutdown: ExchangeCancellation,
        cleanup: Arc<Mutex<JoinSet<()>>>,
    ) -> Self {
        Self {
            manual: ExchangeCancellation::new(),
            activity: Some(activity),
            shutdown,
            cleanup,
            evidence: None,
            initializing_chain: None,
            chain: None,
        }
    }

    pub(super) fn start(
        &mut self,
        observer: Arc<ExchangeObserver>,
        metadata: Arc<ExchangeMetadata>,
        performance: PerformanceRecorder,
    ) {
        self.evidence = Some((observer, metadata, performance));
    }

    pub(super) fn finish(mut self) -> ActivityLease {
        self.evidence = None;
        self.activity
            .take()
            .expect("request owns its activity lease")
    }

    pub(super) fn share_chain(&mut self) -> Arc<ExchangeChain> {
        let chain = Arc::new(
            self.initializing_chain
                .take()
                .expect("initialized exchange chain"),
        );
        self.chain = Some(chain.clone());
        chain
    }
}

impl Drop for RequestCancellation {
    fn drop(&mut self) {
        let Some((observer, metadata, performance)) = self.evidence.take() else {
            return;
        };
        let chain = self
            .chain
            .take()
            .or_else(|| self.initializing_chain.take().map(Arc::new));
        if let Some(chain) = &chain {
            chain.context().cancellation().cancel();
        }
        // Measure the cancellation itself, not the later async cleanup or queue wait.
        performance.mark(Milestone::ExchangeDone);
        let points = performance.snapshot().points;
        let shutdown = self.shutdown.is_cancelled();
        let failure = ExchangeFailure {
            metadata,
            stage: ExchangeStage::Terminal,
            kind: if shutdown {
                ExchangeFailureKind::Shutdown
            } else {
                ExchangeFailureKind::Cancelled
            },
            request_committed: points
                .iter()
                .any(|point| point.milestone == Milestone::UpstreamBegin),
            response_committed: points
                .iter()
                .any(|point| point.milestone == Milestone::ClientResponseBegin),
            message: if shutdown {
                "Proxy stopped while the request was being handled."
            } else if self.manual.is_cancelled() {
                "Request canceled in Transmog."
            } else {
                "The caller closed or reset the request before a response was returned."
            }
            .into(),
        };
        let activity = self.activity.take();
        let mut cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while cleanup.try_join_next().is_some() {}
        cleanup.spawn(async move {
            let _activity = activity;
            if let Some(chain) = chain {
                observe_hook_effects(&chain).await;
                for error in chain.failed(failure.clone()).await.errors() {
                    tracing::warn!(%error, "cancelled request cleanup failed");
                }
                chain.wait_for_terminal_cleanup().await;
            }
            observer
                .emit(ObserverEventKind::Performance(performance.snapshot()))
                .await;
            observer.failed(failure).await;
        });
    }
}
