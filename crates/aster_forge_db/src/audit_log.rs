//! Database-backed audit log table and base store.
//!
//! Aster products share the same audit log storage shape: an actor id, stable
//! action wire value, target entity metadata, optional detail JSON, request
//! metadata, and creation timestamp. Products still own typed action enums,
//! detail schemas, presentation, retention policy, and authorization. This
//! module keeps the common SeaORM table contract, index builders, and simple
//! write/count/delete helpers in one place.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{
    Alias, ColumnDef, Index, IndexCreateStatement, Table, TableCreateStatement, TableDropStatement,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    sea_query::Expr,
};

/// Audit log table name.
pub const AUDIT_LOGS_TABLE: &str = "audit_logs";
/// Stable row id column.
pub const AUDIT_LOG_ID_COLUMN: &str = "id";
/// Actor user id column. System events use `0`.
pub const AUDIT_LOG_USER_ID_COLUMN: &str = "user_id";
/// Stable action wire value column.
pub const AUDIT_LOG_ACTION_COLUMN: &str = "action";
/// Target entity type column.
pub const AUDIT_LOG_ENTITY_TYPE_COLUMN: &str = "entity_type";
/// Optional target entity id column.
pub const AUDIT_LOG_ENTITY_ID_COLUMN: &str = "entity_id";
/// Optional target entity display name column.
pub const AUDIT_LOG_ENTITY_NAME_COLUMN: &str = "entity_name";
/// Optional product-owned JSON detail column.
pub const AUDIT_LOG_DETAILS_COLUMN: &str = "details";
/// Optional client IP column.
pub const AUDIT_LOG_IP_ADDRESS_COLUMN: &str = "ip_address";
/// Optional user-agent column.
pub const AUDIT_LOG_USER_AGENT_COLUMN: &str = "user_agent";
/// Row creation timestamp column.
pub const AUDIT_LOG_CREATED_AT_COLUMN: &str = "created_at";

/// Index name for created-at scans.
pub const AUDIT_LOG_CREATED_AT_INDEX: &str = "idx_audit_logs_created_at";
/// Index name for action filtering.
pub const AUDIT_LOG_ACTION_INDEX: &str = "idx_audit_logs_action";
/// Index name for user filtering.
pub const AUDIT_LOG_USER_ID_INDEX: &str = "idx_audit_logs_user_id";
/// Index name for action/time/user activity aggregation.
pub const AUDIT_LOG_ACTION_CREATED_USER_INDEX: &str = "idx_audit_logs_action_created_user";
/// Index name for cursor scans by created-at/id.
pub const AUDIT_LOG_CREATED_ID_INDEX: &str = "idx_audit_logs_created_id";
/// Index name for user cursor scans.
pub const AUDIT_LOG_USER_CREATED_ID_INDEX: &str = "idx_audit_logs_user_created_id";
/// Index name for action cursor scans.
pub const AUDIT_LOG_ACTION_CREATED_ID_INDEX: &str = "idx_audit_logs_action_created_id";
/// Index name for entity-type cursor scans.
pub const AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX: &str = "idx_audit_logs_entity_type_created_id";

const AUDIT_LOG_ACTION_MAX_LEN: u32 = 64;
const AUDIT_LOG_ACTION_MAX_BYTES: usize = 64;
const AUDIT_LOG_ENTITY_TYPE_MAX_LEN: u32 = 64;
const AUDIT_LOG_ENTITY_TYPE_MAX_BYTES: usize = 64;
const AUDIT_LOG_ENTITY_NAME_MAX_BYTES: usize = 255;
const AUDIT_LOG_IP_ADDRESS_MAX_BYTES: usize = 128;
const AUDIT_LOG_USER_AGENT_MAX_BYTES: usize = 512;

/// Builds the shared `audit_logs` table creation statement.
pub fn create_audit_logs_table(backend: DatabaseBackend) -> TableCreateStatement {
    Table::create()
        .table(audit_logs_table())
        .if_not_exists()
        .col(
            ColumnDef::new(audit_log_id())
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(
            ColumnDef::new(audit_log_user_id())
                .big_integer()
                .not_null()
                .default(0),
        )
        .col(
            ColumnDef::new(audit_log_action())
                .string_len(AUDIT_LOG_ACTION_MAX_LEN)
                .not_null(),
        )
        .col(
            ColumnDef::new(audit_log_entity_type())
                .string_len(AUDIT_LOG_ENTITY_TYPE_MAX_LEN)
                .not_null(),
        )
        .col(ColumnDef::new(audit_log_entity_id()).big_integer().null())
        .col(
            ColumnDef::new(audit_log_entity_name())
                .string_len(255)
                .null(),
        )
        .col(ColumnDef::new(audit_log_details()).text().null())
        .col(
            ColumnDef::new(audit_log_ip_address())
                .string_len(128)
                .null(),
        )
        .col(
            ColumnDef::new(audit_log_user_agent())
                .string_len(512)
                .null(),
        )
        .col(utc_datetime_column(backend, audit_log_created_at()).not_null())
        .to_owned()
}

/// Builds the shared `audit_logs` table drop statement.
pub fn drop_audit_logs_table() -> TableDropStatement {
    Table::drop()
        .table(audit_logs_table())
        .if_exists()
        .to_owned()
}

/// Builds the created-at index.
pub fn create_audit_logs_created_at_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_CREATED_AT_INDEX)
        .table(audit_logs_table())
        .col(audit_log_created_at())
        .if_not_exists()
        .to_owned()
}

/// Builds the action index.
pub fn create_audit_logs_action_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_ACTION_INDEX)
        .table(audit_logs_table())
        .col(audit_log_action())
        .if_not_exists()
        .to_owned()
}

/// Builds the user id index.
pub fn create_audit_logs_user_id_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_USER_ID_INDEX)
        .table(audit_logs_table())
        .col(audit_log_user_id())
        .if_not_exists()
        .to_owned()
}

/// Builds the action/created/user activity index.
pub fn create_audit_logs_action_created_user_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_ACTION_CREATED_USER_INDEX)
        .table(audit_logs_table())
        .col(audit_log_action())
        .col(audit_log_created_at())
        .col(audit_log_user_id())
        .if_not_exists()
        .to_owned()
}

/// Builds the created-at/id cursor index.
pub fn create_audit_logs_created_id_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_CREATED_ID_INDEX)
        .table(audit_logs_table())
        .col(audit_log_created_at())
        .col(audit_log_id())
        .if_not_exists()
        .to_owned()
}

/// Builds the user/created-at/id cursor index.
pub fn create_audit_logs_user_created_id_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_USER_CREATED_ID_INDEX)
        .table(audit_logs_table())
        .col(audit_log_user_id())
        .col(audit_log_created_at())
        .col(audit_log_id())
        .if_not_exists()
        .to_owned()
}

/// Builds the action/created-at/id cursor index.
pub fn create_audit_logs_action_created_id_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_ACTION_CREATED_ID_INDEX)
        .table(audit_logs_table())
        .col(audit_log_action())
        .col(audit_log_created_at())
        .col(audit_log_id())
        .if_not_exists()
        .to_owned()
}

/// Builds the entity-type/created-at/id cursor index.
pub fn create_audit_logs_entity_type_created_id_index() -> IndexCreateStatement {
    Index::create()
        .name(AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX)
        .table(audit_logs_table())
        .col(audit_log_entity_type())
        .col(audit_log_created_at())
        .col(audit_log_id())
        .if_not_exists()
        .to_owned()
}

/// Returns the base index builders used by the current shared schema.
pub fn create_audit_logs_base_indexes() -> [IndexCreateStatement; 3] {
    [
        create_audit_logs_created_at_index(),
        create_audit_logs_action_index(),
        create_audit_logs_user_id_index(),
    ]
}

/// Returns the optional activity/query index builders used by admin views.
pub fn create_audit_logs_query_indexes() -> [IndexCreateStatement; 5] {
    [
        create_audit_logs_action_created_user_index(),
        create_audit_logs_created_id_index(),
        create_audit_logs_user_created_id_index(),
        create_audit_logs_action_created_id_index(),
        create_audit_logs_entity_type_created_id_index(),
    ]
}

fn audit_logs_table() -> Alias {
    Alias::new(AUDIT_LOGS_TABLE)
}

fn audit_log_id() -> Alias {
    Alias::new(AUDIT_LOG_ID_COLUMN)
}

fn audit_log_user_id() -> Alias {
    Alias::new(AUDIT_LOG_USER_ID_COLUMN)
}

fn audit_log_action() -> Alias {
    Alias::new(AUDIT_LOG_ACTION_COLUMN)
}

fn audit_log_entity_type() -> Alias {
    Alias::new(AUDIT_LOG_ENTITY_TYPE_COLUMN)
}

fn audit_log_entity_id() -> Alias {
    Alias::new(AUDIT_LOG_ENTITY_ID_COLUMN)
}

fn audit_log_entity_name() -> Alias {
    Alias::new(AUDIT_LOG_ENTITY_NAME_COLUMN)
}

fn audit_log_details() -> Alias {
    Alias::new(AUDIT_LOG_DETAILS_COLUMN)
}

fn audit_log_ip_address() -> Alias {
    Alias::new(AUDIT_LOG_IP_ADDRESS_COLUMN)
}

fn audit_log_user_agent() -> Alias {
    Alias::new(AUDIT_LOG_USER_AGENT_COLUMN)
}

fn audit_log_created_at() -> Alias {
    Alias::new(AUDIT_LOG_CREATED_AT_COLUMN)
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

/// SeaORM model for `audit_logs` with product-neutral string action values.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_logs")]
pub struct Model {
    /// Stable row id.
    #[sea_orm(primary_key)]
    pub id: i64,
    /// Actor user id. System events use `0`.
    pub user_id: i64,
    /// Stable action wire value.
    pub action: String,
    /// Target entity type.
    pub entity_type: String,
    /// Optional target entity id.
    pub entity_id: Option<i64>,
    /// Optional target entity display name.
    pub entity_name: Option<String>,
    /// Optional product-owned JSON detail payload.
    pub details: Option<String>,
    /// Optional client IP.
    pub ip_address: Option<String>,
    /// Optional user-agent.
    pub user_agent: Option<String>,
    /// Row creation timestamp.
    pub created_at: DateTimeUtc,
}

/// Audit log relations.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Product-neutral request to insert one audit log row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogCreate {
    /// Actor user id. System events use `0`.
    pub user_id: i64,
    /// Stable action wire value.
    pub action: String,
    /// Target entity type.
    pub entity_type: String,
    /// Optional target entity id.
    pub entity_id: Option<i64>,
    /// Optional target entity display name.
    pub entity_name: Option<String>,
    /// Optional product-owned JSON detail payload.
    pub details: Option<String>,
    /// Optional client IP.
    pub ip_address: Option<String>,
    /// Optional user-agent.
    pub user_agent: Option<String>,
    /// Row creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Product-neutral cursor query for audit logs sorted by `(created_at, id)` descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditLogQuery<'a> {
    /// Optional actor user id filter.
    pub user_id: Option<i64>,
    /// Optional action wire value filter.
    pub action: Option<&'a str>,
    /// Optional entity type wire value filter.
    pub entity_type: Option<&'a str>,
    /// Optional entity id filter.
    pub entity_id: Option<i64>,
    /// Optional inclusive lower created-at bound.
    pub after: Option<DateTime<Utc>>,
    /// Optional inclusive upper created-at bound.
    pub before: Option<DateTime<Utc>>,
    /// Requested page size. Clamped to `1..=200`.
    pub limit: u64,
    /// Cursor from the previous page, encoded as `(created_at, id)`.
    pub cursor: Option<(DateTime<Utc>, i64)>,
}

/// Result of an audit log cursor query.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditLogCursorSlice {
    /// Current page items.
    pub items: Vec<Model>,
    /// Total rows matching the filters before cursor slicing.
    pub total: u64,
    /// Whether another page exists after this slice.
    pub has_more: bool,
}

impl AuditLogCursorSlice {
    fn from_overfetch(mut items: Vec<Model>, total: u64, limit: u64) -> crate::Result<Self> {
        let item_count = u64::try_from(items.len()).map_err(|_| {
            crate::DbError::non_retryable("audit log cursor item count is too large")
        })?;
        let has_more = item_count > limit;
        if has_more {
            let target_len = usize::try_from(limit).map_err(|_| {
                crate::DbError::non_retryable("audit log cursor limit is too large")
            })?;
            items.truncate(target_len);
        }
        Ok(Self {
            items,
            total,
            has_more,
        })
    }
}

impl AuditLogCreate {
    /// Converts the request into a validated SeaORM active model.
    pub fn into_active_model(self) -> crate::Result<ActiveModel> {
        validate_create(&self)?;
        Ok(ActiveModel {
            id: Default::default(),
            user_id: Set(self.user_id),
            action: Set(self.action),
            entity_type: Set(self.entity_type),
            entity_id: Set(self.entity_id),
            entity_name: Set(self.entity_name),
            details: Set(self.details),
            ip_address: Set(self.ip_address),
            user_agent: Set(self.user_agent),
            created_at: Set(self.created_at),
        })
    }
}

/// SeaORM-backed audit log store.
#[derive(Debug, Clone)]
pub struct AuditLogDbStore {
    db: DatabaseConnection,
}

impl AuditLogDbStore {
    /// Creates an audit log store from a SeaORM database connection.
    pub const fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Inserts one audit log row.
    pub async fn create(&self, request: AuditLogCreate) -> crate::Result<Model> {
        create_audit_log_row(&self.db, request).await
    }

    /// Inserts multiple already-built audit log active models.
    pub async fn create_many(&self, models: Vec<ActiveModel>) -> crate::Result<()> {
        create_audit_log_rows(&self.db, models).await
    }

    /// Inserts multiple audit log create requests.
    pub async fn create_many_requests(&self, requests: Vec<AuditLogCreate>) -> crate::Result<()> {
        create_audit_log_requests(&self.db, requests).await
    }

    /// Counts rows created in `[start, end)`.
    pub async fn count_created_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> crate::Result<u64> {
        count_audit_logs_created_between(&self.db, start, end).await
    }

    /// Counts rows for any of the supplied action wire values in `[start, end)`.
    pub async fn count_created_between_with_actions(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        actions: &[&str],
    ) -> crate::Result<u64> {
        count_audit_logs_created_between_with_actions(&self.db, start, end, actions).await
    }

    /// Deletes rows created before the supplied cutoff.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> crate::Result<u64> {
        delete_audit_logs_before(&self.db, before).await
    }

    /// Finds audit logs with shared cursor filtering.
    pub async fn find_with_filters_cursor(
        &self,
        query: AuditLogQuery<'_>,
    ) -> crate::Result<AuditLogCursorSlice> {
        find_audit_logs_with_filters_cursor(&self.db, query).await
    }

    /// Counts distinct positive user ids for any supplied action wire value in `[start, end)`.
    pub async fn count_distinct_users_created_between_with_actions(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        actions: &[&str],
    ) -> crate::Result<u64> {
        count_distinct_audit_log_users_created_between_with_actions(&self.db, start, end, actions)
            .await
    }
}

/// Inserts one audit log row using any SeaORM connection or transaction.
pub async fn create_audit_log_row<C>(db: &C, request: AuditLogCreate) -> crate::Result<Model>
where
    C: ConnectionTrait,
{
    request
        .into_active_model()?
        .insert(db)
        .await
        .map_err(crate::DbError::database_operation)
}

/// Inserts many validated audit log create requests.
pub async fn create_audit_log_requests<C>(
    db: &C,
    requests: Vec<AuditLogCreate>,
) -> crate::Result<()>
where
    C: ConnectionTrait,
{
    if requests.is_empty() {
        return Ok(());
    }
    let models = requests
        .into_iter()
        .map(AuditLogCreate::into_active_model)
        .collect::<crate::Result<Vec<_>>>()?;
    create_audit_log_rows(db, models).await
}

/// Inserts many audit log active models.
pub async fn create_audit_log_rows<C>(db: &C, models: Vec<ActiveModel>) -> crate::Result<()>
where
    C: ConnectionTrait,
{
    if models.is_empty() {
        return Ok(());
    }
    Entity::insert_many(models)
        .exec(db)
        .await
        .map_err(crate::DbError::database_operation)?;
    Ok(())
}

/// Counts audit log rows created in `[start, end)`.
pub async fn count_audit_logs_created_between<C>(
    db: &C,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> crate::Result<u64>
where
    C: ConnectionTrait,
{
    Entity::find()
        .filter(Column::CreatedAt.gte(start))
        .filter(Column::CreatedAt.lt(end))
        .count(db)
        .await
        .map_err(crate::DbError::database_operation)
}

/// Counts audit log rows for any of the supplied action wire values in `[start, end)`.
pub async fn count_audit_logs_created_between_with_actions<C>(
    db: &C,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    actions: &[&str],
) -> crate::Result<u64>
where
    C: ConnectionTrait,
{
    if actions.is_empty() {
        return Ok(0);
    }
    Entity::find()
        .filter(Column::CreatedAt.gte(start))
        .filter(Column::CreatedAt.lt(end))
        .filter(Column::Action.is_in(actions.iter().copied()))
        .count(db)
        .await
        .map_err(crate::DbError::database_operation)
}

/// Counts distinct positive user ids for any supplied action wire value in `[start, end)`.
pub async fn count_distinct_audit_log_users_created_between_with_actions<C>(
    db: &C,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    actions: &[&str],
) -> crate::Result<u64>
where
    C: ConnectionTrait,
{
    if actions.is_empty() {
        return Ok(0);
    }
    let count = Entity::find()
        .select_only()
        .column_as(
            Expr::col(Column::UserId).count_distinct(),
            "distinct_user_count",
        )
        .filter(Column::CreatedAt.gte(start))
        .filter(Column::CreatedAt.lt(end))
        .filter(Column::Action.is_in(actions.iter().copied()))
        .filter(Column::UserId.gt(0))
        .into_tuple::<i64>()
        .one(db)
        .await
        .map_err(crate::DbError::database_operation)?
        .unwrap_or(0);

    u64::try_from(count)
        .map_err(|_| crate::DbError::non_retryable("distinct audit log user count is negative"))
}

/// Finds audit logs with shared cursor filtering.
pub async fn find_audit_logs_with_filters_cursor<C>(
    db: &C,
    query: AuditLogQuery<'_>,
) -> crate::Result<AuditLogCursorSlice>
where
    C: ConnectionTrait,
{
    let mut statement = Entity::find();
    let limit = query.limit.clamp(1, 200);

    if let Some(user_id) = query.user_id {
        statement = statement.filter(Column::UserId.eq(user_id));
    }
    if let Some(action) = query.action {
        statement = statement.filter(Column::Action.eq(action));
    }
    if let Some(entity_type) = query.entity_type {
        statement = statement.filter(Column::EntityType.eq(entity_type));
    }
    if let Some(entity_id) = query.entity_id {
        statement = statement.filter(Column::EntityId.eq(entity_id));
    }
    if let Some(after) = query.after {
        statement = statement.filter(Column::CreatedAt.gte(after));
    }
    if let Some(before) = query.before {
        statement = statement.filter(Column::CreatedAt.lte(before));
    }

    let total = statement
        .clone()
        .count(db)
        .await
        .map_err(crate::DbError::database_operation)?;
    if let Some((created_at, id)) = query.cursor {
        statement = statement.filter(
            Condition::any().add(Column::CreatedAt.lt(created_at)).add(
                Condition::all()
                    .add(Column::CreatedAt.eq(created_at))
                    .add(Column::Id.lt(id)),
            ),
        );
    }

    let items = statement
        .order_by_desc(Column::CreatedAt)
        .order_by_desc(Column::Id)
        .limit(limit.saturating_add(1))
        .all(db)
        .await
        .map_err(crate::DbError::database_operation)?;
    AuditLogCursorSlice::from_overfetch(items, total, limit)
}

/// Deletes audit log rows created before the supplied cutoff.
pub async fn delete_audit_logs_before<C>(db: &C, before: DateTime<Utc>) -> crate::Result<u64>
where
    C: ConnectionTrait,
{
    let result = Entity::delete_many()
        .filter(Column::CreatedAt.lt(before))
        .exec(db)
        .await
        .map_err(crate::DbError::database_operation)?;
    Ok(result.rows_affected)
}

fn validate_create(request: &AuditLogCreate) -> crate::Result<()> {
    if request.user_id < 0 {
        return Err(crate::DbError::non_retryable(
            "audit log user id must be non-negative",
        ));
    }
    validate_non_empty("audit log action", &request.action)?;
    validate_max_len(
        "audit log action",
        &request.action,
        AUDIT_LOG_ACTION_MAX_BYTES,
    )?;
    validate_non_empty("audit log entity type", &request.entity_type)?;
    validate_max_len(
        "audit log entity type",
        &request.entity_type,
        AUDIT_LOG_ENTITY_TYPE_MAX_BYTES,
    )?;
    if let Some(value) = &request.entity_name {
        validate_max_len(
            "audit log entity name",
            value,
            AUDIT_LOG_ENTITY_NAME_MAX_BYTES,
        )?;
    }
    if let Some(value) = &request.ip_address {
        validate_max_len(
            "audit log ip address",
            value,
            AUDIT_LOG_IP_ADDRESS_MAX_BYTES,
        )?;
    }
    if let Some(value) = &request.user_agent {
        validate_max_len(
            "audit log user agent",
            value,
            AUDIT_LOG_USER_AGENT_MAX_BYTES,
        )?;
    }
    Ok(())
}

fn validate_non_empty(name: &str, value: &str) -> crate::Result<()> {
    if value.trim().is_empty() {
        return Err(crate::DbError::non_retryable(format!(
            "{name} cannot be empty"
        )));
    }
    Ok(())
}

fn validate_max_len(name: &str, value: &str, max_len: usize) -> crate::Result<()> {
    if value.len() > max_len {
        return Err(crate::DbError::non_retryable(format!(
            "{name} must be at most {max_len} bytes",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use sea_orm::sea_query::SqliteQueryBuilder;
    use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Set};

    use super::{
        AUDIT_LOG_ACTION_CREATED_USER_INDEX, AUDIT_LOG_CREATED_ID_INDEX,
        AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX, AUDIT_LOG_USER_CREATED_ID_INDEX, ActiveModel,
        AuditLogCreate, AuditLogDbStore, AuditLogQuery, Entity,
        create_audit_logs_action_created_user_index, create_audit_logs_base_indexes,
        create_audit_logs_created_id_index, create_audit_logs_entity_type_created_id_index,
        create_audit_logs_query_indexes, create_audit_logs_table,
        create_audit_logs_user_created_id_index,
    };

    async fn sqlite_store() -> AuditLogDbStore {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("audit log test database should connect");
        db.execute(&create_audit_logs_table(DbBackend::Sqlite))
            .await
            .expect("audit logs table builder should execute");
        for index in create_audit_logs_base_indexes() {
            db.execute(&index)
                .await
                .expect("audit logs base index builder should execute");
        }
        for index in create_audit_logs_query_indexes() {
            db.execute(&index)
                .await
                .expect("audit logs query index builder should execute");
        }
        AuditLogDbStore::new(db)
    }

    fn create_request(created_at: chrono::DateTime<Utc>, action: &str) -> AuditLogCreate {
        AuditLogCreate {
            user_id: 42,
            action: action.to_string(),
            entity_type: "system".to_string(),
            entity_id: Some(7),
            entity_name: Some("server".to_string()),
            details: Some(r#"{"ok":true}"#.to_string()),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            created_at,
        }
    }

    #[test]
    fn create_audit_logs_table_uses_stable_shape() {
        let sql = create_audit_logs_table(DbBackend::Sqlite).to_string(SqliteQueryBuilder);
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"audit_logs\""));
        assert!(sql.contains("\"user_id\""));
        assert!(sql.contains("DEFAULT"));
        assert!(sql.contains("\"action\""));
        assert!(sql.contains("\"entity_type\""));
        assert!(sql.contains("\"ip_address\""));

        assert!(
            create_audit_logs_action_created_user_index()
                .to_string(SqliteQueryBuilder)
                .contains(AUDIT_LOG_ACTION_CREATED_USER_INDEX)
        );
        assert!(
            create_audit_logs_created_id_index()
                .to_string(SqliteQueryBuilder)
                .contains(AUDIT_LOG_CREATED_ID_INDEX)
        );
        assert!(
            create_audit_logs_user_created_id_index()
                .to_string(SqliteQueryBuilder)
                .contains(AUDIT_LOG_USER_CREATED_ID_INDEX)
        );
        assert!(
            create_audit_logs_entity_type_created_id_index()
                .to_string(SqliteQueryBuilder)
                .contains(AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX)
        );
    }

    #[tokio::test]
    async fn audit_log_store_creates_counts_and_deletes_rows() {
        let store = sqlite_store().await;
        let now = Utc::now();

        let created = store
            .create(create_request(now, "server_shutdown"))
            .await
            .expect("audit log row should insert");
        assert_eq!(created.user_id, 42);
        assert_eq!(created.action, "server_shutdown");

        let count = store
            .count_created_between(now - Duration::seconds(1), now + Duration::seconds(1))
            .await
            .expect("audit log count should succeed");
        assert_eq!(count, 1);

        let action_count = store
            .count_created_between_with_actions(
                now - Duration::seconds(1),
                now + Duration::seconds(1),
                &["server_shutdown"],
            )
            .await
            .expect("audit log action count should succeed");
        assert_eq!(action_count, 1);

        let deleted = store
            .delete_before(now + Duration::seconds(1))
            .await
            .expect("audit log delete should succeed");
        assert_eq!(deleted, 1);
    }

    #[tokio::test]
    async fn audit_log_store_filters_cursor_pages_and_counts_distinct_users() {
        let store = sqlite_store().await;
        let base = Utc::now();

        store
            .create_many_requests(vec![
                AuditLogCreate {
                    user_id: 1,
                    created_at: base - Duration::seconds(3),
                    ..create_request(base - Duration::seconds(3), "user_login")
                },
                AuditLogCreate {
                    user_id: 1,
                    created_at: base - Duration::seconds(2),
                    ..create_request(base - Duration::seconds(2), "user_login")
                },
                AuditLogCreate {
                    user_id: 2,
                    entity_type: "profile".to_string(),
                    created_at: base - Duration::seconds(1),
                    ..create_request(base - Duration::seconds(1), "profile_update")
                },
                AuditLogCreate {
                    user_id: 0,
                    created_at: base,
                    ..create_request(base, "user_login")
                },
            ])
            .await
            .expect("audit query fixtures should insert");

        let first_page = store
            .find_with_filters_cursor(AuditLogQuery {
                user_id: None,
                action: Some("user_login"),
                entity_type: None,
                entity_id: None,
                after: Some(base - Duration::seconds(10)),
                before: Some(base + Duration::seconds(1)),
                limit: 2,
                cursor: None,
            })
            .await
            .expect("first audit log page should query");
        assert_eq!(first_page.total, 3);
        assert!(first_page.has_more);
        assert_eq!(first_page.items.len(), 2);
        assert_eq!(first_page.items[0].user_id, 0);

        let cursor = first_page
            .items
            .last()
            .map(|item| (item.created_at, item.id))
            .expect("first page should have a cursor item");
        let second_page = store
            .find_with_filters_cursor(AuditLogQuery {
                user_id: None,
                action: Some("user_login"),
                entity_type: None,
                entity_id: None,
                after: Some(base - Duration::seconds(10)),
                before: Some(base + Duration::seconds(1)),
                limit: 2,
                cursor: Some(cursor),
            })
            .await
            .expect("second audit log page should query");
        assert!(!second_page.has_more);
        assert_eq!(second_page.items.len(), 1);

        let distinct = store
            .count_distinct_users_created_between_with_actions(
                base - Duration::seconds(10),
                base + Duration::seconds(1),
                &["user_login", "profile_update"],
            )
            .await
            .expect("distinct user count should query");
        assert_eq!(distinct, 2);
    }

    #[tokio::test]
    async fn audit_log_store_rejects_invalid_create_values() {
        let store = sqlite_store().await;
        let error = store
            .create(AuditLogCreate {
                action: String::new(),
                ..create_request(Utc::now(), "server_shutdown")
            })
            .await
            .expect_err("empty action should be rejected");
        assert!(error.to_string().contains("audit log action"));
    }

    #[tokio::test]
    async fn audit_log_create_many_accepts_empty_and_inserts_batch() {
        let store = sqlite_store().await;
        store
            .create_many(Vec::new())
            .await
            .expect("empty batch should be accepted");

        let now = Utc::now();
        store
            .create_many(vec![
                ActiveModel {
                    id: Default::default(),
                    user_id: Set(1),
                    action: Set("a".to_string()),
                    entity_type: Set("system".to_string()),
                    entity_id: Set(None),
                    entity_name: Set(None),
                    details: Set(None),
                    ip_address: Set(None),
                    user_agent: Set(None),
                    created_at: Set(now),
                },
                ActiveModel {
                    id: Default::default(),
                    user_id: Set(2),
                    action: Set("b".to_string()),
                    entity_type: Set("system".to_string()),
                    entity_id: Set(None),
                    entity_name: Set(None),
                    details: Set(None),
                    ip_address: Set(None),
                    user_agent: Set(None),
                    created_at: Set(now),
                },
            ])
            .await
            .expect("batch insert should succeed");

        let db = store.db.clone();
        let rows = Entity::find()
            .all(&db)
            .await
            .expect("audit log rows should query");
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn audit_log_create_many_requests_validates_and_inserts_batch() {
        let store = sqlite_store().await;
        let now = Utc::now();

        store
            .create_many_requests(vec![
                create_request(now, "server_start"),
                create_request(now, "server_shutdown"),
            ])
            .await
            .expect("request batch insert should succeed");

        let count = store
            .count_created_between(now - Duration::seconds(1), now + Duration::seconds(1))
            .await
            .expect("audit log count should succeed");
        assert_eq!(count, 2);

        let error = store
            .create_many_requests(vec![AuditLogCreate {
                action: " ".to_string(),
                ..create_request(now, "server_shutdown")
            }])
            .await
            .expect_err("invalid request batch should be rejected");
        assert!(error.to_string().contains("audit log action"));
    }

    #[tokio::test]
    async fn audit_logs_builders_execute_on_sqlite_connection() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("audit log builder test database should connect");
        db.execute(&create_audit_logs_table(DbBackend::Sqlite))
            .await
            .expect("audit logs table builder should execute");
        db.execute(&create_audit_logs_created_id_index())
            .await
            .expect("audit log index builder should execute");
        crate::drop_index_if_exists(&db, super::AUDIT_LOGS_TABLE, AUDIT_LOG_CREATED_ID_INDEX)
            .await
            .expect("shared index helper should drop the audit log index");
    }
}
