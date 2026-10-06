#![deny(missing_docs)]

//! Shared, bounded network connection policy.
//!
//! The protocol adapters retain ownership of their sockets and handshakes.
//! This crate supplies the common Happy Eyeballs configuration, bounded DNS
//! candidate ordering, and staggered first-success race used by transports
//! that do not already provide their own race implementation.

use std::{
    collections::HashSet,
    error::Error,
    fmt::{self, Display},
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    time::Duration,
};

use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use thiserror::Error;
use tokio::time::{Instant, sleep_until};

const MAX_ATTEMPT_DELAY: Duration = Duration::from_secs(2);
const MAX_CANDIDATES: usize = 64;

/// Validated configuration for an IPv6/IPv4 connection race.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HappyEyeballsConfig {
    attempt_delay: Duration,
    max_candidates: NonZeroUsize,
}

impl HappyEyeballsConfig {
    /// Creates a finite connection-race policy.
    ///
    /// `attempt_delay` controls when the next interleaved address candidate is
    /// started while an earlier handshake remains pending. `max_candidates`
    /// bounds DNS result retention and concurrent handshake state.
    ///
    /// # Errors
    ///
    /// Returns [`HappyEyeballsConfigError`] for a zero or excessive value.
    pub fn new(
        attempt_delay: Duration,
        max_candidates: usize,
    ) -> Result<Self, HappyEyeballsConfigError> {
        if attempt_delay.is_zero() || attempt_delay > MAX_ATTEMPT_DELAY {
            return Err(HappyEyeballsConfigError::AttemptDelay);
        }
        let max_candidates = NonZeroUsize::new(max_candidates)
            .filter(|value| value.get() <= MAX_CANDIDATES)
            .ok_or(HappyEyeballsConfigError::MaxCandidates)?;
        Ok(Self {
            attempt_delay,
            max_candidates,
        })
    }

    /// Delay between staggered connection attempts.
    pub const fn attempt_delay(self) -> Duration {
        self.attempt_delay
    }

    /// Maximum number of unique DNS candidates retained for one connection.
    pub const fn max_candidates(self) -> NonZeroUsize {
        self.max_candidates
    }
}

impl Default for HappyEyeballsConfig {
    fn default() -> Self {
        Self {
            attempt_delay: Duration::from_millis(250),
            max_candidates: NonZeroUsize::new(16).expect("sixteen is nonzero"),
        }
    }
}

/// Invalid Happy Eyeballs policy.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HappyEyeballsConfigError {
    /// The attempt delay was zero or greater than two seconds.
    #[error("Happy Eyeballs attempt delay must be between 1ms and 2s")]
    AttemptDelay,
    /// The candidate bound was zero or greater than 64.
    #[error("Happy Eyeballs candidate limit must be between 1 and 64")]
    MaxCandidates,
}

/// Resolves and prepares a bounded, family-interleaved candidate list.
///
/// The operating system's first answer determines the preferred address
/// family. Relative order within each family is retained. Duplicate addresses
/// are removed before the configured bound is applied.
///
/// # Errors
///
/// Returns the operating-system resolver failure.
pub async fn resolve_candidates(
    host: &str,
    port: u16,
    config: HappyEyeballsConfig,
) -> Result<Vec<SocketAddr>, std::io::Error> {
    let resolved = tokio::net::lookup_host((host, port)).await?;
    Ok(interleave_candidates(resolved, config.max_candidates()))
}

/// Interleaves unique IPv6 and IPv4 candidates under a finite bound.
///
/// The family of the first unique address is preferred. This function is
/// public so application-owned resolvers can apply the exact same ordering
/// before calling [`race_candidates`].
pub fn interleave_candidates(
    addresses: impl IntoIterator<Item = SocketAddr>,
    limit: NonZeroUsize,
) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    let unique = addresses
        .into_iter()
        .filter(|address| seen.insert(*address))
        .collect::<Vec<_>>();
    let Some(first) = unique.first() else {
        return Vec::new();
    };
    let prefer_ipv6 = first.is_ipv6();
    let (preferred, fallback): (Vec<_>, Vec<_>) = unique
        .into_iter()
        .partition(|address| address.is_ipv6() == prefer_ipv6);
    let mut preferred = preferred.into_iter();
    let mut fallback = fallback.into_iter();
    let mut output = Vec::with_capacity(limit.get());
    while output.len() < limit.get() {
        let mut added = false;
        if let Some(address) = preferred.next() {
            output.push(address);
            added = true;
        }
        if output.len() == limit.get() {
            break;
        }
        if let Some(address) = fallback.next() {
            output.push(address);
            added = true;
        }
        if !added {
            break;
        }
    }
    output
}

/// Races already ordered connection candidates with bounded staggering.
///
/// The first candidate starts immediately. Each later candidate starts after
/// `config.attempt_delay`, or immediately when every in-flight attempt has
/// failed. Dropping this future cancels all in-flight attempt futures. The
/// connector must therefore be cancellation-safe and own its partial socket
/// and handshake state.
///
/// # Errors
///
/// Returns [`HappyEyeballsError::NoCandidates`] for an empty list, or the last
/// connector failure after every candidate fails.
pub async fn race_candidates<T, E, Connector, Attempt>(
    mut candidates: Vec<SocketAddr>,
    config: HappyEyeballsConfig,
    connector: Connector,
) -> Result<(T, SocketAddr), HappyEyeballsError<E>>
where
    T: Send,
    E: Send,
    Connector: Fn(SocketAddr) -> Attempt + Sync,
    Attempt: Future<Output = Result<T, E>> + Send,
{
    candidates.truncate(config.max_candidates().get());
    if candidates.is_empty() {
        return Err(HappyEyeballsError::NoCandidates);
    }

    let mut next = 0_usize;
    let mut attempts: FuturesUnordered<BoxFuture<'_, (SocketAddr, Result<T, E>)>> =
        FuturesUnordered::new();
    push_attempt(&mut attempts, candidates[next], &connector);
    next += 1;
    let mut next_launch = Instant::now() + config.attempt_delay();
    let mut last_error = None;

    loop {
        if next == candidates.len() {
            let Some((address, result)) = attempts.next().await else {
                return Err(last_error.map_or(
                    HappyEyeballsError::NoCandidates,
                    HappyEyeballsError::AttemptsFailed,
                ));
            };
            match result {
                Ok(value) => return Ok((value, address)),
                Err(error) => last_error = Some(error),
            }
            continue;
        }

        if attempts.is_empty() {
            push_attempt(&mut attempts, candidates[next], &connector);
            next += 1;
            next_launch = Instant::now() + config.attempt_delay();
            continue;
        }

        tokio::select! {
            completed = attempts.next() => {
                if let Some((address, result)) = completed {
                    match result {
                        Ok(value) => return Ok((value, address)),
                        Err(error) => last_error = Some(error),
                    }
                }
            }
            () = sleep_until(next_launch) => {
                push_attempt(&mut attempts, candidates[next], &connector);
                next += 1;
                next_launch = Instant::now() + config.attempt_delay();
            }
        }
    }
}

fn push_attempt<'a, T, E, Connector, Attempt>(
    attempts: &mut FuturesUnordered<BoxFuture<'a, (SocketAddr, Result<T, E>)>>,
    address: SocketAddr,
    connector: &'a Connector,
) where
    T: Send + 'a,
    E: Send + 'a,
    Connector: Fn(SocketAddr) -> Attempt + Sync + 'a,
    Attempt: Future<Output = Result<T, E>> + Send + 'a,
{
    attempts.push(async move { (address, connector(address).await) }.boxed());
}

/// Terminal outcome of a Happy Eyeballs candidate race.
#[derive(Debug)]
pub enum HappyEyeballsError<E> {
    /// DNS resolution supplied no usable candidate.
    NoCandidates,
    /// Every candidate failed; the final attempt error is retained.
    AttemptsFailed(E),
}

impl<E: Display> Display for HappyEyeballsError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCandidates => formatter.write_str("no connection candidates were available"),
            Self::AttemptsFailed(error) => {
                write!(formatter, "every connection candidate failed: {error}")
            }
        }
    }
}

impl<E: Error + 'static> Error for HappyEyeballsError<E> {}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, sync::Arc};

    use tokio::{sync::Mutex, time::sleep};

    use super::*;

    fn address(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse::<IpAddr>().unwrap(), port)
    }

    #[test]
    fn configuration_is_finite_and_validated() {
        assert_eq!(
            HappyEyeballsConfig::new(Duration::ZERO, 1),
            Err(HappyEyeballsConfigError::AttemptDelay)
        );
        assert_eq!(
            HappyEyeballsConfig::new(Duration::from_millis(1), 0),
            Err(HappyEyeballsConfigError::MaxCandidates)
        );
        HappyEyeballsConfig::new(Duration::from_secs(2), 64).unwrap();
    }

    #[test]
    fn candidates_are_deduplicated_interleaved_and_bounded() {
        let v6_a = address("2001:db8::1", 443);
        let v6_b = address("2001:db8::2", 443);
        let v4_a = address("192.0.2.1", 443);
        let v4_b = address("192.0.2.2", 443);
        assert_eq!(
            interleave_candidates(
                [v6_a, v6_b, v4_a, v6_a, v4_b],
                NonZeroUsize::new(3).unwrap()
            ),
            vec![v6_a, v4_a, v6_b]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn alternate_family_can_win_while_preferred_handshake_is_pending() {
        let v6 = address("2001:db8::1", 443);
        let v4 = address("192.0.2.1", 443);
        let started = Arc::new(Mutex::new(Vec::new()));
        let config = HappyEyeballsConfig::new(Duration::from_millis(250), 4).unwrap();
        let result = race_candidates(vec![v6, v4], config, {
            let started = Arc::clone(&started);
            move |candidate| {
                let started = Arc::clone(&started);
                async move {
                    started.lock().await.push(candidate);
                    if candidate.is_ipv6() {
                        sleep(Duration::from_secs(30)).await;
                        Err("preferred timed out")
                    } else {
                        Ok("fallback connected")
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result, ("fallback connected", v4));
        assert_eq!(*started.lock().await, vec![v6, v4]);
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_failure_advances_without_waiting_for_the_delay() {
        let first = address("192.0.2.1", 443);
        let second = address("192.0.2.2", 443);
        let config = HappyEyeballsConfig::new(Duration::from_secs(1), 4).unwrap();
        let started = Instant::now();
        let result = race_candidates(vec![first, second], config, |candidate| async move {
            if candidate == first {
                Err("first failed")
            } else {
                Ok(candidate)
            }
        })
        .await
        .unwrap();
        assert_eq!(result, (second, second));
        assert_eq!(Instant::now(), started);
    }

    #[tokio::test(start_paused = true)]
    async fn race_enforces_candidate_bound_for_caller_owned_resolution() {
        let first = address("192.0.2.1", 443);
        let second = address("192.0.2.2", 443);
        let excluded = address("192.0.2.3", 443);
        let started = Arc::new(Mutex::new(Vec::new()));
        let config = HappyEyeballsConfig::new(Duration::from_millis(1), 2).unwrap();
        let result = race_candidates(vec![first, second, excluded], config, {
            let started = Arc::clone(&started);
            move |candidate| {
                let started = Arc::clone(&started);
                async move {
                    started.lock().await.push(candidate);
                    Err::<(), _>(candidate)
                }
            }
        })
        .await;

        assert!(matches!(
            result,
            Err(HappyEyeballsError::AttemptsFailed(error)) if error == second
        ));
        assert_eq!(*started.lock().await, vec![first, second]);
    }
}
