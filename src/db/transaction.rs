//! 事务封装 helper。
//!
//! ## 模式说明
//!
//! 当前 service 层事务的标准模式：
//! ```ignore
//! transaction::with_transaction(db, async |txn| {
//!     repo::operation(txn, ...).await?;
//!     repo::another_operation(txn, ...).await?;
//!     Ok(())
//! })
//! .await?;
//! ```

use crate::errors::{AsterError, MapAsterErr, Result};
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

/// Begin 并返回事务，供调用方处理 commit/rollback。
///
/// 用途：统一 `begin` 的错误映射。
pub async fn begin<C: sea_orm::TransactionTrait>(db: &C) -> Result<C::Transaction> {
    db.begin()
        .await
        .map_aster_err_ctx("begin transaction", AsterError::database_operation)
}

/// Commit 事务并统一错误映射。
pub async fn commit<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.commit()
        .await
        .map_aster_err_ctx("commit transaction", AsterError::database_operation)
}

/// Rollback 事务并统一错误映射。
pub async fn rollback<T: sea_orm::TransactionSession>(txn: T) -> Result<()> {
    txn.rollback()
        .await
        .map_aster_err_ctx("rollback transaction", AsterError::database_operation)
}

/// 在统一 tracing / rollback 守卫下执行事务闭包。
pub async fn with_transaction<C, F, T>(db: &C, operation: F) -> Result<T>
where
    C: sea_orm::TransactionTrait,
    F: for<'txn> AsyncFnOnce(&'txn C::Transaction) -> Result<T>,
{
    let location = Location::caller();
    tracing::debug!(
        file = location.file(),
        line = location.line(),
        "beginning transaction"
    );
    let txn = begin(db).await?;
    let mut rollback_guard = RollbackGuard::new(location);

    match operation(&txn).await {
        Ok(value) => {
            rollback_guard.disarm();
            commit(txn).await?;
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
    use super::{begin, commit, rollback, with_transaction};
    use crate::config::DatabaseConfig;
    use crate::db::repository::system_config_repo;
    use crate::errors::{AsterError, Result};
    use crate::metrics_core::NoopMetrics;

    async fn setup_db() -> sea_orm::DatabaseConnection {
        let db = crate::db::connect_with_metrics(
            &DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            NoopMetrics::arc(),
        )
        .await
        .unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        db
    }

    #[tokio::test]
    async fn with_transaction_commits_callback_changes() {
        let db = setup_db().await;

        let value = with_transaction(&db, async |txn| {
            system_config_repo::upsert_with_actor(txn, "tx_commit_key", "committed", None).await?;
            Ok::<_, AsterError>("callback-result")
        })
        .await
        .unwrap();

        assert_eq!(value, "callback-result");
        let saved = system_config_repo::find_by_key(&db, "tx_commit_key")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.value, "committed");

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn with_transaction_rolls_back_callback_errors() {
        let db = setup_db().await;

        let error = with_transaction(&db, async |txn| -> Result<()> {
            system_config_repo::upsert_with_actor(txn, "tx_rollback_key", "temporary", None)
                .await?;
            Err(AsterError::validation_error("stop transaction"))
        })
        .await
        .unwrap_err();

        assert!(matches!(error, AsterError::ValidationError(_)));
        assert!(
            system_config_repo::find_by_key(&db, "tx_rollback_key")
                .await
                .unwrap()
                .is_none()
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manual_begin_commit_persists_changes() {
        let db = setup_db().await;

        let txn = begin(&db).await.unwrap();
        system_config_repo::upsert_with_actor(&txn, "tx_manual_commit_key", "committed", None)
            .await
            .unwrap();
        commit(txn).await.unwrap();

        let saved = system_config_repo::find_by_key(&db, "tx_manual_commit_key")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.value, "committed");

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manual_begin_rollback_discards_changes() {
        let db = setup_db().await;

        let txn = begin(&db).await.unwrap();
        system_config_repo::upsert_with_actor(&txn, "tx_manual_rollback_key", "temporary", None)
            .await
            .unwrap();
        rollback(txn).await.unwrap();

        assert!(
            system_config_repo::find_by_key(&db, "tx_manual_rollback_key")
                .await
                .unwrap()
                .is_none()
        );

        db.close().await.unwrap();
    }
}
