//! Shared allocator tracking and memory statistics helpers.
//!
//! Applications still own their `#[global_allocator]` selection and any
//! platform-specific allocator configuration. This crate provides the reusable
//! pieces used by Aster services: a debug tracking allocator for system-allocator
//! builds, and a single `stats` API that reports either tracked system
//! allocation counters or jemalloc counters depending on enabled features.
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
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
// SAFETY: `TrackingAlloc` delegates all allocation operations to `System` with
// caller-provided layouts and pointers unchanged, and only updates independent
// atomic counters after successful allocation-size changes.
unsafe impl GlobalAlloc for TrackingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `TrackingAlloc` preserves the caller's `GlobalAlloc::alloc`
        // contract and forwards the exact layout to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `TrackingAlloc` preserves the caller's `GlobalAlloc::alloc_zeroed`
        // contract and forwards the exact layout to the system allocator.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_dealloc(layout.size());
        // SAFETY: `TrackingAlloc` preserves the caller's `GlobalAlloc::dealloc`
        // contract and forwards the original pointer and layout unchanged.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `TrackingAlloc` preserves the caller's `GlobalAlloc::realloc`
        // contract and forwards the original pointer, layout, and requested size.
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
    use std::{
        alloc::{GlobalAlloc, Layout},
        ptr,
        sync::{Mutex, MutexGuard},
    };

    #[cfg(not(feature = "jemalloc"))]
    static TEST_ALLOC_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(not(feature = "jemalloc"))]
    fn reset_tracking_counters() -> MutexGuard<'static, ()> {
        let guard = TEST_ALLOC_LOCK.lock().unwrap();
        super::ALLOCATED.store(0, std::sync::atomic::Ordering::Relaxed);
        super::PEAK.store(0, std::sync::atomic::Ordering::Relaxed);
        guard
    }

    #[cfg(not(feature = "jemalloc"))]
    fn assert_stats_bytes(allocated: usize, peak: usize) {
        let (allocated_mib, peak_mib) = super::stats();

        assert_eq!(allocated_mib, allocated as f64 / 1_048_576.0);
        assert_eq!(peak_mib, peak as f64 / 1_048_576.0);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn stats_returns_non_negative_counters() {
        let (allocated, peak) = super::stats();

        assert!(allocated >= 0.0);
        assert!(peak >= 0.0);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn record_alloc_updates_current_and_peak_bytes() {
        let _guard = reset_tracking_counters();

        super::record_alloc(256);
        assert_stats_bytes(256, 256);

        super::record_alloc(128);
        assert_stats_bytes(384, 384);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn record_dealloc_reduces_current_without_lowering_peak() {
        let _guard = reset_tracking_counters();

        super::record_alloc(512);
        super::record_dealloc(128);

        assert_stats_bytes(384, 512);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn tracking_alloc_records_alloc_and_dealloc() {
        let _guard = reset_tracking_counters();
        let allocator = super::TrackingAlloc;
        let layout = Layout::from_size_align(64, 8).unwrap();

        // SAFETY: `layout` is non-zero and valid. The returned pointer is checked
        // for null before use and released once with the same allocator and layout.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        assert_stats_bytes(64, 64);

        // SAFETY: `ptr` was allocated by `allocator.alloc(layout)` above and has
        // not been deallocated yet.
        unsafe { allocator.dealloc(ptr, layout) };
        assert_stats_bytes(0, 64);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn tracking_alloc_zeroed_returns_zeroed_memory_and_records_size() {
        let _guard = reset_tracking_counters();
        let allocator = super::TrackingAlloc;
        let layout = Layout::from_size_align(32, 8).unwrap();

        // SAFETY: `layout` is non-zero and valid. The returned pointer is checked
        // for null before reading and released once with the same allocator/layout.
        let ptr = unsafe { allocator.alloc_zeroed(layout) };
        assert!(!ptr.is_null());

        // SAFETY: `ptr` references `layout.size()` initialized bytes because
        // `alloc_zeroed` succeeded.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
        assert!(bytes.iter().all(|byte| *byte == 0));
        assert_stats_bytes(32, 32);

        // SAFETY: `ptr` was allocated by `allocator.alloc_zeroed(layout)` above
        // and has not been deallocated yet.
        unsafe { allocator.dealloc(ptr, layout) };
        assert_stats_bytes(0, 32);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn tracking_alloc_realloc_grow_and_shrink_adjusts_counters() {
        let _guard = reset_tracking_counters();
        let allocator = super::TrackingAlloc;
        let initial_layout = Layout::from_size_align(16, 8).unwrap();

        // SAFETY: `initial_layout` is non-zero and valid. The returned pointer is
        // checked for null before use and remains owned until the final dealloc.
        let ptr = unsafe { allocator.alloc(initial_layout) };
        assert!(!ptr.is_null());

        // SAFETY: `ptr` references at least 16 writable bytes from the allocation above.
        unsafe { ptr::write_bytes(ptr, 0xAB, initial_layout.size()) };

        // SAFETY: `ptr` was allocated with `initial_layout` and has not been freed.
        let grown_ptr = unsafe { allocator.realloc(ptr, initial_layout, 64) };
        assert!(!grown_ptr.is_null());
        assert_stats_bytes(64, 64);

        // SAFETY: The first 16 bytes must remain valid after successful `realloc`.
        let preserved = unsafe { std::slice::from_raw_parts(grown_ptr, initial_layout.size()) };
        assert!(preserved.iter().all(|byte| *byte == 0xAB));

        let grown_layout = Layout::from_size_align(64, 8).unwrap();
        // SAFETY: `grown_ptr` was allocated by the successful realloc above with
        // `grown_layout.size()` bytes and has not been freed.
        let shrunk_ptr = unsafe { allocator.realloc(grown_ptr, grown_layout, 24) };
        assert!(!shrunk_ptr.is_null());
        assert_stats_bytes(24, 64);

        let shrunk_layout = Layout::from_size_align(24, 8).unwrap();
        // SAFETY: `shrunk_ptr` is the live pointer from the successful shrink and
        // `shrunk_layout` matches the new allocation size and alignment.
        unsafe { allocator.dealloc(shrunk_ptr, shrunk_layout) };
        assert_stats_bytes(0, 64);
    }

    #[cfg(not(feature = "jemalloc"))]
    #[test]
    fn tracking_alloc_realloc_same_size_leaves_counters_unchanged() {
        let _guard = reset_tracking_counters();
        let allocator = super::TrackingAlloc;
        let layout = Layout::from_size_align(40, 8).unwrap();

        // SAFETY: `layout` is non-zero and valid. The returned pointer is checked
        // for null before use and remains owned until the final dealloc.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        assert_stats_bytes(40, 40);

        // SAFETY: `ptr` was allocated with `layout` and has not been freed. Passing
        // the existing allocation size exercises the equal-size realloc path.
        let same_size_ptr = unsafe { allocator.realloc(ptr, layout, layout.size()) };
        assert!(!same_size_ptr.is_null());
        assert_stats_bytes(40, 40);

        // SAFETY: `same_size_ptr` is the live pointer returned by realloc and
        // `layout` still matches the allocation size and alignment.
        unsafe { allocator.dealloc(same_size_ptr, layout) };
        assert_stats_bytes(0, 40);
    }

    #[cfg(all(feature = "jemalloc", not(feature = "jemalloc-stats")))]
    #[test]
    fn jemalloc_without_stats_returns_zeroes() {
        assert_eq!(super::stats(), (0.0, 0.0));
    }
}
