//! Transaction helpers with consistent error mapping and rollback tracing.
//!
//! Service code can either manually begin/commit/rollback with uniform error conversion or run a
//! callback inside `with_transaction`. The rollback guard logs dropped transactions, which helps
//! catch early-return paths that rely on rollback-on-drop instead of making rollback explicit.
//!
//! ## Pattern
//!
//! Standard service-layer transaction usage:
//! ```ignore
//! transaction::with_transaction(db, async |txn| {
//!     repo::operation(txn, ...).await?;
//!     repo::another_operation(txn, ...).await?;
//!     Ok(())
//! })
//! .await?;
//! ```

use crate::{DbError, Result, database_error_kind};
use sea_orm::TransactionSession;
use std::panic::Location;
use std::{future::Future, ops::AsyncFnOnce, pin::Pin, time::Duration};
use tracing::Instrument;

/// Per-future stage timing; captures no SQL, connection URL, or callback error text.
struct TransactionObservation {
    attempt: u32,
    phase: &'static str,
    started: std::time::Instant,
    phase_started: std::time::Instant,
    finished: bool,
}

impl TransactionObservation {
    fn new(attempt: u32) -> Self {
        tracing::trace!(attempt, phase = "begin", "transaction phase started");
        Self {
            attempt,
            phase: "begin",
            started: std::time::Instant::now(),
            phase_started: std::time::Instant::now(),
            finished: false,
        }
    }
    fn phase(&mut self, phase: &'static str) {
        tracing::trace!(
            attempt = self.attempt,
            phase = self.phase,
            elapsed_seconds = self.phase_started.elapsed().as_secs_f64(),
            "transaction phase finished"
        );
        self.phase = phase;
        self.phase_started = std::time::Instant::now();
        tracing::trace!(attempt = self.attempt, phase, "transaction phase started");
    }
    fn finish(&mut self, outcome: &'static str) {
        self.finished = true;
        if outcome == "committed" || outcome == "callback_error_rolled_back" {
            tracing::trace!(
                attempt = self.attempt,
                phase = self.phase,
                phase_seconds = self.phase_started.elapsed().as_secs_f64(),
                elapsed_seconds = self.started.elapsed().as_secs_f64(),
                outcome,
                "transaction finished"
            );
        } else {
            tracing::warn!(
                attempt = self.attempt,
                phase = self.phase,
                elapsed_seconds = self.started.elapsed().as_secs_f64(),
                outcome,
                "transaction boundary failed"
            );
        }
    }
}

impl Drop for TransactionObservation {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                attempt = self.attempt,
                phase = self.phase,
                elapsed_seconds = self.started.elapsed().as_secs_f64(),
                outcome = "dropped_outcome_unknown",
                "transaction future dropped before boundary completion"
            );
        }
    }
}

/// Begins and returns a transaction so the caller can commit or roll it back.
///
/// This centralizes `begin` error mapping.
///
/// # Errors
///
/// Returns an error when transaction setup, execution, or finalization fails.
pub async fn begin<C: sea_orm::TransactionTrait>(db: &C) -> Result<C::Transaction> {
    db.begin()
        .await
        .map_err(|error| database_operation_with_context(&error, "begin transaction"))
}

/// Commits a transaction and maps errors consistently.
///
/// # Errors
///
/// Returns an error when transaction setup, execution, or finalization fails.
pub async fn commit<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.commit()
        .await
        .map_err(|error| database_operation_with_context(&error, "commit transaction"))
}

/// Rolls back a transaction and maps errors consistently.
///
/// # Errors
///
/// Returns an error when transaction setup, execution, or finalization fails.
pub async fn rollback<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.rollback()
        .await
        .map_err(|error| database_operation_with_context(&error, "rollback transaction"))
}

fn database_operation_with_context(error: &sea_orm::DbErr, context: &str) -> DbError {
    let kind = database_error_kind(error);
    let message = format!("{context}: {error}");
    match kind {
        Some(kind) => DbError::database_operation_classified(message, kind),
        None => DbError::database_operation(message),
    }
}

fn transaction_delay(config: &crate::retry::RetryConfig, attempt: u32) -> Duration {
    aster_forge_utils::backoff::cap_delay(
        aster_forge_utils::backoff::exponential_delay(
            Duration::from_millis(config.base_delay_ms),
            attempt,
        ),
        Duration::from_millis(config.max_delay_ms),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitFailureAction {
    Retry,
    ReturnKnownFailure,
    ReturnOutcomeUnknown,
}

fn commit_failure_action(
    retryable: bool,
    outcome_known_rolled_back: bool,
    attempt: u32,
    max_retries: u32,
) -> CommitFailureAction {
    if !outcome_known_rolled_back {
        CommitFailureAction::ReturnOutcomeUnknown
    } else if retryable {
        if attempt < max_retries {
            CommitFailureAction::Retry
        } else {
            CommitFailureAction::ReturnKnownFailure
        }
    } else {
        CommitFailureAction::ReturnKnownFailure
    }
}

fn commit_outcome_known_rolled_back(kind: Option<crate::DatabaseErrorKind>) -> bool {
    matches!(
        kind,
        Some(crate::DatabaseErrorKind::Deadlock | crate::DatabaseErrorKind::SerializationFailure)
    )
}

/// Runs a complete transaction boundary with bounded retries selected by the product.
///
/// The callback is rerun only after rollback or a commit failure known to have rolled back the
/// transaction. Any commit error with an uncertain server-side outcome is returned as
/// `DbError::CommitOutcomeUnknown`, because the server may have committed the transaction even
/// though the client did not receive a success response.
///
/// Use [`crate::retry::RetryConfig::deadlock`] as the config profile: deadlock and
/// serialization conflicts resolve inside short lock-wait windows, so the fast profile fits.
///
/// # Errors
///
/// Returns an error when transaction setup, execution, or finalization fails.
#[track_caller]
pub fn with_transaction_retry<C, F, T, E, P>(
    db: &C,
    config: &crate::retry::RetryConfig,
    mut operation: F,
    should_retry: P,
) -> impl Future<Output = std::result::Result<T, E>>
where
    C: sea_orm::TransactionTrait,
    F: for<'txn> FnMut(
        &'txn C::Transaction,
    )
        -> Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'txn>>,
    E: From<DbError> + std::fmt::Display,
    P: Fn(&E) -> bool,
{
    let location = Location::caller();
    let span = tracing::info_span!(
        "db_transaction_retry",
        transaction_id = %uuid::Uuid::new_v4(),
        caller_file = location.file(),
        caller_line = location.line()
    );
    async move {
        let mut attempt = 0;
        loop {
            tracing::trace!(attempt, "transaction retry attempt");
            let mut observation = TransactionObservation::new(attempt);
            let txn = match db.begin().await {
                Ok(txn) => txn,
                Err(error) => {
                    observation.finish("begin_error");
                    let error =
                        E::from(database_operation_with_context(&error, "begin transaction"));
                    if attempt < config.max_retries && should_retry(&error) {
                        tokio::time::sleep(transaction_delay(config, attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error);
                }
            };

            observation.phase("callback");
            match operation(&txn).await {
                Ok(value) => {
                    observation.phase("commit");
                    match txn.commit().await {
                        Ok(()) => {
                            observation.finish("committed");
                            return Ok(value);
                        }
                        Err(error) => {
                            let kind = database_error_kind(&error);
                            let classified_error = E::from(match kind {
                                Some(kind) => DbError::database_operation_classified(
                                    format!("commit transaction: {error}"),
                                    kind,
                                ),
                                None => DbError::database_operation(format!(
                                    "commit transaction: {error}"
                                )),
                            });
                            observation.finish(if commit_outcome_known_rolled_back(kind) {
                                "commit_error_rolled_back"
                            } else {
                                "commit_error_outcome_unknown"
                            });
                            match commit_failure_action(
                                should_retry(&classified_error),
                                commit_outcome_known_rolled_back(kind),
                                attempt,
                                config.max_retries,
                            ) {
                                CommitFailureAction::Retry => {
                                    tokio::time::sleep(transaction_delay(config, attempt)).await;
                                    attempt += 1;
                                }
                                CommitFailureAction::ReturnKnownFailure => {
                                    return Err(classified_error);
                                }
                                CommitFailureAction::ReturnOutcomeUnknown => {
                                    return Err(E::from(DbError::commit_outcome_unknown(
                                        format!("commit transaction: {error}"),
                                        kind,
                                    )));
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    observation.phase("rollback");
                    observation.finish(if txn.rollback().await.is_err() {
                        "callback_error_rollback_failed"
                    } else {
                        "callback_error_rolled_back"
                    });
                    if attempt < config.max_retries && should_retry(&error) {
                        tokio::time::sleep(transaction_delay(config, attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }
    .instrument(span)
}

/// Runs a transaction callback with consistent tracing and rollback guarding.
///
/// The callback may return a product-specific error type. Forge-created transaction boundary
/// errors are converted through `E: From<DbError>`, while callback errors are preserved unchanged.
///
/// # Errors
///
/// Returns an error when transaction setup, execution, or finalization fails.
#[track_caller]
pub fn with_transaction<C, F, T, E>(
    db: &C,
    operation: F,
) -> impl Future<Output = std::result::Result<T, E>>
where
    C: sea_orm::TransactionTrait,
    F: for<'txn> AsyncFnOnce(&'txn C::Transaction) -> std::result::Result<T, E>,
    E: From<DbError> + std::fmt::Display,
{
    let location = Location::caller();
    let span = tracing::info_span!(
        "db_transaction",
        transaction_id = %uuid::Uuid::new_v4(),
        caller_file = location.file(),
        caller_line = location.line()
    );
    async move {
        let mut observation = TransactionObservation::new(0);
        let txn = match begin(db).await {
            Ok(txn) => txn,
            Err(error) => {
                observation.finish("begin_error");
                return Err(E::from(error));
            }
        };
        observation.phase("callback");
        match operation(&txn).await {
            Ok(value) => {
                observation.phase("commit");
                match commit(txn).await {
                    Ok(()) => {
                        observation.finish("committed");
                        Ok(value)
                    }
                    Err(error) => {
                        observation.finish(
                            if commit_outcome_known_rolled_back(error.database_error_kind()) {
                                "commit_error_rolled_back"
                            } else {
                                "commit_error_outcome_unknown"
                            },
                        );
                        Err(E::from(error))
                    }
                }
            }
            Err(error) => {
                observation.phase("rollback");
                if rollback(txn).await.is_err() {
                    observation.finish("callback_error_rollback_failed");
                } else {
                    observation.finish("callback_error_rolled_back");
                }
                Err(error)
            }
        }
    }
    .instrument(span)
}

#[cfg(test)]
mod tests {
    use super::{
        CommitFailureAction, commit_failure_action, commit_outcome_known_rolled_back, rollback,
        transaction_delay, with_transaction,
    };
    use crate::{DbError, connection::DatabaseConfig};
    use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait};
    use std::fmt;

    async fn sqlite_db() -> DatabaseConnection {
        crate::connection::connect(&DatabaseConfig {
            url: "sqlite::memory:".into(),
            pool_size: 1,
            retry_count: 0,
        })
        .await
        .expect("sqlite memory database should connect")
    }

    async fn count_rows(db: &DatabaseConnection) -> i64 {
        let statement = Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT COUNT(*) FROM transaction_items",
        );
        let row = db
            .query_one_raw(statement)
            .await
            .expect("count query should succeed")
            .expect("count query should return one row");
        row.try_get_by_index(0).expect("count should decode")
    }

    #[derive(Debug, PartialEq, Eq)]
    enum ProductError {
        Db(String),
        Validation(&'static str),
    }

    impl From<DbError> for ProductError {
        fn from(value: DbError) -> Self {
            Self::Db(value.to_string())
        }
    }

    impl fmt::Display for ProductError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Db(message) => formatter.write_str(message),
                Self::Validation(message) => formatter.write_str(message),
            }
        }
    }

    #[tokio::test]
    async fn with_transaction_commits_successful_callback() {
        let db = sqlite_db().await;
        db.execute_unprepared("CREATE TABLE transaction_items (id INTEGER PRIMARY KEY);")
            .await
            .expect("table should be created");

        let value = with_transaction(&db, async |txn| {
            txn.execute_unprepared("INSERT INTO transaction_items (id) VALUES (1);")
                .await
                .map_err(DbError::from)?;
            Ok::<_, DbError>("committed")
        })
        .await
        .expect("transaction should commit");

        assert_eq!(value, "committed");
        assert_eq!(count_rows(&db).await, 1);
    }

    #[tokio::test]
    async fn with_transaction_rolls_back_callback_error() {
        let db = sqlite_db().await;
        db.execute_unprepared("CREATE TABLE transaction_items (id INTEGER PRIMARY KEY);")
            .await
            .expect("table should be created");

        let error = with_transaction(&db, async |txn| {
            txn.execute_unprepared("INSERT INTO transaction_items (id) VALUES (1);")
                .await
                .map_err(DbError::from)?;
            Err::<(), _>(DbError::database_operation("forced failure"))
        })
        .await
        .expect_err("callback error should propagate");

        assert!(matches!(error, DbError::DatabaseOperation(_)));
        assert_eq!(count_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn with_transaction_preserves_product_callback_errors() {
        let db = sqlite_db().await;
        db.execute_unprepared("CREATE TABLE transaction_items (id INTEGER PRIMARY KEY);")
            .await
            .expect("table should be created");

        let error = with_transaction(&db, async |txn| {
            txn.execute_unprepared("INSERT INTO transaction_items (id) VALUES (1);")
                .await
                .map_err(DbError::from)
                .map_err(ProductError::from)?;
            Err::<(), _>(ProductError::Validation("business validation failed"))
        })
        .await
        .expect_err("callback error should propagate");

        assert_eq!(
            error,
            ProductError::Validation("business validation failed")
        );
        assert_eq!(count_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn with_transaction_retry_restarts_after_callback_failure() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let db = sqlite_db().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let config = crate::retry::RetryConfig {
            max_retries: 2,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let result = {
            let attempts = Arc::clone(&attempts);
            super::with_transaction_retry(
                &db,
                &config,
                move |_txn| {
                    let attempts = Arc::clone(&attempts);
                    Box::pin(async move {
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt < 2 {
                            Err(DbError::database_operation("temporary failure"))
                        } else {
                            Ok::<_, DbError>("ok")
                        }
                    })
                },
                |error| matches!(error, DbError::DatabaseOperation(_)),
            )
            .await
        };

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn transaction_delay_doubles_caps_and_handles_boundaries() {
        let config = crate::retry::RetryConfig {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 250,
        };
        assert_eq!(
            transaction_delay(&config, 0),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            transaction_delay(&config, 1),
            std::time::Duration::from_millis(200)
        );
        assert_eq!(
            transaction_delay(&config, 2),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            transaction_delay(&config, u32::MAX),
            std::time::Duration::from_millis(250)
        );

        let zero_max = crate::retry::RetryConfig {
            max_retries: 1,
            base_delay_ms: 100,
            max_delay_ms: 0,
        };
        assert_eq!(transaction_delay(&zero_max, 0), std::time::Duration::ZERO);

        let initial_above_max = crate::retry::RetryConfig {
            max_retries: 1,
            base_delay_ms: 1_000,
            max_delay_ms: 250,
        };
        assert_eq!(
            transaction_delay(&initial_above_max, 0),
            std::time::Duration::from_millis(250)
        );
    }

    #[tokio::test]
    async fn with_transaction_retry_zero_budget_runs_callback_once() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let db = sqlite_db().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let config = crate::retry::RetryConfig {
            max_retries: 0,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let error = {
            let attempts = attempts.clone();
            super::with_transaction_retry(
                &db,
                &config,
                move |_txn| {
                    let attempts = attempts.clone();
                    Box::pin(async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Err::<(), _>(DbError::database_operation("temporary failure"))
                    })
                },
                |error| matches!(error, DbError::DatabaseOperation(_)),
            )
            .await
            .unwrap_err()
        };

        assert!(matches!(error, DbError::DatabaseOperation(_)));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn commit_failure_action_preserves_known_exhausted_deadlocks() {
        assert_eq!(
            commit_failure_action(true, true, 0, 3),
            CommitFailureAction::Retry
        );
        assert_eq!(
            commit_failure_action(true, true, 3, 3),
            CommitFailureAction::ReturnKnownFailure
        );
        assert_eq!(
            commit_failure_action(false, false, 0, 3),
            CommitFailureAction::ReturnOutcomeUnknown
        );
        assert_eq!(
            commit_failure_action(true, false, 0, 3),
            CommitFailureAction::ReturnOutcomeUnknown
        );
        assert_eq!(
            commit_failure_action(false, true, 0, 3),
            CommitFailureAction::ReturnKnownFailure
        );
        assert!(commit_outcome_known_rolled_back(Some(
            crate::DatabaseErrorKind::Deadlock
        )));
        assert!(commit_outcome_known_rolled_back(Some(
            crate::DatabaseErrorKind::SerializationFailure
        )));
        assert!(!commit_outcome_known_rolled_back(Some(
            crate::DatabaseErrorKind::LockTimeout
        )));
    }

    #[tokio::test]
    async fn rollback_helper_discards_pending_changes() {
        let db = sqlite_db().await;
        db.execute_unprepared("CREATE TABLE transaction_items (id INTEGER PRIMARY KEY);")
            .await
            .expect("table should be created");
        let txn = db.begin().await.expect("transaction should begin");
        txn.execute_unprepared("INSERT INTO transaction_items (id) VALUES (1);")
            .await
            .expect("insert should succeed");

        rollback(txn).await.expect("rollback should succeed");

        assert_eq!(count_rows(&db).await, 0);
    }
    #[tokio::test]
    async fn diagnostics_preserve_caller_operation_and_redact_callback_errors() {
        use tracing::Instrument;
        let db = sqlite_db().await;
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let mut actual_line = 0;
        logs.capture_future(async {
            let parent = tracing::trace_span!("product_operation", operation = "quota_claim");
            async {
                actual_line = line!() + 1;
                super::with_transaction(&db, async |_txn| Ok::<_, DbError>(()))
                    .await
                    .unwrap();
                let error = super::with_transaction(&db, async |_txn| {
                    Err::<(), _>(ProductError::Validation("secret-callback-canary"))
                })
                .await
                .unwrap_err();
                assert_eq!(error, ProductError::Validation("secret-callback-canary"));
            }
            .instrument(parent)
            .await;
        })
        .await;
        let text = logs.text();
        assert!(
            text.contains(&format!("caller_line={actual_line}")),
            "{text}"
        );
        for field in [
            "transaction_id=",
            "operation=\"quota_claim\"",
            "phase=\"begin\"",
            "phase=\"callback\"",
            "phase=\"commit\"",
            "outcome=\"committed\"",
            "outcome=\"callback_error_rolled_back\"",
            "elapsed_seconds=",
        ] {
            assert!(text.contains(field), "missing {field}: {text}");
        }
        assert!(!text.contains("secret-callback-canary"), "{text}");
    }

    #[tokio::test]
    async fn dropping_callback_reports_phase_and_rolls_back_without_logging_error() {
        let db = sqlite_db().await;
        let logs = aster_forge_test::logs::CapturedLogs::default();
        logs.capture_future(async {
            let entered = tokio::sync::Notify::new();
            let mut future = Box::pin(super::with_transaction(&db, async |_txn| {
                entered.notify_one();
                std::future::pending::<()>().await;
                Ok::<_, DbError>(())
            }));
            tokio::select! {
                result = &mut future => panic!("unexpected completion: {result:?}"),
                () = entered.notified() => {}
            }
            drop(future);
        })
        .await;
        let text = logs.text();
        assert!(text.contains("phase=\"callback\""), "{text}");
        assert!(text.contains("dropped_outcome_unknown"), "{text}");
        // Dropping must release the only SQLite connection.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::with_transaction(&db, async |_txn| Ok::<_, DbError>(())),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[tokio::test]
    async fn begin_failure_records_boundary_without_raw_database_error() {
        let db = sqlite_db().await;
        db.close_by_ref().await.unwrap();
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let result = logs
            .capture_future(async {
                super::with_transaction(&db, async |_txn| Ok::<_, DbError>(())).await
            })
            .await;
        assert!(result.is_err());
        let text = logs.text();
        assert!(text.contains("outcome=\"begin_error\""), "{text}");
        assert!(!text.contains("Connection pool closed"), "{text}");
    }
    #[tokio::test]
    async fn deferred_constraint_commit_failure_is_not_reported_as_success() {
        let db = sqlite_db().await;
        db.execute_unprepared("PRAGMA foreign_keys = ON; CREATE TABLE parent (id INTEGER PRIMARY KEY); CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);").await.unwrap();
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let result = logs
            .capture_future(async {
                super::with_transaction(&db, async |txn| {
                    txn.execute_unprepared("INSERT INTO child VALUES (1, 99)")
                        .await
                        .map_err(DbError::from)?;
                    Ok::<_, DbError>(())
                })
                .await
            })
            .await;
        assert!(result.is_err());
        let text = logs.text();
        assert!(text.contains("commit_error_outcome_unknown"), "{text}");
        assert!(!text.contains("INSERT INTO"), "{text}");
        assert!(!text.contains("outcome=\"committed\""), "{text}");
    }

    #[tokio::test]
    async fn rollback_failure_preserves_original_product_error() {
        let db = sqlite_db().await;
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let result = logs
            .capture_future(async {
                super::with_transaction(&db, async |txn| {
                    // Deliberately desynchronize SQLite from the ORM to force rollback failure.
                    txn.execute_unprepared("ROLLBACK")
                        .await
                        .map_err(DbError::from)
                        .map_err(ProductError::from)?;
                    Err::<(), _>(ProductError::Validation("secret-callback-canary"))
                })
                .await
            })
            .await;
        assert_eq!(
            result.unwrap_err(),
            ProductError::Validation("secret-callback-canary")
        );
        let text = logs.text();
        assert!(text.contains("callback_error_rollback_failed"), "{text}");
        assert!(!text.contains("secret-callback-canary"), "{text}");
    }
    #[tokio::test]
    async fn retry_begin_failure_records_every_attempt_without_invoking_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let db = sqlite_db().await;
        db.close_by_ref().await.unwrap();
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let predicates = AtomicUsize::new(0);
        let callbacks = AtomicUsize::new(0);
        let config = crate::retry::RetryConfig {
            max_retries: 2,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let result = logs
            .capture_future(async {
                super::with_transaction_retry(
                    &db,
                    &config,
                    |_txn| {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok::<_, DbError>(()) })
                    },
                    |_| {
                        predicates.fetch_add(1, Ordering::SeqCst);
                        true
                    },
                )
                .await
            })
            .await;
        assert!(result.is_err());
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert_eq!(predicates.load(Ordering::SeqCst), 2);
        let text = logs.text();
        for attempt in 0..=2 {
            assert!(
                text.lines()
                    .any(|line| line.contains(&format!("attempt={attempt}"))
                        && line.contains("outcome=\"begin_error\"")),
                "{text}"
            );
        }
    }

    #[tokio::test]
    async fn retry_commit_unknown_is_never_replayed_even_when_policy_accepts_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let db = sqlite_db().await;
        db.execute_unprepared("PRAGMA foreign_keys=ON; CREATE TABLE parent (id INTEGER PRIMARY KEY); CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);").await.unwrap();
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let callbacks = AtomicUsize::new(0);
        let config = crate::retry::RetryConfig {
            max_retries: 2,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let result = logs
            .capture_future(async {
                super::with_transaction_retry(
                    &db,
                    &config,
                    |txn| {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async move {
                            txn.execute_unprepared("INSERT INTO child VALUES (1,99)")
                                .await
                                .map_err(DbError::from)?;
                            Ok::<_, DbError>(())
                        })
                    },
                    |_| true,
                )
                .await
            })
            .await;
        assert!(matches!(result, Err(DbError::CommitOutcomeUnknown { .. })));
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        let text = logs.text();
        assert!(text.contains("commit_error_outcome_unknown"), "{text}");
        assert!(text.contains("attempt=0"), "{text}");
        assert!(!text.contains("INSERT INTO"), "{text}");
    }

    #[tokio::test]
    async fn retry_rollback_failure_retains_callback_error_and_attempt_identity() {
        let db = sqlite_db().await;
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let config = crate::retry::RetryConfig {
            max_retries: 0,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let result = logs
            .capture_future(async {
                super::with_transaction_retry(
                    &db,
                    &config,
                    |txn| {
                        Box::pin(async move {
                            txn.execute_unprepared("ROLLBACK")
                                .await
                                .map_err(DbError::from)
                                .map_err(ProductError::from)?;
                            Err::<(), _>(ProductError::Validation("secret-retry-canary"))
                        })
                    },
                    |_| false,
                )
                .await
            })
            .await;
        assert_eq!(
            result.unwrap_err(),
            ProductError::Validation("secret-retry-canary")
        );
        let text = logs.text();
        assert!(text.contains("callback_error_rollback_failed"), "{text}");
        assert!(text.contains("attempt=0"), "{text}");
        assert!(!text.contains("secret-retry-canary"), "{text}");
    }
}
