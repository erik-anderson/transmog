//! Bounded in-process decision bridge for interactive controller adapters.
//!
//! This is a lifecycle contract adapter, not a stable IPC or wire protocol.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use thiserror::Error;
use tokio::{sync::mpsc, time::timeout};

use super::{ExchangeCancellation, ExchangeId};

/// Monotonic correlation identifier local to one bridge instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BridgeCorrelationId(pub u64);

struct BridgeEnvelope<T, A> {
    correlation_id: BridgeCorrelationId,
    exchange_id: ExchangeId,
    payload: T,
    reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<A>>>>,
}

/// Bounded producer used by an interceptor to request one correlated decision.
pub struct DecisionBridge<T, A> {
    sender: mpsc::Sender<BridgeEnvelope<T, A>>,
    next_id: Arc<AtomicU64>,
    decision_timeout: Duration,
}

impl<T, A> Clone for DecisionBridge<T, A> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            next_id: Arc::clone(&self.next_id),
            decision_timeout: self.decision_timeout,
        }
    }
}

impl<T, A> std::fmt::Debug for DecisionBridge<T, A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecisionBridge")
            .field("capacity", &self.sender.capacity())
            .field("decision_timeout", &self.decision_timeout)
            .finish_non_exhaustive()
    }
}

impl<T, A> DecisionBridge<T, A>
where
    T: Send + 'static,
    A: Send + 'static,
{
    /// Creates a finite command queue and its single controller endpoint.
    #[must_use]
    pub fn channel(
        capacity: NonZeroUsize,
        decision_timeout: Duration,
    ) -> (Self, DecisionController<T, A>) {
        let (sender, receiver) = mpsc::channel(capacity.get());
        (
            Self {
                sender,
                next_id: Arc::new(AtomicU64::new(1)),
                decision_timeout,
            },
            DecisionController { receiver },
        )
    }

    /// Enqueues one command without waiting for queue capacity, then pauses only
    /// the requesting exchange until a reply, timeout, or cancellation.
    ///
    /// # Errors
    ///
    /// Returns a typed bounded-queue or lifecycle error.
    pub async fn request_decision(
        &self,
        exchange_id: ExchangeId,
        payload: T,
        cancellation: &ExchangeCancellation,
    ) -> Result<A, BridgeError> {
        if self.decision_timeout.is_zero() {
            return Err(BridgeError::InvalidTimeout);
        }
        if cancellation.is_cancelled() {
            return Err(BridgeError::Cancelled);
        }
        let correlation_id = BridgeCorrelationId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (reply_sender, reply_receiver) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(BridgeEnvelope {
                correlation_id,
                exchange_id,
                payload,
                reply: Arc::new(Mutex::new(Some(reply_sender))),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BridgeError::Saturated,
                mpsc::error::TrySendError::Closed(_) => BridgeError::Disconnected,
            })?;

        tokio::select! {
            result = timeout(self.decision_timeout, reply_receiver) => match result {
                Ok(Ok(action)) => Ok(action),
                Ok(Err(_)) => Err(BridgeError::Disconnected),
                Err(_) => Err(BridgeError::TimedOut),
            },
            () = cancellation.cancelled() => Err(BridgeError::Cancelled),
        }
    }
}

/// Single-consumer endpoint owned by the interactive controller adapter.
pub struct DecisionController<T, A> {
    receiver: mpsc::Receiver<BridgeEnvelope<T, A>>,
}

impl<T, A> std::fmt::Debug for DecisionController<T, A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecisionController")
            .finish_non_exhaustive()
    }
}

impl<T, A> DecisionController<T, A> {
    /// Receives the next correlated command, or `None` after every producer is dropped.
    pub async fn recv(&mut self) -> Option<BridgeCommand<T, A>> {
        self.receiver.recv().await.map(|envelope| BridgeCommand {
            correlation_id: envelope.correlation_id,
            exchange_id: envelope.exchange_id,
            payload: envelope.payload,
            reply: envelope.reply,
        })
    }
}

/// One correlated command delivered to a controller.
pub struct BridgeCommand<T, A> {
    correlation_id: BridgeCorrelationId,
    exchange_id: ExchangeId,
    payload: T,
    reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<A>>>>,
}

impl<T, A> std::fmt::Debug for BridgeCommand<T, A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeCommand")
            .field("correlation_id", &self.correlation_id)
            .field("exchange_id", &self.exchange_id)
            .finish_non_exhaustive()
    }
}

impl<T, A> BridgeCommand<T, A> {
    /// Correlation ID that distinguishes commands across exchanges.
    pub fn correlation_id(&self) -> BridgeCorrelationId {
        self.correlation_id
    }

    /// Exchange waiting for this decision.
    pub fn exchange_id(&self) -> ExchangeId {
        self.exchange_id
    }

    /// Immutable controller payload.
    pub fn payload(&self) -> &T {
        &self.payload
    }

    /// Replies once. A second reply is rejected as a duplicate; a reply after
    /// timeout or cancellation is rejected as stale.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeReplyError::Duplicate`] or [`BridgeReplyError::Stale`].
    pub fn reply(&self, action: A) -> Result<(), BridgeReplyError> {
        let sender = self
            .reply
            .lock()
            .map_err(|_| BridgeReplyError::Unavailable)?
            .take()
            .ok_or(BridgeReplyError::Duplicate)?;
        sender.send(action).map_err(|_| BridgeReplyError::Stale)
    }
}

/// Failure while requesting an interactive decision.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BridgeError {
    /// The configured decision timeout was zero.
    #[error("decision timeout must be nonzero")]
    InvalidTimeout,
    /// The finite controller command queue was full.
    #[error("decision bridge queue is saturated")]
    Saturated,
    /// The controller endpoint was dropped or dropped its reply capability.
    #[error("decision bridge controller disconnected")]
    Disconnected,
    /// The command exceeded its finite deadline.
    #[error("interactive decision timed out")]
    TimedOut,
    /// The exchange was cancelled while waiting.
    #[error("interactive decision was cancelled")]
    Cancelled,
}

/// Failure while replying to one correlated command.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BridgeReplyError {
    /// This command already consumed its one reply capability.
    #[error("interactive decision reply is a duplicate")]
    Duplicate,
    /// The exchange timed out, was cancelled, or otherwise ended before reply.
    #[error("interactive decision reply is stale")]
    Stale,
    /// The local reply guard is unavailable after a synchronization failure.
    #[error("interactive decision reply is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bridge() -> (
        DecisionBridge<&'static str, &'static str>,
        DecisionController<&'static str, &'static str>,
    ) {
        DecisionBridge::channel(NonZeroUsize::new(1).unwrap(), Duration::from_millis(30))
    }

    #[tokio::test]
    async fn replies_are_correlated_and_duplicates_are_rejected() {
        let (bridge, mut controller) = make_bridge();
        let cancellation = ExchangeCancellation::new();
        let waiting = tokio::spawn(async move {
            bridge
                .request_decision(ExchangeId(7), "request", &cancellation)
                .await
        });
        let command = controller.recv().await.unwrap();
        assert_eq!(command.exchange_id(), ExchangeId(7));
        assert_eq!(command.payload(), &"request");
        command.reply("continue").unwrap();
        assert_eq!(command.reply("late"), Err(BridgeReplyError::Duplicate));
        assert_eq!(waiting.await.unwrap().unwrap(), "continue");
    }

    #[tokio::test]
    async fn timeout_cancellation_disconnect_and_late_reply_are_typed() {
        let (bridge, mut controller) = make_bridge();
        let cancellation = ExchangeCancellation::new();
        let waiting = tokio::spawn(async move {
            bridge
                .request_decision(ExchangeId(1), "timeout", &cancellation)
                .await
        });
        let command = controller.recv().await.unwrap();
        assert_eq!(waiting.await.unwrap(), Err(BridgeError::TimedOut));
        assert_eq!(command.reply("late"), Err(BridgeReplyError::Stale));

        let (bridge, mut controller) = make_bridge();
        let cancellation = ExchangeCancellation::new();
        let cancelling = cancellation.clone();
        let waiting = tokio::spawn(async move {
            bridge
                .request_decision(ExchangeId(2), "cancel", &cancellation)
                .await
        });
        let command = controller.recv().await.unwrap();
        cancelling.cancel();
        assert_eq!(waiting.await.unwrap(), Err(BridgeError::Cancelled));
        assert_eq!(command.reply("late"), Err(BridgeReplyError::Stale));

        let (bridge, controller) = make_bridge();
        drop(controller);
        assert_eq!(
            bridge
                .request_decision(ExchangeId(3), "disconnect", &ExchangeCancellation::new())
                .await,
            Err(BridgeError::Disconnected)
        );
    }

    #[tokio::test]
    async fn saturated_queue_fails_without_blocking_an_unrelated_exchange() {
        let (bridge, _controller) = make_bridge();
        let occupied_bridge = bridge.clone();
        let occupied = tokio::spawn(async move {
            occupied_bridge
                .request_decision(ExchangeId(1), "occupied", &ExchangeCancellation::new())
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(
            bridge
                .request_decision(ExchangeId(2), "saturated", &ExchangeCancellation::new(),)
                .await,
            Err(BridgeError::Saturated)
        );
        assert_eq!(occupied.await.unwrap(), Err(BridgeError::TimedOut));
    }
}
