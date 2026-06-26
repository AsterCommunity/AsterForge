//! Database-backed scheduled task catalog and claim store.
//!
//! Scheduled task rows persist product runtime schedules across process restarts and coordinate
//! due-work claims across service instances. `aster_forge_tasks` owns the public scheduling DTOs
//! and runner trait; this module only supplies the SeaORM table contract and store implementation.
//! Product crates still own task names, intervals, execution bodies, and outcome records.

use std::time::Duration;

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{
    Alias, ColumnDef, Index, IndexCreateStatement, IndexDropStatement, Table, TableCreateStatement,
    TableDropStatement,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseBackend, DatabaseConnection, EntityTrait,
    QueryFilter, Set, sea_query::Expr,
};

use crate::DbError;
use aster_forge_tasks::{
    ScheduledTaskCatalogEntry, ScheduledTaskClaim, ScheduledTaskClaimRequest,
    ScheduledTaskCompletion,
};

/// Scheduled task table name.
pub const SCHEDULED_TASKS_TABLE: &str = "scheduled_tasks";
/// Stable row identifier column.
pub const SCHEDULED_TASK_ID_COLUMN: &str = "task_id";
/// Product namespace column.
pub const SCHEDULED_TASK_NAMESPACE_COLUMN: &str = "namespace";
/// Product task name column.
pub const SCHEDULED_TASK_NAME_COLUMN: &str = "task_name";
/// Operator-facing display name column.
pub const SCHEDULED_TASK_DISPLAY_NAME_COLUMN: &str = "display_name";
/// Next due timestamp column.
pub const SCHEDULED_TASK_NEXT_RUN_AT_COLUMN: &str = "next_run_at";
/// Current claim owner column.
pub const SCHEDULED_TASK_CLAIM_OWNER_ID_COLUMN: &str = "claim_owner_id";
/// Current claim expiry column.
pub const SCHEDULED_TASK_CLAIM_EXPIRES_AT_COLUMN: &str = "claim_expires_at";
/// Last claim timestamp column.
pub const SCHEDULED_TASK_LAST_CLAIMED_AT_COLUMN: &str = "last_claimed_at";
/// Last completion timestamp column.
pub const SCHEDULED_TASK_LAST_FINISHED_AT_COLUMN: &str = "last_finished_at";
/// Row creation timestamp column.
pub const SCHEDULED_TASK_CREATED_AT_COLUMN: &str = "created_at";
/// Row update timestamp column.
pub const SCHEDULED_TASK_UPDATED_AT_COLUMN: &str = "updated_at";

const SCHEDULED_TASK_ID_MAX_LEN: usize = 191;
const SCHEDULED_TASK_NAMESPACE_MAX_LEN: usize = 64;
const SCHEDULED_TASK_NAME_MAX_LEN: usize = 128;
const SCHEDULED_TASK_DISPLAY_NAME_MAX_LEN: usize = 191;
const SCHEDULED_TASK_OWNER_ID_MAX_LEN: usize = 191;

/// Builds the shared `scheduled_tasks` table creation statement.
pub fn create_scheduled_tasks_table(backend: DatabaseBackend) -> TableCreateStatement {
    Table::create()
        .table(scheduled_tasks_table())
        .if_not_exists()
        .col(
            ColumnDef::new(scheduled_task_id())
                .string_len(191)
                .not_null()
                .primary_key(),
        )
        .col(
            ColumnDef::new(scheduled_task_namespace())
                .string_len(64)
                .not_null(),
        )
        .col(
            ColumnDef::new(scheduled_task_name())
                .string_len(128)
                .not_null(),
        )
        .col(
            ColumnDef::new(scheduled_task_display_name())
                .string_len(191)
                .not_null(),
        )
        .col(utc_datetime_column(backend, scheduled_task_next_run_at()).not_null())
        .col(
            ColumnDef::new(scheduled_task_claim_owner_id())
                .string_len(191)
                .null(),
        )
        .col(utc_datetime_column(backend, scheduled_task_claim_expires_at()).null())
        .col(utc_datetime_column(backend, scheduled_task_last_claimed_at()).null())
        .col(utc_datetime_column(backend, scheduled_task_last_finished_at()).null())
        .col(utc_datetime_column(backend, scheduled_task_created_at()).not_null())
        .col(utc_datetime_column(backend, scheduled_task_updated_at()).not_null())
        .to_owned()
}

/// Builds the shared `scheduled_tasks` table drop statement.
pub fn drop_scheduled_tasks_table() -> TableDropStatement {
    Table::drop()
        .table(scheduled_tasks_table())
        .if_exists()
        .to_owned()
}

/// Builds the unique index for one scheduled task per namespace/name pair.
pub fn create_scheduled_tasks_namespace_name_unique_index() -> IndexCreateStatement {
    Index::create()
        .name("idx_scheduled_tasks_namespace_name_unique")
        .table(scheduled_tasks_table())
        .col(scheduled_task_namespace())
        .col(scheduled_task_name())
        .unique()
        .if_not_exists()
        .to_owned()
}

/// Builds the due-time index used by scheduled task claim checks.
pub fn create_scheduled_tasks_next_run_index() -> IndexCreateStatement {
    Index::create()
        .name("idx_scheduled_tasks_next_run")
        .table(scheduled_tasks_table())
        .col(scheduled_task_next_run_at())
        .if_not_exists()
        .to_owned()
}

/// Builds the scheduled task namespace/name index drop statement.
pub fn drop_scheduled_tasks_namespace_name_unique_index() -> IndexDropStatement {
    Index::drop()
        .name("idx_scheduled_tasks_namespace_name_unique")
        .table(scheduled_tasks_table())
        .if_exists()
        .to_owned()
}

/// Builds the scheduled task due-time index drop statement.
pub fn drop_scheduled_tasks_next_run_index() -> IndexDropStatement {
    Index::drop()
        .name("idx_scheduled_tasks_next_run")
        .table(scheduled_tasks_table())
        .if_exists()
        .to_owned()
}

fn scheduled_tasks_table() -> Alias {
    Alias::new(SCHEDULED_TASKS_TABLE)
}

fn scheduled_task_id() -> Alias {
    Alias::new(SCHEDULED_TASK_ID_COLUMN)
}

fn scheduled_task_namespace() -> Alias {
    Alias::new(SCHEDULED_TASK_NAMESPACE_COLUMN)
}

fn scheduled_task_name() -> Alias {
    Alias::new(SCHEDULED_TASK_NAME_COLUMN)
}

fn scheduled_task_display_name() -> Alias {
    Alias::new(SCHEDULED_TASK_DISPLAY_NAME_COLUMN)
}

fn scheduled_task_next_run_at() -> Alias {
    Alias::new(SCHEDULED_TASK_NEXT_RUN_AT_COLUMN)
}

fn scheduled_task_claim_owner_id() -> Alias {
    Alias::new(SCHEDULED_TASK_CLAIM_OWNER_ID_COLUMN)
}

fn scheduled_task_claim_expires_at() -> Alias {
    Alias::new(SCHEDULED_TASK_CLAIM_EXPIRES_AT_COLUMN)
}

fn scheduled_task_last_claimed_at() -> Alias {
    Alias::new(SCHEDULED_TASK_LAST_CLAIMED_AT_COLUMN)
}

fn scheduled_task_last_finished_at() -> Alias {
    Alias::new(SCHEDULED_TASK_LAST_FINISHED_AT_COLUMN)
}

fn scheduled_task_created_at() -> Alias {
    Alias::new(SCHEDULED_TASK_CREATED_AT_COLUMN)
}

fn scheduled_task_updated_at() -> Alias {
    Alias::new(SCHEDULED_TASK_UPDATED_AT_COLUMN)
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

/// SeaORM model for `scheduled_tasks`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "scheduled_tasks")]
pub struct Model {
    /// Stable row identifier built from namespace and task name.
    #[sea_orm(primary_key, auto_increment = false)]
    pub task_id: String,
    /// Product namespace.
    pub namespace: String,
    /// Stable product task name.
    pub task_name: String,
    /// Operator-facing display name.
    pub display_name: String,
    /// Next due timestamp.
    pub next_run_at: DateTimeUtc,
    /// Runtime owner currently claiming this due run.
    pub claim_owner_id: Option<String>,
    /// Timestamp after which another runtime may reclaim this due run.
    pub claim_expires_at: Option<DateTimeUtc>,
    /// Timestamp of the last successful claim.
    pub last_claimed_at: Option<DateTimeUtc>,
    /// Timestamp of the last successful completion.
    pub last_finished_at: Option<DateTimeUtc>,
    /// Row creation timestamp.
    pub created_at: DateTimeUtc,
    /// Row update timestamp.
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// SeaORM-backed scheduled task store.
#[derive(Clone)]
pub struct ScheduledTaskDbStore {
    db: DatabaseConnection,
}

#[async_trait::async_trait]
impl aster_forge_tasks::ScheduledTaskStore for ScheduledTaskDbStore {
    type Error = DbError;

    async fn ensure_scheduled_task(
        &self,
        entry: ScheduledTaskCatalogEntry<'_>,
    ) -> std::result::Result<(), Self::Error> {
        self.ensure_task(entry).await.map(|_| ())
    }

    async fn claim_scheduled_task(
        &self,
        request: ScheduledTaskClaimRequest<'_>,
    ) -> std::result::Result<Option<ScheduledTaskClaim>, Self::Error> {
        self.claim_due(request).await
    }

    async fn complete_scheduled_task(
        &self,
        completion: ScheduledTaskCompletion,
    ) -> std::result::Result<bool, Self::Error> {
        self.complete_claim(completion).await
    }
}

impl ScheduledTaskDbStore {
    /// Creates a scheduled task store from a SeaORM database connection.
    pub const fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Ensures one product scheduled task is present in the catalog.
    pub async fn ensure_task(&self, entry: ScheduledTaskCatalogEntry<'_>) -> crate::Result<Model> {
        ensure_task(&self.db, entry).await
    }

    /// Attempts to claim one due scheduled task firing.
    pub async fn claim_due(
        &self,
        request: ScheduledTaskClaimRequest<'_>,
    ) -> crate::Result<Option<ScheduledTaskClaim>> {
        claim_due(&self.db, request).await
    }

    /// Completes a claimed firing and advances the next due timestamp.
    pub async fn complete_claim(&self, completion: ScheduledTaskCompletion) -> crate::Result<bool> {
        complete_claim(&self.db, completion).await
    }
}

async fn ensure_task(
    db: &DatabaseConnection,
    entry: ScheduledTaskCatalogEntry<'_>,
) -> crate::Result<Model> {
    validate_catalog_entry(entry)?;
    let task_id = scheduled_task_row_id(entry.namespace, entry.task_name)?;
    let insert_result = ActiveModel {
        task_id: Set(task_id.clone()),
        namespace: Set(entry.namespace.to_string()),
        task_name: Set(entry.task_name.to_string()),
        display_name: Set(entry.display_name.to_string()),
        next_run_at: Set(entry.first_run_at),
        claim_owner_id: Set(None),
        claim_expires_at: Set(None),
        last_claimed_at: Set(None),
        last_finished_at: Set(None),
        created_at: Set(entry.first_run_at),
        updated_at: Set(entry.first_run_at),
    }
    .insert(db)
    .await;

    match insert_result {
        Ok(model) => Ok(model),
        Err(insert_error) => refresh_existing_task(db, task_id, entry, insert_error).await,
    }
}

async fn refresh_existing_task(
    db: &DatabaseConnection,
    task_id: String,
    entry: ScheduledTaskCatalogEntry<'_>,
    insert_error: sea_orm::DbErr,
) -> crate::Result<Model> {
    let existing = Entity::find_by_id(task_id.clone())
        .one(db)
        .await
        .map_err(DbError::from)?;
    let Some(existing) = existing else {
        return Err(DbError::from(insert_error));
    };
    if existing.display_name == entry.display_name {
        return Ok(existing);
    }

    Entity::update_many()
        .col_expr(
            Column::DisplayName,
            Expr::value(entry.display_name.to_string()),
        )
        .col_expr(Column::UpdatedAt, Expr::value(entry.first_run_at))
        .filter(Column::TaskId.eq(task_id.clone()))
        .exec(db)
        .await
        .map_err(DbError::from)?;

    Entity::find_by_id(task_id)
        .one(db)
        .await
        .map_err(DbError::from)?
        .ok_or_else(|| DbError::database_operation("scheduled task disappeared after update"))
}

async fn claim_due(
    db: &DatabaseConnection,
    request: ScheduledTaskClaimRequest<'_>,
) -> crate::Result<Option<ScheduledTaskClaim>> {
    validate_claim_request(request)?;
    let task_id = scheduled_task_row_id(request.namespace, request.task_name)?;
    let Some(existing) = Entity::find_by_id(task_id.clone())
        .one(db)
        .await
        .map_err(DbError::from)?
    else {
        return Ok(None);
    };

    if existing.next_run_at > request.now {
        return Ok(None);
    }
    if is_claim_fresh(&existing, request.now) {
        return Ok(None);
    }

    let claim_expires_at = request
        .now
        .checked_add_signed(chrono_duration_from_std(request.claim_ttl)?)
        .ok_or_else(|| DbError::non_retryable("scheduled task claim expiry overflow"))?;
    let claim_available = Condition::any()
        .add(Column::ClaimOwnerId.is_null())
        .add(Column::ClaimExpiresAt.is_null())
        .add(Column::ClaimExpiresAt.lte(request.now));
    let update = Entity::update_many()
        .col_expr(
            Column::ClaimOwnerId,
            Expr::value(Some(request.owner_id.to_string())),
        )
        .col_expr(Column::ClaimExpiresAt, Expr::value(Some(claim_expires_at)))
        .col_expr(Column::LastClaimedAt, Expr::value(Some(request.now)))
        .col_expr(Column::UpdatedAt, Expr::value(request.now))
        .filter(Column::TaskId.eq(task_id.clone()))
        .filter(Column::NextRunAt.eq(existing.next_run_at))
        .filter(Column::NextRunAt.lte(request.now))
        .filter(claim_available)
        .exec(db)
        .await
        .map_err(DbError::from)?;

    if update.rows_affected != 1 {
        return Ok(None);
    }

    Ok(Some(ScheduledTaskClaim {
        task_id,
        namespace: existing.namespace,
        task_name: existing.task_name,
        owner_id: request.owner_id.to_string(),
        scheduled_at: existing.next_run_at,
        claimed_at: request.now,
        claim_expires_at,
    }))
}

async fn complete_claim(
    db: &DatabaseConnection,
    completion: ScheduledTaskCompletion,
) -> crate::Result<bool> {
    validate_completion(&completion)?;
    let update = Entity::update_many()
        .col_expr(Column::NextRunAt, Expr::value(completion.next_run_at))
        .col_expr(Column::ClaimOwnerId, Expr::value(Option::<String>::None))
        .col_expr(
            Column::ClaimExpiresAt,
            Expr::value(Option::<chrono::DateTime<chrono::Utc>>::None),
        )
        .col_expr(
            Column::LastFinishedAt,
            Expr::value(Some(completion.finished_at)),
        )
        .col_expr(Column::UpdatedAt, Expr::value(completion.finished_at))
        .filter(Column::TaskId.eq(completion.claim.task_id))
        .filter(Column::ClaimOwnerId.eq(completion.claim.owner_id))
        .filter(Column::LastClaimedAt.eq(completion.claim.claimed_at))
        .exec(db)
        .await
        .map_err(DbError::from)?;

    Ok(update.rows_affected == 1)
}

fn is_claim_fresh(model: &Model, now: chrono::DateTime<chrono::Utc>) -> bool {
    matches!(
        (&model.claim_owner_id, model.claim_expires_at),
        (Some(_), Some(expires_at)) if expires_at > now
    )
}

fn scheduled_task_row_id(namespace: &str, task_name: &str) -> crate::Result<String> {
    let task_id = format!("{namespace}:{task_name}");
    if task_id.len() > SCHEDULED_TASK_ID_MAX_LEN {
        return Err(DbError::non_retryable(format!(
            "scheduled task id must be at most {SCHEDULED_TASK_ID_MAX_LEN} bytes"
        )));
    }
    Ok(task_id)
}

fn validate_catalog_entry(entry: ScheduledTaskCatalogEntry<'_>) -> crate::Result<()> {
    validate_non_empty("scheduled task namespace", entry.namespace)?;
    validate_non_empty("scheduled task name", entry.task_name)?;
    validate_non_empty("scheduled task display name", entry.display_name)?;
    validate_max_len(
        "scheduled task namespace",
        entry.namespace,
        SCHEDULED_TASK_NAMESPACE_MAX_LEN,
    )?;
    validate_max_len(
        "scheduled task name",
        entry.task_name,
        SCHEDULED_TASK_NAME_MAX_LEN,
    )?;
    validate_max_len(
        "scheduled task display name",
        entry.display_name,
        SCHEDULED_TASK_DISPLAY_NAME_MAX_LEN,
    )
}

fn validate_claim_request(request: ScheduledTaskClaimRequest<'_>) -> crate::Result<()> {
    validate_non_empty("scheduled task owner id", request.owner_id)?;
    validate_max_len(
        "scheduled task owner id",
        request.owner_id,
        SCHEDULED_TASK_OWNER_ID_MAX_LEN,
    )?;
    validate_max_len(
        "scheduled task namespace",
        request.namespace,
        SCHEDULED_TASK_NAMESPACE_MAX_LEN,
    )?;
    validate_max_len(
        "scheduled task name",
        request.task_name,
        SCHEDULED_TASK_NAME_MAX_LEN,
    )?;
    if request.claim_ttl.is_zero() {
        return Err(DbError::non_retryable(
            "scheduled task claim TTL must not be zero",
        ));
    }
    Ok(())
}

fn validate_completion(completion: &ScheduledTaskCompletion) -> crate::Result<()> {
    if completion.next_run_at <= completion.claim.scheduled_at {
        return Err(DbError::non_retryable(
            "scheduled task next run must be after the claimed scheduled time",
        ));
    }
    Ok(())
}

fn validate_non_empty(name: &str, value: &str) -> crate::Result<()> {
    if value.trim().is_empty() {
        return Err(DbError::non_retryable(format!("{name} must not be empty")));
    }
    Ok(())
}

fn validate_max_len(name: &str, value: &str, max_len: usize) -> crate::Result<()> {
    if value.len() > max_len {
        return Err(DbError::non_retryable(format!(
            "{name} must be at most {max_len} bytes"
        )));
    }
    Ok(())
}

fn chrono_duration_from_std(duration: Duration) -> crate::Result<chrono::Duration> {
    chrono::Duration::from_std(duration)
        .map_err(|_| DbError::non_retryable("duration is too large for chrono"))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use sea_orm::sea_query::{MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Schema};

    use super::{
        Entity, ScheduledTaskCatalogEntry, ScheduledTaskClaimRequest, ScheduledTaskCompletion,
        ScheduledTaskDbStore, create_scheduled_tasks_namespace_name_unique_index,
        create_scheduled_tasks_next_run_index, create_scheduled_tasks_table,
    };

    async fn sqlite_store() -> ScheduledTaskDbStore {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let schema = Schema::new(db.get_database_backend());
        let statement = schema.create_table_from_entity(Entity);
        db.execute(&statement)
            .await
            .expect("scheduled tasks table should be created");
        ScheduledTaskDbStore::new(db)
    }

    async fn sqlite_store_from_builders() -> ScheduledTaskDbStore {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let backend = db.get_database_backend();
        db.execute(&create_scheduled_tasks_table(backend))
            .await
            .expect("scheduled tasks table builder should execute");
        db.execute(&create_scheduled_tasks_namespace_name_unique_index())
            .await
            .expect("scheduled tasks unique index builder should execute");
        db.execute(&create_scheduled_tasks_next_run_index())
            .await
            .expect("scheduled tasks due index builder should execute");
        ScheduledTaskDbStore::new(db)
    }

    fn create_table_sql(backend: DatabaseBackend) -> String {
        let table = create_scheduled_tasks_table(backend);
        match backend {
            DatabaseBackend::MySql => table.to_string(MysqlQueryBuilder),
            DatabaseBackend::Postgres => table.to_string(PostgresQueryBuilder),
            DatabaseBackend::Sqlite => table.to_string(SqliteQueryBuilder),
            _ => unreachable!("unsupported backend in scheduled task table test"),
        }
    }

    fn entry(first_run_at: chrono::DateTime<Utc>) -> ScheduledTaskCatalogEntry<'static> {
        ScheduledTaskCatalogEntry {
            namespace: "aster_yggdrasil",
            task_name: "audit-cleanup",
            display_name: "Audit cleanup",
            first_run_at,
        }
    }

    #[test]
    fn create_scheduled_tasks_table_uses_stable_shape() {
        let sqlite_sql = create_table_sql(DatabaseBackend::Sqlite);
        assert!(sqlite_sql.contains("CREATE TABLE IF NOT EXISTS \"scheduled_tasks\""));
        assert!(sqlite_sql.contains("\"task_id\" varchar(191) NOT NULL PRIMARY KEY"));
        assert!(sqlite_sql.contains("\"namespace\" varchar(64) NOT NULL"));
        assert!(sqlite_sql.contains("\"next_run_at\" timestamp_with_timezone_text NOT NULL"));
        let namespace_index =
            create_scheduled_tasks_namespace_name_unique_index().to_string(SqliteQueryBuilder);
        assert!(namespace_index.contains("idx_scheduled_tasks_namespace_name_unique"));
        assert!(namespace_index.contains("\"namespace\", \"task_name\""));
        let next_run_index = create_scheduled_tasks_next_run_index().to_string(SqliteQueryBuilder);
        assert!(next_run_index.contains("idx_scheduled_tasks_next_run"));

        let mysql_sql = create_table_sql(DatabaseBackend::MySql);
        assert!(mysql_sql.contains("`next_run_at` datetime(6) NOT NULL"));

        let postgres_sql = create_table_sql(DatabaseBackend::Postgres);
        assert!(postgres_sql.contains("\"next_run_at\" timestamp with time zone NOT NULL"));
    }

    #[tokio::test]
    async fn scheduled_tasks_builders_execute_on_sqlite() {
        let store = sqlite_store_from_builders().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();

        let inserted = store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert with builder-created schema");

        assert_eq!(inserted.task_id, "aster_yggdrasil:audit-cleanup");
    }

    #[tokio::test]
    async fn ensure_task_rejects_invalid_catalog_values() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();

        let empty = store
            .ensure_task(ScheduledTaskCatalogEntry {
                namespace: " ",
                ..entry(now)
            })
            .await
            .expect_err("empty namespace should be rejected");
        assert!(empty.to_string().contains("must not be empty"));

        let too_long = store
            .ensure_task(ScheduledTaskCatalogEntry {
                task_name: "x".repeat(129).as_str(),
                ..entry(now)
            })
            .await
            .expect_err("long task name should be rejected");
        assert!(too_long.to_string().contains("at most 128 bytes"));
    }

    #[tokio::test]
    async fn ensure_task_inserts_and_refreshes_display_name_without_resetting_schedule() {
        let store = sqlite_store().await;
        let first_run_at = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();

        let inserted = store
            .ensure_task(entry(first_run_at))
            .await
            .expect("scheduled task should insert");
        assert_eq!(inserted.task_id, "aster_yggdrasil:audit-cleanup");
        assert_eq!(inserted.next_run_at, first_run_at);

        let refreshed = store
            .ensure_task(ScheduledTaskCatalogEntry {
                display_name: "Audit cleanup v2",
                first_run_at: first_run_at + ChronoDuration::hours(1),
                ..entry(first_run_at)
            })
            .await
            .expect("scheduled task should refresh");
        assert_eq!(refreshed.display_name, "Audit cleanup v2");
        assert_eq!(refreshed.next_run_at, first_run_at);
    }

    #[tokio::test]
    async fn claim_due_claims_once_until_completion_or_expiry() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert");

        let claim = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("claim should succeed")
            .expect("task should be due");
        assert_eq!(claim.scheduled_at, now);

        let blocked = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-b",
                now: now + ChronoDuration::seconds(1),
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("standby claim should succeed");
        assert!(blocked.is_none());

        let reclaimed = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-b",
                now: now + ChronoDuration::seconds(31),
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("expired claim should be reclaimable")
            .expect("task should still be due");
        assert_eq!(reclaimed.owner_id, "runtime-b");
    }

    #[tokio::test]
    async fn claim_due_blocks_duplicate_fresh_claim_from_same_owner() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert");

        let first = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("first claim should query")
            .expect("task should be due");
        assert_eq!(first.owner_id, "runtime-a");

        let duplicate = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now: now + ChronoDuration::seconds(1),
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("duplicate claim should query");
        assert!(duplicate.is_none());
    }

    #[tokio::test]
    async fn claim_due_skips_not_due_and_rejects_zero_ttl() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now + ChronoDuration::minutes(5)))
            .await
            .expect("scheduled task should insert");

        let not_due = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("not-due claim check should succeed");
        assert!(not_due.is_none());

        let zero_ttl = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::ZERO,
            })
            .await
            .expect_err("zero TTL should be rejected");
        assert!(zero_ttl.to_string().contains("must not be zero"));
    }

    #[tokio::test]
    async fn completing_claim_advances_next_run_and_clears_claim() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert");
        let claim = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("claim should succeed")
            .expect("task should be due");
        let next_run_at = now + ChronoDuration::hours(1);

        assert!(
            store
                .complete_claim(ScheduledTaskCompletion {
                    claim,
                    finished_at: now + ChronoDuration::seconds(5),
                    next_run_at,
                })
                .await
                .expect("completion should succeed")
        );

        let second = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now: now + ChronoDuration::minutes(30),
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("claim check should succeed");
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn complete_claim_requires_matching_owner_and_claim_timestamp() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert");
        let claim = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("claim should succeed")
            .expect("task should be due");

        let mut wrong_owner = claim.clone();
        wrong_owner.owner_id = "runtime-b".to_string();
        assert!(
            !store
                .complete_claim(ScheduledTaskCompletion {
                    claim: wrong_owner,
                    finished_at: now + ChronoDuration::seconds(5),
                    next_run_at: now + ChronoDuration::hours(1),
                })
                .await
                .expect("wrong owner completion should query")
        );

        let mut wrong_claim_time = claim;
        wrong_claim_time.claimed_at += ChronoDuration::seconds(1);
        assert!(
            !store
                .complete_claim(ScheduledTaskCompletion {
                    claim: wrong_claim_time,
                    finished_at: now + ChronoDuration::seconds(5),
                    next_run_at: now + ChronoDuration::hours(1),
                })
                .await
                .expect("wrong claim timestamp completion should query")
        );
    }

    #[tokio::test]
    async fn complete_claim_rejects_next_run_that_does_not_advance_schedule() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        store
            .ensure_task(entry(now))
            .await
            .expect("scheduled task should insert");
        let claim = store
            .claim_due(ScheduledTaskClaimRequest {
                namespace: "aster_yggdrasil",
                task_name: "audit-cleanup",
                owner_id: "runtime-a",
                now,
                claim_ttl: std::time::Duration::from_secs(30),
            })
            .await
            .expect("claim should succeed")
            .expect("task should be due");

        let error = store
            .complete_claim(ScheduledTaskCompletion {
                claim,
                finished_at: now + ChronoDuration::seconds(5),
                next_run_at: now,
            })
            .await
            .expect_err("non-advancing next run should be rejected");

        assert!(error.to_string().contains("must be after"));
    }
}
