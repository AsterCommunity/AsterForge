//! System config repository.

use crate::config::definitions::{ALL_CONFIGS, ConfigDef};
use crate::db::repository::pagination_repo::fetch_offset_page;
use crate::entities::system_config::{self, Entity as SystemConfig};
use crate::errors::{AsterError, Result};
use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, TryInsertResult,
};

fn find_definition(key: &str) -> Option<&'static ConfigDef> {
    ALL_CONFIGS.iter().find(|def| def.key == key)
}

fn build_system_active_model(
    def: &ConfigDef,
    value: String,
    now: chrono::DateTime<Utc>,
    updated_by: Option<i64>,
) -> system_config::ActiveModel {
    system_config::ActiveModel {
        key: Set(def.key.to_string()),
        value: Set(value),
        value_type: Set(def.value_type),
        requires_restart: Set(def.requires_restart),
        is_sensitive: Set(def.is_sensitive),
        source: Set(SystemConfigSource::System),
        visibility: Set(SystemConfigVisibility::Private),
        namespace: Set(String::new()),
        category: Set(def.category.to_string()),
        description: Set(def.description.to_string()),
        updated_at: Set(now),
        updated_by: Set(updated_by),
        ..Default::default()
    }
}

fn build_custom_active_model(
    key: &str,
    value: String,
    visibility: SystemConfigVisibility,
    now: chrono::DateTime<Utc>,
    updated_by: Option<i64>,
) -> system_config::ActiveModel {
    system_config::ActiveModel {
        key: Set(key.to_string()),
        value: Set(value),
        value_type: Set(SystemConfigValueType::String),
        requires_restart: Set(false),
        is_sensitive: Set(false),
        source: Set(SystemConfigSource::Custom),
        visibility: Set(visibility),
        namespace: Set(String::new()),
        category: Set(String::new()),
        description: Set(String::new()),
        updated_at: Set(now),
        updated_by: Set(updated_by),
        ..Default::default()
    }
}

pub async fn find_all<C: ConnectionTrait>(db: &C) -> Result<Vec<system_config::Model>> {
    SystemConfig::find()
        .order_by_asc(system_config::Column::Id)
        .all(db)
        .await
        .map_err(AsterError::from)
}

pub async fn find_paginated<C: ConnectionTrait>(
    db: &C,
    limit: u64,
    offset: u64,
) -> Result<(Vec<system_config::Model>, u64)> {
    fetch_offset_page(
        db,
        SystemConfig::find().order_by_asc(system_config::Column::Id),
        limit,
        offset,
    )
    .await
}

pub async fn find_by_key<C: ConnectionTrait>(
    db: &C,
    key: &str,
) -> Result<Option<system_config::Model>> {
    SystemConfig::find()
        .filter(system_config::Column::Key.eq(key))
        .one(db)
        .await
        .map_err(AsterError::from)
}

pub async fn find_visible_custom<C: ConnectionTrait>(
    db: &C,
    include_authenticated: bool,
) -> Result<Vec<system_config::Model>> {
    let mut visibility_filter =
        Condition::any().add(system_config::Column::Visibility.eq(SystemConfigVisibility::Public));
    if include_authenticated {
        visibility_filter = visibility_filter
            .add(system_config::Column::Visibility.eq(SystemConfigVisibility::Authenticated));
    }

    SystemConfig::find()
        .filter(system_config::Column::Source.eq(SystemConfigSource::Custom))
        .filter(visibility_filter)
        .order_by_asc(system_config::Column::Key)
        .all(db)
        .await
        .map_err(AsterError::from)
}

pub async fn lock_by_key<C: ConnectionTrait>(db: &C, key: &str) -> Result<()> {
    let query = SystemConfig::find().filter(system_config::Column::Key.eq(key));
    let config = match db.get_database_backend() {
        DbBackend::Postgres | DbBackend::MySql => query
            .lock_exclusive()
            .one(db)
            .await
            .map_err(AsterError::from)?,
        _ => query.one(db).await.map_err(AsterError::from)?,
    };

    config
        .map(|_| ())
        .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))
}

pub async fn upsert<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &str,
    updated_by: i64,
) -> Result<system_config::Model> {
    upsert_with_options(db, key, value, None, Some(updated_by)).await
}

pub async fn upsert_with_actor<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &str,
    updated_by: Option<i64>,
) -> Result<system_config::Model> {
    upsert_with_options(db, key, value, None, updated_by).await
}

pub async fn upsert_with_options<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &str,
    visibility: Option<SystemConfigVisibility>,
    updated_by: Option<i64>,
) -> Result<system_config::Model> {
    let now = Utc::now();
    let definition = find_definition(key);
    let is_custom_key = definition.is_none();
    let active = definition
        .map(|def| build_system_active_model(def, value.to_string(), now, updated_by))
        .unwrap_or_else(|| {
            build_custom_active_model(
                key,
                value.to_string(),
                visibility.unwrap_or_default(),
                now,
                updated_by,
            )
        });
    let inserted = match SystemConfig::insert(active)
        .on_conflict_do_nothing_on([system_config::Column::Key])
        .exec(db)
        .await
        .map_err(AsterError::from)?
    {
        TryInsertResult::Inserted(_) => true,
        TryInsertResult::Conflicted => false,
        TryInsertResult::Empty => {
            return Err(AsterError::internal_error(
                "system config upsert produced empty insert result",
            ));
        }
    };

    if !inserted {
        let existing = find_by_key(db, key)
            .await?
            .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))?;
        let mut active: system_config::ActiveModel = existing.into();
        active.value = Set(value.to_string());
        if is_custom_key && let Some(visibility) = visibility {
            active.visibility = Set(visibility);
        }
        active.updated_at = Set(now);
        active.updated_by = Set(updated_by);
        active.update(db).await.map_err(AsterError::from)?;
    }

    find_by_key(db, key)
        .await?
        .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))
}

pub async fn delete_by_key<C: ConnectionTrait>(db: &C, key: &str) -> Result<()> {
    let existing = find_by_key(db, key)
        .await?
        .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))?;

    if existing.source == SystemConfigSource::System {
        return Err(AsterError::auth_forbidden(
            "cannot delete system configuration",
        ));
    }

    SystemConfig::delete_by_id(existing.id)
        .exec(db)
        .await
        .map_err(AsterError::from)?;
    Ok(())
}

pub async fn ensure_system_value_if_missing<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &str,
) -> Result<bool> {
    let def = find_definition(key)
        .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))?;
    let now = Utc::now();
    let inserted =
        match SystemConfig::insert(build_system_active_model(def, value.to_string(), now, None))
            .on_conflict_do_nothing_on([system_config::Column::Key])
            .exec(db)
            .await
            .map_err(AsterError::from)?
        {
            TryInsertResult::Inserted(_) => true,
            TryInsertResult::Conflicted => false,
            TryInsertResult::Empty => {
                return Err(AsterError::internal_error(
                    "ensure_system_value_if_missing produced empty insert result",
                ));
            }
        };

    Ok(inserted)
}

pub async fn ensure_defaults<C: ConnectionTrait>(db: &C) -> Result<usize> {
    let mut count = 0;

    for def in ALL_CONFIGS {
        let now = Utc::now();
        let inserted = match SystemConfig::insert(build_system_active_model(
            def,
            (def.default_fn)(),
            now,
            None,
        ))
        .on_conflict_do_nothing_on([system_config::Column::Key])
        .exec(db)
        .await
        .map_err(AsterError::from)?
        {
            TryInsertResult::Inserted(_) => true,
            TryInsertResult::Conflicted => false,
            TryInsertResult::Empty => {
                return Err(AsterError::internal_error(
                    "ensure_defaults produced empty insert result",
                ));
            }
        };

        if inserted {
            count += 1;
            continue;
        }

        let existing = find_by_key(db, def.key)
            .await?
            .ok_or_else(|| AsterError::record_not_found(format!("config key '{}'", def.key)))?;
        let mut active: system_config::ActiveModel = existing.into();
        active.source = Set(SystemConfigSource::System);
        active.value_type = Set(def.value_type);
        active.requires_restart = Set(def.requires_restart);
        active.is_sensitive = Set(def.is_sensitive);
        active.category = Set(def.category.to_string());
        active.description = Set(def.description.to_string());
        active.update(db).await.map_err(AsterError::from)?;
    }

    if count > 0 {
        tracing::info!("initialized {count} default configuration items");
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{
        delete_by_key, ensure_defaults, ensure_system_value_if_missing, find_all, find_by_key,
        find_paginated, find_visible_custom, lock_by_key, upsert, upsert_with_actor,
        upsert_with_options,
    };
    use crate::config::{
        DatabaseConfig,
        definitions::{
            ALL_CONFIGS, AUTH_COOKIE_SECURE_KEY, BRANDING_TITLE_KEY, PUBLIC_SITE_URL_KEY,
        },
    };
    use crate::entities::system_config;
    use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
    use sea_orm::{ActiveModelTrait, Set};

    async fn build_test_db() -> sea_orm::DatabaseConnection {
        let db = crate::db::connect_with_metrics(
            &DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            crate::metrics_core::NoopMetrics::arc(),
        )
        .await
        .expect("system config repo test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("system config repo test migrations should succeed");
        db
    }

    #[tokio::test]
    async fn ensure_defaults_inserts_once_and_repairs_definition_metadata() {
        let db = build_test_db().await;

        let inserted = ensure_defaults(&db).await.unwrap();
        assert_eq!(inserted, ALL_CONFIGS.len());
        assert_eq!(ensure_defaults(&db).await.unwrap(), 0);

        let mut active: system_config::ActiveModel = find_by_key(&db, BRANDING_TITLE_KEY)
            .await
            .unwrap()
            .unwrap()
            .into();
        active.source = Set(SystemConfigSource::Custom);
        active.value_type = Set(SystemConfigValueType::Number);
        active.requires_restart = Set(true);
        active.is_sensitive = Set(true);
        active.category = Set("wrong".to_string());
        active.description = Set("wrong".to_string());
        active.update(&db).await.unwrap();

        assert_eq!(ensure_defaults(&db).await.unwrap(), 0);
        let repaired = find_by_key(&db, BRANDING_TITLE_KEY).await.unwrap().unwrap();
        assert_eq!(repaired.source, SystemConfigSource::System);
        assert_eq!(repaired.value_type, SystemConfigValueType::String);
        assert!(!repaired.requires_restart);
        assert!(!repaired.is_sensitive);
        assert_eq!(repaired.category, "site");
        assert_eq!(
            repaired.description,
            "Application title shown in the embedded frontend"
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn upsert_system_and_custom_config_preserves_expected_metadata() {
        let db = build_test_db().await;

        let system = upsert(&db, BRANDING_TITLE_KEY, "Custom Title", 42)
            .await
            .unwrap();
        assert_eq!(system.value, "Custom Title");
        assert_eq!(system.updated_by, Some(42));
        assert_eq!(system.source, SystemConfigSource::System);
        assert_eq!(system.visibility, SystemConfigVisibility::Private);
        assert_eq!(system.value_type, SystemConfigValueType::String);

        let updated_system = upsert_with_actor(&db, BRANDING_TITLE_KEY, "New Title", None)
            .await
            .unwrap();
        assert_eq!(updated_system.id, system.id);
        assert_eq!(updated_system.value, "New Title");
        assert_eq!(updated_system.updated_by, None);

        let custom = upsert_with_options(
            &db,
            "custom_public_banner",
            "hello",
            Some(SystemConfigVisibility::Public),
            Some(7),
        )
        .await
        .unwrap();
        assert_eq!(custom.source, SystemConfigSource::Custom);
        assert_eq!(custom.visibility, SystemConfigVisibility::Public);
        assert_eq!(custom.value_type, SystemConfigValueType::String);
        assert_eq!(custom.updated_by, Some(7));

        let updated_custom = upsert_with_options(
            &db,
            "custom_public_banner",
            "hello again",
            Some(SystemConfigVisibility::Authenticated),
            None,
        )
        .await
        .unwrap();
        assert_eq!(updated_custom.id, custom.id);
        assert_eq!(updated_custom.value, "hello again");
        assert_eq!(
            updated_custom.visibility,
            SystemConfigVisibility::Authenticated
        );
        assert_eq!(updated_custom.updated_by, None);

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_visible_custom_filters_visibility_and_orders_by_key() {
        let db = build_test_db().await;
        ensure_defaults(&db).await.unwrap();
        upsert_with_options(
            &db,
            "visible_public",
            "public",
            Some(SystemConfigVisibility::Public),
            None,
        )
        .await
        .unwrap();
        upsert_with_options(
            &db,
            "visible_authenticated",
            "authenticated",
            Some(SystemConfigVisibility::Authenticated),
            None,
        )
        .await
        .unwrap();
        upsert_with_options(
            &db,
            "visible_private",
            "private",
            Some(SystemConfigVisibility::Private),
            None,
        )
        .await
        .unwrap();

        let public_only = find_visible_custom(&db, false).await.unwrap();
        assert_eq!(
            public_only
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["visible_public"]
        );

        let public_and_authenticated = find_visible_custom(&db, true).await.unwrap();
        assert_eq!(
            public_and_authenticated
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["visible_authenticated", "visible_public"]
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn delete_rejects_system_config_and_removes_custom_config() {
        let db = build_test_db().await;
        ensure_defaults(&db).await.unwrap();
        upsert_with_actor(&db, "custom_delete_me", "value", None)
            .await
            .unwrap();

        let system_error = delete_by_key(&db, BRANDING_TITLE_KEY).await.unwrap_err();
        assert!(
            system_error
                .message()
                .contains("cannot delete system configuration")
        );

        delete_by_key(&db, "custom_delete_me").await.unwrap();
        assert!(
            find_by_key(&db, "custom_delete_me")
                .await
                .unwrap()
                .is_none()
        );

        let missing_error = delete_by_key(&db, "missing_custom").await.unwrap_err();
        assert!(
            missing_error
                .message()
                .contains("config key 'missing_custom'")
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_paginated_and_lock_by_key_follow_repository_contract() {
        let db = build_test_db().await;
        ensure_defaults(&db).await.unwrap();

        let all = find_all(&db).await.unwrap();
        assert_eq!(all.len(), ALL_CONFIGS.len());

        let (page, total) = find_paginated(&db, 2, 1).await.unwrap();
        assert_eq!(total, ALL_CONFIGS.len() as u64);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, all[1].id);
        assert_eq!(page[1].id, all[2].id);

        lock_by_key(&db, BRANDING_TITLE_KEY).await.unwrap();
        let missing = lock_by_key(&db, "missing_lock_key").await.unwrap_err();
        assert!(missing.message().contains("config key 'missing_lock_key'"));

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn ensure_system_value_if_missing_inserts_known_keys_only() {
        let db = build_test_db().await;

        assert!(
            ensure_system_value_if_missing(&db, PUBLIC_SITE_URL_KEY, r#"["https://example.com"]"#)
                .await
                .unwrap()
        );
        assert!(
            !ensure_system_value_if_missing(&db, PUBLIC_SITE_URL_KEY, r#"["https://ignored.com"]"#)
                .await
                .unwrap()
        );
        let stored = find_by_key(&db, PUBLIC_SITE_URL_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.value, r#"["https://example.com"]"#);
        assert_eq!(stored.value_type, SystemConfigValueType::StringArray);

        let unknown = ensure_system_value_if_missing(&db, "unknown_config_key", "value")
            .await
            .unwrap_err();
        assert!(unknown.message().contains("unknown_config_key"));

        assert!(
            ensure_system_value_if_missing(&db, AUTH_COOKIE_SECURE_KEY, "true")
                .await
                .unwrap()
        );

        db.close().await.unwrap();
    }
}
