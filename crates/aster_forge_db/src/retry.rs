//! Retry helpers for transient database operations.
//!
//! The retry loop centralizes backoff, jitter, and retryability decisions for database setup and
//! operations that may fail while a database is starting or briefly unavailable. Non-retryable
//! errors return immediately so application bugs are not hidden behind sleep loops.

use std::time::Duration;
use tokio::time::sleep;

use crate::{DbError, Result};

/// Retry configuration for async database operations.
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial attempt.
    pub max_retries: u32,
    /// Base exponential-backoff delay in milliseconds.
    pub base_delay_ms: u64,
    /// Maximum backoff delay in milliseconds.
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        }
    }
}

/// Execute an async operation with exponential backoff retry
pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt == config.max_retries || !is_retryable(&e) {
                    return Err(e);
                }
                let delay = calculate_delay(config, attempt);
                tracing::warn!(
                    attempt = attempt + 1,
                    max = config.max_retries,
                    delay_ms = duration_millis_u64(delay),
                    error = %e,
                    "retrying operation"
                );
                last_err = Some(e);
                sleep(delay).await;
            }
        }
    }
    Err(last_err.unwrap_or(DbError::RetryExhausted))
}

fn is_retryable(err: &DbError) -> bool {
    err.is_retryable()
}

fn calculate_delay(config: &RetryConfig, attempt: u32) -> Duration {
    use rand::RngExt;
    let multiplier = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let base = config.base_delay_ms.saturating_mul(multiplier);
    // Add jitter: 50%-150% of the exponential delay, then enforce the configured cap.
    let mut rng = rand::rng();
    let jitter = rng.random_range(50_u64..=150_u64);
    let jittered = base.saturating_mul(jitter) / 100;
    Duration::from_millis(jittered.min(config.max_delay_ms))
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn zero_delay_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            base_delay_ms: 0,
            max_delay_ms: 0,
        }
    }

    #[test]
    fn retryable_error_classification_only_allows_database_errors() {
        assert!(is_retryable(&DbError::database_operation("deadlock")));
        assert!(is_retryable(&DbError::database_connection(
            "connection lost"
        )));
        assert!(!is_retryable(&DbError::non_retryable("invalid input")));
        assert!(!is_retryable(&DbError::non_retryable("forbidden")));
    }

    #[test]
    fn calculate_delay_applies_jitter_and_hard_caps_max_delay() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 250,
        };

        let expected_bounds = [(0, 50, 150), (1, 100, 250), (2, 200, 250), (8, 250, 250)];

        for (attempt, min_ms, max_ms) in expected_bounds {
            for _ in 0..64 {
                let delay_ms = duration_millis_u64(calculate_delay(&config, attempt));
                assert!(
                    (min_ms..=max_ms).contains(&delay_ms),
                    "attempt {attempt} produced {delay_ms}ms outside [{min_ms}, {max_ms}]"
                );
            }
        }
    }

    #[tokio::test]
    async fn with_retry_retries_retryable_errors_until_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result = {
            let attempts = Arc::clone(&attempts);
            with_retry(&zero_delay_config(3), move || {
                let attempts = Arc::clone(&attempts);
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Err(DbError::database_operation("temporary failure"))
                    } else {
                        Ok("ok")
                    }
                }
            })
            .await
        };

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn with_retry_stops_immediately_for_non_retryable_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result = {
            let attempts = Arc::clone(&attempts);
            with_retry(&zero_delay_config(3), move || {
                let attempts = Arc::clone(&attempts);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(DbError::non_retryable("bad request"))
                }
            })
            .await
        };

        assert!(matches!(result.unwrap_err(), DbError::NonRetryable(_)));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn with_retry_stops_after_exhausting_retry_budget() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result = {
            let attempts = Arc::clone(&attempts);
            with_retry(&zero_delay_config(2), move || {
                let attempts = Arc::clone(&attempts);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(DbError::database_connection("still failing"))
                }
            })
            .await
        };

        assert!(matches!(
            result.unwrap_err(),
            DbError::DatabaseConnection(_)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
