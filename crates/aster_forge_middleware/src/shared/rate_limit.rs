//! Framework-neutral governor quotas and normalized keyed rate limiting.
use governor::clock::{Clock, DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{NotUntil, Quota, RateLimiter};
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::Duration;
type StringKeyedLimiter =
    RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock, NoOpMiddleware>;
/// Returns the retry delay in whole seconds for a governor rejection.
///
/// Sub-second waits round up to one second so clients never see a zero delay
/// that invites an immediate retry.
#[must_use]
pub fn retry_after_seconds(not_until: &NotUntil<QuantaInstant>) -> u64 {
    let delay = not_until.wait_time_from(DefaultClock::default().now());
    ceil_retry_after(delay)
}

fn ceil_retry_after(delay: Duration) -> u64 {
    delay
        .as_secs()
        .saturating_add(u64::from(delay.subsec_nanos() > 0))
        .max(1)
}

/// A keyed string rate limiter with product-neutral key normalization.
///
/// The limiter trims surrounding whitespace and lowercases keys before checking
/// the quota. This suits usernames, email addresses, provider IDs, and similar
/// business-unique identifiers where accidental case differences should not
/// bypass a rate limit.
#[derive(Clone)]
pub struct NormalizedStringRateLimiter {
    enabled: bool,
    limiter: Arc<StringKeyedLimiter>,
}

impl NormalizedStringRateLimiter {
    /// Whether checks are enabled. Disabled transport adapters bypass key extraction too.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Removes expired buckets; schedule maintenance in the owning product runtime.
    pub fn retain_recent(&self) {
        self.limiter.retain_recent();
    }

    /// Builds a limiter from a non-zero quota and enabled flag.
    #[must_use]
    pub fn new(enabled: bool, seconds_per_request: NonZeroU64, burst_size: NonZeroU32) -> Self {
        Self {
            enabled,
            limiter: Arc::new(RateLimiter::keyed(rate_limit_quota(
                seconds_per_request,
                burst_size,
            ))),
        }
    }

    /// Checks a raw key after trimming whitespace and lowercasing it.
    #[must_use]
    pub fn check(&self, raw_key: &str) -> Option<RateLimitRejection> {
        if !self.enabled {
            return None;
        }

        let key = raw_key.trim().to_ascii_lowercase();
        self.limiter
            .check_key(&key)
            .err()
            .map(|not_until| RateLimitRejection::from_not_until(&not_until))
    }
}

/// Product-neutral rate-limit rejection metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitRejection {
    retry_after_seconds: u64,
}

impl RateLimitRejection {
    pub(crate) fn from_not_until(not_until: &NotUntil<QuantaInstant>) -> Self {
        Self {
            retry_after_seconds: retry_after_seconds(not_until),
        }
    }

    /// Returns how many seconds clients should wait before retrying.
    #[must_use]
    pub const fn retry_after_seconds(self) -> u64 {
        self.retry_after_seconds
    }
}

/// Builds a governor quota from non-zero seconds and burst size.
///
/// # Panics
/// Panics only if governor rejects a non-zero period.
#[expect(clippy::expect_used, reason = "NonZeroU64 creates a non-zero duration")]
#[must_use]
pub fn rate_limit_quota(seconds_per_request: NonZeroU64, burst_size: NonZeroU32) -> Quota {
    Quota::with_period(Duration::from_secs(seconds_per_request.get()))
        .expect("non-zero period")
        .allow_burst(burst_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::{NonZeroU32, NonZeroU64};
    #[test]
    fn quota_refills_at_the_exact_boundary_without_sleep() {
        use governor::clock::FakeRelativeClock;
        let clock = FakeRelativeClock::default();
        let quota = rate_limit_quota(NonZeroU64::new(2).unwrap(), NonZeroU32::new(2).unwrap());
        let limiter = RateLimiter::dashmap_with_clock(quota, clock.clone());
        assert!(limiter.check_key(&"one").is_ok());
        assert!(limiter.check_key(&"one").is_ok());
        assert!(limiter.check_key(&"one").is_err());
        assert!(limiter.check_key(&"two").is_ok());
        clock.advance(Duration::from_millis(1999));
        assert!(limiter.check_key(&"one").is_err());
        clock.advance(Duration::from_millis(1));
        assert!(limiter.check_key(&"one").is_ok());
        assert!(limiter.check_key(&"one").is_err());
    }

    #[test]
    fn multi_second_retry_delay_rounds_up_instead_of_inviting_early_retry() {
        assert_eq!(ceil_retry_after(Duration::from_millis(1999)), 2);
        assert_eq!(ceil_retry_after(Duration::from_millis(59_999)), 60);
        assert_eq!(ceil_retry_after(Duration::from_secs(2)), 2);
        assert_eq!(ceil_retry_after(Duration::ZERO), 1);
    }
    #[test]
    fn retry_after_seconds_rounds_sub_second_waits_up_to_one() {
        let quota = governor::Quota::with_period(std::time::Duration::from_secs(1))
            .unwrap()
            .allow_burst(NonZeroU32::new(1).unwrap());
        let limiter = governor::RateLimiter::keyed(quota);

        assert!(limiter.check_key(&"key").is_ok());
        let not_until = limiter
            .check_key(&"key")
            .expect_err("second immediate check should be rate limited");

        // The remaining wait is strictly below one second (some nanoseconds have
        // elapsed since the first check), so truncating whole seconds would
        // report 0 and tell the client to retry immediately.
        assert_eq!(retry_after_seconds(&not_until), 1);
    }
    #[test]
    fn normalized_string_limiter_can_be_disabled() {
        let limiter = NormalizedStringRateLimiter::new(
            false,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );

        assert!(limiter.check("admin@example.com").is_none());
        assert!(limiter.check("admin@example.com").is_none());
    }
    #[test]
    fn normalized_string_limiter_trims_and_lowercases_keys() {
        let limiter = NormalizedStringRateLimiter::new(
            true,
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );

        assert!(limiter.check("Admin@Example.com").is_none());
        assert!(limiter.check("other@example.com").is_none());
        assert!(limiter.check(" admin@example.com ").is_some());
    }
}
