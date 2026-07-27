//! Bounded non-blocking bridge from FUSE callback threads to an explicit Tokio runtime.

use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use futures::FutureExt;
use tokio::{runtime::Handle, sync::OwnedSemaphorePermit};

use crate::{LinuxCloudFilesError, LinuxErrorCode, Result};

#[derive(Default)]
struct MetricsInner {
    accepted: AtomicU64,
    saturated: AtomicU64,
    closing: AtomicU64,
    completed: AtomicU64,
    panicked: AtomicU64,
}

/// Snapshot of bounded callback-to-async dispatch outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinuxDispatchMetrics {
    /// Requests accepted into the explicit runtime.
    pub accepted: u64,
    /// Requests rejected because every in-flight permit was occupied.
    pub saturated: u64,
    /// Requests rejected after dispatcher closing began.
    pub closing: u64,
    /// Accepted tasks that reached a normal or panic terminal state.
    pub completed: u64,
    /// Accepted task futures that panicked. Dropping their owned FUSE reply produces `EIO`.
    pub panicked: u64,
}

/// Reason a callback could not reserve bounded asynchronous execution capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDispatchRejection {
    /// Every configured in-flight permit was occupied.
    Saturated,
    /// The mount session is closing and accepts no new work.
    Closing,
}

impl LinuxDispatchRejection {
    /// Returns the FUSE-visible portable error classification.
    pub const fn error_code(self) -> LinuxErrorCode {
        match self {
            Self::Saturated => LinuxErrorCode::TryAgain,
            Self::Closing => LinuxErrorCode::Io,
        }
    }
}

struct DispatcherInner {
    runtime: Handle,
    permits: Arc<tokio::sync::Semaphore>,
    closing: AtomicBool,
    metrics: MetricsInner,
}

/// Explicit bounded dispatcher shared by one mount session.
#[derive(Clone)]
pub struct LinuxRequestDispatcher {
    inner: Arc<DispatcherInner>,
}

impl LinuxRequestDispatcher {
    /// Creates a dispatcher backed by a caller-owned runtime.
    pub fn new(runtime: Handle, max_in_flight: usize) -> Result<Self> {
        if max_in_flight == 0 {
            return Err(LinuxCloudFilesError::InvalidConfiguration {
                reason: "max in-flight FUSE requests must be greater than zero",
            });
        }
        Ok(Self {
            inner: Arc::new(DispatcherInner {
                runtime,
                permits: Arc::new(tokio::sync::Semaphore::new(max_in_flight)),
                closing: AtomicBool::new(false),
                metrics: MetricsInner::default(),
            }),
        })
    }

    /// Reserves one in-flight slot without blocking the callback thread.
    pub fn reserve(&self) -> std::result::Result<LinuxDispatchReservation, LinuxDispatchRejection> {
        if self.inner.closing.load(Ordering::Acquire) {
            self.inner.metrics.closing.fetch_add(1, Ordering::Relaxed);
            return Err(LinuxDispatchRejection::Closing);
        }
        match self.inner.permits.clone().try_acquire_owned() {
            Ok(permit) => {
                self.inner.metrics.accepted.fetch_add(1, Ordering::Relaxed);
                Ok(LinuxDispatchReservation {
                    inner: self.inner.clone(),
                    permit,
                })
            }
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                self.inner.metrics.saturated.fetch_add(1, Ordering::Relaxed);
                Err(LinuxDispatchRejection::Saturated)
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                self.inner.metrics.closing.fetch_add(1, Ordering::Relaxed);
                Err(LinuxDispatchRejection::Closing)
            }
        }
    }

    /// Begins idempotent closing and rejects every subsequent reservation.
    pub fn close(&self) {
        if !self.inner.closing.swap(true, Ordering::AcqRel) {
            self.inner.permits.close();
        }
    }

    /// Returns whether new callback work is rejected.
    pub fn is_closing(&self) -> bool {
        self.inner.closing.load(Ordering::Acquire)
    }

    /// Returns an atomic metrics snapshot.
    pub fn metrics(&self) -> LinuxDispatchMetrics {
        LinuxDispatchMetrics {
            accepted: self.inner.metrics.accepted.load(Ordering::Relaxed),
            saturated: self.inner.metrics.saturated.load(Ordering::Relaxed),
            closing: self.inner.metrics.closing.load(Ordering::Relaxed),
            completed: self.inner.metrics.completed.load(Ordering::Relaxed),
            panicked: self.inner.metrics.panicked.load(Ordering::Relaxed),
        }
    }
}

/// Owned capacity reservation that transfers one native reply into asynchronous work.
#[must_use = "dropping a reservation releases its bounded capacity without dispatching work"]
pub struct LinuxDispatchReservation {
    inner: Arc<DispatcherInner>,
    permit: OwnedSemaphorePermit,
}

impl LinuxDispatchReservation {
    /// Spawns one accepted reply-owning future on the caller-provided runtime.
    pub fn spawn<F>(self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let inner = self.inner;
        let runtime = inner.runtime.clone();
        let permit = self.permit;
        let task = async move {
            let outcome = AssertUnwindSafe(future).catch_unwind().await;
            if outcome.is_err() {
                inner.metrics.panicked.fetch_add(1, Ordering::Relaxed);
            }
            inner.metrics.completed.fetch_add(1, Ordering::Release);
            drop(permit);
        };
        std::mem::drop(runtime.spawn(task));
    }
}
