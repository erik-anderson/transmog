use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use thiserror::Error;

/// Origin key whose alternatives may not be shared with another authority.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Origin {
    /// Lowercase DNS host or normalized IP literal.
    pub host: String,
    /// Origin port.
    pub port: u16,
}

/// Validated HTTP/3 alternative.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AltSvcEntry {
    /// UDP port advertised for the same origin host.
    pub port: u16,
    /// Absolute local expiry.
    pub expires_at: Instant,
    /// Do not retry the alternative before this time.
    pub broken_until: Option<Instant>,
}

/// Size-bounded, proxy-private Alt-Svc cache.
#[derive(Debug)]
pub struct AltSvcCache {
    capacity: usize,
    entries: HashMap<Origin, AltSvcEntry>,
}

impl AltSvcCache {
    /// Creates a cache with a hard entry count.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
        }
    }

    /// Applies one Alt-Svc field for the response origin.
    ///
    /// V1 accepts only `h3=\":port\"; ma=seconds`; alternatives for another host
    /// are rejected to avoid redirecting authenticated origin traffic.
    ///
    /// # Errors
    ///
    /// Returns [`AltSvcError`] when the advertisement is malformed, unsafe, or
    /// cannot fit within the configured cache bound.
    pub fn observe(
        &mut self,
        origin: Origin,
        value: &str,
        now: Instant,
    ) -> Result<(), AltSvcError> {
        if value.trim().eq_ignore_ascii_case("clear") {
            self.entries.remove(&origin);
            return Ok(());
        }
        let mut alternatives = value.split(',').map(str::trim);
        let first = alternatives.next().ok_or(AltSvcError::Malformed)?;
        if alternatives.next().is_some() {
            return Err(AltSvcError::MultipleAlternativesUnsupported);
        }
        let mut parts = first.split(';').map(str::trim);
        let protocol_and_authority = parts.next().ok_or(AltSvcError::Malformed)?;
        let (protocol, quoted_authority) = protocol_and_authority
            .split_once('=')
            .ok_or(AltSvcError::Malformed)?;
        if !protocol.trim().eq_ignore_ascii_case("h3") {
            return Err(AltSvcError::UnsupportedProtocol);
        }
        let authority = quoted_authority
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.strip_suffix('"'))
            .ok_or(AltSvcError::Malformed)?;
        let port_text = authority.strip_prefix(':').ok_or(AltSvcError::CrossHost)?;
        let port = port_text
            .parse::<u16>()
            .map_err(|_| AltSvcError::InvalidPort)?;
        if port == 0 {
            return Err(AltSvcError::InvalidPort);
        }

        let mut max_age = Duration::from_secs(86_400);
        for parameter in parts {
            if let Some(value) = parameter.strip_prefix("ma=") {
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| AltSvcError::InvalidMaxAge)?;
                max_age = Duration::from_secs(seconds.min(30 * 86_400));
            }
        }
        if max_age.is_zero() {
            self.entries.remove(&origin);
            return Ok(());
        }
        if !self.entries.contains_key(&origin) && self.entries.len() >= self.capacity {
            return Err(AltSvcError::CapacityExceeded);
        }
        self.entries.insert(
            origin,
            AltSvcEntry {
                port,
                expires_at: now + max_age,
                broken_until: None,
            },
        );
        Ok(())
    }

    /// Returns a currently usable alternative, expiring stale state eagerly.
    pub fn get(&mut self, origin: &Origin, now: Instant) -> Option<&AltSvcEntry> {
        let remove = self
            .entries
            .get(origin)
            .is_some_and(|entry| entry.expires_at <= now);
        if remove {
            self.entries.remove(origin);
            return None;
        }
        self.entries
            .get(origin)
            .filter(|entry| entry.broken_until.is_none_or(|deadline| deadline <= now))
    }

    /// Marks an alternative broken with bounded exponential backoff supplied by policy.
    pub fn mark_broken(&mut self, origin: &Origin, until: Instant) {
        if let Some(entry) = self.entries.get_mut(origin) {
            entry.broken_until = Some(until.min(entry.expires_at));
        }
    }

    /// Clears all learned alternatives, for example during deterministic test setup.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Invalid or unsupported Alt-Svc advertisement.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AltSvcError {
    /// Syntax is invalid.
    #[error("malformed Alt-Svc advertisement")]
    Malformed,
    /// Initial implementation accepts exactly one alternative per field.
    #[error("multiple Alt-Svc alternatives are not supported")]
    MultipleAlternativesUnsupported,
    /// Only the current `h3` identifier is accepted.
    #[error("unsupported Alt-Svc protocol")]
    UnsupportedProtocol,
    /// Alternative attempted to name a different host.
    #[error("cross-host Alt-Svc alternatives are forbidden")]
    CrossHost,
    /// Port was invalid.
    #[error("invalid Alt-Svc port")]
    InvalidPort,
    /// Max age was invalid.
    #[error("invalid Alt-Svc max age")]
    InvalidMaxAge,
    /// Cache reached its configured hard limit.
    #[error("Alt-Svc cache capacity exceeded")]
    CapacityExceeded,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn origin() -> Origin {
        Origin {
            host: "example.test".to_owned(),
            port: 443,
        }
    }

    #[test]
    fn learns_expires_clears_and_backs_off() {
        let start = Instant::now();
        let mut cache = AltSvcCache::new(1);
        cache
            .observe(origin(), "h3=\":8443\"; ma=10", start)
            .unwrap();
        assert_eq!(cache.get(&origin(), start).unwrap().port, 8443);
        cache.mark_broken(&origin(), start + Duration::from_secs(5));
        assert!(
            cache
                .get(&origin(), start + Duration::from_secs(1))
                .is_none()
        );
        assert!(
            cache
                .get(&origin(), start + Duration::from_secs(6))
                .is_some()
        );
        assert!(
            cache
                .get(&origin(), start + Duration::from_secs(11))
                .is_none()
        );

        cache
            .observe(origin(), "h3=\":443\"; ma=10", start)
            .unwrap();
        cache.observe(origin(), "clear", start).unwrap();
        assert!(cache.get(&origin(), start).is_none());
    }

    #[test]
    fn rejects_cross_host_and_bounds_capacity() {
        let start = Instant::now();
        let mut cache = AltSvcCache::new(1);
        assert_eq!(
            cache.observe(origin(), "h3=\"evil.test:443\"", start),
            Err(AltSvcError::CrossHost)
        );
        cache.observe(origin(), "h3=\":443\"", start).unwrap();
        assert_eq!(
            cache.observe(
                Origin {
                    host: "other.test".to_owned(),
                    port: 443,
                },
                "h3=\":443\"",
                start
            ),
            Err(AltSvcError::CapacityExceeded)
        );
    }
}
