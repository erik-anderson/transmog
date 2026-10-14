//! Chromium's bounded, newest-first client ticket cache.
use boring::ssl::SslSessionRef;
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// Native ticket validity, including `BoringSSL`'s single-use classification.
#[derive(Clone, Copy, Debug)]
pub struct SessionLifetime {
    created: u64,
    timeout: u32,
    single_use: bool,
}
impl SessionLifetime {
    /// Reads the validity fixed when `BoringSSL` created the ticket.
    pub fn from_session(session: &SslSessionRef) -> Self {
        Self {
            created: session.time(),
            timeout: session.timeout(),
            single_use: transmog_tls_native::session_should_be_single_use(session),
        }
    }
    fn expired(self, now: u64) -> bool {
        now < self.created.saturating_sub(1)
            || now >= self.created.saturating_add(u64::from(self.timeout))
    }
}

#[derive(Debug)]
struct Ticket<T> {
    value: T,
    lifetime: SessionLifetime,
}
#[derive(Debug)]
struct State<K, T> {
    entries: VecDeque<(K, VecDeque<Ticket<T>>)>,
    generation: u64,
    lookups: usize,
}
/// At most 1024 origins and two tickets per origin, scoped by the transport's key.
/// A flush also rejects tickets arriving late from an old physical connection.
#[derive(Debug)]
pub struct SessionCache<K, T> {
    state: Mutex<State<K, T>>,
}
impl<K, T> Default for SessionCache<K, T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                entries: VecDeque::new(),
                generation: 0,
                lookups: 0,
            }),
        }
    }
}
impl<K: Eq, T: Clone> SessionCache<K, T> {
    /// Generation to freeze before starting a new handshake.
    pub fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }
    /// Drops tickets and invalidates in-flight insertions.
    pub fn flush(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.entries.clear();
    }
    /// Returns a valid ticket, consuming it if `BoringSSL` requires single use.
    pub fn take(&self, key: &K) -> Option<T> {
        self.take_at(key, now())
    }
    fn take_at(&self, key: &K, now: u64) -> Option<T> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.lookups += 1;
        if state.lookups == 256 {
            state.lookups = 0;
            state.entries.retain_mut(|(_, tickets)| {
                if tickets
                    .front()
                    .is_none_or(|ticket| ticket.lifetime.expired(now))
                {
                    return false;
                }
                tickets.retain(|ticket| !ticket.lifetime.expired(now));
                !tickets.is_empty()
            });
        }
        let index = state.entries.iter().position(|(entry, _)| entry == key)?;
        let (key, mut tickets) = state.entries.remove(index)?;
        let ticket = tickets.front()?;
        let value = (!ticket.lifetime.expired(now)).then(|| ticket.value.clone());
        if ticket.lifetime.single_use {
            tickets.pop_front();
        }
        if tickets
            .front()
            .is_some_and(|ticket| ticket.lifetime.expired(now))
        {
            tickets.clear();
        }
        tickets.retain(|ticket| !ticket.lifetime.expired(now));
        if !tickets.is_empty() {
            state.entries.push_front((key, tickets));
        }
        value
    }
    /// Retains a new ticket only in the generation which started its handshake.
    pub fn insert(&self, generation: u64, key: K, value: T, lifetime: SessionLifetime) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != generation {
            return;
        }
        let mut tickets = state
            .entries
            .iter()
            .position(|(entry, _)| entry == &key)
            .and_then(|index| state.entries.remove(index))
            .map_or_else(VecDeque::new, |(_, tickets)| tickets);
        if tickets
            .front()
            .is_some_and(|ticket| !ticket.lifetime.single_use)
        {
            tickets.pop_front();
        }
        tickets.push_front(Ticket { value, lifetime });
        tickets.truncate(2);
        state.entries.push_front((key, tickets));
        state.entries.truncate(1024);
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lifetime(single_use: bool) -> SessionLifetime {
        SessionLifetime {
            created: 10,
            timeout: 10,
            single_use,
        }
    }
    #[test]
    fn newest_two_single_use_and_reusable_tickets_obey_expiry() {
        let cache = SessionCache::default();
        for value in 1..=3 {
            cache.insert(0, "origin", value, lifetime(true));
        }
        assert_eq!(cache.take_at(&"origin", 9), Some(3));
        assert_eq!(cache.take_at(&"origin", 10), Some(2));
        assert_eq!(cache.take_at(&"origin", 10), None);
        cache.insert(0, "origin", 4, lifetime(false));
        assert_eq!(cache.take_at(&"origin", 19), Some(4));
        assert_eq!(cache.take_at(&"origin", 19), Some(4));
        assert_eq!(cache.take_at(&"origin", 20), None);
        cache.insert(0, "origin", 5, lifetime(true));
        assert_eq!(cache.take_at(&"origin", 8), None);
    }
    #[test]
    fn origins_are_bounded_and_partitioned_and_flush_rejects_late_tickets() {
        let cache = SessionCache::default();
        for origin in 0..1025 {
            cache.insert(0, (origin, 1), origin, lifetime(false));
        }
        assert_eq!(cache.take_at(&(0, 1), 10), None);
        assert_eq!(cache.take_at(&(1024, 2), 10), None);
        assert_eq!(cache.take_at(&(1024, 1), 10), Some(1024));
        cache.flush();
        cache.insert(0, (1, 1), 1, lifetime(false));
        assert_eq!(cache.take_at(&(1, 1), 10), None);
        cache.insert(cache.generation(), (1, 1), 2, lifetime(false));
        assert_eq!(cache.take_at(&(1, 1), 10), Some(2));
    }
}
