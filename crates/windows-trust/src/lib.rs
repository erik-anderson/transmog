//! Chromium-style local Windows trust, separate from the OS public root program.
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(windows)]
mod windows;

/// Immutable public certificate bytes with their distinct Windows trust roles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalCertificates {
    /// Notification generation; distinct from public PKI metadata versions.
    pub generation: u64,
    /// Digest of role-tagged, sorted DER and server-authentication eligibility.
    pub sha256: String,
    /// ROOT entries eligible for TLS server authentication.
    pub anchors: Vec<Vec<u8>>,
    /// CA entries used only to find issuers.
    pub intermediates: Vec<Vec<u8>>,
    /// Machine `TrustedPeople` entries; trusted only as exact server leaves.
    pub trusted_leaves: Vec<Vec<u8>>,
    /// Disallowed entries; explicit distrust for every purpose.
    pub disallowed: Vec<Vec<u8>>,
}
impl LocalCertificates {
    /// Canonicalizes an owned inventory and computes its public provenance.
    #[must_use]
    pub fn canonical(mut self) -> Self {
        let mut digest = Sha256::new();
        for (certs, role) in [
            &mut self.anchors,
            &mut self.intermediates,
            &mut self.trusted_leaves,
            &mut self.disallowed,
        ]
        .into_iter()
        .zip(0_u8..)
        {
            certs.sort();
            certs.dedup();
            digest.update([role]);
            digest.update((certs.len() as u64).to_be_bytes());
            for der in certs {
                digest.update((der.len() as u64).to_be_bytes());
                digest.update(der);
            }
        }
        self.sha256 = format!("{:x}", digest.finalize());
        self
    }
}

/// Monitoring/enumeration failure. A dirty inventory is never returned as current.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalTrustError {
    /// The initial inventory or a notified refresh is still being prepared.
    Refreshing,
    /// An owned monitor was stopped.
    Closed,
    /// Windows rejected a read or monitoring operation.
    Windows {
        /// Failed native operation, without private certificate data.
        operation: &'static str,
        /// Original Windows error code.
        code: u32,
    },
    /// Public data exceeded allocation or enumeration limits.
    Limit,
    /// A worker could not be started.
    Worker,
}
impl fmt::Display for LocalTrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refreshing => f.write_str("Windows local certificate trust is refreshing"),
            Self::Closed => f.write_str("Windows local certificate monitoring stopped"),
            Self::Windows { operation, code } => write!(
                f,
                "Windows local certificate {operation} failed (code {code})"
            ),
            Self::Limit => {
                f.write_str("Windows local certificate data exceeds its supported bounds")
            }
            Self::Worker => f.write_str("Windows local certificate monitoring could not start"),
        }
    }
}
impl std::error::Error for LocalTrustError {}

struct State {
    epoch: AtomicU64,
    dirty: AtomicBool,
    result: Mutex<Result<Arc<LocalCertificates>, LocalTrustError>>,
    ready: Condvar,
}
impl State {
    fn new() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            dirty: AtomicBool::new(true),
            result: Mutex::new(Err(LocalTrustError::Refreshing)),
            ready: Condvar::new(),
        }
    }
    fn changed(&self) {
        let _result = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
    }
    fn publish(&self, value: Result<LocalCertificates, LocalTrustError>) {
        let mut result = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(snapshot) = &value {
            self.dirty.store(
                snapshot.generation != self.epoch.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
        *result = value.map(Arc::new);
        self.ready.notify_all();
    }
    fn current(&self) -> Result<Arc<LocalCertificates>, LocalTrustError> {
        let result = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*result {
            Ok(value)
                if !self.dirty.load(Ordering::Acquire)
                    && value.generation == self.epoch.load(Ordering::Acquire) =>
            {
                Ok(value.clone())
            }
            Ok(_) => Err(LocalTrustError::Refreshing),
            Err(error) => Err(error.clone()),
        }
    }
}

/// Shared read-only monitor. All registry/certificate enumeration runs on its worker.
/// Dropping the final owner signals shutdown without blocking its caller.
#[derive(Clone)]
pub struct LocalTrustMonitor {
    state: Arc<State>,
    #[cfg(windows)]
    control: Option<Arc<windows::Control>>,
}
/// Owner-controlled publication for an embedding's explicit local inventory.
/// Automatic Windows monitors are read-only and do not expose this capability.
#[derive(Clone)]
pub struct LocalTrustPublisher {
    monitor: LocalTrustMonitor,
    serial: Arc<Mutex<()>>,
}
impl LocalTrustPublisher {
    /// Replaces explicit embedding trust and invalidates prior pool admissions.
    pub fn replace(&self, mut certificates: LocalCertificates) {
        let _serial = self
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.monitor.state.changed();
        certificates.generation = self.monitor.generation();
        self.monitor.state.publish(Ok(certificates.canonical()));
    }
}
impl fmt::Debug for LocalTrustMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalTrustMonitor")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}
impl LocalTrustMonitor {
    /// Whether Windows collection notifications maintain this inventory.
    pub fn is_monitored(&self) -> bool {
        #[cfg(windows)]
        {
            self.control.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    /// Creates an explicit dynamic inventory and its separate publication owner.
    #[must_use]
    pub fn controlled(certificates: LocalCertificates) -> (Self, LocalTrustPublisher) {
        let monitor = Self::fixed(certificates);
        let publisher = LocalTrustPublisher {
            monitor: monitor.clone(),
            serial: Arc::new(Mutex::new(())),
        };
        (monitor, publisher)
    }
    /// Starts local Windows collections, or an empty ready inventory on other OSes.
    /// # Errors
    /// Returns event/worker creation failures; inventory errors are exposed by current.
    pub fn start() -> Result<Self, LocalTrustError> {
        let state = Arc::new(State::new());
        #[cfg(windows)]
        {
            let control = windows::start(state.clone())?;
            Ok(Self {
                state,
                control: Some(control),
            })
        }
        #[cfg(not(windows))]
        {
            state.publish(Ok(LocalCertificates {
                generation: 1,
                ..Default::default()
            }
            .canonical()));
            Ok(Self { state })
        }
    }
    /// Current notification generation, including an in-progress refresh.
    pub fn generation(&self) -> u64 {
        self.observe_notification();
        self.state.epoch.load(Ordering::Acquire)
    }
    fn observe_notification(&self) {
        #[cfg(windows)]
        if self
            .control
            .as_ref()
            .is_some_and(|control| control.changed.is_signaled())
        {
            self.state.changed();
        }
    }
    /// Cheap copied snapshot, with no registry IO. Dirty/failed state returns an error.
    /// # Errors
    /// Returns refresh, read or shutdown state rather than an outdated inventory.
    pub fn current(&self) -> Result<Arc<LocalCertificates>, LocalTrustError> {
        self.observe_notification();
        self.state.current()
    }
    /// Creates a fixed owned inventory for explicit embedding trust or fixtures.
    #[must_use]
    pub fn fixed(mut certificates: LocalCertificates) -> Self {
        certificates.generation = certificates.generation.max(1);
        let state = Arc::new(State::new());
        state
            .epoch
            .store(certificates.generation, Ordering::Release);
        state.publish(Ok(certificates.canonical()));
        Self {
            state,
            #[cfg(windows)]
            control: None,
        }
    }
    /// Waits for an up-to-date inventory. Use off UI/async transport threads.
    /// # Errors
    /// Returns a read failure or Refreshing when the bounded wait expires.
    pub fn wait_ready(&self, timeout: Duration) -> Result<Arc<LocalCertificates>, LocalTrustError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.current() {
                Err(LocalTrustError::Refreshing) => {}
                result => return result,
            }
            let result = self
                .state
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(error) = &*result
                && *error != LocalTrustError::Refreshing
            {
                return Err(error.clone());
            }
            if !self.state.dirty.load(Ordering::Acquire) {
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(LocalTrustError::Refreshing);
            }
            drop(
                self.state
                    .ready
                    .wait_timeout(result, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_or_failed_inventory_is_not_returned_but_old_snapshots_remain_immutable() {
        let state = State::new();
        state.publish(Ok(LocalCertificates {
            generation: 1,
            anchors: vec![vec![1]],
            ..Default::default()
        }
        .canonical()));
        let old = state.current().unwrap();
        state.changed();
        assert_eq!(state.current(), Err(LocalTrustError::Refreshing));
        state.publish(Err(LocalTrustError::Limit));
        assert_eq!(state.current(), Err(LocalTrustError::Limit));
        state.publish(Ok(LocalCertificates {
            generation: 2,
            ..Default::default()
        }
        .canonical()));
        assert!(state.current().unwrap().anchors.is_empty());
        assert_eq!(old.anchors, [vec![1]]);
    }
    #[test]
    fn public_digest_is_order_independent_but_distinguishes_certificate_roles() {
        let first = LocalCertificates {
            anchors: vec![vec![2], vec![1], vec![1]],
            ..Default::default()
        }
        .canonical();
        let second = LocalCertificates {
            anchors: vec![vec![1], vec![2]],
            ..Default::default()
        }
        .canonical();
        let third = LocalCertificates {
            disallowed: vec![vec![1], vec![2]],
            ..Default::default()
        }
        .canonical();
        assert_eq!(first.sha256, second.sha256);
        assert_ne!(first.sha256, third.sha256);
    }
}
