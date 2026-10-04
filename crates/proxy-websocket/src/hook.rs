use std::{
    collections::BTreeSet,
    fmt::Debug,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use futures_util::FutureExt;
use thiserror::Error;
use tokio::{sync::Semaphore, time::timeout};

use crate::{CloseFrame, ControlFrame, DataKind, Direction, Message, SessionCancellation};

/// Heap-owned future returned by WebSocket hooks.
pub type BoxWebSocketFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stable validated WebSocket hook ID.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WebSocketHookId(Arc<str>);

impl WebSocketHookId {
    /// Creates a stable identifier.
    ///
    /// # Errors
    ///
    /// IDs must be non-empty ASCII letters, digits, dots, underscores,
    /// hyphens, or slashes.
    pub fn new(value: impl Into<Arc<str>>) -> Result<Self, WebSocketHookError> {
        let value = value.into();
        if value.is_empty()
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
            })
        {
            return Err(WebSocketHookError::InvalidId(value.to_string()));
        }
        Ok(Self(value))
    }

    /// Returns the caller-owned stable identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Immutable identity assigned to one configured hook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketHookIdentity {
    /// Stable machine identity.
    pub id: WebSocketHookId,
    /// Operator-facing label.
    pub display_name: Arc<str>,
    /// Zero-based request-direction position.
    pub chain_position: usize,
}

/// Metadata shared by every hook created for one upgraded session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketSessionMetadata {
    /// Stable session ID assigned by the embedding runtime.
    pub session_id: u128,
    /// Redaction-safe normalized target label.
    pub target: Arc<str>,
    /// Negotiated application subprotocol.
    pub subprotocol: Option<Arc<str>>,
}

/// Message callback input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageEventHook {
    /// Session metadata.
    pub metadata: Arc<WebSocketSessionMetadata>,
    /// Traffic direction.
    pub direction: Direction,
    /// Current message after preceding replacements.
    pub message: Message,
}

/// Control-frame callback input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlEvent {
    /// Session metadata.
    pub metadata: Arc<WebSocketSessionMetadata>,
    /// Traffic direction.
    pub direction: Direction,
    /// Current control frame after preceding replacements.
    pub frame: ControlFrame,
}

/// Hook decision for a complete application message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageAction {
    /// Preserve the current message.
    Continue,
    /// Replace the decompressed payload, retaining text/binary kind.
    Replace(Bytes),
    /// Suppress the message.
    Drop,
    /// Send a close frame instead and stop this direction.
    Close(CloseFrame),
}

/// Hook decision for a control frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlAction {
    /// Preserve the current control frame.
    Continue,
    /// Replace it with another valid control frame.
    Replace(ControlFrame),
    /// Suppress it.
    Drop,
    /// Send this close frame instead and stop this direction.
    Close(CloseFrame),
}

/// Application-defined WebSocket interception callbacks.
pub trait WebSocketInterceptor: Send + Sync + Debug {
    /// Observes or changes one complete, decompressed message.
    fn on_message(&self, _event: MessageEventHook) -> BoxWebSocketFuture<'_, MessageAction> {
        Box::pin(async { MessageAction::Continue })
    }

    /// Observes or changes one control frame.
    fn on_control(&self, _event: ControlEvent) -> BoxWebSocketFuture<'_, ControlAction> {
        Box::pin(async { ControlAction::Continue })
    }
}

/// Creates exchange-local hook state for one upgraded session.
pub trait WebSocketInterceptorFactory: Send + Sync + Debug {
    /// Creates one interceptor instance.
    ///
    /// # Errors
    ///
    /// Returns a redaction-safe initialization message.
    fn create(
        &self,
        metadata: &WebSocketSessionMetadata,
    ) -> Result<Arc<dyn WebSocketInterceptor>, String>;
}

/// One configured identified hook.
#[derive(Clone)]
pub struct WebSocketHookRegistration {
    id: WebSocketHookId,
    display_name: Arc<str>,
    factory: Arc<dyn WebSocketInterceptorFactory>,
}

impl Debug for WebSocketHookRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebSocketHookRegistration")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .finish_non_exhaustive()
    }
}

impl WebSocketHookRegistration {
    /// Creates a validated registration.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketHookError::InvalidId`] for an invalid stable ID.
    pub fn new(
        id: impl Into<Arc<str>>,
        display_name: impl Into<Arc<str>>,
        factory: Arc<dyn WebSocketInterceptorFactory>,
    ) -> Result<Self, WebSocketHookError> {
        Ok(Self {
            id: WebSocketHookId::new(id)?,
            display_name: display_name.into(),
            factory,
        })
    }

    /// Stable registration ID.
    pub fn id(&self) -> &WebSocketHookId {
        &self.id
    }
}

/// Callback deadlines and finite pause/body limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketHookLimits {
    /// Deadline for one callback.
    pub callback_timeout: Duration,
    /// Maximum simultaneous callbacks across sessions using this factory.
    pub max_concurrent_callbacks: usize,
    /// Maximum payload a hook may return.
    pub max_replacement_bytes: usize,
}

impl Default for WebSocketHookLimits {
    fn default() -> Self {
        Self {
            callback_timeout: Duration::from_secs(5),
            max_concurrent_callbacks: 128,
            max_replacement_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Immutable factory for session-local hook chains.
#[derive(Clone, Debug)]
pub struct WebSocketHookFactory {
    registrations: Arc<[WebSocketHookRegistration]>,
    limits: WebSocketHookLimits,
    callback_slots: Arc<Semaphore>,
}

impl WebSocketHookFactory {
    /// Creates a factory and validates IDs and bounds.
    ///
    /// # Errors
    ///
    /// Rejects zero limits and duplicate stable IDs.
    pub fn new(
        registrations: Vec<WebSocketHookRegistration>,
        limits: WebSocketHookLimits,
    ) -> Result<Self, WebSocketHookError> {
        if limits.callback_timeout.is_zero()
            || limits.max_concurrent_callbacks == 0
            || limits.max_replacement_bytes == 0
        {
            return Err(WebSocketHookError::InvalidLimits);
        }
        let mut ids = BTreeSet::new();
        for registration in &registrations {
            if !ids.insert(registration.id.clone()) {
                return Err(WebSocketHookError::DuplicateId(
                    registration.id.as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            registrations: registrations.into(),
            limits,
            callback_slots: Arc::new(Semaphore::new(limits.max_concurrent_callbacks)),
        })
    }

    /// Creates an empty factory, selecting the byte-transparent relay path.
    pub fn empty() -> Self {
        let limits = WebSocketHookLimits::default();
        Self {
            registrations: Arc::from([]),
            limits,
            callback_slots: Arc::new(Semaphore::new(limits.max_concurrent_callbacks)),
        }
    }

    /// Whether no inspection hook is configured.
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Creates isolated hook instances for one upgraded session.
    ///
    /// # Errors
    ///
    /// Fails closed when any interceptor cannot initialize.
    pub fn create_session(
        &self,
        metadata: WebSocketSessionMetadata,
        cancellation: SessionCancellation,
    ) -> Result<WebSocketHookChain, WebSocketHookError> {
        let metadata = Arc::new(metadata);
        let mut hooks = Vec::with_capacity(self.registrations.len());
        for (chain_position, registration) in self.registrations.iter().enumerate() {
            let interceptor =
                catch_unwind(AssertUnwindSafe(|| registration.factory.create(&metadata)))
                    .map_err(|_| WebSocketHookError::InitializationPanicked {
                        id: registration.id.as_str().to_owned(),
                    })?
                    .map_err(|message| WebSocketHookError::Initialization {
                        id: registration.id.as_str().to_owned(),
                        message,
                    })?;
            hooks.push(SessionHook {
                identity: WebSocketHookIdentity {
                    id: registration.id.clone(),
                    display_name: Arc::clone(&registration.display_name),
                    chain_position,
                },
                interceptor,
            });
        }
        Ok(WebSocketHookChain {
            metadata,
            hooks,
            limits: self.limits,
            callback_slots: Arc::clone(&self.callback_slots),
            cancellation,
            effects: Mutex::new(Vec::new()),
        })
    }
}

#[derive(Clone)]
struct SessionHook {
    identity: WebSocketHookIdentity,
    interceptor: Arc<dyn WebSocketInterceptor>,
}

impl Debug for SessionHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionHook")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Audited action category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookActionKind {
    /// No mutation.
    Continue,
    /// Payload or control replacement.
    Replace,
    /// Item suppression.
    Drop,
    /// Hook-initiated close.
    Close,
}

/// One centrally attributed hook decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketEffect {
    /// Monotonic decision order within this session.
    pub order: u64,
    /// Hook identity and configured chain position.
    pub hook: WebSocketHookIdentity,
    /// Traffic direction.
    pub direction: Direction,
    /// Decision category.
    pub action: HookActionKind,
    /// Payload length before the callback.
    pub input_bytes: usize,
    /// Payload length after the callback, when forwarded.
    pub output_bytes: Option<usize>,
}

/// Result of applying all hooks to an application message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageOutcome {
    /// Forward this message.
    Forward(Message),
    /// Suppress it.
    Drop,
    /// Send a close frame and stop this direction.
    Close(CloseFrame),
}

/// Result of applying all hooks to a control frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlOutcome {
    /// Forward this control frame.
    Forward(ControlFrame),
    /// Suppress it.
    Drop,
    /// Send a close frame and stop this direction.
    Close(CloseFrame),
}

/// Session-local directional hook chain and authoritative audit trail.
#[derive(Debug)]
pub struct WebSocketHookChain {
    metadata: Arc<WebSocketSessionMetadata>,
    hooks: Vec<SessionHook>,
    limits: WebSocketHookLimits,
    callback_slots: Arc<Semaphore>,
    cancellation: SessionCancellation,
    effects: Mutex<Vec<WebSocketEffect>>,
}

impl WebSocketHookChain {
    /// Applies hooks in request order client-to-server and reverse order
    /// server-to-client.
    ///
    /// # Errors
    ///
    /// Fails on cancellation, timeout, saturation, poison, invalid UTF-8, or
    /// oversized replacement.
    pub async fn process_message(
        &self,
        direction: Direction,
        mut message: Message,
    ) -> Result<MessageOutcome, WebSocketHookError> {
        for index in directional_indices(self.hooks.len(), direction) {
            let hook = &self.hooks[index];
            let input_bytes = message.payload.len();
            let action = self.call_message(hook, direction, message.clone()).await?;
            let (kind, output_bytes, terminal) = match action {
                MessageAction::Continue => {
                    (HookActionKind::Continue, Some(message.payload.len()), None)
                }
                MessageAction::Replace(payload) => {
                    validate_replacement(
                        message.kind,
                        &payload,
                        self.limits.max_replacement_bytes,
                    )?;
                    message.payload = payload;
                    (HookActionKind::Replace, Some(message.payload.len()), None)
                }
                MessageAction::Drop => (HookActionKind::Drop, None, Some(MessageOutcome::Drop)),
                MessageAction::Close(close) => (
                    HookActionKind::Close,
                    None,
                    Some(MessageOutcome::Close(close)),
                ),
            };
            self.record(&hook.identity, direction, kind, input_bytes, output_bytes)?;
            if let Some(terminal) = terminal {
                return Ok(terminal);
            }
        }
        Ok(MessageOutcome::Forward(message))
    }

    /// Applies hooks to a control frame using directional ordering.
    ///
    /// # Errors
    ///
    /// Fails on cancellation, timeout, saturation, poison, or invalid
    /// replacement control payload.
    pub async fn process_control(
        &self,
        direction: Direction,
        mut frame: ControlFrame,
    ) -> Result<ControlOutcome, WebSocketHookError> {
        for index in directional_indices(self.hooks.len(), direction) {
            let hook = &self.hooks[index];
            let input_bytes = control_length(&frame)?;
            let action = self.call_control(hook, direction, frame.clone()).await?;
            let (kind, output_bytes, terminal) = match action {
                ControlAction::Continue => (
                    HookActionKind::Continue,
                    Some(control_length(&frame)?),
                    None,
                ),
                ControlAction::Replace(replacement) => {
                    let length = control_length(&replacement)?;
                    frame = replacement;
                    (HookActionKind::Replace, Some(length), None)
                }
                ControlAction::Drop => (HookActionKind::Drop, None, Some(ControlOutcome::Drop)),
                ControlAction::Close(close) => (
                    HookActionKind::Close,
                    None,
                    Some(ControlOutcome::Close(close)),
                ),
            };
            self.record(&hook.identity, direction, kind, input_bytes, output_bytes)?;
            if let Some(terminal) = terminal {
                return Ok(terminal);
            }
        }
        Ok(ControlOutcome::Forward(frame))
    }

    /// Snapshot of decisions recorded so far.
    ///
    /// # Errors
    ///
    /// Returns an error if application code poisoned the audit mutex.
    pub fn effects(&self) -> Result<Vec<WebSocketEffect>, WebSocketHookError> {
        self.effects
            .lock()
            .map(|effects| effects.clone())
            .map_err(|_| WebSocketHookError::AuditPoisoned)
    }

    async fn call_message(
        &self,
        hook: &SessionHook,
        direction: Direction,
        message: Message,
    ) -> Result<MessageAction, WebSocketHookError> {
        let permit = self
            .callback_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| WebSocketHookError::Saturated)?;
        let future = AssertUnwindSafe(hook.interceptor.on_message(MessageEventHook {
            metadata: Arc::clone(&self.metadata),
            direction,
            message,
        }))
        .catch_unwind();
        let outcome = tokio::select! {
            () = self.cancellation.cancelled() => Err(WebSocketHookError::Cancelled),
            outcome = timeout(self.limits.callback_timeout, future) => {
                outcome
                    .map_err(|_| WebSocketHookError::TimedOut)?
                    .map_err(|_| WebSocketHookError::Panicked)
            }
        };
        drop(permit);
        outcome
    }

    async fn call_control(
        &self,
        hook: &SessionHook,
        direction: Direction,
        frame: ControlFrame,
    ) -> Result<ControlAction, WebSocketHookError> {
        let permit = self
            .callback_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| WebSocketHookError::Saturated)?;
        let future = AssertUnwindSafe(hook.interceptor.on_control(ControlEvent {
            metadata: Arc::clone(&self.metadata),
            direction,
            frame,
        }))
        .catch_unwind();
        let outcome = tokio::select! {
            () = self.cancellation.cancelled() => Err(WebSocketHookError::Cancelled),
            outcome = timeout(self.limits.callback_timeout, future) => {
                outcome
                    .map_err(|_| WebSocketHookError::TimedOut)?
                    .map_err(|_| WebSocketHookError::Panicked)
            }
        };
        drop(permit);
        outcome
    }

    fn record(
        &self,
        identity: &WebSocketHookIdentity,
        direction: Direction,
        action: HookActionKind,
        input_bytes: usize,
        output_bytes: Option<usize>,
    ) -> Result<(), WebSocketHookError> {
        let mut effects = self
            .effects
            .lock()
            .map_err(|_| WebSocketHookError::AuditPoisoned)?;
        let order = u64::try_from(effects.len())
            .map_err(|_| WebSocketHookError::AuditOverflow)?
            .checked_add(1)
            .ok_or(WebSocketHookError::AuditOverflow)?;
        effects.push(WebSocketEffect {
            order,
            hook: identity.clone(),
            direction,
            action,
            input_bytes,
            output_bytes,
        });
        Ok(())
    }
}

fn directional_indices(
    length: usize,
    direction: Direction,
) -> Box<dyn Iterator<Item = usize> + Send> {
    match direction {
        Direction::ClientToServer => Box::new(0..length),
        Direction::ServerToClient => Box::new((0..length).rev()),
    }
}

fn validate_replacement(
    kind: DataKind,
    payload: &[u8],
    limit: usize,
) -> Result<(), WebSocketHookError> {
    if payload.len() > limit {
        return Err(WebSocketHookError::ReplacementLimitExceeded);
    }
    if kind == DataKind::Text && std::str::from_utf8(payload).is_err() {
        return Err(WebSocketHookError::InvalidTextReplacement);
    }
    Ok(())
}

fn control_length(frame: &ControlFrame) -> Result<usize, WebSocketHookError> {
    match frame {
        ControlFrame::Close(close) => close
            .encode()
            .map(|payload| payload.len())
            .map_err(|error| WebSocketHookError::InvalidControlReplacement(error.to_string())),
        ControlFrame::Ping(payload) | ControlFrame::Pong(payload) if payload.len() <= 125 => {
            Ok(payload.len())
        }
        ControlFrame::Ping(_) | ControlFrame::Pong(_) => {
            Err(WebSocketHookError::InvalidControlReplacement(
                "control payload exceeds 125 bytes".to_owned(),
            ))
        }
    }
}

/// Hook configuration or execution failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WebSocketHookError {
    /// Stable ID had invalid syntax.
    #[error("invalid WebSocket hook id: {0}")]
    InvalidId(String),
    /// Stable ID was configured more than once.
    #[error("duplicate WebSocket hook id: {0}")]
    DuplicateId(String),
    /// A callback or replacement bound was zero.
    #[error("invalid WebSocket hook limits")]
    InvalidLimits,
    /// A session-local interceptor could not be created.
    #[error("WebSocket hook {id} initialization failed: {message}")]
    Initialization {
        /// Stable hook ID.
        id: String,
        /// Redaction-safe failure message.
        message: String,
    },
    /// Session-local hook factory panicked.
    #[error("WebSocket hook {id} initialization panicked")]
    InitializationPanicked {
        /// Stable hook ID.
        id: String,
    },
    /// Global callback concurrency bound was exhausted.
    #[error("WebSocket hook callback capacity is saturated")]
    Saturated,
    /// Session was cancelled while a hook was running.
    #[error("WebSocket hook was cancelled")]
    Cancelled,
    /// Hook exceeded its per-callback deadline.
    #[error("WebSocket hook timed out")]
    TimedOut,
    /// Hook callback panicked.
    #[error("WebSocket hook panicked")]
    Panicked,
    /// Replacement exceeded its finite byte limit.
    #[error("WebSocket hook replacement limit exceeded")]
    ReplacementLimitExceeded,
    /// Text replacement was not UTF-8.
    #[error("WebSocket text replacement is not UTF-8")]
    InvalidTextReplacement,
    /// Control replacement could not be represented safely.
    #[error("invalid WebSocket control replacement: {0}")]
    InvalidControlReplacement(String),
    /// Audit lock was poisoned by panicking application code.
    #[error("WebSocket hook audit trail is poisoned")]
    AuditPoisoned,
    /// Audit sequence could not be represented.
    #[error("WebSocket hook audit sequence overflow")]
    AuditOverflow,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug)]
    struct FixedFactory {
        hook: Arc<dyn WebSocketInterceptor>,
    }

    impl WebSocketInterceptorFactory for FixedFactory {
        fn create(
            &self,
            _metadata: &WebSocketSessionMetadata,
        ) -> Result<Arc<dyn WebSocketInterceptor>, String> {
            Ok(Arc::clone(&self.hook))
        }
    }

    #[derive(Debug)]
    struct AppendHook(&'static [u8]);

    impl WebSocketInterceptor for AppendHook {
        fn on_message(&self, event: MessageEventHook) -> BoxWebSocketFuture<'_, MessageAction> {
            let mut value = event.message.payload.to_vec();
            value.extend_from_slice(self.0);
            Box::pin(async move { MessageAction::Replace(Bytes::from(value)) })
        }
    }

    fn registration(id: &str, suffix: &'static [u8]) -> WebSocketHookRegistration {
        WebSocketHookRegistration::new(
            id,
            id,
            Arc::new(FixedFactory {
                hook: Arc::new(AppendHook(suffix)),
            }),
        )
        .unwrap()
    }

    fn metadata(id: u128) -> WebSocketSessionMetadata {
        WebSocketSessionMetadata {
            session_id: id,
            target: Arc::from("wss://example.test/socket"),
            subprotocol: None,
        }
    }

    #[tokio::test]
    async fn direction_order_and_attribution_are_deterministic() {
        let factory = WebSocketHookFactory::new(
            vec![registration("one", b"1"), registration("two", b"2")],
            WebSocketHookLimits::default(),
        )
        .unwrap();
        let chain = factory
            .create_session(metadata(1), SessionCancellation::new())
            .unwrap();
        let original = Message {
            kind: DataKind::Text,
            payload: Bytes::from_static(b"x"),
            was_compressed: false,
        };
        let MessageOutcome::Forward(request) = chain
            .process_message(Direction::ClientToServer, original.clone())
            .await
            .unwrap()
        else {
            panic!("expected forward");
        };
        let MessageOutcome::Forward(response) = chain
            .process_message(Direction::ServerToClient, original)
            .await
            .unwrap()
        else {
            panic!("expected forward");
        };
        assert_eq!(request.payload, "x12");
        assert_eq!(response.payload, "x21");
        let effects = chain.effects().unwrap();
        assert_eq!(effects.len(), 4);
        assert_eq!(effects[0].hook.id.as_str(), "one");
        assert_eq!(effects[1].hook.id.as_str(), "two");
        assert_eq!(effects[2].hook.id.as_str(), "two");
        assert_eq!(effects[3].hook.id.as_str(), "one");
        assert_eq!(effects[0].order, 1);
        assert_eq!(effects[3].order, 4);
    }

    #[derive(Debug)]
    struct BlockingHook {
        entered: Arc<AtomicUsize>,
    }

    impl WebSocketInterceptor for BlockingHook {
        fn on_message(&self, _event: MessageEventHook) -> BoxWebSocketFuture<'_, MessageAction> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn saturation_is_bounded_and_does_not_wait_behind_another_session() {
        let entered = Arc::new(AtomicUsize::new(0));
        let registration = WebSocketHookRegistration::new(
            "block",
            "block",
            Arc::new(FixedFactory {
                hook: Arc::new(BlockingHook {
                    entered: Arc::clone(&entered),
                }),
            }),
        )
        .unwrap();
        let factory = WebSocketHookFactory::new(
            vec![registration],
            WebSocketHookLimits {
                callback_timeout: Duration::from_millis(100),
                max_concurrent_callbacks: 1,
                max_replacement_bytes: 1024,
            },
        )
        .unwrap();
        let first = Arc::new(
            factory
                .create_session(metadata(1), SessionCancellation::new())
                .unwrap(),
        );
        let second = factory
            .create_session(metadata(2), SessionCancellation::new())
            .unwrap();
        let message = Message {
            kind: DataKind::Binary,
            payload: Bytes::new(),
            was_compressed: false,
        };
        let task = tokio::spawn({
            let first = Arc::clone(&first);
            let message = message.clone();
            async move {
                first
                    .process_message(Direction::ClientToServer, message)
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert_eq!(
            second
                .process_message(Direction::ClientToServer, message)
                .await,
            Err(WebSocketHookError::Saturated)
        );
        assert_eq!(task.await.unwrap(), Err(WebSocketHookError::TimedOut));
    }

    #[derive(Debug)]
    struct PanicHook;

    impl WebSocketInterceptor for PanicHook {
        fn on_message(&self, _event: MessageEventHook) -> BoxWebSocketFuture<'_, MessageAction> {
            Box::pin(async { panic!("test hook panic") })
        }
    }

    #[tokio::test]
    async fn callback_panics_are_contained_as_typed_failures() {
        let registration = WebSocketHookRegistration::new(
            "panic",
            "panic",
            Arc::new(FixedFactory {
                hook: Arc::new(PanicHook),
            }),
        )
        .unwrap();
        let factory =
            WebSocketHookFactory::new(vec![registration], WebSocketHookLimits::default()).unwrap();
        let chain = factory
            .create_session(metadata(4), SessionCancellation::new())
            .unwrap();
        assert_eq!(
            chain
                .process_message(
                    Direction::ClientToServer,
                    Message {
                        kind: DataKind::Binary,
                        payload: Bytes::new(),
                        was_compressed: false,
                    },
                )
                .await,
            Err(WebSocketHookError::Panicked)
        );
    }
}
