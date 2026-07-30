//! Extension-instance lifecycle and non-cloneable accepted-request leases.

use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use aster_forge_cloud_files_core::{SessionGeneration, SessionState};

use crate::{MacosBridgeError, Result};

#[derive(Debug)]
struct SessionLifecycle {
    state: SessionState,
    active_requests: usize,
}

#[derive(Debug)]
struct SessionInner {
    generation: SessionGeneration,
    lifecycle: Mutex<SessionLifecycle>,
}

/// Shareable lifecycle fence for one File Provider extension instance.
#[derive(Debug, Clone)]
pub struct MacosExtensionSession {
    inner: Arc<SessionInner>,
}

impl MacosExtensionSession {
    /// Starts one accepting extension generation.
    pub fn new(generation: SessionGeneration) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                generation,
                lifecycle: Mutex::new(SessionLifecycle {
                    state: SessionState::Accepting,
                    active_requests: 0,
                }),
            }),
        }
    }

    /// Returns the monotonic extension-instance generation.
    pub fn generation(&self) -> SessionGeneration {
        self.inner.generation
    }

    /// Returns the current lifecycle state.
    pub fn state(&self) -> SessionState {
        lock(&self.inner.lifecycle).state
    }

    /// Returns the number of accepted requests still owned by downstream work.
    pub fn active_requests(&self) -> usize {
        lock(&self.inner.lifecycle).active_requests
    }

    pub(crate) fn into_ffi_handle(self) -> *const () {
        Arc::into_raw(self.inner).cast()
    }

    pub(crate) unsafe fn clone_from_ffi_handle(handle: *const ()) -> Result<Self> {
        if handle.is_null() {
            return Err(MacosBridgeError::InvalidFfiInput {
                reason: "extension session handle must not be null",
            });
        }
        let pointer = handle.cast::<SessionInner>();
        // SAFETY: the C ABI contract requires `handle` to be one live pointer returned by
        // `into_ffi_handle`. Incrementing first creates an independent temporary Arc owner.
        unsafe { Arc::increment_strong_count(pointer) };
        // SAFETY: the increment above established one owned strong reference for this Arc.
        let inner = unsafe { Arc::from_raw(pointer) };
        Ok(Self { inner })
    }

    pub(crate) unsafe fn release_ffi_handle(handle: *const ()) -> Result<()> {
        if handle.is_null() {
            return Ok(());
        }
        let pointer = handle.cast::<SessionInner>();
        // SAFETY: the C ABI contract requires one live unreleased session handle returned by
        // `into_ffi_handle`. Reconstructing and dropping consumes that exact owner once.
        let session = unsafe { Arc::from_raw(pointer) };
        drop(session);
        Ok(())
    }

    /// Accepts one request only while the exact generation is accepting work.
    pub fn begin_request(
        &self,
        callback_generation: SessionGeneration,
    ) -> Result<MacosExtensionRequestLease> {
        if callback_generation != self.inner.generation {
            return Err(MacosBridgeError::StaleSessionGeneration {
                expected: self.inner.generation.get(),
                actual: callback_generation.get(),
            });
        }
        let mut lifecycle = lock(&self.inner.lifecycle);
        if lifecycle.state != SessionState::Accepting {
            return Err(MacosBridgeError::SessionNotAccepting {
                state: lifecycle.state,
            });
        }
        lifecycle.active_requests = lifecycle
            .active_requests
            .checked_add(1)
            .ok_or(MacosBridgeError::ActiveRequestCountOverflow)?;
        drop(lifecycle);
        Ok(MacosExtensionRequestLease {
            session: self.inner.clone(),
            released: false,
        })
    }

    /// Moves `Accepting -> Closing` and rejects later request ingress.
    pub fn begin_closing(&self) -> bool {
        let mut lifecycle = lock(&self.inner.lifecycle);
        if lifecycle.state == SessionState::Accepting {
            lifecycle.state = SessionState::Closing;
            true
        } else {
            false
        }
    }

    /// Marks the native extension/domain bridge disconnected and begins draining accepted work.
    pub fn mark_disconnected(&self) -> Result<()> {
        let mut lifecycle = lock(&self.inner.lifecycle);
        match lifecycle.state {
            SessionState::Closing => {
                lifecycle.state = if lifecycle.active_requests == 0 {
                    SessionState::Closed
                } else {
                    SessionState::Draining
                };
                Ok(())
            }
            SessionState::Draining | SessionState::Closed => Ok(()),
            SessionState::Accepting => Err(MacosBridgeError::InvalidSessionTransition {
                from: SessionState::Accepting,
                to: SessionState::Draining,
            }),
        }
    }
}

/// Non-cloneable ownership proof for one request accepted before closing.
pub struct MacosExtensionRequestLease {
    session: Arc<SessionInner>,
    released: bool,
}

impl MacosExtensionRequestLease {
    /// Returns the generation that accepted the request.
    pub fn generation(&self) -> SessionGeneration {
        self.session.generation
    }

    /// Returns whether completion still belongs to the supplied extension instance.
    pub fn accepts_completion(&self, session: &MacosExtensionSession) -> bool {
        !self.released
            && Arc::ptr_eq(&self.session, &session.inner)
            && lock(&self.session.lifecycle).state != SessionState::Closed
    }

    /// Releases the accepted request count explicitly.
    pub fn release(mut self) {
        self.release_inner();
    }

    pub(crate) fn into_ffi_handle(self) -> *mut () {
        Box::into_raw(Box::new(self)).cast()
    }

    pub(crate) unsafe fn release_ffi_handle(handle: *mut ()) -> Result<()> {
        if handle.is_null() {
            return Ok(());
        }
        // SAFETY: the C ABI contract requires one live unreleased request handle returned by
        // `into_ffi_handle`. Reconstructing the Box consumes that exact allocation once.
        let lease = unsafe { Box::from_raw(handle.cast::<Self>()) };
        drop(lease);
        Ok(())
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        let mut lifecycle = lock(&self.session.lifecycle);
        lifecycle.active_requests = lifecycle.active_requests.saturating_sub(1);
        if lifecycle.state == SessionState::Draining && lifecycle.active_requests == 0 {
            lifecycle.state = SessionState::Closed;
        }
        self.released = true;
    }
}

impl fmt::Debug for MacosExtensionRequestLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosExtensionRequestLease")
            .field("generation", &self.session.generation)
            .field("released", &self.released)
            .finish()
    }
}

impl Drop for MacosExtensionRequestLease {
    fn drop(&mut self) {
        self.release_inner();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
