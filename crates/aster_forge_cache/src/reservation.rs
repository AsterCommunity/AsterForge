//! Lightweight local reservation set for insert-if-absent semantics.
//!
//! The memory and Redis fallback paths use this module to prevent concurrent callers from both
//! winning a local `set_if_absent` operation before the cached value is visible. Reservations are
//! bounded and TTL based so failed writers do not reserve a key forever.

use dashmap::{DashMap, mapref::entry::Entry};
use std::time::{Duration, Instant};

const RESERVATION_MAX_ENTRIES: usize = 64 * 1024;

#[derive(Debug)]
pub struct ReservationSet {
    default_ttl: u64,
    entries: DashMap<String, Instant>,
}

impl ReservationSet {
    pub fn new(default_ttl: u64) -> Self {
        Self {
            default_ttl,
            entries: DashMap::new(),
        }
    }

    pub fn reserve(&self, key: &str, ttl_secs: Option<u64>) -> bool {
        let now = Instant::now();
        if self.entries.len() >= RESERVATION_MAX_ENTRIES {
            self.prune_expired(now);
            if self.entries.len() >= RESERVATION_MAX_ENTRIES && !self.entries.contains_key(key) {
                return false;
            }
        }

        match self.entries.entry(key.to_string()) {
            Entry::Occupied(mut entry) => {
                if *entry.get() > now {
                    return false;
                }

                entry.insert(self.expires_at(now, ttl_secs));
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(self.expires_at(now, ttl_secs));
                true
            }
        }
    }

    /// Reserves `key` and returns a guard releasing the reservation on drop.
    ///
    /// Callers that lose an insert-if-absent race between reserving and
    /// publishing the value must not keep the reservation: it would falsely
    /// block every later caller until its TTL expired, including after the
    /// winning value itself is evicted (TTL eviction never touches this set).
    /// Returning early with the guard in scope releases it automatically;
    /// call [`ReservationGuard::commit`] once the value is published to keep
    /// the reservation for its co-lifetime with the value.
    pub fn reserve_guarded<'a>(
        &'a self,
        key: &'a str,
        ttl_secs: Option<u64>,
    ) -> Option<ReservationGuard<'a>> {
        if !self.reserve(key, ttl_secs) {
            return None;
        }
        Some(ReservationGuard {
            set: self,
            key,
            committed: false,
        })
    }

    pub fn remove(&self, key: &str) {
        self.entries.remove(key);
    }

    pub fn invalidate_prefix(&self, prefix: &str) {
        self.entries.retain(|key, _| !key.starts_with(prefix));
    }

    fn expires_at(&self, now: Instant, ttl_secs: Option<u64>) -> Instant {
        // A reservation lives exactly as long as the value it protects: once the value has
        // expired, a later caller must be able to re-reserve and re-insert the key. For a
        // zero-TTL value the reservation may expire immediately as well — the value is
        // unobservable either way, so which concurrent caller "won" it is meaningless.
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        now.checked_add(Duration::from_secs(ttl)).unwrap_or(now)
    }

    fn prune_expired(&self, now: Instant) {
        self.entries.retain(|_, expires_at| *expires_at > now);
    }
}

/// RAII handle for one held reservation.
///
/// Dropping the guard releases the reservation unless it has been committed.
/// This makes the lose-the-race path of insert-if-absent unable to leak the
/// reservation by construction instead of by discipline at every return site.
#[derive(Debug)]
pub struct ReservationGuard<'a> {
    set: &'a ReservationSet,
    key: &'a str,
    committed: bool,
}

impl ReservationGuard<'_> {
    /// Keeps the reservation after the guard drops. Call once the value the
    /// reservation protects has been published; the reservation then lives
    /// for its co-lifetime with that value.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ReservationGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.set.remove(self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReservationSet;
    use std::sync::Arc;

    #[tokio::test]
    async fn reserve_allows_one_concurrent_insert() {
        let reservations = Arc::new(ReservationSet::new(60));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let reservations = reservations.clone();
            tasks.push(tokio::spawn(async move {
                reservations.reserve("nonce", Some(60))
            }));
        }

        let successes = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|result| result.expect("reservation task should not panic"))
            .filter(|inserted| *inserted)
            .count();

        assert_eq!(successes, 1);
    }

    #[test]
    fn remove_allows_new_reservation() {
        let reservations = ReservationSet::new(60);
        assert!(reservations.reserve("nonce", Some(60)));
        assert!(!reservations.reserve("nonce", Some(60)));

        reservations.remove("nonce");
        assert!(reservations.reserve("nonce", Some(60)));
    }

    #[test]
    fn zero_ttl_reservation_expires_with_its_value() {
        let reservations = ReservationSet::new(60);
        assert!(reservations.reserve("nonce", Some(0)));
        // The value this reservation protected expired at insert, so the key must be
        // re-reservable immediately.
        assert!(reservations.reserve("nonce", Some(0)));
    }

    #[test]
    fn uncommitted_guard_releases_reservation_on_drop() {
        let reservations = ReservationSet::new(60);
        let guard = reservations
            .reserve_guarded("nonce", Some(60))
            .expect("first reservation should succeed");
        assert!(reservations.reserve_guarded("nonce", Some(60)).is_none());

        drop(guard);

        // Losing the race drops the guard uncommitted, so the key is reservable again.
        assert!(reservations.reserve_guarded("nonce", Some(60)).is_some());
    }

    #[test]
    fn committed_guard_keeps_reservation() {
        let reservations = ReservationSet::new(60);
        let guard = reservations
            .reserve_guarded("nonce", Some(60))
            .expect("first reservation should succeed");

        guard.commit();

        // The published value's co-lifetime reservation survives the guard.
        assert!(reservations.reserve_guarded("nonce", Some(60)).is_none());
    }
}
