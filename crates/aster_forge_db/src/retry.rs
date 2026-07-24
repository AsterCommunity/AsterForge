//! Retry helpers for transient database operations.
//!
//! Three levels exist, from preferred to most specific:
//!
//! - Multi-statement transactions: [`crate::transaction::with_transaction_retry`] retries the
//!   whole transaction callback and classifies commit outcomes. This is the only retry level
//!   that is safe for transactions.
//! - Idempotent single statements (reads, upserts, deletes by key): [`with_sea_orm_retry`] and
//!   [`with_sea_orm_retry_timeout`].
//! - Crate-internal [`DbError`] workflows such as connection setup: [`with_retry`].
//!
//! Every retryability decision derives from [`crate::database_error_kind`] and driver-native
//! error codes; error message text is never inspected. Non-retryable errors return
//! immediately so application bugs are not hidden behind sleep loops.

use std::time::Duration;
use tokio::time::{sleep, timeout};

use crate::{DbError, Result};

/// Retry configuration for async database operations.
///
/// One config type serves every retry level; construct it through a profile
/// ([`RetryConfig::connection`], [`RetryConfig::deadlock`]) and override individual fields
/// when the product needs a different budget.
#[derive(Clone, Copy, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial attempt.
    pub max_retries: u32,
    /// Base exponential-backoff delay in milliseconds.
    pub base_delay_ms: u64,
    /// Maximum backoff delay in milliseconds.
    pub max_delay_ms: u64,
}

impl RetryConfig {
    /// Profile for connection setup and acquisition: the peer may need time to recover,
    /// so back off slowly (3 retries, 100ms base, 5s cap).
    pub fn connection() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        }
    }

    /// Profile for deadlock/serialization-failure retries: lock-wait windows are short,
    /// so retry quickly (3 retries, 5ms base, 50ms cap).
    pub fn deadlock() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 5,
            max_delay_ms: 50,
        }
    }
}

impl Default for RetryConfig {
    /// Defaults to the [`RetryConfig::connection`] profile.
    fn default() -> Self {
        Self::connection()
    }
}

/// Returns whether a SeaORM error represents a transient database failure.
///
/// Connection acquisition and connection failures are always retryable: they happen before
/// the statement ran, so retrying cannot duplicate work. Query and execution failures are
/// retried only when [`crate::database_error_kind`] proves a transient locking conflict
/// (deadlock, serialization failure, lock timeout) from driver-native error codes. Error
/// message text is never inspected.
pub fn is_retryable_sea_orm_error(error: &sea_orm::DbErr) -> bool {
    use sea_orm::DbErr;

    match error {
        DbErr::ConnectionAcquire(_) | DbErr::Conn(_) => true,
        _ => crate::database_error_kind(error)
            .is_some_and(crate::DatabaseErrorKind::is_transient_locking),
    }
}

/// Executes a SeaORM operation with shared transient-error classification and backoff.
///
/// This is a statement-level escape hatch for idempotent single statements only (reads,
/// upserts, deletes by key). **Never use it around a multi-statement transaction**: a
/// deadlock rolls the whole transaction back, and re-running individual statements
/// afterwards executes them in autocommit mode, producing partial writes. Transactions
/// belong in [`crate::transaction::with_transaction_retry`], which retries the entire
/// callback and classifies commit outcomes.
pub async fn with_sea_orm_retry<F, Fut, T>(
    operation_name: &str,
    config: RetryConfig,
    mut operation: F,
) -> std::result::Result<T, sea_orm::DbErr>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, sea_orm::DbErr>>,
{
    let mut attempt = 0_u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt < config.max_retries && is_retryable_sea_orm_error(&error) => {
                let delay = calculate_delay(&config, attempt);
                tracing::warn!(
                    operation = operation_name,
                    attempt = attempt + 1,
                    max_attempts = config.max_retries + 1,
                    delay_ms = duration_millis_u64(delay),
                    error = %error,
                    "retrying SeaORM operation"
                );
                sleep(delay).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Executes a SeaORM operation with retry and a timeout applied to every attempt.
///
/// Same classification and boundary rules as [`with_sea_orm_retry`]; a timed-out attempt
/// is retried regardless of error classification because the operation produced no outcome.
/// Only wrap work that stays safe when a timed-out attempt keeps running in the background.
pub async fn with_sea_orm_retry_timeout<F, Fut, T>(
    operation_name: &str,
    config: RetryConfig,
    attempt_timeout: Duration,
    mut operation: F,
) -> std::result::Result<T, sea_orm::DbErr>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, sea_orm::DbErr>>,
{
    let mut attempt = 0_u32;
    loop {
        match timeout(attempt_timeout, operation()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error))
                if attempt < config.max_retries && is_retryable_sea_orm_error(&error) =>
            {
                let delay = calculate_delay(&config, attempt);
                tracing::warn!(
                    operation = operation_name,
                    attempt = attempt + 1,
                    max_attempts = config.max_retries + 1,
                    delay_ms = duration_millis_u64(delay),
                    error = %error,
                    "retrying SeaORM operation"
                );
                sleep(delay).await;
                attempt += 1;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) if attempt < config.max_retries => {
                let delay = calculate_delay(&config, attempt);
                tracing::warn!(
                    operation = operation_name,
                    attempt = attempt + 1,
                    max_attempts = config.max_retries + 1,
                    timeout_ms = duration_millis_u64(attempt_timeout),
                    delay_ms = duration_millis_u64(delay),
                    "SeaORM operation attempt timed out; retrying"
                );
                sleep(delay).await;
                attempt += 1;
            }
            Err(_) => {
                return Err(sea_orm::DbErr::Custom(format!(
                    "operation '{operation_name}' timed out after {}ms",
                    duration_millis_u64(attempt_timeout)
                )));
            }
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
    use aster_forge_utils::backoff::{cap_delay, exponential_delay, randomized_jitter};

    let raw = exponential_delay(Duration::from_millis(config.base_delay_ms), attempt);
    cap_delay(
        randomized_jitter(raw, 50, 150),
        Duration::from_millis(config.max_delay_ms),
    )
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
        assert!(is_retryable(&DbError::database_connection(
            "connection lost"
        )));
        assert!(is_retryable(&DbError::database_operation_classified(
            "deadlock",
            crate::DatabaseErrorKind::Deadlock
        )));
        // Unclassified operation errors carry no driver evidence of retry safety.
        assert!(!is_retryable(&DbError::database_operation("deadlock")));
        assert!(!is_retryable(&DbError::non_retryable("invalid input")));
        assert!(!is_retryable(&DbError::non_retryable("forbidden")));
    }

    #[test]
    fn sea_orm_retryability_allows_connection_failures_before_any_statement() {
        let conn_error = sea_orm::DbErr::Conn(sea_orm::error::RuntimeErr::Internal(
            "connection reset".to_string(),
        ));

        assert!(is_retryable_sea_orm_error(&conn_error));
    }

    #[test]
    fn sea_orm_retryability_rejects_non_driver_errors_without_reading_messages() {
        // Message text must not influence classification: these mention deadlock but are
        // not driver database errors, so they are not retryable.
        assert!(!is_retryable_sea_orm_error(&sea_orm::DbErr::Custom(
            "deadlock detected".to_string()
        )));
        assert!(!is_retryable_sea_orm_error(
            &sea_orm::DbErr::RecordNotFound("deadlock".to_string())
        ));
    }

    #[tokio::test]
    async fn sea_orm_retryability_classifies_real_sqlite_busy_as_retryable() {
        use aster_forge_test::temp::SqliteTestDatabase;
        use sea_orm::{ConnectOptions, ConnectionTrait, SqlxSqliteConnector};

        let database = SqliteTestDatabase::new("retry-busy");
        let locker = SqlxSqliteConnector::connect(ConnectOptions::new(database.url()))
            .await
            .unwrap();
        let contender = SqlxSqliteConnector::connect(ConnectOptions::new(database.url()))
            .await
            .unwrap();
        contender
            .execute_unprepared("PRAGMA busy_timeout=0;")
            .await
            .unwrap();
        locker
            .execute_unprepared("CREATE TABLE items (id INTEGER PRIMARY KEY);")
            .await
            .unwrap();
        locker.execute_unprepared("BEGIN IMMEDIATE;").await.unwrap();
        locker
            .execute_unprepared("INSERT INTO items (id) VALUES (1);")
            .await
            .unwrap();

        let error = contender
            .execute_unprepared("INSERT INTO items (id) VALUES (2);")
            .await
            .unwrap_err();

        locker.execute_unprepared("ROLLBACK;").await.unwrap();
        contender.close().await.unwrap();
        locker.close().await.unwrap();

        assert!(
            is_retryable_sea_orm_error(&error),
            "SQLITE_BUSY from a locked database should be retryable, got: {error}"
        );
        assert_eq!(
            crate::database_error_kind(&error),
            Some(crate::DatabaseErrorKind::LockTimeout)
        );
    }

    #[tokio::test]
    async fn sea_orm_retryability_rejects_sqlite_unique_violation() {
        use sea_orm::{ConnectionTrait, Database};

        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT UNIQUE);")
            .await
            .unwrap();
        db.execute_unprepared("INSERT INTO items (name) VALUES ('a');")
            .await
            .unwrap();

        let error = db
            .execute_unprepared("INSERT INTO items (name) VALUES ('a');")
            .await
            .unwrap_err();

        assert!(!is_retryable_sea_orm_error(&error));
        assert_eq!(
            crate::database_error_kind(&error),
            Some(crate::DatabaseErrorKind::UniqueConstraint)
        );
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

    #[test]
    fn calculate_delay_handles_zero_and_initial_above_max_boundaries() {
        let zero_base = RetryConfig {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 100,
        };
        let zero_max = RetryConfig {
            max_retries: 1,
            base_delay_ms: 100,
            max_delay_ms: 0,
        };
        let initial_above_max = RetryConfig {
            max_retries: 1,
            base_delay_ms: 1_000,
            max_delay_ms: 250,
        };

        for attempt in [0, 1, u32::MAX] {
            assert_eq!(calculate_delay(&zero_base, attempt), Duration::ZERO);
            assert_eq!(calculate_delay(&zero_max, attempt), Duration::ZERO);
            assert_eq!(
                calculate_delay(&initial_above_max, attempt),
                Duration::from_millis(250)
            );
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
                        Err(DbError::database_operation_classified(
                            "deadlock",
                            crate::DatabaseErrorKind::Deadlock,
                        ))
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

    #[tokio::test]
    async fn with_retry_zero_budget_runs_exactly_one_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result = {
            let attempts = Arc::clone(&attempts);
            with_retry(&zero_delay_config(0), move || {
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
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
