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

use crate::{DbError, Result};
use std::ops::AsyncFnOnce;
use std::panic::Location;

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
        .map_err(|error| DbError::database_operation(format!("begin transaction: {error}")))
}

/// Commits a transaction and maps errors consistently.
pub async fn commit<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.commit()
        .await
        .map_err(|error| DbError::database_operation(format!("commit transaction: {error}")))
}

/// Rolls back a transaction and maps errors consistently.
pub async fn rollback<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.rollback()
        .await
        .map_err(|error| DbError::database_operation(format!("rollback transaction: {error}")))
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
    use super::{rollback, with_transaction};
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
