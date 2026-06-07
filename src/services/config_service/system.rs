use crate::api::pagination::{OffsetPage, load_offset_page};
use crate::config::definitions::ALL_CONFIGS;
use crate::config::system_config as shared_system_config;
use crate::db::repository::system_config_repo;
use crate::entities::system_config;
use crate::errors::{AsterError, Result};
use crate::runtime::SharedRuntimeState;
use crate::services::audit_service::{self, AuditContext};
use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub enum SystemConfigValue {
    String(String),
    StringArray(Vec<String>),
}

impl SystemConfigValue {
    fn from_storage(value_type: SystemConfigValueType, value: String) -> Self {
        if !value_type.is_string_list() {
            return Self::String(value);
        }

        match serde_json::from_str::<Vec<String>>(&value) {
            Ok(items) => Self::StringArray(items),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    value_type = %value_type,
                    "invalid stored string list config value; returning an empty array"
                );
                Self::StringArray(Vec::new())
            }
        }
    }

    pub fn to_storage_for_type(&self, value_type: SystemConfigValueType) -> Result<String> {
        match (value_type, self) {
            (
                SystemConfigValueType::StringArray | SystemConfigValueType::StringEnumSet,
                Self::StringArray(values),
            ) => serde_json::to_string(values).map_err(|error| {
                AsterError::internal_error(format!(
                    "failed to serialize {} config value: {error}",
                    value_type.as_str()
                ))
            }),
            (
                SystemConfigValueType::StringArray | SystemConfigValueType::StringEnumSet,
                Self::String(_),
            ) => Err(AsterError::validation_error(format!(
                "{} config value must be a JSON array",
                value_type.as_str()
            ))),
            (_, Self::String(value)) => Ok(value.clone()),
            (_, Self::StringArray(_)) => Err(AsterError::validation_error(
                "string array values are only supported for string_array and string_enum_set config keys",
            )),
        }
    }

    pub fn to_audit_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::StringArray(values) => serde_json::to_string(values)
                .unwrap_or_else(|_| "<invalid string list value>".to_string()),
        }
    }
}

impl From<&str> for SystemConfigValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<&String> for SystemConfigValue {
    fn from(value: &String) -> Self {
        Self::String(value.clone())
    }
}

impl From<String> for SystemConfigValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<String>> for SystemConfigValue {
    fn from(value: Vec<String>) -> Self {
        Self::StringArray(value)
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct SystemConfig {
    pub id: i64,
    pub key: String,
    pub value: SystemConfigValue,
    pub value_type: SystemConfigValueType,
    pub requires_restart: bool,
    pub is_sensitive: bool,
    pub source: SystemConfigSource,
    pub visibility: SystemConfigVisibility,
    pub namespace: String,
    pub category: String,
    pub description: String,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<i64>,
}

impl From<system_config::Model> for SystemConfig {
    fn from(model: system_config::Model) -> Self {
        let value = if model.is_sensitive {
            SystemConfigValue::String("***REDACTED***".to_string())
        } else {
            SystemConfigValue::from_storage(model.value_type, model.value)
        };
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

pub async fn ensure_defaults<C: ConnectionTrait>(db: &C) -> Result<()> {
    system_config_repo::ensure_defaults(db).await?;
    Ok(())
}

pub async fn list_paginated(
    state: &impl SharedRuntimeState,
    limit: u64,
    offset: u64,
) -> Result<OffsetPage<SystemConfig>> {
    let page = load_offset_page(limit, offset, 100, |limit, offset| async move {
        system_config_repo::find_paginated(state.reader_db(), limit, offset).await
    })
    .await?;
    let items = page
        .items
        .into_iter()
        .map(apply_system_config_definition)
        .map(Into::into)
        .collect();
    Ok(OffsetPage::new(items, page.total, page.limit, page.offset))
}

pub async fn get_by_key(state: &impl SharedRuntimeState, key: &str) -> Result<SystemConfig> {
    system_config_repo::find_by_key(state.reader_db(), key)
        .await?
        .map(apply_system_config_definition)
        .map(Into::into)
        .ok_or_else(|| AsterError::record_not_found(format!("config key '{key}'")))
}

pub async fn set(
    state: &impl SharedRuntimeState,
    key: &str,
    value: impl Into<SystemConfigValue>,
    updated_by: i64,
) -> Result<SystemConfig> {
    set_with_visibility(state, key, value, None, updated_by).await
}

pub async fn set_with_visibility(
    state: &impl SharedRuntimeState,
    key: &str,
    value: impl Into<SystemConfigValue>,
    visibility: Option<SystemConfigVisibility>,
    updated_by: i64,
) -> Result<SystemConfig> {
    let value = value.into();
    let saved = save_config(state, key, &value, visibility, Some(updated_by)).await?;
    Ok(saved.into())
}

pub async fn delete(state: &impl SharedRuntimeState, key: &str) -> Result<()> {
    system_config_repo::delete_by_key(state.writer_db(), key).await?;
    state.runtime_config().remove(key);
    tracing::debug!(key, "deleted runtime config");
    Ok(())
}

pub async fn delete_with_audit(
    state: &impl SharedRuntimeState,
    key: &str,
    audit_ctx: &AuditContext,
) -> Result<()> {
    let config = get_by_key(state, key).await?;
    delete(state, key).await?;
    audit_service::log(
        state,
        audit_ctx,
        audit_service::AuditAction::AdminDeleteConfig,
        audit_service::AuditEntityType::SystemConfig,
        Some(config.id),
        Some(key),
        None,
    )
    .await;
    Ok(())
}

pub async fn set_with_audit(
    state: &impl SharedRuntimeState,
    key: &str,
    value: &SystemConfigValue,
    updated_by: i64,
    audit_ctx: &AuditContext,
) -> Result<SystemConfig> {
    set_with_audit_and_visibility(state, key, value, None, updated_by, audit_ctx).await
}

pub async fn set_with_audit_and_visibility(
    state: &impl SharedRuntimeState,
    key: &str,
    value: &SystemConfigValue,
    visibility: Option<SystemConfigVisibility>,
    updated_by: i64,
    audit_ctx: &AuditContext,
) -> Result<SystemConfig> {
    let prior_visibility = system_config_repo::find_by_key(state.reader_db(), key)
        .await?
        .map(|config| config.visibility);
    let saved = save_config(state, key, value, visibility, Some(updated_by)).await?;
    audit_config_update(state, audit_ctx, &saved, prior_visibility).await;
    Ok(saved.into())
}

async fn save_config(
    state: &impl SharedRuntimeState,
    key: &str,
    value: &SystemConfigValue,
    visibility: Option<SystemConfigVisibility>,
    updated_by: Option<i64>,
) -> Result<system_config::Model> {
    validate_visibility_target(key, visibility)?;
    let value_type = ALL_CONFIGS
        .iter()
        .find(|def| def.key == key)
        .map(|def| def.value_type)
        .unwrap_or(SystemConfigValueType::String);
    let mut normalized_value = value.to_storage_for_type(value_type)?;

    if let Some(def) = ALL_CONFIGS.iter().find(|def| def.key == key) {
        shared_system_config::validate_value_type(def.value_type, &normalized_value)?;
        normalized_value = shared_system_config::normalize_system_value(
            state.runtime_config(),
            key,
            &normalized_value,
        )?;
    }

    let saved = system_config_repo::upsert_with_options(
        state.writer_db(),
        key,
        &normalized_value,
        visibility,
        updated_by,
    )
    .await?;
    let saved = apply_system_config_definition(saved);
    state.runtime_config().apply(saved.clone());
    Ok(saved)
}

async fn audit_config_update(
    state: &impl SharedRuntimeState,
    audit_ctx: &AuditContext,
    config: &system_config::Model,
    prior_visibility: Option<SystemConfigVisibility>,
) {
    audit_service::log_with_details(
        state,
        audit_ctx,
        audit_service::AuditAction::ConfigUpdate,
        audit_service::AuditEntityType::SystemConfig,
        Some(config.id),
        Some(&config.key),
        || {
            let audit_value = if config.is_sensitive {
                "***REDACTED***".to_string()
            } else {
                SystemConfigValue::from_storage(config.value_type, config.value.clone())
                    .to_audit_string()
            };
            audit_service::details(audit_service::ConfigUpdateDetails {
                value: &audit_value,
                visibility: config.visibility,
                prior_visibility,
            })
        },
    )
    .await;
}

fn validate_visibility_target(key: &str, visibility: Option<SystemConfigVisibility>) -> Result<()> {
    if visibility.is_some() && ALL_CONFIGS.iter().any(|def| def.key == key) {
        return Err(AsterError::validation_error(
            "visibility can only be changed for custom configuration",
        ));
    }
    Ok(())
}

fn apply_system_config_definition(config: system_config::Model) -> system_config::Model {
    shared_system_config::apply_definition(config)
}

#[cfg(test)]
mod tests {
    use super::{
        SystemConfigValue, delete, delete_with_audit, ensure_defaults, get_by_key, list_paginated,
        set, set_with_audit, set_with_visibility,
    };
    use crate::cache;
    use crate::config::definitions::{ALL_CONFIGS, BRANDING_TITLE_KEY, PUBLIC_SITE_URL_KEY};
    use crate::config::{CacheConfig, Config, DatabaseConfig, RuntimeConfig};
    use crate::db::repository::system_config_repo;
    use crate::db::{self, DbHandles};
    use crate::runtime::{AppState, SharedRuntimeState};
    use crate::services::audit_service::{AuditContext, flush_global_audit_log_manager};
    use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
    use std::sync::Arc;

    async fn build_test_state() -> AppState {
        let db_cfg = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            pool_size: 1,
            retry_count: 0,
        };
        let db = db::connect_with_metrics(&db_cfg, crate::metrics_core::NoopMetrics::arc())
            .await
            .expect("config service test database should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("config service test migrations should succeed");
        ensure_defaults(&db)
            .await
            .expect("config service defaults should be seeded");

        let runtime_config = Arc::new(RuntimeConfig::new());
        runtime_config
            .reload(&db)
            .await
            .expect("config service runtime config should reload");

        let config = Arc::new(Config {
            database: db_cfg,
            cache: CacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        });
        let cache = cache::create_cache(&config.cache).await;

        AppState {
            db_handles: DbHandles::single(db),
            config,
            runtime_config,
            cache,
            mail_sender: crate::services::mail_service::memory_sender(),
            metrics: crate::metrics_core::NoopMetrics::arc(),
            background_task_dispatch_wakeup: AppState::new_background_task_dispatch_wakeup(),
        }
    }

    #[test]
    fn system_config_value_storage_rules_match_value_type() {
        assert_eq!(
            SystemConfigValue::String("value".to_string())
                .to_storage_for_type(SystemConfigValueType::String)
                .unwrap(),
            "value"
        );
        assert_eq!(
            SystemConfigValue::StringArray(vec!["a".to_string(), "b".to_string()])
                .to_storage_for_type(SystemConfigValueType::StringArray)
                .unwrap(),
            r#"["a","b"]"#
        );
        assert_eq!(
            SystemConfigValue::StringArray(vec!["b".to_string(), "a".to_string()])
                .to_audit_string(),
            r#"["b","a"]"#
        );
        assert!(
            SystemConfigValue::String("not-an-array".to_string())
                .to_storage_for_type(SystemConfigValueType::StringArray)
                .is_err()
        );
        assert!(
            SystemConfigValue::StringArray(vec!["a".to_string()])
                .to_storage_for_type(SystemConfigValueType::String)
                .is_err()
        );
    }

    #[test]
    fn system_config_response_redacts_sensitive_values_and_parses_lists() {
        let mut model = system_config_model("demo.list", r#"["a","b"]"#);
        model.value_type = SystemConfigValueType::StringArray;
        let config = super::SystemConfig::from(model);
        assert_eq!(
            config.value,
            SystemConfigValue::StringArray(vec!["a".to_string(), "b".to_string()])
        );

        let mut sensitive = system_config_model("demo.secret", "secret");
        sensitive.is_sensitive = true;
        let config = super::SystemConfig::from(sensitive);
        assert_eq!(
            config.value,
            SystemConfigValue::String("***REDACTED***".to_string())
        );

        let mut invalid_list = system_config_model("demo.invalid", "not json");
        invalid_list.value_type = SystemConfigValueType::StringArray;
        let config = super::SystemConfig::from(invalid_list);
        assert_eq!(config.value, SystemConfigValue::StringArray(Vec::new()));
    }

    fn system_config_model(key: &str, value: &str) -> crate::entities::system_config::Model {
        crate::entities::system_config::Model {
            id: 1,
            key: key.to_string(),
            value: value.to_string(),
            value_type: SystemConfigValueType::String,
            requires_restart: false,
            is_sensitive: false,
            source: SystemConfigSource::System,
            visibility: SystemConfigVisibility::Private,
            namespace: String::new(),
            category: String::new(),
            description: String::new(),
            updated_at: chrono::Utc::now(),
            updated_by: None,
        }
    }

    #[tokio::test]
    async fn list_get_and_set_system_config_updates_runtime_snapshot() {
        let state = build_test_state().await;

        let page = list_paginated(&state, 2, 0).await.unwrap();
        assert_eq!(page.total, ALL_CONFIGS.len() as u64);
        assert_eq!(page.limit, 2);
        assert_eq!(page.offset, 0);
        assert_eq!(page.items.len(), 2);

        let initial = get_by_key(&state, BRANDING_TITLE_KEY).await.unwrap();
        assert_eq!(initial.key, BRANDING_TITLE_KEY);
        assert_eq!(
            initial.value,
            SystemConfigValue::String("AsterForge".to_string())
        );
        assert_eq!(initial.source, SystemConfigSource::System);

        let updated = set(&state, BRANDING_TITLE_KEY, "  Template Title  ", 42)
            .await
            .unwrap();
        assert_eq!(
            updated.value,
            SystemConfigValue::String("Template Title".to_string())
        );
        assert_eq!(updated.updated_by, Some(42));
        assert_eq!(
            state.runtime_config().get(BRANDING_TITLE_KEY).as_deref(),
            Some("Template Title")
        );

        let origins = set(
            &state,
            PUBLIC_SITE_URL_KEY,
            vec![
                "https://forge.example.com/".to_string(),
                " https://admin.example.com ".to_string(),
            ],
            43,
        )
        .await
        .unwrap();
        assert_eq!(
            origins.value,
            SystemConfigValue::StringArray(vec![
                "https://forge.example.com".to_string(),
                "https://admin.example.com".to_string(),
            ])
        );
        assert_eq!(
            state.runtime_config().get(PUBLIC_SITE_URL_KEY).as_deref(),
            Some(r#"["https://forge.example.com","https://admin.example.com"]"#)
        );

        let error = set(&state, PUBLIC_SITE_URL_KEY, "not-an-array", 44)
            .await
            .unwrap_err();
        assert!(error.message().contains("must be a JSON array"));

        let missing = get_by_key(&state, "missing.config.key").await.unwrap_err();
        assert!(missing.message().contains("missing.config.key"));
    }

    #[tokio::test]
    async fn custom_config_visibility_is_mutable_but_system_visibility_is_fixed() {
        let state = build_test_state().await;

        let custom = set_with_visibility(
            &state,
            "custom.banner",
            "hello",
            Some(SystemConfigVisibility::Public),
            7,
        )
        .await
        .unwrap();
        assert_eq!(custom.source, SystemConfigSource::Custom);
        assert_eq!(custom.visibility, SystemConfigVisibility::Public);
        assert_eq!(
            state.runtime_config().get("custom.banner").as_deref(),
            Some("hello")
        );

        let updated = set_with_visibility(
            &state,
            "custom.banner",
            "hello again",
            Some(SystemConfigVisibility::Authenticated),
            8,
        )
        .await
        .unwrap();
        assert_eq!(updated.id, custom.id);
        assert_eq!(updated.visibility, SystemConfigVisibility::Authenticated);
        assert_eq!(
            state.runtime_config().get("custom.banner").as_deref(),
            Some("hello again")
        );

        let error = set_with_visibility(
            &state,
            BRANDING_TITLE_KEY,
            "Visible Title",
            Some(SystemConfigVisibility::Public),
            9,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .message()
                .contains("visibility can only be changed for custom configuration")
        );
    }

    #[tokio::test]
    async fn delete_removes_custom_config_from_storage_and_runtime_snapshot() {
        let state = build_test_state().await;
        set_with_visibility(
            &state,
            "custom.delete_me",
            "value",
            Some(SystemConfigVisibility::Public),
            7,
        )
        .await
        .unwrap();

        delete(&state, "custom.delete_me").await.unwrap();
        assert!(state.runtime_config().get("custom.delete_me").is_none());
        assert!(
            system_config_repo::find_by_key(state.writer_db(), "custom.delete_me")
                .await
                .unwrap()
                .is_none()
        );

        let system_error = delete(&state, BRANDING_TITLE_KEY).await.unwrap_err();
        assert!(
            system_error
                .message()
                .contains("cannot delete system configuration")
        );

        let missing = delete(&state, "custom.missing").await.unwrap_err();
        assert!(missing.message().contains("custom.missing"));
    }

    #[tokio::test]
    async fn audit_wrapped_mutations_keep_primary_config_behavior() {
        let state = build_test_state().await;
        let audit_ctx = AuditContext {
            user_id: 99,
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("config-service-test".to_string()),
        };

        let updated = set_with_audit(
            &state,
            BRANDING_TITLE_KEY,
            &SystemConfigValue::String("Audited Title".to_string()),
            99,
            &audit_ctx,
        )
        .await
        .unwrap();
        assert_eq!(
            updated.value,
            SystemConfigValue::String("Audited Title".to_string())
        );
        assert_eq!(
            state.runtime_config().get(BRANDING_TITLE_KEY).as_deref(),
            Some("Audited Title")
        );

        set_with_visibility(
            &state,
            "custom.audit_delete",
            "value",
            Some(SystemConfigVisibility::Public),
            99,
        )
        .await
        .unwrap();
        delete_with_audit(&state, "custom.audit_delete", &audit_ctx)
            .await
            .unwrap();
        flush_global_audit_log_manager().await;

        assert!(state.runtime_config().get("custom.audit_delete").is_none());
    }
}
