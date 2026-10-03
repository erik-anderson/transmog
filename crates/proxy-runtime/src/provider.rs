//! Replaceable clocks and identifiers used by runtime orchestration.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime},
};

/// Runtime identifier namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeIdKind {
    /// A downstream transport connection.
    Connection,
    /// One logical request/response exchange.
    Exchange,
    /// A protocol stream or local HTTP/1 request sequence.
    Stream,
}

/// Source of stable runtime identifiers.
pub trait RuntimeIdGenerator: Send + Sync {
    /// Returns the next nonzero identifier in `kind`'s namespace.
    fn next_id(&self, kind: RuntimeIdKind) -> u128;
}

/// Process-local monotonic identifier source.
#[derive(Debug)]
pub struct AtomicRuntimeIdGenerator {
    connection: AtomicU64,
    exchange: AtomicU64,
    stream: AtomicU64,
}

impl AtomicRuntimeIdGenerator {
    /// Creates three independent monotonic namespaces beginning at one.
    pub const fn new() -> Self {
        Self {
            connection: AtomicU64::new(1),
            exchange: AtomicU64::new(1),
            stream: AtomicU64::new(1),
        }
    }
}

impl Default for AtomicRuntimeIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeIdGenerator for AtomicRuntimeIdGenerator {
    fn next_id(&self, kind: RuntimeIdKind) -> u128 {
        let counter = match kind {
            RuntimeIdKind::Connection => &self.connection,
            RuntimeIdKind::Exchange => &self.exchange,
            RuntimeIdKind::Stream => &self.stream,
        };
        u128::from(counter.fetch_add(1, Ordering::Relaxed))
    }
}

/// Source of wall-clock evidence and monotonic cache deadlines.
pub trait RuntimeClock: Send + Sync {
    /// Returns the current wall-clock time.
    fn system_time(&self) -> SystemTime;

    /// Returns the current monotonic time.
    fn instant(&self) -> Instant;
}

/// Operating-system runtime clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRuntimeClock;

impl RuntimeClock for SystemRuntimeClock {
    fn system_time(&self) -> SystemTime {
        SystemTime::now()
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_namespaces_are_independent_and_nonzero() {
        let ids = AtomicRuntimeIdGenerator::new();
        assert_eq!(ids.next_id(RuntimeIdKind::Connection), 1);
        assert_eq!(ids.next_id(RuntimeIdKind::Connection), 2);
        assert_eq!(ids.next_id(RuntimeIdKind::Exchange), 1);
        assert_eq!(ids.next_id(RuntimeIdKind::Stream), 1);
    }
}
