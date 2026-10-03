use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use tokio::sync::Notify;

use crate::{
    ConnectionId, ExchangeExtensions, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
};

/// Stable identifier for one Hooks v2 exchange.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExchangeId(pub u128);

impl From<SessionId> for ExchangeId {
    fn from(value: SessionId) -> Self {
        Self(value.0)
    }
}

/// Immutable target derived from the client-facing request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginalTarget(Target);

impl OriginalTarget {
    /// Captures a client target as immutable exchange evidence.
    pub fn new(target: Target) -> Self {
        Self(target)
    }

    /// Returns the captured client target.
    pub fn as_target(&self) -> &Target {
        &self.0
    }
}

/// Immutable metadata shared by every callback for one exchange.
#[derive(Clone, Debug)]
pub struct ExchangeMetadata {
    /// Stable exchange identifier.
    pub exchange_id: ExchangeId,
    /// Browser-facing transport connection.
    pub downstream_connection_id: ConnectionId,
    /// Browser-facing protocol stream or local HTTP/1 sequence.
    pub downstream_stream_id: StreamId,
    /// Client peer address.
    pub client_addr: SocketAddr,
    /// Proxy listener address.
    pub listener_addr: SocketAddr,
    /// Browser-facing HTTP version.
    pub ingress_version: HttpLegVersion,
    /// Immutable target supplied by the client.
    pub original_target: OriginalTarget,
    /// Wall-clock exchange start for operator evidence.
    pub started_at: SystemTime,
}

impl ExchangeMetadata {
    /// Builds Hooks v2 metadata from the current runtime session model.
    pub fn from_session(session: &SessionMetadata, original_target: Target) -> Self {
        Self {
            exchange_id: session.session_id.into(),
            downstream_connection_id: session.downstream_connection_id,
            downstream_stream_id: session.stream_id,
            client_addr: session.client_addr,
            listener_addr: session.proxy_addr,
            ingress_version: session.ingress_version,
            original_target: OriginalTarget::new(original_target),
            started_at: SystemTime::now(),
        }
    }
}

/// Cloneable cancellation signal scoped to one exchange.
#[derive(Clone, Debug, Default)]
pub struct ExchangeCancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Debug, Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl ExchangeCancellation {
    /// Creates a non-cancelled signal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the exchange cancelled and wakes all waiters.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Whether cancellation has already been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Waits until cancellation is requested.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.inner.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Shared context supplied to each hook call for one exchange.
#[derive(Clone, Debug)]
pub struct HookContext {
    metadata: Arc<ExchangeMetadata>,
    extensions: ExchangeExtensions,
    cancellation: ExchangeCancellation,
    timeout: Duration,
}

impl HookContext {
    /// Creates context with exchange-local extensions and cancellation.
    pub fn new(metadata: ExchangeMetadata, timeout: Duration) -> Self {
        Self {
            metadata: Arc::new(metadata),
            extensions: ExchangeExtensions::new(),
            cancellation: ExchangeCancellation::new(),
            timeout,
        }
    }

    /// Immutable metadata for this exchange.
    pub fn metadata(&self) -> &Arc<ExchangeMetadata> {
        &self.metadata
    }

    /// Exchange-local typed state.
    pub fn extensions(&self) -> &ExchangeExtensions {
        &self.extensions
    }

    /// Cloneable cancellation signal.
    pub fn cancellation(&self) -> &ExchangeCancellation {
        &self.cancellation
    }

    /// Maximum duration of one hook callback.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_wakes_existing_and_late_waiters() {
        let cancellation = ExchangeCancellation::new();
        let waiting = cancellation.clone();
        let task = tokio::spawn(async move { waiting.cancelled().await });
        cancellation.cancel();
        task.await.unwrap();
        cancellation.cancelled().await;
        assert!(cancellation.is_cancelled());
    }
}
