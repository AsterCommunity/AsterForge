//! Lightweight local reservation set for insert-if-absent semantics.
//!
//! The memory and Redis fallback paths use this module to prevent concurrent callers from both
//! winning a local `set_if_absent` operation before the cached value is visible. Reservations are
//! bounded and TTL based so failed writers do not reserve a key forever.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const RESERVATION_MAX_ENTRIES: usize = 64 * 1024;

#[derive(Debug)]
pub struct ReservationSet {
    default_ttl: u64,
    state: Mutex<ReservationState>,
}

#[derive(Debug, Default)]
struct ReservationState {
    entries: HashMap<String, Instant>,
    expirations: BTreeSet<(Instant, String)>,
}

impl ReservationState {
    fn remove(&mut self, key: &str) {
        if let Some(expiry) = self.entries.remove(key) {
            self.expirations.remove(&(expiry, key.to_owned()));
        }
    }

    fn expire(&mut self, now: Instant) {
        while self
            .expirations
            .first()
            .is_some_and(|(expiry, _)| *expiry <= now)
        {
            if let Some((_, key)) = self.expirations.pop_first() {
                self.entries.remove(&key);
            }
        }
    }
}

impl ReservationSet {
    pub fn new(default_ttl: u64) -> Self {
        Self {
            default_ttl,
            state: Mutex::new(ReservationState::default()),
        }
    }

    pub fn reserve(&self, key: &str, ttl_secs: Option<u64>) -> bool {
        let now = Instant::now();
        let mut state = self.state();
        state.expire(now);
        if state.entries.contains_key(key) || state.entries.len() >= RESERVATION_MAX_ENTRIES {
            return false;
        }
        let expires = self.expires_at(now, ttl_secs);
        state.entries.insert(key.to_owned(), expires);
        state.expirations.insert((expires, key.to_owned()));
        true
    }

    fn state(&self) -> MutexGuard<'_, ReservationState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        self.state().remove(key);
    }

    pub fn invalidate_prefix(&self, prefix: &str) {
        let mut state = self.state();
        let keys: Vec<_> = state
            .entries
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        for key in keys {
            state.remove(&key);
        }
    }

    fn expires_at(&self, now: Instant, ttl_secs: Option<u64>) -> Instant {
        // A reservation lives exactly as long as the value it protects: once the value has
        // expired, a later caller must be able to re-reserve and re-insert the key. For a
        // zero-TTL value the reservation may expire immediately as well — the value is
        // unobservable either way, so which concurrent caller "won" it is meaningless.
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        now.checked_add(Duration::from_secs(ttl)).unwrap_or(now)
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
    #[test]
    fn saturation_rejection_and_prefix_removal_preserve_capacity() {
        let reservations = ReservationSet::new(60);
        for i in 0..super::RESERVATION_MAX_ENTRIES {
            assert!(reservations.reserve(&format!("live:{i}"), None));
        }
        for _ in 0..100 {
            assert!(!reservations.reserve("absent", None));
        }
        assert!(!reservations.reserve("live:0", None));
        reservations.remove("live:0");
        assert!(reservations.reserve("replacement", None));
        reservations.invalidate_prefix("live:");
        assert!(reservations.reserve("live:1", None));
        assert!(!reservations.reserve("replacement", None));
    }

    #[test]
    fn expired_reservations_and_removed_keys_do_not_leave_stale_deadlines() {
        let reservations = ReservationSet::new(60);
        for _ in 0..10_000 {
            assert!(reservations.reserve("zero", Some(0)));
            reservations.remove("zero");
        }
        assert!(reservations.reserve("zero", None));
        assert!(!reservations.reserve("zero", None));
        reservations.invalidate_prefix("zero");
        assert!(reservations.reserve("zero", None));
    }
    #[test]
    #[ignore = "controlled before/after CPU measurement"]
    fn measure_saturated_reservation_workload() {
        let reservations = ReservationSet::new(60);
        for i in 0..super::RESERVATION_MAX_ENTRIES {
            assert!(reservations.reserve(&format!("key{i}"), None));
        }
        let start = std::time::Instant::now();
        for _ in 0..256 {
            assert!(!reservations.reserve("absent", None));
        }
        println!("cache_saturated_us={}", start.elapsed().as_micros());
    }
}
