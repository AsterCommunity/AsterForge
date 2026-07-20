//! Shared exponential backoff calculations.
//!
//! This module owns only mechanical delay arithmetic. Callers retain the
//! policy for retryability, cancellation, reset conditions, and observability.

use std::time::Duration;

/// Calculates an uncapped exponential delay for a zero-based retry index.
pub fn exponential_delay(initial_delay: Duration, retry_index: u32) -> Duration {
    let mut delay = initial_delay;
    let mut remaining = retry_index.min(127);
    while remaining > 0 && delay < Duration::MAX {
        delay = delay.saturating_add(delay);
        remaining -= 1;
    }
    delay
}

/// Applies a hard upper bound to a delay.
pub fn cap_delay(delay: Duration, max_delay: Duration) -> Duration {
    delay.min(max_delay)
}

/// Applies an explicit percentage to a delay, saturating at `Duration::MAX`.
pub fn apply_jitter(delay: Duration, percent: u16) -> Duration {
    let nanos = delay.as_nanos().saturating_mul(u128::from(percent));
    let nanos = nanos.checked_div(100).unwrap_or(0);
    duration_from_nanos(nanos)
}

/// Applies a sampled percentage using the default thread-local random generator.
///
/// The bounds are inclusive and clamped to a practical percentage range. Callers that need
/// deterministic behavior should use [`apply_jitter`] instead.
pub fn randomized_jitter(delay: Duration, min_percent: u16, max_percent: u16) -> Duration {
    use rand::RngExt;

    let min_percent = min_percent.min(1000);
    let max_percent = max_percent.min(1000).max(min_percent);
    apply_jitter(delay, rand::rng().random_range(min_percent..=max_percent))
}

fn duration_from_nanos(nanos: u128) -> Duration {
    let max_nanos = Duration::MAX.as_nanos();
    let nanos = nanos.min(max_nanos);
    let seconds = nanos / 1_000_000_000;
    let subsec_nanos = u32::try_from(nanos % 1_000_000_000).unwrap_or(999_999_999);
    Duration::new(u64::try_from(seconds).unwrap_or(u64::MAX), subsec_nanos)
}

#[cfg(test)]
mod tests {
    use super::{apply_jitter, cap_delay, exponential_delay};
    use std::time::Duration;

    #[test]
    fn exponential_delay_doubles_and_saturates() {
        let initial = Duration::from_millis(100);
        assert_eq!(exponential_delay(initial, 0), Duration::from_millis(100));
        assert_eq!(exponential_delay(initial, 1), Duration::from_millis(200));
        assert_eq!(exponential_delay(initial, 2), Duration::from_millis(400));
        assert_eq!(exponential_delay(initial, u32::MAX), Duration::MAX);
    }

    #[test]
    fn exponential_delay_handles_zero_and_duration_max() {
        assert_eq!(exponential_delay(Duration::ZERO, u32::MAX), Duration::ZERO);
        assert_eq!(exponential_delay(Duration::MAX, 0), Duration::MAX);
        assert_eq!(exponential_delay(Duration::MAX, 1), Duration::MAX);
    }

    #[test]
    fn cap_delay_handles_zero_equal_and_inverted_bounds() {
        assert_eq!(
            cap_delay(Duration::from_secs(1), Duration::ZERO),
            Duration::ZERO
        );
        assert_eq!(
            cap_delay(Duration::from_secs(1), Duration::from_secs(1)),
            Duration::from_secs(1)
        );
        assert_eq!(
            cap_delay(Duration::from_secs(1), Duration::from_secs(2)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn apply_jitter_supports_exact_boundaries_and_saturates() {
        let delay = Duration::from_millis(100);
        assert_eq!(apply_jitter(delay, 0), Duration::ZERO);
        assert_eq!(apply_jitter(delay, 50), Duration::from_millis(50));
        assert_eq!(apply_jitter(delay, 100), delay);
        assert_eq!(apply_jitter(delay, 150), Duration::from_millis(150));
        assert_eq!(apply_jitter(Duration::MAX, u16::MAX), Duration::MAX);
    }

    #[test]
    fn composition_keeps_cap_order_explicit() {
        let raw = exponential_delay(Duration::from_millis(100), 2);
        assert_eq!(
            cap_delay(apply_jitter(raw, 150), Duration::from_millis(250)),
            Duration::from_millis(250)
        );
        assert_eq!(
            apply_jitter(cap_delay(raw, Duration::from_millis(250)), 150),
            Duration::from_millis(375)
        );
    }

    #[test]
    fn randomized_jitter_stays_inside_configured_range() {
        for _ in 0..64 {
            let delay = super::randomized_jitter(Duration::from_millis(100), 50, 100);
            assert!((50..=100).contains(&delay.as_millis()));
        }
    }

    #[test]
    fn randomized_jitter_normalizes_degenerate_and_large_bounds() {
        assert_eq!(
            super::randomized_jitter(Duration::from_millis(100), 200, 50),
            Duration::from_millis(200)
        );
        assert_eq!(
            super::randomized_jitter(Duration::from_millis(100), u16::MAX, u16::MAX),
            Duration::from_millis(1_000)
        );
        assert_eq!(
            super::randomized_jitter(Duration::ZERO, 50, 150),
            Duration::ZERO
        );
    }
}
