//! Database-backed mail outbox table and dispatch store.
//!
//! Aster products share the same mail outbox persistence mechanics: enqueue a
//! rendered-template payload, claim due rows, move failed deliveries to retry or
//! failed, clear sensitive payload JSON after terminal states, and count active
//! rows. Products still own template rendering, audit records, and the business
//! context that creates outbox rows.

use std::future::Future;

use chrono::{DateTime, Duration, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{
    Alias, ColumnDef, Index, IndexCreateStatement, IndexDropStatement, Table, TableCreateStatement,
    TableDropStatement,
};
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    sea_query::Expr,
};

use crate::DbError;
use aster_forge_mail::{
    DispatchStats, MailOutboxDispatchConfig, MailOutboxDispatchContext, MailOutboxDispatchRow,
    MailOutboxStatus, MailTemplateCode, StoredMailPayload,
};

/// Mail outbox table name.
pub const MAIL_OUTBOX_TABLE: &str = "mail_outbox";
/// Stable row id column.
pub const MAIL_OUTBOX_ID_COLUMN: &str = "id";
/// Template code column.
pub const MAIL_OUTBOX_TEMPLATE_CODE_COLUMN: &str = "template_code";
/// Recipient email address column.
pub const MAIL_OUTBOX_TO_ADDRESS_COLUMN: &str = "to_address";
/// Optional recipient display name column.
pub const MAIL_OUTBOX_TO_NAME_COLUMN: &str = "to_name";
/// Stored template payload JSON column.
pub const MAIL_OUTBOX_PAYLOAD_JSON_COLUMN: &str = "payload_json";
/// Dispatch status column.
pub const MAIL_OUTBOX_STATUS_COLUMN: &str = "status";
/// Delivery attempt count column.
pub const MAIL_OUTBOX_ATTEMPT_COUNT_COLUMN: &str = "attempt_count";
/// Next delivery attempt timestamp column.
pub const MAIL_OUTBOX_NEXT_ATTEMPT_AT_COLUMN: &str = "next_attempt_at";
/// Processing claim timestamp column.
pub const MAIL_OUTBOX_PROCESSING_STARTED_AT_COLUMN: &str = "processing_started_at";
/// Sent timestamp column.
pub const MAIL_OUTBOX_SENT_AT_COLUMN: &str = "sent_at";
/// Last delivery error column.
pub const MAIL_OUTBOX_LAST_ERROR_COLUMN: &str = "last_error";
/// Row creation timestamp column.
pub const MAIL_OUTBOX_CREATED_AT_COLUMN: &str = "created_at";
/// Row update timestamp column.
pub const MAIL_OUTBOX_UPDATED_AT_COLUMN: &str = "updated_at";

const MAIL_TEMPLATE_CODE_MAX_LEN: u32 = 64;
const MAIL_TEMPLATE_CODE_MAX_BYTES: usize = 64;
const MAIL_OUTBOX_TO_ADDRESS_MAX_LEN: usize = 255;
const MAIL_OUTBOX_TO_NAME_MAX_LEN: usize = 255;

/// Builds the shared `mail_outbox` table creation statement.
pub fn create_mail_outbox_table(backend: DatabaseBackend) -> TableCreateStatement {
    Table::create()
        .table(mail_outbox_table())
        .if_not_exists()
        .col(
            ColumnDef::new(mail_outbox_id())
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(
            ColumnDef::new(mail_outbox_template_code())
                .string_len(MAIL_TEMPLATE_CODE_MAX_LEN)
                .not_null(),
        )
        .col(
            ColumnDef::new(mail_outbox_to_address())
                .string_len(255)
                .not_null(),
        )
        .col(ColumnDef::new(mail_outbox_to_name()).string_len(255).null())
        .col(ColumnDef::new(mail_outbox_payload_json()).text().not_null())
        .col(
            ColumnDef::new(mail_outbox_status())
                .string_len(16)
                .not_null(),
        )
        .col(
            ColumnDef::new(mail_outbox_attempt_count())
                .integer()
                .not_null()
                .default(0),
        )
        .col(utc_datetime_column(backend, mail_outbox_next_attempt_at()).not_null())
        .col(utc_datetime_column(backend, mail_outbox_processing_started_at()).null())
        .col(utc_datetime_column(backend, mail_outbox_sent_at()).null())
        .col(ColumnDef::new(mail_outbox_last_error()).text().null())
        .col(utc_datetime_column(backend, mail_outbox_created_at()).not_null())
        .col(utc_datetime_column(backend, mail_outbox_updated_at()).not_null())
        .to_owned()
}

/// Builds the shared `mail_outbox` table drop statement.
pub fn drop_mail_outbox_table() -> TableDropStatement {
    Table::drop()
        .table(mail_outbox_table())
        .if_exists()
        .to_owned()
}

/// Builds the due-row index used by dispatch polling.
pub fn create_mail_outbox_due_index() -> IndexCreateStatement {
    Index::create()
        .name("idx_mail_outbox_due")
        .table(mail_outbox_table())
        .col(mail_outbox_status())
        .col(mail_outbox_next_attempt_at())
        .col(mail_outbox_created_at())
        .if_not_exists()
        .to_owned()
}

/// Builds the processing-stale index used by dispatch recovery.
pub fn create_mail_outbox_processing_index() -> IndexCreateStatement {
    Index::create()
        .name("idx_mail_outbox_processing")
        .table(mail_outbox_table())
        .col(mail_outbox_status())
        .col(mail_outbox_processing_started_at())
        .col(mail_outbox_created_at())
        .if_not_exists()
        .to_owned()
}

/// Builds the sent timestamp index used by retention and admin queries.
pub fn create_mail_outbox_sent_at_index() -> IndexCreateStatement {
    Index::create()
        .name("idx_mail_outbox_sent_at")
        .table(mail_outbox_table())
        .col(mail_outbox_sent_at())
        .if_not_exists()
        .to_owned()
}

/// Builds the due-row index drop statement.
pub fn drop_mail_outbox_due_index() -> IndexDropStatement {
    Index::drop()
        .name("idx_mail_outbox_due")
        .table(mail_outbox_table())
        .if_exists()
        .to_owned()
}

/// Builds the processing-stale index drop statement.
pub fn drop_mail_outbox_processing_index() -> IndexDropStatement {
    Index::drop()
        .name("idx_mail_outbox_processing")
        .table(mail_outbox_table())
        .if_exists()
        .to_owned()
}

/// Builds the sent timestamp index drop statement.
pub fn drop_mail_outbox_sent_at_index() -> IndexDropStatement {
    Index::drop()
        .name("idx_mail_outbox_sent_at")
        .table(mail_outbox_table())
        .if_exists()
        .to_owned()
}

fn mail_outbox_table() -> Alias {
    Alias::new(MAIL_OUTBOX_TABLE)
}

fn mail_outbox_id() -> Alias {
    Alias::new(MAIL_OUTBOX_ID_COLUMN)
}

fn mail_outbox_template_code() -> Alias {
    Alias::new(MAIL_OUTBOX_TEMPLATE_CODE_COLUMN)
}

fn mail_outbox_to_address() -> Alias {
    Alias::new(MAIL_OUTBOX_TO_ADDRESS_COLUMN)
}

fn mail_outbox_to_name() -> Alias {
    Alias::new(MAIL_OUTBOX_TO_NAME_COLUMN)
}

fn mail_outbox_payload_json() -> Alias {
    Alias::new(MAIL_OUTBOX_PAYLOAD_JSON_COLUMN)
}

fn mail_outbox_status() -> Alias {
    Alias::new(MAIL_OUTBOX_STATUS_COLUMN)
}

fn mail_outbox_attempt_count() -> Alias {
    Alias::new(MAIL_OUTBOX_ATTEMPT_COUNT_COLUMN)
}

fn mail_outbox_next_attempt_at() -> Alias {
    Alias::new(MAIL_OUTBOX_NEXT_ATTEMPT_AT_COLUMN)
}

fn mail_outbox_processing_started_at() -> Alias {
    Alias::new(MAIL_OUTBOX_PROCESSING_STARTED_AT_COLUMN)
}

fn mail_outbox_sent_at() -> Alias {
    Alias::new(MAIL_OUTBOX_SENT_AT_COLUMN)
}

fn mail_outbox_last_error() -> Alias {
    Alias::new(MAIL_OUTBOX_LAST_ERROR_COLUMN)
}

fn mail_outbox_created_at() -> Alias {
    Alias::new(MAIL_OUTBOX_CREATED_AT_COLUMN)
}

fn mail_outbox_updated_at() -> Alias {
    Alias::new(MAIL_OUTBOX_UPDATED_AT_COLUMN)
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

/// SeaORM model for `mail_outbox`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "mail_outbox")]
pub struct Model {
    /// Stable row id.
    #[sea_orm(primary_key)]
    pub id: i64,
    /// Shared Aster mail template code.
    pub template_code: MailTemplateCode,
    /// Recipient email address.
    pub to_address: String,
    /// Optional recipient display name.
    pub to_name: Option<String>,
    /// Stored template payload JSON.
    pub payload_json: StoredMailPayload,
    /// Dispatch status.
    pub status: MailOutboxStatus,
    /// Delivery attempt count.
    pub attempt_count: i32,
    /// Next delivery attempt timestamp.
    pub next_attempt_at: DateTimeUtc,
    /// Processing claim timestamp.
    pub processing_started_at: Option<DateTimeUtc>,
    /// Sent timestamp.
    pub sent_at: Option<DateTimeUtc>,
    /// Last delivery error.
    pub last_error: Option<String>,
    /// Row creation timestamp.
    pub created_at: DateTimeUtc,
    /// Row update timestamp.
    pub updated_at: DateTimeUtc,
}

impl MailOutboxDispatchRow for Model {
    fn id(&self) -> i64 {
        self.id
    }

    fn attempt_count(&self) -> i32 {
        self.attempt_count
    }

    fn template_code(&self) -> &str {
        self.template_code.as_str()
    }

    fn to_address(&self) -> &str {
        &self.to_address
    }

    fn to_name(&self) -> Option<&str> {
        self.to_name.as_deref()
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Product request to enqueue one mail outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailOutboxCreate {
    /// Shared Aster mail template code.
    pub template_code: MailTemplateCode,
    /// Recipient email address.
    pub to_address: String,
    /// Optional recipient display name.
    pub to_name: Option<String>,
    /// Stored template payload JSON.
    pub payload_json: StoredMailPayload,
    /// Initial next attempt timestamp.
    pub next_attempt_at: DateTime<Utc>,
    /// Row creation/update timestamp.
    pub now: DateTime<Utc>,
}

/// SeaORM-backed mail outbox store.
#[derive(Clone)]
pub struct MailOutboxDbStore {
    db: DatabaseConnection,
}

impl MailOutboxDbStore {
    /// Creates a mail outbox store from a SeaORM database connection.
    pub const fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Enqueues one pending mail outbox row.
    pub async fn create(&self, request: MailOutboxCreate) -> crate::Result<Model> {
        create_mail_outbox_row(&self.db, request).await
    }

    /// Lists rows that are due or stale enough to be reclaimed.
    pub async fn list_claimable(
        &self,
        now: DateTime<Utc>,
        stale_before: DateTime<Utc>,
        limit: u64,
    ) -> crate::Result<Vec<Model>> {
        list_claimable(&self.db, now, stale_before, limit).await
    }

    /// Attempts to claim one row for processing.
    pub async fn try_claim(
        &self,
        id: i64,
        now: DateTime<Utc>,
        stale_before: DateTime<Utc>,
    ) -> crate::Result<bool> {
        try_claim(&self.db, id, now, stale_before).await
    }

    /// Marks a processing row as sent and clears its sensitive payload.
    pub async fn mark_sent(&self, id: i64, sent_at: DateTime<Utc>) -> crate::Result<bool> {
        mark_sent(&self.db, id, sent_at).await
    }

    /// Marks a processing row for retry.
    pub async fn mark_retry(
        &self,
        id: i64,
        attempt_count: i32,
        next_attempt_at: DateTime<Utc>,
        last_error: &str,
    ) -> crate::Result<bool> {
        mark_retry(&self.db, id, attempt_count, next_attempt_at, last_error).await
    }

    /// Marks a processing row as permanently failed and clears its sensitive payload.
    pub async fn mark_failed(
        &self,
        id: i64,
        attempt_count: i32,
        failed_at: DateTime<Utc>,
        last_error: &str,
    ) -> crate::Result<bool> {
        mark_failed(&self.db, id, attempt_count, failed_at, last_error).await
    }

    /// Counts pending or retry rows.
    pub async fn count_active(&self) -> crate::Result<u64> {
        count_active(&self.db).await
    }

    /// Runs one shared dispatch pass using the standard database-backed outbox mechanics.
    ///
    /// Forge owns list/claim/mark/retry/failure persistence for the shared `mail_outbox` table.
    /// Products only provide rendering/delivery and audit hooks, which keeps every Aster service on
    /// the same state machine without copying payload-heavy rows across persistence callbacks.
    pub async fn dispatch_due<E, Deliver, DeliverFut, OnSent, OnSentFut, OnFailed, OnFailedFut>(
        &self,
        config: &MailOutboxDispatchConfig,
        deliver: Deliver,
        on_sent: OnSent,
        on_failed: OnFailed,
    ) -> std::result::Result<DispatchStats, E>
    where
        E: From<DbError> + std::fmt::Display,
        Deliver: FnMut(Model) -> DeliverFut,
        DeliverFut: Future<Output = std::result::Result<String, E>>,
        OnSent: FnMut(MailOutboxDispatchContext, i32, String) -> OnSentFut,
        OnSentFut: Future<Output = ()>,
        OnFailed: FnMut(MailOutboxDispatchContext, i32, String) -> OnFailedFut,
        OnFailedFut: Future<Output = ()>,
    {
        aster_forge_mail::dispatch_mail_outbox(
            config,
            |batch_size, stale_secs| async move {
                let now = Utc::now();
                let stale_before = now - Duration::seconds(stale_secs);
                self.list_claimable(now, stale_before, batch_size)
                    .await
                    .map_err(E::from)
            },
            |id| async move {
                let now = Utc::now();
                let stale_before = now - Duration::seconds(config.processing_stale_secs);
                self.try_claim(id, now, stale_before).await.map_err(E::from)
            },
            deliver,
            |id, _attempt| async move { self.mark_sent(id, Utc::now()).await.map_err(E::from) },
            |id, attempt_count, retry_delay_secs, error_message| async move {
                let retry_at = Utc::now() + Duration::seconds(retry_delay_secs);
                self.mark_retry(id, attempt_count, retry_at, &error_message)
                    .await
                    .map_err(E::from)
            },
            |id, attempt_count, error_message| async move {
                self.mark_failed(id, attempt_count, Utc::now(), &error_message)
                    .await
                    .map_err(E::from)
            },
            on_sent,
            on_failed,
        )
        .await
    }
}

/// Enqueues one pending mail outbox row using any SeaORM connection or transaction.
pub async fn create_mail_outbox_row<C>(db: &C, request: MailOutboxCreate) -> crate::Result<Model>
where
    C: ConnectionTrait,
{
    validate_create(&request)?;
    ActiveModel {
        template_code: Set(request.template_code),
        to_address: Set(request.to_address),
        to_name: Set(request.to_name),
        payload_json: Set(request.payload_json),
        status: Set(MailOutboxStatus::Pending),
        attempt_count: Set(0),
        next_attempt_at: Set(request.next_attempt_at),
        processing_started_at: Set(None),
        sent_at: Set(None),
        last_error: Set(None),
        created_at: Set(request.now),
        updated_at: Set(request.now),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(DbError::from)
}

async fn list_claimable<C>(
    db: &C,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
    limit: u64,
) -> crate::Result<Vec<Model>>
where
    C: ConnectionTrait,
{
    Entity::find()
        .filter(claimable_condition(now, stale_before))
        .order_by_asc(Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
        .map_err(DbError::from)
}

async fn try_claim<C>(
    db: &C,
    id: i64,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> crate::Result<bool>
where
    C: ConnectionTrait,
{
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(MailOutboxStatus::Processing.to_value()),
        )
        .col_expr(Column::ProcessingStartedAt, Expr::value(Some(now)))
        .col_expr(Column::UpdatedAt, Expr::value(now))
        .filter(Column::Id.eq(id))
        .filter(claimable_condition(now, stale_before))
        .exec(db)
        .await
        .map_err(DbError::from)?;
    Ok(result.rows_affected == 1)
}

async fn mark_sent<C>(db: &C, id: i64, sent_at: DateTime<Utc>) -> crate::Result<bool>
where
    C: ConnectionTrait,
{
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(MailOutboxStatus::Sent.to_value()),
        )
        .col_expr(Column::SentAt, Expr::value(Some(sent_at)))
        .col_expr(
            Column::ProcessingStartedAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .col_expr(Column::LastError, Expr::value(Option::<String>::None))
        .col_expr(
            Column::PayloadJson,
            Expr::value(StoredMailPayload::CLEARED_JSON),
        )
        .col_expr(Column::UpdatedAt, Expr::value(sent_at))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(MailOutboxStatus::Processing))
        .exec(db)
        .await
        .map_err(DbError::from)?;
    Ok(result.rows_affected == 1)
}

async fn mark_retry<C>(
    db: &C,
    id: i64,
    attempt_count: i32,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
) -> crate::Result<bool>
where
    C: ConnectionTrait,
{
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(MailOutboxStatus::Retry.to_value()),
        )
        .col_expr(Column::AttemptCount, Expr::value(attempt_count))
        .col_expr(Column::NextAttemptAt, Expr::value(next_attempt_at))
        .col_expr(
            Column::ProcessingStartedAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .col_expr(Column::LastError, Expr::value(Some(last_error)))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(MailOutboxStatus::Processing))
        .exec(db)
        .await
        .map_err(DbError::from)?;
    Ok(result.rows_affected == 1)
}

async fn mark_failed<C>(
    db: &C,
    id: i64,
    attempt_count: i32,
    failed_at: DateTime<Utc>,
    last_error: &str,
) -> crate::Result<bool>
where
    C: ConnectionTrait,
{
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(MailOutboxStatus::Failed.to_value()),
        )
        .col_expr(Column::AttemptCount, Expr::value(attempt_count))
        .col_expr(Column::NextAttemptAt, Expr::value(failed_at))
        .col_expr(
            Column::ProcessingStartedAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .col_expr(Column::LastError, Expr::value(Some(last_error)))
        .col_expr(
            Column::PayloadJson,
            Expr::value(StoredMailPayload::CLEARED_JSON),
        )
        .col_expr(Column::UpdatedAt, Expr::value(failed_at))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(MailOutboxStatus::Processing))
        .exec(db)
        .await
        .map_err(DbError::from)?;
    Ok(result.rows_affected == 1)
}

async fn count_active<C>(db: &C) -> crate::Result<u64>
where
    C: ConnectionTrait,
{
    Entity::find()
        .filter(Column::Status.is_in([MailOutboxStatus::Pending, MailOutboxStatus::Retry]))
        .count(db)
        .await
        .map_err(DbError::from)
}

fn claimable_condition(now: DateTime<Utc>, stale_before: DateTime<Utc>) -> Condition {
    Condition::any()
        .add(
            Condition::all()
                .add(Column::Status.is_in([MailOutboxStatus::Pending, MailOutboxStatus::Retry]))
                .add(Column::NextAttemptAt.lte(now)),
        )
        .add(
            Condition::all()
                .add(Column::Status.eq(MailOutboxStatus::Processing))
                .add(Column::ProcessingStartedAt.lte(stale_before)),
        )
}

fn validate_create(request: &MailOutboxCreate) -> crate::Result<()> {
    validate_non_empty("mail outbox recipient address", &request.to_address)?;
    validate_max_len(
        "mail outbox recipient address",
        &request.to_address,
        MAIL_OUTBOX_TO_ADDRESS_MAX_LEN,
    )?;
    if let Some(to_name) = &request.to_name {
        validate_max_len(
            "mail outbox recipient name",
            to_name,
            MAIL_OUTBOX_TO_NAME_MAX_LEN,
        )?;
    }
    validate_max_len(
        "mail outbox template code",
        request.template_code.as_str(),
        MAIL_TEMPLATE_CODE_MAX_BYTES,
    )
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

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use sea_orm::sea_query::{MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, EntityTrait};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{
        Entity, MailOutboxCreate, MailOutboxDbStore, create_mail_outbox_due_index,
        create_mail_outbox_processing_index, create_mail_outbox_sent_at_index,
        create_mail_outbox_table,
    };
    use crate::DbError;
    use aster_forge_mail::{
        DEFAULT_ERROR_MAX_LEN, MailOutboxDispatchConfig, MailOutboxDispatchContext,
        MailOutboxRetryPolicy, MailOutboxStatus, MailTemplateCode, StoredMailPayload,
    };

    async fn sqlite_store() -> MailOutboxDbStore {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let backend = db.get_database_backend();
        db.execute(&create_mail_outbox_table(backend))
            .await
            .expect("mail outbox table builder should execute");
        db.execute(&create_mail_outbox_due_index())
            .await
            .expect("mail outbox due index builder should execute");
        db.execute(&create_mail_outbox_processing_index())
            .await
            .expect("mail outbox processing index builder should execute");
        db.execute(&create_mail_outbox_sent_at_index())
            .await
            .expect("mail outbox sent index builder should execute");
        MailOutboxDbStore::new(db)
    }

    fn create_table_sql(backend: DatabaseBackend) -> String {
        let table = create_mail_outbox_table(backend);
        match backend {
            DatabaseBackend::MySql => table.to_string(MysqlQueryBuilder),
            DatabaseBackend::Postgres => table.to_string(PostgresQueryBuilder),
            DatabaseBackend::Sqlite => table.to_string(SqliteQueryBuilder),
            _ => unreachable!("unsupported backend in mail outbox table test"),
        }
    }

    fn create_request(now: chrono::DateTime<Utc>) -> MailOutboxCreate {
        MailOutboxCreate {
            template_code: MailTemplateCode::LoginEmailCode,
            to_address: "operator@example.com".to_string(),
            to_name: Some("Operator".to_string()),
            payload_json: StoredMailPayload::from("{\"code\":\"123456\"}".to_string()),
            next_attempt_at: now,
            now,
        }
    }

    #[test]
    fn create_mail_outbox_table_uses_stable_shape() {
        let sqlite_sql = create_table_sql(DatabaseBackend::Sqlite);
        assert!(sqlite_sql.contains("CREATE TABLE IF NOT EXISTS \"mail_outbox\""));
        assert!(sqlite_sql.contains("\"template_code\" varchar(64) NOT NULL"));
        assert!(sqlite_sql.contains("\"status\" varchar(16) NOT NULL"));
        assert!(sqlite_sql.contains("\"next_attempt_at\" timestamp_with_timezone_text NOT NULL"));
        let due_index = create_mail_outbox_due_index().to_string(SqliteQueryBuilder);
        assert!(due_index.contains("idx_mail_outbox_due"));
        assert!(due_index.contains("\"status\", \"next_attempt_at\", \"created_at\""));

        let mysql_sql = create_table_sql(DatabaseBackend::MySql);
        assert!(mysql_sql.contains("`next_attempt_at` datetime(6) NOT NULL"));

        let postgres_sql = create_table_sql(DatabaseBackend::Postgres);
        assert!(postgres_sql.contains("\"next_attempt_at\" timestamp with time zone NOT NULL"));
    }

    #[tokio::test]
    async fn mail_outbox_store_creates_and_counts_active_rows() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();

        let row = store
            .create(create_request(now))
            .await
            .expect("mail outbox row should insert");

        assert_eq!(row.template_code, MailTemplateCode::LoginEmailCode);
        assert_eq!(row.status, MailOutboxStatus::Pending);
        assert_eq!(store.count_active().await.expect("count should query"), 1);
    }

    #[tokio::test]
    async fn mail_outbox_store_rejects_invalid_create_values() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();

        let error = store
            .create(MailOutboxCreate {
                to_address: " ".to_string(),
                ..create_request(now)
            })
            .await
            .expect_err("blank recipient should be rejected");
        assert!(error.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn mail_outbox_store_claims_due_rows_once() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        let row = store
            .create(create_request(now))
            .await
            .expect("mail outbox row should insert");

        let claimable = store
            .list_claimable(now, now - ChronoDuration::minutes(5), 10)
            .await
            .expect("claimable rows should query");
        assert_eq!(claimable.len(), 1);

        assert!(
            store
                .try_claim(row.id, now, now - ChronoDuration::minutes(5))
                .await
                .expect("claim should query")
        );
        assert!(
            !store
                .try_claim(
                    row.id,
                    now + ChronoDuration::seconds(1),
                    now - ChronoDuration::minutes(5)
                )
                .await
                .expect("fresh duplicate claim should query")
        );
    }

    #[tokio::test]
    async fn mail_outbox_store_reclaims_stale_processing_rows() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        let row = store
            .create(create_request(now))
            .await
            .expect("mail outbox row should insert");
        assert!(
            store
                .try_claim(row.id, now, now - ChronoDuration::minutes(5))
                .await
                .expect("claim should query")
        );

        let reclaimed = store
            .list_claimable(
                now + ChronoDuration::minutes(10),
                now + ChronoDuration::minutes(1),
                10,
            )
            .await
            .expect("stale processing rows should query");
        assert_eq!(reclaimed.len(), 1);
    }

    #[tokio::test]
    async fn mail_outbox_store_marks_sent_and_clears_payload() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        let row = store
            .create(create_request(now))
            .await
            .expect("mail outbox row should insert");
        assert!(
            store
                .try_claim(row.id, now, now - ChronoDuration::minutes(5))
                .await
                .expect("claim should query")
        );

        assert!(
            store
                .mark_sent(row.id, now + ChronoDuration::seconds(2))
                .await
                .expect("mark sent should query")
        );

        let stored = Entity::find_by_id(row.id)
            .one(&store.db)
            .await
            .expect("sent row should query")
            .expect("sent row should exist");
        assert_eq!(stored.status, MailOutboxStatus::Sent);
        assert_eq!(
            stored.payload_json.as_ref(),
            StoredMailPayload::CLEARED_JSON
        );
        assert_eq!(store.count_active().await.expect("count should query"), 0);
    }

    #[tokio::test]
    async fn mail_outbox_store_marks_retry_and_failed_only_from_processing() {
        let store = sqlite_store().await;
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        let row = store
            .create(create_request(now))
            .await
            .expect("mail outbox row should insert");
        assert!(
            !store
                .mark_retry(row.id, 1, now + ChronoDuration::seconds(5), "smtp down")
                .await
                .expect("retry without processing should query")
        );
        assert!(
            store
                .try_claim(row.id, now, now - ChronoDuration::minutes(5))
                .await
                .expect("claim should query")
        );
        assert!(
            store
                .mark_retry(row.id, 1, now + ChronoDuration::seconds(5), "smtp down")
                .await
                .expect("retry should query")
        );

        let retry = Entity::find_by_id(row.id)
            .one(&store.db)
            .await
            .expect("retry row should query")
            .expect("retry row should exist");
        assert_eq!(retry.status, MailOutboxStatus::Retry);
        assert_eq!(retry.attempt_count, 1);
        assert_eq!(retry.last_error.as_deref(), Some("smtp down"));

        assert!(
            store
                .try_claim(row.id, now + ChronoDuration::seconds(6), now)
                .await
                .expect("retry claim should query")
        );
        assert!(
            store
                .mark_failed(row.id, 2, now + ChronoDuration::seconds(7), "permanent")
                .await
                .expect("failed should query")
        );

        let failed = Entity::find_by_id(row.id)
            .one(&store.db)
            .await
            .expect("failed row should query")
            .expect("failed row should exist");
        assert_eq!(failed.status, MailOutboxStatus::Failed);
        assert_eq!(
            failed.payload_json.as_ref(),
            StoredMailPayload::CLEARED_JSON
        );
    }

    #[tokio::test]
    async fn mail_outbox_store_dispatch_due_marks_sent_and_reports_context() {
        let store = sqlite_store().await;
        let now = Utc::now();
        let row = store
            .create(create_request(now - ChronoDuration::seconds(5)))
            .await
            .expect("mail outbox row should insert");
        let config = MailOutboxDispatchConfig::new(
            20,
            60,
            1,
            MailOutboxRetryPolicy::new(3, DEFAULT_ERROR_MAX_LEN),
        );
        let delivered_payload_len = Arc::new(AtomicUsize::new(0));
        let delivered_payload_len_for_deliver = delivered_payload_len.clone();
        let sent_context = Arc::new(Mutex::new(None::<MailOutboxDispatchContext>));
        let sent_context_for_hook = sent_context.clone();

        let stats = store
            .dispatch_due(
                &config,
                move |row| {
                    let delivered_payload_len = delivered_payload_len_for_deliver.clone();
                    async move {
                        delivered_payload_len
                            .store(row.payload_json.as_ref().len(), Ordering::SeqCst);
                        Ok::<_, DbError>("Sent subject".to_string())
                    }
                },
                move |context, _attempt_count, _subject| {
                    let sent_context = sent_context_for_hook.clone();
                    async move {
                        *sent_context
                            .lock()
                            .expect("sent context mutex should not be poisoned") = Some(context);
                    }
                },
                |_context, _attempt_count, _error_message| async {},
            )
            .await
            .expect("dispatch should succeed");

        assert_eq!(stats.sent, 1);
        assert_eq!(stats.claimed, 1);
        assert_eq!(
            delivered_payload_len.load(Ordering::SeqCst),
            "{\"code\":\"123456\"}".len()
        );
        let context = sent_context
            .lock()
            .expect("sent context mutex should not be poisoned")
            .clone()
            .expect("sent hook should receive context");
        assert_eq!(context.id, row.id);
        assert_eq!(context.to_name.as_deref(), Some("Operator"));

        let stored = Entity::find_by_id(row.id)
            .one(&store.db)
            .await
            .expect("sent row should query")
            .expect("sent row should exist");
        assert_eq!(stored.status, MailOutboxStatus::Sent);
        assert_eq!(
            stored.payload_json.as_ref(),
            StoredMailPayload::CLEARED_JSON
        );
    }

    #[tokio::test]
    async fn mail_outbox_store_dispatch_due_retries_delivery_errors() {
        let store = sqlite_store().await;
        let now = Utc::now();
        let row = store
            .create(create_request(now - ChronoDuration::seconds(5)))
            .await
            .expect("mail outbox row should insert");
        let config = MailOutboxDispatchConfig::new(
            20,
            60,
            1,
            MailOutboxRetryPolicy::new(3, DEFAULT_ERROR_MAX_LEN),
        );

        let stats = store
            .dispatch_due(
                &config,
                |_row| async { Err::<String, _>(DbError::non_retryable("smtp down")) },
                |_context, _attempt_count, _subject| async {},
                |_context, _attempt_count, _error_message| async {},
            )
            .await
            .expect("dispatch should handle delivery failure as retry state");

        assert_eq!(stats.claimed, 1);
        assert_eq!(stats.retried, 1);
        let stored = Entity::find_by_id(row.id)
            .one(&store.db)
            .await
            .expect("retry row should query")
            .expect("retry row should exist");
        assert_eq!(stored.status, MailOutboxStatus::Retry);
        assert_eq!(stored.attempt_count, 1);
        assert_eq!(
            stored.last_error.as_deref(),
            Some("non-retryable error: smtp down")
        );
    }
}
