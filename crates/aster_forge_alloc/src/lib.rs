//! Shared allocator tracking and memory statistics helpers.
//!
//! Applications still own their `#[global_allocator]` selection and any
//! platform-specific allocator configuration. This crate provides the reusable
//! pieces used by Aster services: a debug tracking allocator for system-allocator
//! builds, and a single `stats` API that reports either tracked system
//! allocation counters or jemalloc counters depending on enabled features.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

#[cfg(not(feature = "jemalloc"))]
use std::alloc::{GlobalAlloc, Layout, System};
#[cfg(not(feature = "jemalloc"))]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Current tracked heap allocation in bytes for system-allocator builds.
#[cfg(not(feature = "jemalloc"))]
pub static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
/// Peak tracked heap allocation in bytes for system-allocator builds.
#[cfg(not(feature = "jemalloc"))]
pub static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Global allocator wrapper that records current and peak allocation sizes.
#[cfg(not(feature = "jemalloc"))]
pub struct TrackingAlloc;

#[cfg(not(feature = "jemalloc"))]
#[inline]
fn record_alloc(size: usize) {
    let current = ALLOCATED.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(current, Ordering::Relaxed);
}

#[cfg(not(feature = "jemalloc"))]
#[inline]
fn record_dealloc(size: usize) {
    ALLOCATED.fetch_sub(size, Ordering::Relaxed);
}

#[cfg(not(feature = "jemalloc"))]
unsafe impl GlobalAlloc for TrackingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_dealloc(layout.size());
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            match new_size.cmp(&layout.size()) {
                std::cmp::Ordering::Greater => record_alloc(new_size - layout.size()),
                std::cmp::Ordering::Less => record_dealloc(layout.size() - new_size),
                std::cmp::Ordering::Equal => {}
            }
        }
        new_ptr
    }
}

/// Returns current and peak tracked allocations in MiB for system-allocator builds.
#[cfg(not(feature = "jemalloc"))]
pub fn stats() -> (f64, f64) {
    let allocated = ALLOCATED.load(Ordering::Relaxed) as f64 / 1_048_576.0;
    let peak = PEAK.load(Ordering::Relaxed) as f64 / 1_048_576.0;
    (allocated, peak)
}

/// Returns current allocated and resident memory in MiB for jemalloc stats builds.
#[cfg(feature = "jemalloc-stats")]
pub fn stats() -> (f64, f64) {
    if let Err(error) = tikv_jemalloc_ctl::epoch::advance() {
        tracing::warn!(error = %error, "failed to refresh jemalloc stats epoch");
    }

    let allocated = tikv_jemalloc_ctl::stats::allocated::read().unwrap_or(0) as f64 / 1_048_576.0;
    let resident = tikv_jemalloc_ctl::stats::resident::read().unwrap_or(0) as f64 / 1_048_576.0;
    (allocated, resident)
}

/// Returns zeroed counters for jemalloc builds without the stats feature.
#[cfg(all(feature = "jemalloc", not(feature = "jemalloc-stats")))]
pub fn stats() -> (f64, f64) {
    (0.0, 0.0)
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn stats_returns_non_negative_counters() {
        let (allocated, peak) = super::stats();

        assert!(allocated >= 0.0);
        assert!(peak >= 0.0);
    }

    #[cfg(all(feature = "jemalloc", not(feature = "jemalloc-stats")))]
    #[test]
    fn jemalloc_without_stats_returns_zeroes() {
        assert_eq!(super::stats(), (0.0, 0.0));
    }
}
