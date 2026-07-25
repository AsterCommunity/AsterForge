use std::future::Future;
use std::pin::Pin;

use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, Statement,
    TransactionTrait,
};

const DEFAULT_MYSQL_LOCK_TIMEOUT_SECONDS: u64 = 300;
const MYSQL_LOCK_NAME_MAX_BYTES: usize = 64;

/// Boxed migration callback future tied to the coordinated database transaction.
pub type MigrationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DbErr>> + Send + 'a>>;

/// Stable cross-process migration lock configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationLockOptions {
    namespace: String,
    postgres_advisory_key: i64,
    mysql_timeout_seconds: u64,
}

impl MigrationLockOptions {
    /// Creates options using a deterministic PostgreSQL advisory key derived from `namespace`.
    ///
    /// Products migrating from an existing lock implementation should call
    /// [`Self::with_postgres_advisory_key`] to preserve the old key during rolling upgrades.
    pub fn new(namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        Self {
            postgres_advisory_key: stable_advisory_key(&namespace),
            namespace,
            mysql_timeout_seconds: DEFAULT_MYSQL_LOCK_TIMEOUT_SECONDS,
        }
    }

    /// Overrides the PostgreSQL advisory key while preserving the shared namespace for MySQL.
    pub const fn with_postgres_advisory_key(mut self, key: i64) -> Self {
        self.postgres_advisory_key = key;
        self
    }

    /// Overrides the MySQL named-lock wait timeout in whole seconds.
    pub const fn with_mysql_timeout_seconds(mut self, seconds: u64) -> Self {
        self.mysql_timeout_seconds = seconds;
        self
    }

    /// Returns the stable product namespace used for MySQL named locks.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the PostgreSQL transaction-scoped advisory-lock key.
    pub const fn postgres_advisory_key(&self) -> i64 {
        self.postgres_advisory_key
    }

    /// Returns the MySQL named-lock wait timeout in seconds.
    pub const fn mysql_timeout_seconds(&self) -> u64 {
        self.mysql_timeout_seconds
    }

    fn validate(&self) -> Result<(), DbErr> {
        if self.namespace.is_empty() || self.namespace.len() > MYSQL_LOCK_NAME_MAX_BYTES {
            return Err(DbErr::Custom(format!(
                "migration lock namespace must contain 1-{MYSQL_LOCK_NAME_MAX_BYTES} bytes"
            )));
        }
        if self.namespace.contains('\0') {
            return Err(DbErr::Custom(
                "migration lock namespace must not contain NUL bytes".to_string(),
            ));
        }
        i64::try_from(self.mysql_timeout_seconds).map_err(|_| {
            DbErr::Custom("MySQL migration lock timeout exceeds signed 64-bit range".to_string())
        })?;
        Ok(())
    }
}

/// Runs a product migration callback while holding the backend's process-wide migration lock.
///
/// PostgreSQL uses a transaction-scoped advisory lock. MySQL uses a connection-bound named lock
/// and releases it before the transaction is committed. SQLite runs the callback in a transaction
/// without an additional external lock.
pub async fn with_migration_lock<T, F>(
    database: &DatabaseConnection,
    options: &MigrationLockOptions,
    operation: F,
) -> Result<T, DbErr>
where
    F: for<'a> FnOnce(&'a DatabaseTransaction) -> MigrationFuture<'a, T>,
{
    options.validate()?;
    let backend = database.get_database_backend();
    let transaction = database.begin().await?;
    match backend {
        DbBackend::Postgres => acquire_postgres_lock(&transaction, options).await?,
        DbBackend::MySql => acquire_mysql_lock(&transaction, options).await?,
        DbBackend::Sqlite => {}
        _ => {
            return Err(DbErr::Custom(
                "unsupported database backend for migration coordination".to_string(),
            ));
        }
    }

    let operation_result = operation(&transaction).await;
    let release_result = if backend == DbBackend::MySql {
        release_mysql_lock(&transaction, options).await
    } else {
        Ok(())
    };

    match (operation_result, release_result) {
        (Ok(value), Ok(())) => {
            transaction.commit().await?;
            Ok(value)
        }
        (Err(error), Ok(())) => rollback_preserving_error(transaction, error).await,
        (Ok(_), Err(release_error)) => {
            rollback_after_infrastructure_error(transaction, &release_error).await;
            Err(release_error)
        }
        (Err(operation_error), Err(release_error)) => {
            let combined = DbErr::Custom(format!(
                "migration operation failed: {operation_error}; additionally failed to release \
                 the MySQL migration lock: {release_error}"
            ));
            rollback_after_infrastructure_error(transaction, &combined).await;
            Err(combined)
        }
    }
}

/// Runs a standard SeaORM migrator while holding the backend migration lock.
pub async fn run_migrator_with_lock<M>(
    database: &DatabaseConnection,
    options: &MigrationLockOptions,
    steps: Option<u32>,
) -> Result<(), DbErr>
where
    M: MigratorTrait + 'static,
{
    with_migration_lock(database, options, |transaction| {
        Box::pin(M::up(transaction, steps))
    })
    .await
}

async fn acquire_postgres_lock(
    transaction: &DatabaseTransaction,
    options: &MigrationLockOptions,
) -> Result<(), DbErr> {
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [options.postgres_advisory_key.into()],
        ))
        .await?;
    Ok(())
}

async fn acquire_mysql_lock(
    transaction: &DatabaseTransaction,
    options: &MigrationLockOptions,
) -> Result<(), DbErr> {
    let timeout = i64::try_from(options.mysql_timeout_seconds).map_err(|_| {
        DbErr::Custom("MySQL migration lock timeout exceeds signed 64-bit range".to_string())
    })?;
    let acquired = mysql_lock_query_result(
        transaction,
        "SELECT GET_LOCK(?, ?)",
        [options.namespace.clone().into(), timeout.into()],
        "acquire",
    )
    .await?;
    if acquired {
        Ok(())
    } else {
        Err(DbErr::Custom(format!(
            "timed out after {} seconds waiting for MySQL migration lock '{}'",
            options.mysql_timeout_seconds, options.namespace
        )))
    }
}

async fn release_mysql_lock(
    transaction: &DatabaseTransaction,
    options: &MigrationLockOptions,
) -> Result<(), DbErr> {
    let released = mysql_lock_query_result(
        transaction,
        "SELECT RELEASE_LOCK(?)",
        [options.namespace.clone().into()],
        "release",
    )
    .await?;
    if released {
        Ok(())
    } else {
        Err(DbErr::Custom(format!(
            "MySQL migration lock '{}' was not owned by the migration connection",
            options.namespace
        )))
    }
}

async fn mysql_lock_query_result<const N: usize>(
    transaction: &DatabaseTransaction,
    sql: &str,
    values: [sea_orm_migration::sea_orm::Value; N],
    operation: &str,
) -> Result<bool, DbErr> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await?
        .ok_or_else(|| {
            DbErr::Custom(format!(
                "MySQL migration lock {operation} query returned no rows"
            ))
        })?;

    if let Ok(value) = row.try_get_by_index::<Option<i64>>(0) {
        return value.map(|value| value == 1).ok_or_else(|| {
            DbErr::Custom(format!(
                "MySQL migration lock {operation} query returned NULL"
            ))
        });
    }
    if let Ok(value) = row.try_get_by_index::<Option<i32>>(0) {
        return value.map(|value| value == 1).ok_or_else(|| {
            DbErr::Custom(format!(
                "MySQL migration lock {operation} query returned NULL"
            ))
        });
    }

    Err(DbErr::Custom(format!(
        "failed to decode MySQL migration lock {operation} result"
    )))
}

async fn rollback_preserving_error<T>(
    transaction: DatabaseTransaction,
    error: DbErr,
) -> Result<T, DbErr> {
    if let Err(rollback_error) = transaction.rollback().await {
        tracing::warn!(%rollback_error, "failed to rollback migration transaction after callback error");
    }
    Err(error)
}

async fn rollback_after_infrastructure_error(transaction: DatabaseTransaction, error: &DbErr) {
    if let Err(rollback_error) = transaction.rollback().await {
        tracing::warn!(%error, %rollback_error, "failed to rollback migration transaction");
    }
}

fn stable_advisory_key(namespace: &str) -> i64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let hash = namespace.bytes().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    });
    i64::from_be_bytes(hash.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DbErr};

    use super::{MigrationLockOptions, stable_advisory_key, with_migration_lock};

    #[test]
    fn advisory_key_is_stable_and_namespace_sensitive() {
        assert_eq!(
            stable_advisory_key("aster_drive:database_migrations"),
            stable_advisory_key("aster_drive:database_migrations")
        );
        assert_ne!(
            stable_advisory_key("aster_drive:database_migrations"),
            stable_advisory_key("aster_yggdrasil:database_migrations")
        );
    }

    #[test]
    fn migration_lock_options_validate_namespace_boundaries() {
        assert!(MigrationLockOptions::new("a").validate().is_ok());
        assert!(MigrationLockOptions::new("a".repeat(64)).validate().is_ok());
        assert!(MigrationLockOptions::new("").validate().is_err());
        assert!(
            MigrationLockOptions::new("a".repeat(65))
                .validate()
                .is_err()
        );
        assert!(MigrationLockOptions::new("bad\0name").validate().is_err());
    }

    #[tokio::test]
    async fn sqlite_callback_commits_on_success() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        with_migration_lock(
            &database,
            &MigrationLockOptions::new("sqlite-success"),
            |transaction| {
                Box::pin(async move {
                    transaction
                        .execute_unprepared("CREATE TABLE example (id INTEGER PRIMARY KEY)")
                        .await?;
                    Ok(())
                })
            },
        )
        .await
        .unwrap();

        database
            .execute_unprepared("INSERT INTO example (id) VALUES (1)")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sqlite_callback_error_rolls_back_and_is_preserved() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        let error = with_migration_lock(
            &database,
            &MigrationLockOptions::new("sqlite-error"),
            |transaction| {
                Box::pin(async move {
                    transaction
                        .execute_unprepared("CREATE TABLE rolled_back (id INTEGER PRIMARY KEY)")
                        .await?;
                    Err::<(), _>(DbErr::Custom("product migration failed".to_string()))
                })
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "Custom Error: product migration failed");
        assert!(
            database
                .execute_unprepared("INSERT INTO rolled_back (id) VALUES (1)")
                .await
                .is_err()
        );
    }
}
