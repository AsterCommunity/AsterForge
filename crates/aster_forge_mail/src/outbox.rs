//! Product-neutral mail outbox dispatch helpers.

use std::future::Future;
use std::time::Duration;

/// Default maximum stored delivery error length.
pub const DEFAULT_ERROR_MAX_LEN: usize = 1024;

/// Default retry delays for persisting "sent" after successful delivery.
///
/// The first attempt is immediate. Later attempts provide a short best-effort
/// window for transient database failures after SMTP has already accepted the
/// message.
pub const DEFAULT_MARK_SENT_RETRY_DELAYS_MS: &[u64] = &[0, 100, 500, 2_000, 5_000];

/// Aggregate counters returned by an outbox dispatch or drain pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DispatchStats {
    /// Rows claimed for processing.
    pub claimed: usize,
    /// Rows delivered and marked sent.
    pub sent: usize,
    /// Rows scheduled for retry.
    pub retried: usize,
    /// Rows permanently failed.
    pub failed: usize,
}

impl DispatchStats {
    /// Adds another counter set into this one.
    pub fn merge(&mut self, other: Self) {
        self.claimed += other.claimed;
        self.sent += other.sent;
        self.retried += other.retried;
        self.failed += other.failed;
    }

    /// Returns whether the dispatch pass did any visible work.
    pub const fn is_empty(self) -> bool {
        self.claimed == 0 && self.sent == 0 && self.retried == 0 && self.failed == 0
    }
}

/// Retry and truncation policy for an outbox dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailOutboxRetryPolicy {
    /// Maximum number of delivery attempts before permanent failure.
    pub max_attempts: i32,
    /// Maximum stored error string length, counted in Unicode scalar values.
    pub max_error_len: usize,
}

/// Decision returned after a mail delivery attempt fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailOutboxDeliveryFailureDecision {
    /// The row exhausted delivery attempts and should be moved to a terminal failure state.
    PermanentFailure {
        /// Attempt count that should be persisted on the outbox row.
        attempt_count: i32,
        /// Truncated delivery error safe for storage.
        error_message: String,
    },
    /// The row should be scheduled for another delivery attempt.
    Retry {
        /// Attempt count that should be persisted on the outbox row.
        attempt_count: i32,
        /// Delay before the next delivery attempt, in seconds.
        retry_delay_secs: i64,
        /// Truncated delivery error safe for storage.
        error_message: String,
    },
}

impl MailOutboxRetryPolicy {
    /// Creates a retry policy.
    pub const fn new(max_attempts: i32, max_error_len: usize) -> Self {
        Self {
            max_attempts,
            max_error_len,
        }
    }

    /// Returns whether `attempt_count` should permanently fail.
    pub const fn should_permanently_fail(&self, attempt_count: i32) -> bool {
        attempt_count >= self.max_attempts
    }

    /// Returns the delay before the next delivery retry.
    pub const fn retry_delay_secs(&self, attempt_count: i32) -> i64 {
        retry_delay_secs(attempt_count)
    }

    /// Truncates a delivery error according to this policy.
    pub fn truncate_error(&self, error: &str) -> String {
        truncate_error(error, self.max_error_len)
    }

    /// Classifies a failed delivery attempt.
    ///
    /// `attempt_count` should be the post-delivery attempt count that the
    /// product crate will persist on the outbox row.
    pub fn delivery_failure_decision(
        &self,
        attempt_count: i32,
        error: impl AsRef<str>,
    ) -> MailOutboxDeliveryFailureDecision {
        let error_message = self.truncate_error(error.as_ref());
        if self.should_permanently_fail(attempt_count) {
            MailOutboxDeliveryFailureDecision::PermanentFailure {
                attempt_count,
                error_message,
            }
        } else {
            MailOutboxDeliveryFailureDecision::Retry {
                attempt_count,
                retry_delay_secs: self.retry_delay_secs(attempt_count),
                error_message,
            }
        }
    }
}

/// Returns the default mail delivery retry delay for an attempt count.
pub const fn retry_delay_secs(attempt_count: i32) -> i64 {
    match attempt_count {
        1 => 5,
        2 => 15,
        3 => 60,
        4 => 300,
        5 => 900,
        _ => 1800,
    }
}

/// Truncates an error string without splitting UTF-8 code points.
pub fn truncate_error(error: &str, max_len: usize) -> String {
    error.chars().take(max_len).collect()
}

/// Retries a product-provided `mark_sent` operation after SMTP success.
///
/// This helper exists to narrow the duplicate-delivery window where SMTP has
/// accepted a message but the database row still says `Processing`. The caller
/// provides the actual persistence function so repositories, transactions,
/// timestamps, and product errors stay in the product crate.
pub async fn retry_mark_sent<F, Fut, E>(
    id: i64,
    retry_delays_ms: &[u64],
    mut mark_sent: F,
) -> Result<bool, E>
where
    F: FnMut(i64, usize) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
    E: std::fmt::Display,
{
    let mut last_err = None;
    for (index, delay_ms) in retry_delays_ms.iter().enumerate() {
        if *delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
        }

        let attempt = index + 1;
        match mark_sent(id, attempt).await {
            Ok(updated) => {
                tracing::debug!(
                    mail_outbox_id = id,
                    attempt,
                    updated,
                    "marked mail outbox row as sent"
                );
                return Ok(updated);
            }
            Err(error) => {
                tracing::warn!(
                    mail_outbox_id = id,
                    attempt,
                    "mark_sent failed, will retry: {error}"
                );
                last_err = Some(error);
            }
        }
    }

    match last_err {
        Some(error) => Err(error),
        None => mark_sent(id, retry_delays_ms.len() + 1).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ERROR_MAX_LEN, DispatchStats, MailOutboxRetryPolicy, retry_delay_secs,
        retry_mark_sent, truncate_error,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn dispatch_stats_merge_adds_all_counters() {
        let mut stats = DispatchStats {
            claimed: 1,
            sent: 2,
            retried: 3,
            failed: 4,
        };
        stats.merge(DispatchStats {
            claimed: 10,
            sent: 20,
            retried: 30,
            failed: 40,
        });

        assert_eq!(
            stats,
            DispatchStats {
                claimed: 11,
                sent: 22,
                retried: 33,
                failed: 44,
            }
        );
        assert!(!stats.is_empty());
        assert!(DispatchStats::default().is_empty());
    }

    #[test]
    fn retry_policy_matches_default_mail_backoff() {
        let policy = MailOutboxRetryPolicy::new(6, DEFAULT_ERROR_MAX_LEN);

        assert!(!policy.should_permanently_fail(5));
        assert!(policy.should_permanently_fail(6));
        assert_eq!(policy.retry_delay_secs(1), 5);
        assert_eq!(policy.retry_delay_secs(2), 15);
        assert_eq!(policy.retry_delay_secs(3), 60);
        assert_eq!(policy.retry_delay_secs(4), 300);
        assert_eq!(policy.retry_delay_secs(5), 900);
        assert_eq!(retry_delay_secs(99), 1800);
    }

    #[test]
    fn retry_policy_classifies_delivery_failures() {
        let policy = MailOutboxRetryPolicy::new(2, 3);

        assert_eq!(
            policy.delivery_failure_decision(1, "abcdef"),
            super::MailOutboxDeliveryFailureDecision::Retry {
                attempt_count: 1,
                retry_delay_secs: 5,
                error_message: "abc".to_string(),
            }
        );
        assert_eq!(
            policy.delivery_failure_decision(2, "abcdef"),
            super::MailOutboxDeliveryFailureDecision::PermanentFailure {
                attempt_count: 2,
                error_message: "abc".to_string(),
            }
        );
    }

    #[test]
    fn truncate_error_preserves_utf8_boundaries() {
        let value = "界".repeat(4);
        assert_eq!(truncate_error(&value, 3), "界界界");
    }

    #[tokio::test]
    async fn retry_mark_sent_retries_until_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = attempts.clone();

        let updated = retry_mark_sent(42, &[0, 0, 0], move |_id, _attempt| {
            let attempts = attempts_for_closure.clone();
            async move {
                let current = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if current < 3 {
                    Err("temporary db error")
                } else {
                    Ok(true)
                }
            }
        })
        .await
        .expect("mark_sent should eventually succeed");

        assert!(updated);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_mark_sent_without_delays_runs_one_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = attempts.clone();

        let error = retry_mark_sent(42, &[], move |_id, _attempt| {
            let attempts = attempts_for_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<bool, _>("db down")
            }
        })
        .await
        .expect_err("mark_sent should fail");

        assert_eq!(error, "db down");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
