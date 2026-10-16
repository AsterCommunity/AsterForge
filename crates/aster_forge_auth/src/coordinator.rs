//! Bounded, instance-owned refresh serialization with cancellation-safe resource reclamation.

use crate::{AuthError, RefreshJti, Result};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex as MapMutex, Weak},
};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

struct Inner {
    locks: MapMutex<HashMap<RefreshJti, Weak<Mutex<()>>>>,
    budget: Arc<Semaphore>,
    maximum: usize,
    closed: CancellationToken,
}

/// Instance resource shared by all refresh requests. No global singleton or historical-JTI leak.
/// The finite budget includes running requests and same-JTI waiters; saturation rejects promptly.
/// It complements, never replaces, the product's authoritative cross-instance database CAS.
#[derive(Clone)]
pub struct RefreshCoordinator(Arc<Inner>);

impl fmt::Debug for RefreshCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshCoordinator")
            .field("maximum", &self.0.maximum)
            .field("active_requests", &self.active_requests())
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl Default for RefreshCoordinator {
    fn default() -> Self {
        Self::from_limit(1024)
    }
}

impl RefreshCoordinator {
    /// Creates a bounded instance resource.
    ///
    /// # Errors
    /// Returns `InvalidPolicy` for zero or a limit beyond Tokio's semaphore capacity.
    pub fn new(maximum: usize) -> Result<Self> {
        if maximum == 0 || maximum > Semaphore::MAX_PERMITS {
            return Err(AuthError::InvalidPolicy);
        }
        Ok(Self::from_limit(maximum))
    }
    fn from_limit(maximum: usize) -> Self {
        Self(Arc::new(Inner {
            locks: MapMutex::new(HashMap::new()),
            budget: Arc::new(Semaphore::new(maximum)),
            maximum,
            closed: CancellationToken::new(),
        }))
    }
    /// Rejects new requests and wakes queued waiters. Already-started store transactions complete.
    pub fn close(&self) {
        self.0.closed.cancel();
    }
    /// Reports whether shutdown has started.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.closed.is_cancelled()
    }
    /// Number of admitted running/queued requests, for resource diagnostics and deterministic tests.
    #[must_use]
    pub fn active_requests(&self) -> usize {
        self.0.maximum - self.0.budget.available_permits()
    }
    /// Number of live JTI lock entries. No raw identity is exposed to observation.
    #[must_use]
    pub fn tracked_ids(&self) -> usize {
        self.0
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub(crate) async fn acquire(
        &self,
        jti: RefreshJti,
        cancel: &CancellationToken,
    ) -> Result<Reservation> {
        if self.is_closed() || cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        let permit = self
            .0
            .budget
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::ResourceLimit)?;
        let lock = {
            let mut map = self
                .0
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lock) = map.get(&jti).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                map.insert(jti.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        // Reserve before awaiting so cancellation/drop during lock acquisition also reclaims entries.
        let mut reservation = Reservation {
            jti,
            owner: self.clone(),
            lock: Some(lock.clone()),
            guard: None,
            _permit: permit,
        };
        let guard = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(AuthError::Cancelled),
            () = self.0.closed.cancelled() => return Err(AuthError::Cancelled),
            guard = lock.lock_owned() => guard,
        };
        reservation.guard = Some(guard);
        Ok(reservation)
    }
}

pub(crate) struct Reservation {
    jti: RefreshJti,
    owner: RefreshCoordinator,
    lock: Option<Arc<Mutex<()>>>,
    guard: Option<OwnedMutexGuard<()>>,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        drop(self.guard.take());
        drop(self.lock.take());
        // Registration and removal share one mutex; removal cannot erase a newly-created live lock.
        let mut map = self
            .owner
            .0
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map
            .get(&self.jti)
            .is_some_and(|weak| weak.strong_count() == 0)
        {
            map.remove(&self.jti);
        }
    }
}
