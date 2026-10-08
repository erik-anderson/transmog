#![deny(missing_docs)]

//! Finite in-process delivery for the experimental same-build control model.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::mpsc,
    time::{Instant, timeout_at},
};
use transmog_control_model::{
    BreakpointInput, BreakpointRequest, Capability, ControlEvent, DecisionAction, DecisionCommand,
    DecisionId, EXPERIMENTAL_CONTROL_REVISION, Handshake, ModelError,
};

/// Finite transport configuration.
#[derive(Clone, Copy, Debug)]
pub struct TransportConfig {
    /// Maximum queued lifecycle events.
    pub event_capacity: NonZeroUsize,
    /// Maximum queued breakpoint requests.
    pub decision_capacity: NonZeroUsize,
    /// Finite deadline for a breakpoint reply.
    pub decision_timeout: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            event_capacity: NonZeroUsize::new(256).expect("256 is nonzero"),
            decision_capacity: NonZeroUsize::new(64).expect("64 is nonzero"),
            decision_timeout: Duration::from_secs(30),
        }
    }
}

/// Cooperative cancellation handle for one pending request.
#[derive(Clone, Debug, Default)]
pub struct RequestCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl RequestCancellation {
    /// Creates an unset cancellation handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels current and future waiters.
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

struct DecisionEnvelope {
    request: BreakpointRequest,
    reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<DecisionAction>>>>,
    max_body_edit_bytes: usize,
    deadline: Instant,
}

/// Immutable result of the v0 handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegotiatedSession {
    /// Agreed experimental revision.
    pub revision: u32,
    /// Exact same-build identity.
    pub build_id: Arc<str>,
    /// Capability intersection understood by both peers.
    pub capabilities: std::collections::BTreeSet<Capability>,
    /// Smaller body-edit bound advertised by either peer.
    pub max_body_edit_bytes: usize,
}

/// Proxy-side endpoint for events and decisions.
#[derive(Clone)]
pub struct ControlProducer {
    events: mpsc::Sender<ControlEvent>,
    decisions: mpsc::Sender<DecisionEnvelope>,
    next_decision: Arc<AtomicU64>,
    decision_timeout: Duration,
    max_body_edit_bytes: usize,
    negotiated: Arc<NegotiatedSession>,
}

impl std::fmt::Debug for ControlProducer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlProducer")
            .field("event_capacity", &self.events.capacity())
            .field("decision_capacity", &self.decisions.capacity())
            .field("decision_timeout", &self.decision_timeout)
            .finish_non_exhaustive()
    }
}

impl ControlProducer {
    /// Returns immutable negotiated connection properties.
    pub fn negotiated(&self) -> &NegotiatedSession {
        &self.negotiated
    }

    /// Publishes without waiting for queue capacity.
    ///
    /// # Errors
    ///
    /// Returns a typed saturation or disconnection error.
    pub fn publish(&self, event: ControlEvent) -> Result<(), TransportError> {
        self.events.try_send(event).map_err(map_send_error)
    }

    /// Requests one correlated breakpoint decision.
    ///
    /// # Errors
    ///
    /// Returns a typed validation, saturation, cancellation, timeout, or
    /// disconnection error.
    pub async fn request(
        &self,
        input: BreakpointInput,
        cancellation: &RequestCancellation,
    ) -> Result<DecisionAction, TransportError> {
        if self.decision_timeout.is_zero() {
            return Err(TransportError::InvalidTimeout);
        }
        if cancellation.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        if input
            .body
            .as_ref()
            .is_some_and(|value| value.len() > self.max_body_edit_bytes)
        {
            return Err(TransportError::InvalidModel(ModelError::BodyEditTooLarge {
                actual: input.body.as_ref().map_or(0, Vec::len),
                limit: self.max_body_edit_bytes,
            }));
        }
        let decision_id = DecisionId(self.next_decision.fetch_add(1, Ordering::Relaxed));
        let request = BreakpointRequest {
            decision_id,
            exchange_id: input.exchange_id,
            phase: input.phase,
            request_head: input.request_head,
            response_head: input.response_head,
            body: input.body,
        };
        let (reply_sender, reply_receiver) = tokio::sync::oneshot::channel();
        let deadline = Instant::now() + self.decision_timeout;
        self.decisions
            .try_send(DecisionEnvelope {
                request,
                reply: Arc::new(Mutex::new(Some(reply_sender))),
                max_body_edit_bytes: self.max_body_edit_bytes,
                deadline,
            })
            .map_err(map_send_error)?;

        tokio::select! {
            result = timeout_at(deadline, reply_receiver) => match result {
                Ok(Ok(action)) => Ok(action),
                Ok(Err(_)) => Err(TransportError::Disconnected),
                Err(_) => Err(TransportError::TimedOut),
            },
            () = cancellation.cancelled() => Err(TransportError::Cancelled),
        }
    }
}

/// Single controller-side endpoint.
pub struct ControlController {
    events: mpsc::Receiver<ControlEvent>,
    decisions: mpsc::Receiver<DecisionEnvelope>,
    negotiated: Arc<NegotiatedSession>,
}

/// Next item received by a controller without starving either finite queue.
#[derive(Debug)]
pub enum ControllerMessage {
    /// Sequenced observation event.
    Event(ControlEvent),
    /// Phase-typed request awaiting a single decision.
    Decision(PendingDecision),
}

impl std::fmt::Debug for ControlController {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlController")
            .finish_non_exhaustive()
    }
}

impl ControlController {
    /// Returns immutable negotiated connection properties.
    pub fn negotiated(&self) -> &NegotiatedSession {
        &self.negotiated
    }

    /// Receives one sequenced lifecycle event.
    pub async fn recv_event(&mut self) -> Option<ControlEvent> {
        self.events.recv().await
    }

    /// Receives one pending breakpoint decision.
    pub async fn recv_decision(&mut self) -> Option<PendingDecision> {
        self.decisions.recv().await.map(|envelope| PendingDecision {
            request: envelope.request,
            reply: envelope.reply,
            max_body_edit_bytes: envelope.max_body_edit_bytes,
            deadline: envelope.deadline,
        })
    }

    /// Receives whichever bounded controller queue becomes ready first.
    pub async fn recv(&mut self) -> Option<ControllerMessage> {
        tokio::select! {
            biased;
            decision = self.decisions.recv() => decision.map(|envelope| {
                ControllerMessage::Decision(PendingDecision {
                    request: envelope.request,
                    reply: envelope.reply,
                    max_body_edit_bytes: envelope.max_body_edit_bytes,
            deadline: envelope.deadline,
                })
            }),
            event = self.events.recv() => event.map(ControllerMessage::Event),
        }
    }
}

/// One pending request carrying a single-use reply capability.
pub struct PendingDecision {
    request: BreakpointRequest,
    reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<DecisionAction>>>>,
    max_body_edit_bytes: usize,
    deadline: Instant,
}

impl std::fmt::Debug for PendingDecision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingDecision")
            .field("decision_id", &self.request.decision_id)
            .field("exchange_id", &self.request.exchange_id)
            .field("phase", &self.request.phase)
            .finish_non_exhaustive()
    }
}

impl PendingDecision {
    /// Immutable controller request.
    pub fn request(&self) -> &BreakpointRequest {
        &self.request
    }

    /// Remaining time before the producer's actual decision deadline.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Whether the producer still accepts a decision before its deadline.
    pub fn is_pending(&self) -> bool {
        !self.remaining().is_zero()
            && self
                .reply
                .lock()
                .is_ok_and(|reply| reply.as_ref().is_some_and(|sender| !sender.is_closed()))
    }

    /// Replies exactly once after validating correlation and negotiated bounds.
    ///
    /// # Errors
    ///
    /// Returns a typed correlation, validation, duplicate, stale, or local
    /// synchronization error.
    pub fn reply(&self, command: DecisionCommand) -> Result<(), ReplyError> {
        if command.decision_id != self.request.decision_id
            || command.exchange_id != self.request.exchange_id
        {
            return Err(ReplyError::WrongCorrelation);
        }
        if self.remaining().is_zero() {
            return Err(ReplyError::Stale);
        }
        command
            .validate(self.max_body_edit_bytes)
            .map_err(ReplyError::InvalidModel)?;
        let sender = self
            .reply
            .lock()
            .map_err(|_| ReplyError::Unavailable)?
            .take()
            .ok_or(ReplyError::Duplicate)?;
        sender.send(command.action).map_err(|_| ReplyError::Stale)
    }
}

/// Negotiates an exact same-build v0 in-process connection.
///
/// # Errors
///
/// Returns a typed handshake error for invalid or incompatible peers.
pub fn connect(
    producer: &Handshake,
    controller: &Handshake,
    config: TransportConfig,
) -> Result<(ControlProducer, ControlController), HandshakeError> {
    producer
        .validate()
        .map_err(HandshakeError::InvalidProducer)?;
    controller
        .validate()
        .map_err(HandshakeError::InvalidController)?;
    if producer.revision != EXPERIMENTAL_CONTROL_REVISION
        || controller.revision != EXPERIMENTAL_CONTROL_REVISION
        || producer.revision != controller.revision
    {
        return Err(HandshakeError::RevisionMismatch {
            producer: producer.revision,
            controller: controller.revision,
        });
    }
    if producer.build_id != controller.build_id {
        return Err(HandshakeError::BuildMismatch);
    }
    if config.decision_timeout.is_zero() {
        return Err(HandshakeError::InvalidTimeout);
    }
    let (event_sender, event_receiver) = mpsc::channel(config.event_capacity.get());
    let (decision_sender, decision_receiver) = mpsc::channel(config.decision_capacity.get());
    let max_body_edit_bytes = producer
        .max_body_edit_bytes
        .min(controller.max_body_edit_bytes);
    let negotiated = Arc::new(NegotiatedSession {
        revision: producer.revision,
        build_id: Arc::from(producer.build_id.as_str()),
        capabilities: producer
            .capabilities
            .intersection(&controller.capabilities)
            .copied()
            .collect(),
        max_body_edit_bytes,
    });
    Ok((
        ControlProducer {
            events: event_sender,
            decisions: decision_sender,
            next_decision: Arc::new(AtomicU64::new(1)),
            decision_timeout: config.decision_timeout,
            max_body_edit_bytes,
            negotiated: Arc::clone(&negotiated),
        },
        ControlController {
            events: event_receiver,
            decisions: decision_receiver,
            negotiated,
        },
    ))
}

fn map_send_error<T>(error: mpsc::error::TrySendError<T>) -> TransportError {
    match error {
        mpsc::error::TrySendError::Full(value) => {
            drop(value);
            TransportError::Saturated
        }
        mpsc::error::TrySendError::Closed(value) => {
            drop(value);
            TransportError::Disconnected
        }
    }
}

/// Handshake rejection.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HandshakeError {
    /// Producer supplied an invalid handshake.
    #[error("invalid producer handshake: {0}")]
    InvalidProducer(ModelError),
    /// Controller supplied an invalid handshake.
    #[error("invalid controller handshake: {0}")]
    InvalidController(ModelError),
    /// Experimental revisions differed or were unsupported.
    #[error("control revision mismatch: producer {producer}, controller {controller}")]
    RevisionMismatch {
        /// Producer revision.
        producer: u32,
        /// Controller revision.
        controller: u32,
    },
    /// v0 build identities differed.
    #[error("experimental control peers must use the same build")]
    BuildMismatch,
    /// Decision timeout was zero.
    #[error("control decision timeout must be nonzero")]
    InvalidTimeout,
}

/// Producer-side bounded delivery failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TransportError {
    /// Queue was full.
    #[error("control queue is saturated")]
    Saturated,
    /// Controller endpoint disconnected.
    #[error("control endpoint disconnected")]
    Disconnected,
    /// Decision deadline elapsed.
    #[error("control decision timed out")]
    TimedOut,
    /// Request was cancelled.
    #[error("control decision was cancelled")]
    Cancelled,
    /// Decision timeout was zero.
    #[error("control decision timeout must be nonzero")]
    InvalidTimeout,
    /// Payload violated the negotiated model.
    #[error("invalid control payload: {0}")]
    InvalidModel(ModelError),
}

/// Controller reply rejection.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ReplyError {
    /// IDs did not identify this pending decision.
    #[error("control reply correlation does not match the pending decision")]
    WrongCorrelation,
    /// Reply capability was already consumed.
    #[error("control reply is a duplicate")]
    Duplicate,
    /// Request ended before the reply arrived.
    #[error("control reply is stale")]
    Stale,
    /// Reply guard was poisoned.
    #[error("control reply is unavailable")]
    Unavailable,
    /// Reply violated a negotiated model bound.
    #[error("invalid control reply: {0}")]
    InvalidModel(ModelError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_control_model::{BreakpointPhase, ControlEvent, ControlExchangeId, EventKind};

    fn endpoints(timeout: Duration) -> (ControlProducer, ControlController) {
        let handshake = Handshake::v0("test-build", 4);
        connect(
            &handshake,
            &handshake,
            TransportConfig {
                event_capacity: NonZeroUsize::new(1).unwrap(),
                decision_capacity: NonZeroUsize::new(1).unwrap(),
                decision_timeout: timeout,
            },
        )
        .unwrap()
    }

    fn command(pending: &PendingDecision, action: DecisionAction) -> DecisionCommand {
        DecisionCommand {
            decision_id: pending.request().decision_id,
            exchange_id: pending.request().exchange_id,
            action,
        }
    }

    fn input(exchange_id: u128, phase: BreakpointPhase) -> BreakpointInput {
        BreakpointInput {
            exchange_id: ControlExchangeId(exchange_id),
            phase,
            request_head: None,
            response_head: None,
            body: None,
        }
    }

    #[test]
    fn handshake_rejects_revision_build_and_invalid_values() {
        let valid = Handshake::v0("a", 1);
        let mut other = valid.clone();
        other.revision = 1;
        assert!(matches!(
            connect(&valid, &other, TransportConfig::default()),
            Err(HandshakeError::RevisionMismatch { .. })
        ));
        other = Handshake::v0("b", 1);
        assert!(matches!(
            connect(&valid, &other, TransportConfig::default()),
            Err(HandshakeError::BuildMismatch)
        ));
        assert!(matches!(
            connect(&Handshake::v0("", 1), &valid, TransportConfig::default()),
            Err(HandshakeError::InvalidProducer(_))
        ));

        let mut producer = Handshake::v0("same", 8);
        producer
            .capabilities
            .extend([Capability::RequestHead, Capability::ResponseHead]);
        let mut controller = Handshake::v0("same", 4);
        controller
            .capabilities
            .extend([Capability::RequestHead, Capability::RequestBody]);
        let (producer_endpoint, controller_endpoint) =
            connect(&producer, &controller, TransportConfig::default()).unwrap();
        assert_eq!(
            producer_endpoint.negotiated(),
            controller_endpoint.negotiated()
        );
        assert_eq!(producer_endpoint.negotiated().max_body_edit_bytes, 4);
        assert_eq!(
            producer_endpoint.negotiated().capabilities,
            [Capability::RequestHead].into_iter().collect()
        );
    }

    #[tokio::test]
    async fn events_are_bounded_and_ordered() {
        let (producer, mut controller) = endpoints(Duration::from_secs(1));
        let first = ControlEvent {
            sequence: 1,
            exchange_id: ControlExchangeId(1),
            kind: EventKind::ExchangeStarted,
        };
        producer.publish(first.clone()).unwrap();
        assert_eq!(
            producer.publish(ControlEvent {
                sequence: 2,
                exchange_id: ControlExchangeId(1),
                kind: EventKind::Completed,
            }),
            Err(TransportError::Saturated)
        );
        assert_eq!(controller.recv_event().await, Some(first));
    }

    #[tokio::test]
    async fn decisions_reject_wrong_duplicate_stale_and_oversized_replies() {
        let (producer, mut controller) = endpoints(Duration::from_millis(20));
        let waiting = tokio::spawn(async move {
            producer
                .request(
                    input(7, BreakpointPhase::RequestHead),
                    &RequestCancellation::new(),
                )
                .await
        });
        let pending = controller.recv_decision().await.unwrap();
        let mut wrong = command(&pending, DecisionAction::Continue);
        wrong.exchange_id = ControlExchangeId(8);
        assert_eq!(pending.reply(wrong), Err(ReplyError::WrongCorrelation));
        assert!(matches!(
            pending.reply(command(
                &pending,
                DecisionAction::ReplaceBody { body: vec![0; 5] }
            )),
            Err(ReplyError::InvalidModel(
                ModelError::BodyEditTooLarge { .. }
            ))
        ));
        pending
            .reply(command(&pending, DecisionAction::Continue))
            .unwrap();
        assert_eq!(
            pending.reply(command(&pending, DecisionAction::Continue)),
            Err(ReplyError::Duplicate)
        );
        assert_eq!(waiting.await.unwrap(), Ok(DecisionAction::Continue));

        let (producer, mut controller) = endpoints(Duration::from_millis(5));
        let waiting = tokio::spawn(async move {
            producer
                .request(
                    input(9, BreakpointPhase::ResponseHead),
                    &RequestCancellation::new(),
                )
                .await
        });
        let pending = controller.recv_decision().await.unwrap();
        assert_eq!(waiting.await.unwrap(), Err(TransportError::TimedOut));
        assert!(pending.remaining().is_zero());
        assert!(!pending.is_pending());
        assert_eq!(
            pending.reply(command(&pending, DecisionAction::Continue)),
            Err(ReplyError::Stale)
        );
    }

    #[tokio::test]
    async fn saturation_disconnect_and_cancellation_are_isolated() {
        let (producer, controller) = endpoints(Duration::from_millis(20));
        let first = producer.clone();
        let occupied = tokio::spawn(async move {
            first
                .request(
                    input(1, BreakpointPhase::RequestHead),
                    &RequestCancellation::new(),
                )
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(
            producer
                .request(
                    input(2, BreakpointPhase::RequestHead),
                    &RequestCancellation::new(),
                )
                .await,
            Err(TransportError::Saturated)
        );
        drop(controller);
        assert_eq!(occupied.await.unwrap(), Err(TransportError::Disconnected));

        let (producer, mut controller) = endpoints(Duration::from_secs(1));
        let cancellation = RequestCancellation::new();
        let trigger = cancellation.clone();
        let waiting = tokio::spawn(async move {
            producer
                .request(input(3, BreakpointPhase::ResponseBody), &cancellation)
                .await
        });
        let pending = controller.recv_decision().await.unwrap();
        assert!(pending.is_pending());
        trigger.cancel();
        assert_eq!(waiting.await.unwrap(), Err(TransportError::Cancelled));
        assert!(!pending.is_pending());
        assert_eq!(
            pending.reply(command(&pending, DecisionAction::Continue)),
            Err(ReplyError::Stale)
        );
    }

    #[tokio::test]
    async fn out_of_order_replies_remain_correlated() {
        let handshake = Handshake::v0("test-build", 4);
        let (producer, mut controller) = connect(
            &handshake,
            &handshake,
            TransportConfig {
                event_capacity: NonZeroUsize::new(1).unwrap(),
                decision_capacity: NonZeroUsize::new(2).unwrap(),
                decision_timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let first_producer = producer.clone();
        let first = tokio::spawn(async move {
            first_producer
                .request(
                    input(10, BreakpointPhase::RequestHead),
                    &RequestCancellation::new(),
                )
                .await
        });
        let second = tokio::spawn(async move {
            producer
                .request(
                    input(11, BreakpointPhase::ResponseHead),
                    &RequestCancellation::new(),
                )
                .await
        });
        let one = controller.recv_decision().await.unwrap();
        let two = controller.recv_decision().await.unwrap();
        for pending in [&two, &one] {
            let action = if pending.request().exchange_id == ControlExchangeId(10) {
                DecisionAction::Continue
            } else {
                DecisionAction::Abort {
                    reason: "second".to_owned(),
                }
            };
            pending.reply(command(pending, action)).unwrap();
        }

        assert_eq!(first.await.unwrap(), Ok(DecisionAction::Continue));
        assert_eq!(
            second.await.unwrap(),
            Ok(DecisionAction::Abort {
                reason: "second".to_owned()
            })
        );
    }
}
