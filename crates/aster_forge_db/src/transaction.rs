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

struct RollbackGuard {
    file: &'static str,
    line: u32,
    armed: bool,
}

impl RollbackGuard {
    fn new(location: &'static Location<'static>) -> Self {
        Self {
            file: location.file(),
            line: location.line(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RollbackGuard {
    fn drop(&mut self) {
        if self.armed {
            tracing::warn!(
                file = self.file,
                line = self.line,
                "transaction dropped before explicit commit/rollback; relying on rollback-on-drop"
            );
        }
    }
}

/// Begins and returns a transaction so the caller can commit or roll it back.
///
/// This centralizes `begin` error mapping.
pub async fn begin<C: sea_orm::TransactionTrait>(db: &C) -> Result<C::Transaction> {
    db.begin()
        .await
        .map_err(|error| database_operation_with_context(error, "begin transaction"))
}

/// Commits a transaction and maps errors consistently.
pub async fn commit<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.commit()
        .await
        .map_err(|error| database_operation_with_context(error, "commit transaction"))
}

/// Rolls back a transaction and maps errors consistently.
pub async fn rollback<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.rollback()
        .await
        .map_err(|error| database_operation_with_context(error, "rollback transaction"))
}

fn database_operation_with_context(error: sea_orm::DbErr, context: &str) -> DbError {
    let kind = database_error_kind(&error);
    let message = format!("{context}: {error}");
    match kind {
        Some(kind) => DbError::database_operation_classified(message, kind),
        None => DbError::database_operation(message),
    }
}

/// Bounded transaction retry settings. Products choose which classified errors are retryable.
#[derive(Clone, Debug)]
pub struct TransactionRetryConfig {
    /// Maximum number of retries after the initial transaction attempt.
    pub max_retries: u32,
    /// Exponential backoff base in milliseconds.
    pub base_delay_ms: u64,
    /// Maximum backoff delay in milliseconds.
    pub max_delay_ms: u64,
}

impl Default for TransactionRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 5,
            max_delay_ms: 50,
        }
    }
}

fn transaction_delay(config: &TransactionRetryConfig, attempt: u32) -> Duration {
    let multiplier = 1_u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_millis(
        config
            .base_delay_ms
            .saturating_mul(multiplier)
            .min(config.max_delay_ms),
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
pub async fn with_transaction_retry<C, F, T, E, P>(
    db: &C,
    config: &TransactionRetryConfig,
    mut operation: F,
    should_retry: P,
) -> std::result::Result<T, E>
where
    C: sea_orm::TransactionTrait,
    F: for<'txn> FnMut(
        &'txn C::Transaction,
    )
        -> Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'txn>>,
    E: From<DbError> + std::fmt::Display,
    P: Fn(&E) -> bool,
{
    let mut attempt = 0;
    loop {
        let txn = match db.begin().await {
            Ok(txn) => txn,
            Err(error) => {
                let error = E::from(database_operation_with_context(error, "begin transaction"));
                if attempt < config.max_retries && should_retry(&error) {
                    tokio::time::sleep(transaction_delay(config, attempt)).await;
                    attempt += 1;
                    continue;
                }
                return Err(error);
            }
        };

        match operation(&txn).await {
            Ok(value) => match txn.commit().await {
                Ok(()) => return Ok(value),
                Err(error) => {
                    let kind = database_error_kind(&error);
                    let classified_error = E::from(match kind {
                        Some(kind) => DbError::database_operation_classified(
                            format!("commit transaction: {error}"),
                            kind,
                        ),
                        None => DbError::database_operation(format!("commit transaction: {error}")),
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
                            continue;
                        }
                        CommitFailureAction::ReturnKnownFailure => return Err(classified_error),
                        CommitFailureAction::ReturnOutcomeUnknown => {
                            return Err(E::from(DbError::commit_outcome_unknown(
                                format!("commit transaction: {error}"),
                                kind,
                            )));
                        }
                    }
                }
            },
            Err(error) => {
                if let Err(rollback_error) = txn.rollback().await {
                    tracing::warn!(
                        callback_error = %error,
                        rollback_error = %rollback_error,
                        "transaction rollback failed after callback error"
                    );
                }
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

/// Runs a transaction callback with consistent tracing and rollback guarding.
///
/// The callback may return a product-specific error type. Forge-created transaction boundary
/// errors are converted through `E: From<DbError>`, while callback errors are preserved unchanged.
pub async fn with_transaction<C, F, T, E>(db: &C, operation: F) -> std::result::Result<T, E>
where
    C: sea_orm::TransactionTrait,
    F: for<'txn> AsyncFnOnce(&'txn C::Transaction) -> std::result::Result<T, E>,
    E: From<DbError> + std::fmt::Display,
{
    let location = Location::caller();
    tracing::debug!(
        file = location.file(),
        line = location.line(),
        "beginning transaction"
    );
    let txn = begin(db).await.map_err(E::from)?;
    let mut rollback_guard = RollbackGuard::new(location);

    match operation(&txn).await {
        Ok(value) => {
            rollback_guard.disarm();
            commit(txn).await.map_err(E::from)?;
            tracing::debug!(
                file = location.file(),
                line = location.line(),
                "committed transaction"
            );
            Ok(value)
        }
        Err(error) => {
            tracing::debug!(
                file = location.file(),
                line = location.line(),
                error = %error,
                "rolling back transaction after callback error"
            );
            rollback_guard.disarm();
            if let Err(rollback_error) = rollback(txn).await {
                tracing::error!(
                    file = location.file(),
                    line = location.line(),
                    callback_error = %error,
                    rollback_error = %rollback_error,
                    "transaction rollback failed after callback error"
                );
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitFailureAction, commit_failure_action, commit_outcome_known_rolled_back, rollback,
        with_transaction,
    };
    use crate::{DbError, connection::DatabaseConfig};
    use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait};
    use std::fmt;

    async fn sqlite_db() -> DatabaseConnection {
        crate::connection::connect(&DatabaseConfig {
            url: "sqlite::memory:".to_string(),
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
        let config = super::TransactionRetryConfig {
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
}
