//! Database-backed runtime system configuration store.
//!
//! Aster products share the same `system_config` table shape and persistence
//! rules: system definitions are seeded from a product registry, custom values
//! are stored as scalar strings, public/authenticated custom values can be
//! exposed to clients, and startup repairs system metadata without overwriting
//! user-provided values. Product crates still own their configuration
//! definitions, validation callbacks, audit records, and API presentation.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{
    Alias, ColumnDef, Index, IndexCreateStatement, IndexDropStatement, Table, TableCreateStatement,
    TableDropStatement,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DbBackend, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    TryInsertResult,
};

use crate::DbError;
use aster_forge_config::{
    ConfigDefinition, ConfigRegistry, ConfigSeedRecord, ConfigSource, ConfigValue, ConfigValueType,
    ConfigVisibility, RuntimeConfigRecord, present_config_value,
};

/// Shared system configuration table name.
pub const SYSTEM_CONFIG_TABLE: &str = "system_config";
/// Stable row id column.
pub const SYSTEM_CONFIG_ID_COLUMN: &str = "id";
/// Stable configuration key column.
pub const SYSTEM_CONFIG_KEY_COLUMN: &str = "key";
/// Storage value column.
pub const SYSTEM_CONFIG_VALUE_COLUMN: &str = "value";
/// Storage value type column.
pub const SYSTEM_CONFIG_VALUE_TYPE_COLUMN: &str = "value_type";
/// Restart-required marker column.
pub const SYSTEM_CONFIG_REQUIRES_RESTART_COLUMN: &str = "requires_restart";
/// Sensitive-value marker column.
pub const SYSTEM_CONFIG_IS_SENSITIVE_COLUMN: &str = "is_sensitive";
/// System/custom source column.
pub const SYSTEM_CONFIG_SOURCE_COLUMN: &str = "source";
/// Consumer visibility column.
pub const SYSTEM_CONFIG_VISIBILITY_COLUMN: &str = "visibility";
/// Optional product namespace column.
pub const SYSTEM_CONFIG_NAMESPACE_COLUMN: &str = "namespace";
/// Product category column.
pub const SYSTEM_CONFIG_CATEGORY_COLUMN: &str = "category";
/// Product description column.
pub const SYSTEM_CONFIG_DESCRIPTION_COLUMN: &str = "description";
/// Last update timestamp column.
pub const SYSTEM_CONFIG_UPDATED_AT_COLUMN: &str = "updated_at";
/// Optional actor user id column.
pub const SYSTEM_CONFIG_UPDATED_BY_COLUMN: &str = "updated_by";
/// Unique index name for configuration keys.
pub const SYSTEM_CONFIG_KEY_UNIQUE_INDEX: &str = "idx_system_config_key_unique";

/// Builds the shared `system_config` table creation statement.
pub fn create_system_config_table(backend: DatabaseBackend) -> TableCreateStatement {
    Table::create()
        .table(system_config_table())
        .if_not_exists()
        .col(
            ColumnDef::new(system_config_id())
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(
            ColumnDef::new(system_config_key())
                .string_len(128)
                .not_null(),
        )
        .col(ColumnDef::new(system_config_value()).text().not_null())
        .col(
            ColumnDef::new(system_config_value_type())
                .string_len(32)
                .not_null()
                .default(ConfigValueType::String.as_str()),
        )
        .col(
            ColumnDef::new(system_config_requires_restart())
                .boolean()
                .not_null()
                .default(false),
        )
        .col(
            ColumnDef::new(system_config_is_sensitive())
                .boolean()
                .not_null()
                .default(false),
        )
        .col(
            ColumnDef::new(system_config_source())
                .string_len(16)
                .not_null()
                .default(ConfigSource::System.as_str()),
        )
        .col(
            ColumnDef::new(system_config_visibility())
                .string_len(16)
                .not_null()
                .default(ConfigVisibility::Private.as_str()),
        )
        .col(
            ColumnDef::new(system_config_namespace())
                .string_len(64)
                .not_null()
                .default(""),
        )
        .col(
            ColumnDef::new(system_config_category())
                .string_len(64)
                .not_null(),
        )
        .col(
            ColumnDef::new(system_config_description())
                .string_len(512)
                .not_null(),
        )
        .col(utc_datetime_column(backend, system_config_updated_at()).not_null())
        .col(
            ColumnDef::new(system_config_updated_by())
                .big_integer()
                .null(),
        )
        .to_owned()
}

/// Builds the shared `system_config` table drop statement.
pub fn drop_system_config_table() -> TableDropStatement {
    Table::drop()
        .table(system_config_table())
        .if_exists()
        .to_owned()
}

/// Builds the unique index for stable configuration keys.
pub fn create_system_config_key_unique_index() -> IndexCreateStatement {
    Index::create()
        .name(SYSTEM_CONFIG_KEY_UNIQUE_INDEX)
        .table(system_config_table())
        .col(system_config_key())
        .unique()
        .if_not_exists()
        .to_owned()
}

/// Builds the system config key unique index drop statement.
pub fn drop_system_config_key_unique_index() -> IndexDropStatement {
    Index::drop()
        .name(SYSTEM_CONFIG_KEY_UNIQUE_INDEX)
        .table(system_config_table())
        .if_exists()
        .to_owned()
}

fn system_config_table() -> Alias {
    Alias::new(SYSTEM_CONFIG_TABLE)
}

fn system_config_id() -> Alias {
    Alias::new(SYSTEM_CONFIG_ID_COLUMN)
}

fn system_config_key() -> Alias {
    Alias::new(SYSTEM_CONFIG_KEY_COLUMN)
}

fn system_config_value() -> Alias {
    Alias::new(SYSTEM_CONFIG_VALUE_COLUMN)
}

fn system_config_value_type() -> Alias {
    Alias::new(SYSTEM_CONFIG_VALUE_TYPE_COLUMN)
}

fn system_config_requires_restart() -> Alias {
    Alias::new(SYSTEM_CONFIG_REQUIRES_RESTART_COLUMN)
}

fn system_config_is_sensitive() -> Alias {
    Alias::new(SYSTEM_CONFIG_IS_SENSITIVE_COLUMN)
}

fn system_config_source() -> Alias {
    Alias::new(SYSTEM_CONFIG_SOURCE_COLUMN)
}

fn system_config_visibility() -> Alias {
    Alias::new(SYSTEM_CONFIG_VISIBILITY_COLUMN)
}

fn system_config_namespace() -> Alias {
    Alias::new(SYSTEM_CONFIG_NAMESPACE_COLUMN)
}

fn system_config_category() -> Alias {
    Alias::new(SYSTEM_CONFIG_CATEGORY_COLUMN)
}

fn system_config_description() -> Alias {
    Alias::new(SYSTEM_CONFIG_DESCRIPTION_COLUMN)
}

fn system_config_updated_at() -> Alias {
    Alias::new(SYSTEM_CONFIG_UPDATED_AT_COLUMN)
}

fn system_config_updated_by() -> Alias {
    Alias::new(SYSTEM_CONFIG_UPDATED_BY_COLUMN)
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

/// Runtime system configuration SeaORM model.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "system_config")]
pub struct Model {
    /// Stable row id.
    #[sea_orm(primary_key)]
    pub id: i64,
    /// Stable configuration key.
    #[sea_orm(unique)]
    pub key: String,
    /// Storage value. List-like values are JSON strings.
    pub value: String,
    /// Storage value type.
    pub value_type: ConfigValueType,
    /// Whether changes require process restart to take effect.
    pub requires_restart: bool,
    /// Whether APIs and audit logs should redact this value.
    pub is_sensitive: bool,
    /// System-defined or custom user-defined source.
    pub source: ConfigSource,
    /// Consumer visibility.
    pub visibility: ConfigVisibility,
    /// Optional product namespace. Existing Aster services use an empty namespace.
    pub namespace: String,
    /// Product category for UI grouping.
    pub category: String,
    /// Product description for admin UIs.
    pub description: String,
    /// Last update timestamp.
    pub updated_at: DateTimeUtc,
    /// Optional actor user id.
    pub updated_by: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl RuntimeConfigRecord for Model {
    fn config_key(&self) -> &str {
        &self.key
    }

    fn config_value(&self) -> &str {
        &self.value
    }

    fn config_requires_restart(&self) -> bool {
        self.requires_restart
    }
}

/// API-facing representation of a stored system configuration row.
///
/// This keeps the product-neutral field mapping, sensitive-value redaction, and lossy historical
/// value parsing in Forge while leaving product API envelopes, permissions, warning calculation,
/// and OpenAPI schema ownership in each service.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentedSystemConfig {
    /// Stable row id.
    pub id: i64,
    /// Stable configuration key.
    pub key: String,
    /// API-facing value with sensitive rows redacted.
    pub value: ConfigValue,
    /// Storage value type.
    pub value_type: ConfigValueType,
    /// Whether changes require process restart to take effect.
    pub requires_restart: bool,
    /// Whether APIs and audit logs should redact this value.
    pub is_sensitive: bool,
    /// System-defined or custom user-defined source.
    pub source: ConfigSource,
    /// Consumer visibility.
    pub visibility: ConfigVisibility,
    /// Optional product namespace.
    pub namespace: String,
    /// Product category for UI grouping.
    pub category: String,
    /// Product description for admin UIs.
    pub description: String,
    /// Last update timestamp.
    pub updated_at: DateTimeUtc,
    /// Optional actor user id.
    pub updated_by: Option<i64>,
}

impl PresentedSystemConfig {
    /// Converts a stored model into an API-facing row.
    pub fn from_model(
        model: Model,
        on_invalid: impl FnOnce(&aster_forge_config::ConfigCoreError),
    ) -> Self {
        let value = present_config_value(
            model.value_type,
            model.value,
            model.is_sensitive,
            on_invalid,
        );
        Self {
            id: model.id,
            key: model.key,
            value,
            value_type: model.value_type,
            requires_restart: model.requires_restart,
            is_sensitive: model.is_sensitive,
            source: model.source,
            visibility: model.visibility,
            namespace: model.namespace,
            category: model.category,
            description: model.description,
            updated_at: model.updated_at,
            updated_by: model.updated_by,
        }
    }
}

/// Converts a stored system config row into an API-facing presentation row.
pub fn present_system_config(
    model: Model,
    on_invalid: impl FnOnce(&aster_forge_config::ConfigCoreError),
) -> PresentedSystemConfig {
    PresentedSystemConfig::from_model(model, on_invalid)
}

/// Page slice returned by cursor-style repository queries.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemConfigCursorSlice {
    /// Items to expose after overfetch trimming.
    pub items: Vec<Model>,
    /// Total number of matching rows.
    pub total: u64,
    /// Whether one more row was found beyond the requested limit.
    pub has_more: bool,
}

impl SystemConfigCursorSlice {
    fn empty(total: u64) -> Self {
        Self {
            items: Vec::new(),
            total,
            has_more: false,
        }
    }

    fn from_overfetch(mut items: Vec<Model>, total: u64, limit: u64) -> crate::Result<Self> {
        let item_count = u64::try_from(items.len())
            .map_err(|_| DbError::non_retryable("system config page item count is too large"))?;
        let has_more = item_count > limit;
        if has_more {
            let limit = usize::try_from(limit)
                .map_err(|_| DbError::non_retryable("system config page limit is too large"))?;
            items.truncate(limit);
        }
        Ok(Self {
            items,
            total,
            has_more,
        })
    }
}

/// Product binding for the shared system config store.
///
/// A product normally has one static binding that supplies its registry and deprecated-key list.
/// Repository code can then focus on product error mapping, authorization, and API cursor shape
/// instead of repeatedly passing the same registry values into every call.
#[derive(Clone, Copy)]
pub struct SystemConfigDbBinding {
    registry: &'static ConfigRegistry,
    deprecated_keys: &'static [&'static str],
}

impl SystemConfigDbBinding {
    /// Creates a product binding from a config registry and deprecated key list.
    pub const fn new(
        registry: &'static ConfigRegistry,
        deprecated_keys: &'static [&'static str],
    ) -> Self {
        Self {
            registry,
            deprecated_keys,
        }
    }

    /// Lists all rows by stable id order.
    pub async fn find_all<C: ConnectionTrait>(&self, db: &C) -> crate::Result<Vec<Model>> {
        find_all(db).await
    }

    /// Lists one id-cursor page by stable id order.
    pub async fn find_cursor<C: ConnectionTrait>(
        &self,
        db: &C,
        limit: u64,
        after_id: Option<i64>,
    ) -> crate::Result<SystemConfigCursorSlice> {
        find_cursor(db, limit, after_id).await
    }

    /// Finds one row by key.
    pub async fn find_by_key<C: ConnectionTrait>(
        &self,
        db: &C,
        key: &str,
    ) -> crate::Result<Option<Model>> {
        find_by_key(db, key).await
    }

    /// Lists visible custom rows ordered by key.
    pub async fn find_visible_custom<C: ConnectionTrait>(
        &self,
        db: &C,
        include_authenticated: bool,
    ) -> crate::Result<Vec<Model>> {
        find_visible_custom(db, include_authenticated).await
    }

    /// Locks one row by key where the database supports row locks.
    pub async fn lock_by_key<C: ConnectionTrait>(&self, db: &C, key: &str) -> crate::Result<()> {
        lock_by_key(db, key).await
    }

    /// Upserts one row using this binding's registry metadata for known system keys.
    pub async fn upsert<C: ConnectionTrait>(
        &self,
        db: &C,
        request: SystemConfigUpsert<'_>,
    ) -> crate::Result<Model> {
        upsert(db, self.registry, request).await
    }

    /// Deletes a custom row.
    pub async fn delete_by_key<C: ConnectionTrait>(&self, db: &C, key: &str) -> crate::Result<()> {
        delete_by_key(db, key).await
    }

    /// Inserts one system value if no row exists.
    pub async fn ensure_system_value_if_missing<C: ConnectionTrait>(
        &self,
        db: &C,
        key: &str,
        value: &str,
    ) -> crate::Result<bool> {
        ensure_system_value_if_missing(db, self.registry, key, value).await
    }

    /// Deletes deprecated system keys configured by the product.
    pub async fn delete_deprecated_keys<C: ConnectionTrait>(&self, db: &C) -> crate::Result<u64> {
        delete_deprecated_keys(db, self.deprecated_keys).await
    }

    /// Ensures default rows exist and repairs metadata for existing system rows.
    pub async fn ensure_defaults<C: ConnectionTrait>(&self, db: &C) -> crate::Result<usize> {
        ensure_defaults(db, self.registry, self.deprecated_keys).await
    }
}

/// Product request to upsert one system or custom configuration value.
#[derive(Debug, Clone, Copy)]
pub struct SystemConfigUpsert<'a> {
    /// Config key.
    pub key: &'a str,
    /// New storage value.
    pub value: &'a str,
    /// Visibility override for custom keys. System visibility comes from the registry.
    pub visibility: Option<ConfigVisibility>,
    /// Optional actor user id.
    pub updated_by: Option<i64>,
}

/// SeaORM-backed system configuration store.
#[derive(Clone)]
pub struct SystemConfigDbStore {
    db: DatabaseConnection,
    registry: &'static ConfigRegistry,
    deprecated_keys: &'static [&'static str],
}

impl SystemConfigDbStore {
    /// Creates a store from a database connection and product config registry.
    pub const fn new(
        db: DatabaseConnection,
        registry: &'static ConfigRegistry,
        deprecated_keys: &'static [&'static str],
    ) -> Self {
        Self {
            db,
            registry,
            deprecated_keys,
        }
    }

    /// Lists all rows by stable id order.
    pub async fn find_all(&self) -> crate::Result<Vec<Model>> {
        find_all(&self.db).await
    }

    /// Lists one id-cursor page by stable id order.
    pub async fn find_cursor(
        &self,
        limit: u64,
        after_id: Option<i64>,
    ) -> crate::Result<SystemConfigCursorSlice> {
        find_cursor(&self.db, limit, after_id).await
    }

    /// Finds one row by key.
    pub async fn find_by_key(&self, key: &str) -> crate::Result<Option<Model>> {
        find_by_key(&self.db, key).await
    }

    /// Lists visible custom rows ordered by key.
    pub async fn find_visible_custom(
        &self,
        include_authenticated: bool,
    ) -> crate::Result<Vec<Model>> {
        find_visible_custom(&self.db, include_authenticated).await
    }

    /// Locks one row by key where the database supports row locks.
    pub async fn lock_by_key(&self, key: &str) -> crate::Result<()> {
        lock_by_key(&self.db, key).await
    }

    /// Upserts one row using registry metadata for known system keys.
    pub async fn upsert(&self, request: SystemConfigUpsert<'_>) -> crate::Result<Model> {
        upsert(&self.db, self.registry, request).await
    }

    /// Deletes a custom row.
    pub async fn delete_by_key(&self, key: &str) -> crate::Result<()> {
        delete_by_key(&self.db, key).await
    }

    /// Inserts one system value if no row exists.
    pub async fn ensure_system_value_if_missing(
        &self,
        key: &str,
        value: &str,
    ) -> crate::Result<bool> {
        ensure_system_value_if_missing(&self.db, self.registry, key, value).await
    }

    /// Deletes deprecated system keys configured by the product.
    pub async fn delete_deprecated_keys(&self) -> crate::Result<u64> {
        delete_deprecated_keys(&self.db, self.deprecated_keys).await
    }

    /// Ensures default rows exist and repairs metadata for existing system rows.
    pub async fn ensure_defaults(&self) -> crate::Result<usize> {
        ensure_defaults(&self.db, self.registry, self.deprecated_keys).await
    }
}

/// Lists all rows by stable id order.
pub async fn find_all<C: ConnectionTrait>(db: &C) -> crate::Result<Vec<Model>> {
    Entity::find()
        .order_by_asc(Column::Id)
        .all(db)
        .await
        .map_err(DbError::from)
}

/// Lists one id-cursor page by stable id order.
pub async fn find_cursor<C: ConnectionTrait>(
    db: &C,
    limit: u64,
    after_id: Option<i64>,
) -> crate::Result<SystemConfigCursorSlice> {
    let limit = limit.clamp(1, 100);
    let base = Entity::find();
    let total = base.clone().count(db).await.map_err(DbError::from)?;
    if total == 0 {
        return Ok(SystemConfigCursorSlice::empty(total));
    }

    let mut query = base;
    if let Some(after_id) = after_id {
        query = query.filter(Column::Id.gt(after_id));
    }

    let items = query
        .order_by_asc(Column::Id)
        .limit(limit.saturating_add(1))
        .all(db)
        .await
        .map_err(DbError::from)?;
    SystemConfigCursorSlice::from_overfetch(items, total, limit)
}

/// Finds one row by key.
pub async fn find_by_key<C: ConnectionTrait>(db: &C, key: &str) -> crate::Result<Option<Model>> {
    Entity::find()
        .filter(Column::Key.eq(key))
        .one(db)
        .await
        .map_err(DbError::from)
}

/// Lists visible custom rows ordered by key.
pub async fn find_visible_custom<C: ConnectionTrait>(
    db: &C,
    include_authenticated: bool,
) -> crate::Result<Vec<Model>> {
    let mut visibility_filter =
        Condition::any().add(Column::Visibility.eq(ConfigVisibility::Public));
    if include_authenticated {
        visibility_filter =
            visibility_filter.add(Column::Visibility.eq(ConfigVisibility::Authenticated));
    }

    Entity::find()
        .filter(Column::Source.eq(ConfigSource::Custom))
        .filter(visibility_filter)
        .order_by_asc(Column::Key)
        .all(db)
        .await
        .map_err(DbError::from)
}

/// Locks one row by key where the database supports row locks.
pub async fn lock_by_key<C: ConnectionTrait>(db: &C, key: &str) -> crate::Result<()> {
    let query = Entity::find().filter(Column::Key.eq(key));
    let config = match db.get_database_backend() {
        DbBackend::Postgres | DbBackend::MySql => query
            .lock_exclusive()
            .one(db)
            .await
            .map_err(DbError::from)?,
        _ => query.one(db).await.map_err(DbError::from)?,
    };

    config
        .map(|_| ())
        .ok_or_else(|| DbError::non_retryable(format!("config key '{key}' not found")))
}

/// Upserts one row using registry metadata for known system keys.
pub async fn upsert<C: ConnectionTrait>(
    db: &C,
    registry: &'static ConfigRegistry,
    request: SystemConfigUpsert<'_>,
) -> crate::Result<Model> {
    let now = Utc::now();
    let definition = registry.get(request.key);
    let is_custom_key = definition.is_none();
    let active = definition
        .map(|def| {
            build_system_active_model(def, request.value.to_string(), now, request.updated_by)
        })
        .unwrap_or_else(|| {
            build_custom_active_model(
                request.key,
                request.value.to_string(),
                request.visibility.unwrap_or_default(),
                now,
                request.updated_by,
            )
        });
    let inserted = insert_do_nothing(active, db, "system config upsert").await?;

    if !inserted {
        let existing = find_by_key(db, request.key).await?.ok_or_else(|| {
            DbError::non_retryable(format!("config key '{}' not found", request.key))
        })?;
        let mut active: ActiveModel = existing.into();
        active.value = Set(request.value.to_string());
        if is_custom_key && let Some(visibility) = request.visibility {
            active.visibility = Set(visibility);
        }
        active.updated_at = Set(now);
        active.updated_by = Set(request.updated_by);
        active.update(db).await.map_err(DbError::from)?;
    }

    find_by_key(db, request.key)
        .await?
        .ok_or_else(|| DbError::non_retryable(format!("config key '{}' not found", request.key)))
}

/// Deletes a custom row.
pub async fn delete_by_key<C: ConnectionTrait>(db: &C, key: &str) -> crate::Result<()> {
    let existing = find_by_key(db, key)
        .await?
        .ok_or_else(|| DbError::non_retryable(format!("config key '{key}' not found")))?;

    if existing.source == ConfigSource::System {
        return Err(DbError::non_retryable("cannot delete system configuration"));
    }

    Entity::delete_by_id(existing.id)
        .exec(db)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// Inserts one system value if no row exists.
pub async fn ensure_system_value_if_missing<C: ConnectionTrait>(
    db: &C,
    registry: &'static ConfigRegistry,
    key: &str,
    value: &str,
) -> crate::Result<bool> {
    let def = registry
        .get(key)
        .ok_or_else(|| DbError::non_retryable(format!("config key '{key}' not found")))?;
    let now = Utc::now();
    insert_do_nothing(
        build_system_active_model(def, value.to_string(), now, None),
        db,
        "ensure_system_value_if_missing",
    )
    .await
}

/// Deletes deprecated system keys configured by the product.
pub async fn delete_deprecated_keys<C: ConnectionTrait>(
    db: &C,
    deprecated_keys: &'static [&'static str],
) -> crate::Result<u64> {
    if deprecated_keys.is_empty() {
        return Ok(0);
    }

    let result = Entity::delete_many()
        .filter(Column::Key.is_in(deprecated_keys.iter().copied()))
        .exec(db)
        .await
        .map_err(DbError::from)?;

    if result.rows_affected > 0 {
        tracing::info!(
            count = result.rows_affected,
            keys = ?deprecated_keys,
            "deleted deprecated system config keys"
        );
    }

    Ok(result.rows_affected)
}

/// Ensures default rows exist and repairs metadata for existing system rows.
pub async fn ensure_defaults<C: ConnectionTrait>(
    db: &C,
    registry: &'static ConfigRegistry,
    deprecated_keys: &'static [&'static str],
) -> crate::Result<usize> {
    let mut count = 0;

    delete_deprecated_keys(db, deprecated_keys).await?;

    for seed in registry
        .default_seed_records()
        .map_err(DbError::non_retryable)?
    {
        let now = Utc::now();
        let key = seed.key.clone();
        let inserted = insert_do_nothing(
            build_system_active_model_from_seed(seed, now, None),
            db,
            "ensure_defaults",
        )
        .await?;

        if inserted {
            count += 1;
            continue;
        }

        let def = registry.require(&key).map_err(DbError::non_retryable)?;
        let existing = find_by_key(db, def.key)
            .await?
            .ok_or_else(|| DbError::non_retryable(format!("config key '{}' not found", def.key)))?;
        let mut active: ActiveModel = existing.into();
        active.source = Set(ConfigSource::System);
        active.value_type = Set(def.value_type);
        active.requires_restart = Set(def.requires_restart);
        active.is_sensitive = Set(def.is_sensitive);
        active.visibility = Set(def.visibility);
        active.category = Set(def.category.to_string());
        active.description = Set(def.description.to_string());
        active.update(db).await.map_err(DbError::from)?;
    }

    if count > 0 {
        tracing::info!("initialized {count} default configuration items");
    }

    Ok(count)
}

async fn insert_do_nothing<C: ConnectionTrait>(
    active: ActiveModel,
    db: &C,
    operation: &'static str,
) -> crate::Result<bool> {
    match Entity::insert(active)
        .on_conflict_do_nothing_on([Column::Key])
        .exec(db)
        .await
        .map_err(DbError::from)?
    {
        TryInsertResult::Inserted(_) => Ok(true),
        TryInsertResult::Conflicted => Ok(false),
        TryInsertResult::Empty => Err(DbError::database_operation(format!(
            "{operation} produced empty insert result"
        ))),
    }
}

fn build_system_active_model(
    def: &ConfigDefinition,
    value: String,
    now: DateTime<Utc>,
    updated_by: Option<i64>,
) -> ActiveModel {
    ActiveModel {
        key: Set(def.key.to_string()),
        value: Set(value),
        value_type: Set(def.value_type),
        requires_restart: Set(def.requires_restart),
        is_sensitive: Set(def.is_sensitive),
        source: Set(ConfigSource::System),
        visibility: Set(def.visibility),
        namespace: Set(String::new()),
        category: Set(def.category.to_string()),
        description: Set(def.description.to_string()),
        updated_at: Set(now),
        updated_by: Set(updated_by),
        ..Default::default()
    }
}

fn build_system_active_model_from_seed(
    seed: ConfigSeedRecord,
    now: DateTime<Utc>,
    updated_by: Option<i64>,
) -> ActiveModel {
    ActiveModel {
        key: Set(seed.key),
        value: Set(seed.value),
        value_type: Set(seed.value_type),
        requires_restart: Set(seed.requires_restart),
        is_sensitive: Set(seed.is_sensitive),
        source: Set(seed.source),
        visibility: Set(seed.visibility),
        namespace: Set(String::new()),
        category: Set(seed.category),
        description: Set(seed.description),
        updated_at: Set(now),
        updated_by: Set(updated_by),
        ..Default::default()
    }
}

fn build_custom_active_model(
    key: &str,
    value: String,
    visibility: ConfigVisibility,
    now: DateTime<Utc>,
    updated_by: Option<i64>,
) -> ActiveModel {
    ActiveModel {
        key: Set(key.to_string()),
        value: Set(value),
        value_type: Set(ConfigValueType::String),
        requires_restart: Set(false),
        is_sensitive: Set(false),
        source: Set(ConfigSource::Custom),
        visibility: Set(visibility),
        namespace: Set(String::new()),
        category: Set(String::new()),
        description: Set(String::new()),
        updated_at: Set(now),
        updated_by: Set(updated_by),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveModel, Column, Entity, Model, SystemConfigDbBinding, SystemConfigDbStore,
        SystemConfigUpsert, create_system_config_key_unique_index, create_system_config_table,
        present_system_config,
    };
    use aster_forge_config::{
        ConfigDefinition, ConfigRegistry, ConfigSource, ConfigValue, ConfigValueType,
        ConfigVisibility, config_value_audit_string,
    };
    use chrono::Utc;
    use sea_orm::sea_query::{MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};
    use sea_orm::{
        ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseBackend, EntityTrait,
        QueryFilter, Set,
    };

    const PRIMARY_KEY: &str = "primary_key";
    const ARRAY_KEY: &str = "array_key";
    const DEPRECATED_KEY: &str = "deprecated_key";

    fn primary_default() -> String {
        "primary default".to_string()
    }

    fn array_default() -> String {
        "[\"https://example.com\"]".to_string()
    }

    const PRIMARY: ConfigDefinition = ConfigDefinition {
        key: PRIMARY_KEY,
        default_fn: primary_default,
        value_type: ConfigValueType::String,
        category: "site.branding",
        description: "Primary config",
        ..ConfigDefinition::private_system()
    };

    const ARRAY: ConfigDefinition = ConfigDefinition {
        key: ARRAY_KEY,
        default_fn: array_default,
        value_type: ConfigValueType::StringArray,
        category: "site.public",
        description: "Array config",
        visibility: ConfigVisibility::Public,
        ..ConfigDefinition::private_system()
    };

    static REGISTRY: ConfigRegistry = ConfigRegistry::new(&[PRIMARY, ARRAY]);
    static DEPRECATED: &[&str] = &[DEPRECATED_KEY];
    static BINDING: SystemConfigDbBinding = SystemConfigDbBinding::new(&REGISTRY, DEPRECATED);

    async fn sqlite_store() -> SystemConfigDbStore {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let backend = db.get_database_backend();
        db.execute(&create_system_config_table(backend))
            .await
            .expect("system_config table builder should execute");
        db.execute(&create_system_config_key_unique_index())
            .await
            .expect("system_config key index builder should execute");
        SystemConfigDbStore::new(db, &REGISTRY, DEPRECATED)
    }

    async fn sqlite_db_from_builders() -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        let backend = db.get_database_backend();
        db.execute(&create_system_config_table(backend))
            .await
            .expect("system_config table builder should execute");
        db.execute(&create_system_config_key_unique_index())
            .await
            .expect("system_config key index builder should execute");
        db
    }

    fn create_table_sql(backend: DatabaseBackend) -> String {
        let table = create_system_config_table(backend);
        match backend {
            DatabaseBackend::MySql => table.to_string(MysqlQueryBuilder),
            DatabaseBackend::Postgres => table.to_string(PostgresQueryBuilder),
            DatabaseBackend::Sqlite => table.to_string(SqliteQueryBuilder),
            _ => unreachable!("unsupported backend in system config table test"),
        }
    }

    #[test]
    fn create_system_config_table_uses_stable_shape() {
        let sqlite_sql = create_table_sql(DatabaseBackend::Sqlite);
        assert!(sqlite_sql.contains("CREATE TABLE IF NOT EXISTS \"system_config\""));
        assert!(sqlite_sql.contains("\"key\" varchar(128) NOT NULL"));
        assert!(sqlite_sql.contains("\"value_type\" varchar(32) NOT NULL DEFAULT 'string'"));
        assert!(sqlite_sql.contains("\"source\" varchar(16) NOT NULL DEFAULT 'system'"));
        assert!(sqlite_sql.contains("\"visibility\" varchar(16) NOT NULL DEFAULT 'private'"));
        assert!(sqlite_sql.contains("\"category\" varchar(64) NOT NULL"));
        assert!(sqlite_sql.contains("\"description\" varchar(512) NOT NULL"));
        assert!(sqlite_sql.contains("\"updated_at\" timestamp_with_timezone_text NOT NULL"));

        let key_index = create_system_config_key_unique_index().to_string(SqliteQueryBuilder);
        assert!(key_index.contains("idx_system_config_key_unique"));
        assert!(key_index.contains("\"key\""));

        let mysql_sql = create_table_sql(DatabaseBackend::MySql);
        assert!(mysql_sql.contains("`updated_at` datetime(6) NOT NULL"));

        let postgres_sql = create_table_sql(DatabaseBackend::Postgres);
        assert!(postgres_sql.contains("\"updated_at\" timestamp with time zone NOT NULL"));
    }

    #[tokio::test]
    async fn ensure_defaults_inserts_once_and_repairs_metadata() {
        let store = sqlite_store().await;

        assert_eq!(store.ensure_defaults().await.unwrap(), 2);
        assert_eq!(store.ensure_defaults().await.unwrap(), 0);

        let mut active: ActiveModel = store
            .find_by_key(PRIMARY_KEY)
            .await
            .unwrap()
            .unwrap()
            .into();
        active.source = Set(ConfigSource::Custom);
        active.value_type = Set(ConfigValueType::Number);
        active.requires_restart = Set(true);
        active.is_sensitive = Set(true);
        active.visibility = Set(ConfigVisibility::Authenticated);
        active.category = Set("wrong".to_string());
        active.description = Set("wrong".to_string());
        active.update(&store.db).await.unwrap();

        assert_eq!(store.ensure_defaults().await.unwrap(), 0);
        let repaired = store.find_by_key(PRIMARY_KEY).await.unwrap().unwrap();
        assert_eq!(repaired.source, ConfigSource::System);
        assert_eq!(repaired.value_type, ConfigValueType::String);
        assert!(!repaired.requires_restart);
        assert!(!repaired.is_sensitive);
        assert_eq!(repaired.visibility, ConfigVisibility::Private);
        assert_eq!(repaired.category, "site.branding");
        assert_eq!(repaired.description, "Primary config");
    }

    #[tokio::test]
    async fn ensure_defaults_deletes_deprecated_keys() {
        let store = sqlite_store().await;

        store
            .upsert(SystemConfigUpsert {
                key: DEPRECATED_KEY,
                value: "old",
                visibility: None,
                updated_by: None,
            })
            .await
            .unwrap();

        assert_eq!(store.ensure_defaults().await.unwrap(), 2);
        assert!(store.find_by_key(DEPRECATED_KEY).await.unwrap().is_none());
        assert_eq!(store.delete_deprecated_keys().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn binding_uses_product_registry_and_deprecated_keys() {
        let db = sqlite_db_from_builders().await;

        BINDING
            .upsert(
                &db,
                SystemConfigUpsert {
                    key: DEPRECATED_KEY,
                    value: "old",
                    visibility: None,
                    updated_by: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(BINDING.ensure_defaults(&db).await.unwrap(), 2);
        assert!(
            BINDING
                .find_by_key(&db, DEPRECATED_KEY)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(BINDING.delete_deprecated_keys(&db).await.unwrap(), 0);

        let known = BINDING
            .find_by_key(&db, PRIMARY_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(known.source, ConfigSource::System);
        assert_eq!(known.value, "primary default");
    }

    #[tokio::test]
    async fn product_integration_flow_uses_builders_store_and_presentation_helpers() {
        let db = sqlite_db_from_builders().await;

        assert_eq!(BINDING.ensure_defaults(&db).await.unwrap(), 2);
        let system = BINDING
            .upsert(
                &db,
                SystemConfigUpsert {
                    key: PRIMARY_KEY,
                    value: "operator title",
                    visibility: None,
                    updated_by: Some(42),
                },
            )
            .await
            .unwrap();
        assert_eq!(system.source, ConfigSource::System);
        assert_eq!(system.updated_by, Some(42));

        let custom = BINDING
            .upsert(
                &db,
                SystemConfigUpsert {
                    key: "custom.banner",
                    value: "hello",
                    visibility: Some(ConfigVisibility::Public),
                    updated_by: Some(7),
                },
            )
            .await
            .unwrap();
        assert_eq!(custom.source, ConfigSource::Custom);

        let visible = BINDING.find_visible_custom(&db, true).await.unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["custom.banner"]
        );

        let presented = present_system_config(custom, |_| {
            unreachable!("valid scalar config should not report invalid storage")
        });
        assert_eq!(presented.value, ConfigValue::String("hello".to_string()));

        let sensitive = Model {
            is_sensitive: true,
            value: "secret".to_string(),
            ..system
        };
        let presented_sensitive = present_system_config(sensitive.clone(), |_| {
            unreachable!("sensitive config should not parse storage")
        });
        assert_eq!(presented_sensitive.value, ConfigValue::redacted());

        let audit_value = config_value_audit_string(
            sensitive.value_type,
            sensitive.value,
            sensitive.is_sensitive,
            |_| unreachable!("sensitive config should not parse storage"),
        );
        assert_eq!(audit_value, ConfigValue::REDACTED);
    }

    #[tokio::test]
    async fn upsert_system_and_custom_config_preserves_metadata() {
        let store = sqlite_store().await;

        let system = store
            .upsert(SystemConfigUpsert {
                key: PRIMARY_KEY,
                value: "Custom Title",
                visibility: None,
                updated_by: Some(42),
            })
            .await
            .unwrap();
        assert_eq!(system.value, "Custom Title");
        assert_eq!(system.updated_by, Some(42));
        assert_eq!(system.source, ConfigSource::System);
        assert_eq!(system.visibility, ConfigVisibility::Private);
        assert_eq!(system.value_type, ConfigValueType::String);

        let custom = store
            .upsert(SystemConfigUpsert {
                key: "custom_public_banner",
                value: "hello",
                visibility: Some(ConfigVisibility::Public),
                updated_by: Some(7),
            })
            .await
            .unwrap();
        assert_eq!(custom.source, ConfigSource::Custom);
        assert_eq!(custom.visibility, ConfigVisibility::Public);
        assert_eq!(custom.value_type, ConfigValueType::String);
        assert_eq!(custom.updated_by, Some(7));

        let updated_custom = store
            .upsert(SystemConfigUpsert {
                key: "custom_public_banner",
                value: "hello again",
                visibility: Some(ConfigVisibility::Authenticated),
                updated_by: None,
            })
            .await
            .unwrap();
        assert_eq!(updated_custom.id, custom.id);
        assert_eq!(updated_custom.value, "hello again");
        assert_eq!(updated_custom.visibility, ConfigVisibility::Authenticated);
        assert_eq!(updated_custom.updated_by, None);
    }

    #[tokio::test]
    async fn find_visible_custom_filters_visibility_and_orders_by_key() {
        let store = sqlite_store().await;
        store.ensure_defaults().await.unwrap();
        for (key, visibility) in [
            ("visible_public", ConfigVisibility::Public),
            ("visible_authenticated", ConfigVisibility::Authenticated),
            ("visible_private", ConfigVisibility::Private),
        ] {
            store
                .upsert(SystemConfigUpsert {
                    key,
                    value: key,
                    visibility: Some(visibility),
                    updated_by: None,
                })
                .await
                .unwrap();
        }

        let public_only = store.find_visible_custom(false).await.unwrap();
        assert_eq!(
            public_only
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["visible_public"]
        );

        let public_and_authenticated = store.find_visible_custom(true).await.unwrap();
        assert_eq!(
            public_and_authenticated
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["visible_authenticated", "visible_public"]
        );
    }

    #[tokio::test]
    async fn delete_rejects_system_config_and_removes_custom_config() {
        let store = sqlite_store().await;
        store.ensure_defaults().await.unwrap();
        store
            .upsert(SystemConfigUpsert {
                key: "custom_delete_me",
                value: "value",
                visibility: None,
                updated_by: None,
            })
            .await
            .unwrap();

        let system_error = store.delete_by_key(PRIMARY_KEY).await.unwrap_err();
        assert!(
            system_error
                .to_string()
                .contains("cannot delete system configuration")
        );

        store.delete_by_key("custom_delete_me").await.unwrap();
        assert!(
            store
                .find_by_key("custom_delete_me")
                .await
                .unwrap()
                .is_none()
        );

        let missing_error = store.delete_by_key("missing_custom").await.unwrap_err();
        assert!(missing_error.to_string().contains("missing_custom"));
    }

    #[tokio::test]
    async fn find_cursor_and_lock_by_key_follow_repository_contract() {
        let store = sqlite_store().await;
        store.ensure_defaults().await.unwrap();

        let all = store.find_all().await.unwrap();
        assert_eq!(all.len(), 2);

        let page = store.find_cursor(1, Some(all[0].id)).await.unwrap();
        assert_eq!(page.total, 2);
        assert!(!page.has_more);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, all[1].id);

        store.lock_by_key(PRIMARY_KEY).await.unwrap();
        let missing = store.lock_by_key("missing_lock_key").await.unwrap_err();
        assert!(missing.to_string().contains("missing_lock_key"));
    }

    #[tokio::test]
    async fn ensure_system_value_if_missing_inserts_known_keys_only() {
        let store = sqlite_store().await;

        assert!(
            store
                .ensure_system_value_if_missing(ARRAY_KEY, r#"["https://example.com"]"#)
                .await
                .unwrap()
        );
        assert!(
            !store
                .ensure_system_value_if_missing(ARRAY_KEY, r#"["https://ignored.com"]"#)
                .await
                .unwrap()
        );
        let stored = store.find_by_key(ARRAY_KEY).await.unwrap().unwrap();
        assert_eq!(stored.value, r#"["https://example.com"]"#);
        assert_eq!(stored.value_type, ConfigValueType::StringArray);

        let unknown = store
            .ensure_system_value_if_missing("unknown_config_key", "value")
            .await
            .unwrap_err();
        assert!(unknown.to_string().contains("unknown_config_key"));
    }

    #[tokio::test]
    async fn free_functions_work_with_any_connection() {
        let store = sqlite_store().await;
        super::upsert(
            &store.db,
            &REGISTRY,
            SystemConfigUpsert {
                key: PRIMARY_KEY,
                value: "direct",
                visibility: None,
                updated_by: None,
            },
        )
        .await
        .unwrap();

        let stored = Entity::find()
            .filter(Column::Key.eq(PRIMARY_KEY))
            .one(&store.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.value, "direct");
        assert!(stored.updated_at <= Utc::now());
    }
}
