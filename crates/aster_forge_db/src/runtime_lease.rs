//! Database-backed runtime lease store.
//!
//! Runtime leases coordinate process-level singleton components across service
//! instances. The table is deliberately small and infrastructure-owned: it
//! records the current owner of a lease key and the timestamp at which another
//! process may take over. Product crates still decide which worker groups are
//! singleton and run their own migrations; this module provides the shared
//! entity, store implementation, and stable table/column names.

use aster_forge_runtime::{
    RuntimeLeaseAcquire, RuntimeLeaseClaim, RuntimeLeaseOwner, RuntimeLeaseStore,
};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{Alias, ColumnDef, Table, TableCreateStatement, TableDropStatement};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseBackend, DatabaseConnection, EntityTrait,
    QueryFilter, Set, sea_query::Expr,
};

use crate::DbError;

/// Runtime lease table name.
pub const RUNTIME_LEASES_TABLE: &str = "runtime_leases";
/// Runtime lease identifier column name.
pub const RUNTIME_LEASE_ID_COLUMN: &str = "lease_id";
/// Runtime lease owner column name.
pub const RUNTIME_LEASE_OWNER_ID_COLUMN: &str = "owner_id";
/// Runtime lease expiry column name.
pub const RUNTIME_LEASE_EXPIRES_AT_COLUMN: &str = "expires_at";
/// Runtime lease last-renewed column name.
pub const RUNTIME_LEASE_LAST_RENEWED_AT_COLUMN: &str = "last_renewed_at";
/// Runtime lease created-at column name.
pub const RUNTIME_LEASE_CREATED_AT_COLUMN: &str = "created_at";
/// Runtime lease updated-at column name.
pub const RUNTIME_LEASE_UPDATED_AT_COLUMN: &str = "updated_at";

/// Builds the shared `runtime_leases` table creation statement.
///
/// Product migration crates should call this helper instead of duplicating the
/// table shape. Forge owns this table contract because [`RuntimeLeaseDbStore`]
/// owns its row semantics and update rules.
pub fn create_runtime_leases_table(backend: DatabaseBackend) -> TableCreateStatement {
    Table::create()
        .table(runtime_leases_table())
        .if_not_exists()
        .col(
            ColumnDef::new(runtime_lease_id())
                .string_len(191)
                .not_null()
                .primary_key(),
        )
        .col(
            ColumnDef::new(runtime_lease_owner_id())
                .string_len(191)
                .not_null(),
        )
        .col(utc_datetime_column(backend, runtime_lease_expires_at()).not_null())
        .col(utc_datetime_column(backend, runtime_lease_last_renewed_at()).not_null())
        .col(utc_datetime_column(backend, runtime_lease_created_at()).not_null())
        .col(utc_datetime_column(backend, runtime_lease_updated_at()).not_null())
        .to_owned()
}

/// Builds the shared `runtime_leases` table drop statement.
pub fn drop_runtime_leases_table() -> TableDropStatement {
    Table::drop()
        .table(runtime_leases_table())
        .if_exists()
        .to_owned()
}

fn runtime_leases_table() -> Alias {
    Alias::new(RUNTIME_LEASES_TABLE)
}

fn runtime_lease_id() -> Alias {
    Alias::new(RUNTIME_LEASE_ID_COLUMN)
}

fn runtime_lease_owner_id() -> Alias {
    Alias::new(RUNTIME_LEASE_OWNER_ID_COLUMN)
}

fn runtime_lease_expires_at() -> Alias {
    Alias::new(RUNTIME_LEASE_EXPIRES_AT_COLUMN)
}

fn runtime_lease_last_renewed_at() -> Alias {
    Alias::new(RUNTIME_LEASE_LAST_RENEWED_AT_COLUMN)
}

fn runtime_lease_created_at() -> Alias {
    Alias::new(RUNTIME_LEASE_CREATED_AT_COLUMN)
}

fn runtime_lease_updated_at() -> Alias {
    Alias::new(RUNTIME_LEASE_UPDATED_AT_COLUMN)
}

fn utc_datetime_column(backend: DatabaseBackend, column: Alias) -> ColumnDef {
    let mut definition = ColumnDef::new(column);
    match backend {
        DatabaseBackend::MySql => {
            definition.custom(Alias::new("datetime(6)"));
        }
        _ => {
            definition.timestamp_with_time_zone();
        }
    }
    definition
}

/// SeaORM model for `runtime_leases`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_leases")]
pub struct Model {
    /// Stable lease key shared by all service instances.
    #[sea_orm(primary_key, auto_increment = false)]
    pub lease_id: String,
    /// Owner identifier stored by the active process.
    pub owner_id: String,
    /// Timestamp after which another owner may take over.
    pub expires_at: DateTimeUtc,
    /// Timestamp of the last successful acquisition or renewal.
    pub last_renewed_at: DateTimeUtc,
    /// Row creation timestamp.
    pub created_at: DateTimeUtc,
    /// Row update timestamp.
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// SeaORM-backed implementation of [`RuntimeLeaseStore`].
#[derive(Clone)]
pub struct RuntimeLeaseDbStore {
    db: DatabaseConnection,
}

impl RuntimeLeaseDbStore {
    /// Creates a runtime lease store from a SeaORM database connection.
    pub const fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Returns the underlying database connection.
    pub const fn db(&self) -> &DatabaseConnection {
        &self.db
    }
}

#[async_trait::async_trait]
impl RuntimeLeaseStore for RuntimeLeaseDbStore {
    type Error = DbError;

    async fn try_acquire(
        &self,
        claim: RuntimeLeaseClaim<'_>,
    ) -> std::result::Result<RuntimeLeaseAcquire, Self::Error> {
        try_insert_lease(&self.db, claim).await
    }

    async fn renew(
        &self,
        lease_id: &str,
        owner_id: &str,
        now: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> std::result::Result<bool, Self::Error> {
        renew_lease(&self.db, lease_id, owner_id, now, expires_at).await
    }

    async fn release(
        &self,
        lease_id: &str,
        owner_id: &str,
    ) -> std::result::Result<(), Self::Error> {
        release_lease(&self.db, lease_id, owner_id).await
    }
}

async fn try_insert_lease(
    db: &DatabaseConnection,
    claim: RuntimeLeaseClaim<'_>,
) -> crate::Result<RuntimeLeaseAcquire> {
    let insert_result = ActiveModel {
        lease_id: Set(claim.lease_id.to_string()),
        owner_id: Set(claim.owner_id.to_string()),
        expires_at: Set(claim.expires_at),
        last_renewed_at: Set(claim.now),
        created_at: Set(claim.now),
        updated_at: Set(claim.now),
    }
    .insert(db)
    .await;

    match insert_result {
        Ok(_) => Ok(RuntimeLeaseAcquire::Acquired),
        Err(insert_error) => acquire_existing_lease(db, claim, insert_error).await,
    }
}

async fn acquire_existing_lease(
    db: &DatabaseConnection,
    claim: RuntimeLeaseClaim<'_>,
    insert_error: sea_orm::DbErr,
) -> crate::Result<RuntimeLeaseAcquire> {
    let existing = Entity::find_by_id(claim.lease_id.to_string())
        .one(db)
        .await
        .map_err(DbError::from)?;
    let Some(existing) = existing else {
        return Err(DbError::from(insert_error));
    };

    if existing.owner_id != claim.owner_id && existing.expires_at > claim.now {
        return Ok(standby_from_model(existing));
    }

    let owner_or_expired = Condition::any()
        .add(Column::OwnerId.eq(claim.owner_id))
        .add(Column::ExpiresAt.lte(claim.now));
    let update = Entity::update_many()
        .col_expr(Column::OwnerId, Expr::value(claim.owner_id.to_string()))
        .col_expr(Column::ExpiresAt, Expr::value(claim.expires_at))
        .col_expr(Column::LastRenewedAt, Expr::value(claim.now))
        .col_expr(Column::UpdatedAt, Expr::value(claim.now))
        .filter(Column::LeaseId.eq(claim.lease_id))
        .filter(owner_or_expired)
        .exec(db)
        .await
        .map_err(DbError::from)?;

    if update.rows_affected == 1 {
        return Ok(RuntimeLeaseAcquire::Acquired);
    }

    Entity::find_by_id(claim.lease_id.to_string())
        .one(db)
        .await
        .map_err(DbError::from)?
        .map_or(Ok(RuntimeLeaseAcquire::Standby { owner: None }), |model| {
            Ok(standby_from_model(model))
        })
}

async fn renew_lease(
    db: &DatabaseConnection,
    lease_id: &str,
    owner_id: &str,
    now: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> crate::Result<bool> {
    let update = Entity::update_many()
        .col_expr(Column::ExpiresAt, Expr::value(expires_at))
        .col_expr(Column::LastRenewedAt, Expr::value(now))
        .col_expr(Column::UpdatedAt, Expr::value(now))
        .filter(Column::LeaseId.eq(lease_id))
        .filter(Column::OwnerId.eq(owner_id))
        .exec(db)
        .await
        .map_err(DbError::from)?;

    Ok(update.rows_affected == 1)
}

async fn release_lease(
    db: &DatabaseConnection,
    lease_id: &str,
    owner_id: &str,
) -> crate::Result<()> {
    Entity::delete_many()
        .filter(Column::LeaseId.eq(lease_id))
        .filter(Column::OwnerId.eq(owner_id))
        .exec(db)
        .await
        .map_err(DbError::from)?;

    Ok(())
}

fn standby_from_model(model: Model) -> RuntimeLeaseAcquire {
    RuntimeLeaseAcquire::Standby {
        owner: Some(RuntimeLeaseOwner {
            owner_id: model.owner_id,
            expires_at: model.expires_at,
        }),
    }
}

#[cfg(test)]
mod tests {
    use aster_forge_runtime::{RuntimeLeaseAcquire, RuntimeLeaseClaim, RuntimeLeaseStore};
    use chrono::Utc;
    use sea_orm::sea_query::{MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Schema};

    use super::{Entity, RuntimeLeaseDbStore, create_runtime_leases_table};

    async fn sqlite_store() -> RuntimeLeaseDbStore {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let schema = Schema::new(db.get_database_backend());
        let statement = schema.create_table_from_entity(Entity);
        db.execute(&statement)
            .await
            .expect("runtime leases table should be created");
        RuntimeLeaseDbStore::new(db)
    }

    fn claim<'a>(
        lease_id: &'a str,
        owner_id: &'a str,
        now: chrono::DateTime<Utc>,
        ttl_secs: i64,
    ) -> RuntimeLeaseClaim<'a> {
        RuntimeLeaseClaim {
            lease_id,
            owner_id,
            now,
            expires_at: now + chrono::Duration::seconds(ttl_secs),
        }
    }

    fn create_table_sql(backend: DatabaseBackend) -> String {
        let table = create_runtime_leases_table(backend);
        match backend {
            DatabaseBackend::MySql => table.to_string(MysqlQueryBuilder),
            DatabaseBackend::Postgres => table.to_string(PostgresQueryBuilder),
            DatabaseBackend::Sqlite => table.to_string(SqliteQueryBuilder),
            _ => unreachable!("unsupported backend in runtime lease table test"),
        }
    }

    #[test]
    fn create_runtime_leases_table_uses_stable_shape() {
        let sqlite_sql = create_table_sql(DatabaseBackend::Sqlite);
        assert!(sqlite_sql.contains("CREATE TABLE IF NOT EXISTS \"runtime_leases\""));
        assert!(sqlite_sql.contains("\"lease_id\" varchar(191) NOT NULL PRIMARY KEY"));
        assert!(sqlite_sql.contains("\"owner_id\" varchar(191) NOT NULL"));
        assert!(sqlite_sql.contains("\"expires_at\" timestamp_with_timezone_text NOT NULL"));

        let mysql_sql = create_table_sql(DatabaseBackend::MySql);
        assert!(mysql_sql.contains("`expires_at` datetime(6) NOT NULL"));

        let postgres_sql = create_table_sql(DatabaseBackend::Postgres);
        assert!(postgres_sql.contains("\"expires_at\" timestamp with time zone NOT NULL"));
    }

    #[tokio::test]
    async fn acquiring_new_lease_inserts_owner() {
        let store = sqlite_store().await;
        let now = Utc::now();

        let result = store
            .try_acquire(claim("aster.test", "node-a", now, 30))
            .await
            .expect("acquire should succeed");

        assert_eq!(result, RuntimeLeaseAcquire::Acquired);
    }

    #[tokio::test]
    async fn held_unexpired_lease_returns_standby_owner() {
        let store = sqlite_store().await;
        let now = Utc::now();
        store
            .try_acquire(claim("aster.test", "node-a", now, 30))
            .await
            .expect("initial acquire should succeed");

        let result = store
            .try_acquire(claim("aster.test", "node-b", now, 30))
            .await
            .expect("standby acquire should succeed");

        match result {
            RuntimeLeaseAcquire::Standby { owner: Some(owner) } => {
                assert_eq!(owner.owner_id, "node-a");
            }
            other => panic!("expected standby owner, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn expired_lease_can_be_acquired_by_new_owner() {
        let store = sqlite_store().await;
        let now = Utc::now();
        store
            .try_acquire(claim("aster.test", "node-a", now, 1))
            .await
            .expect("initial acquire should succeed");

        let result = store
            .try_acquire(claim(
                "aster.test",
                "node-b",
                now + chrono::Duration::seconds(2),
                30,
            ))
            .await
            .expect("expired acquire should succeed");

        assert_eq!(result, RuntimeLeaseAcquire::Acquired);
    }

    #[tokio::test]
    async fn same_owner_reacquire_renews_lease() {
        let store = sqlite_store().await;
        let now = Utc::now();
        store
            .try_acquire(claim("aster.test", "node-a", now, 1))
            .await
            .expect("initial acquire should succeed");

        let result = store
            .try_acquire(claim(
                "aster.test",
                "node-a",
                now + chrono::Duration::seconds(1),
                30,
            ))
            .await
            .expect("same owner acquire should succeed");

        assert_eq!(result, RuntimeLeaseAcquire::Acquired);
    }

    #[tokio::test]
    async fn renew_requires_matching_owner() {
        let store = sqlite_store().await;
        let now = Utc::now();
        store
            .try_acquire(claim("aster.test", "node-a", now, 30))
            .await
            .expect("initial acquire should succeed");

        let renewed = store
            .renew(
                "aster.test",
                "node-b",
                now + chrono::Duration::seconds(1),
                now + chrono::Duration::seconds(31),
            )
            .await
            .expect("renew should query");

        assert!(!renewed);
    }

    #[tokio::test]
    async fn release_requires_matching_owner() {
        let store = sqlite_store().await;
        let now = Utc::now();
        store
            .try_acquire(claim("aster.test", "node-a", now, 30))
            .await
            .expect("initial acquire should succeed");
        store
            .release("aster.test", "node-b")
            .await
            .expect("wrong owner release should be ignored");

        let result = store
            .try_acquire(claim("aster.test", "node-c", now, 30))
            .await
            .expect("standby acquire should succeed");

        assert!(matches!(result, RuntimeLeaseAcquire::Standby { .. }));
    }
}
