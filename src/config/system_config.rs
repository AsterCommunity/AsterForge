//! Runtime system configuration helpers.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::config::audit;
use crate::config::auth_runtime;
use crate::config::bool_like::parse_bool_like;
use crate::config::branding;
use crate::config::cors;
use crate::config::definitions::{ALL_CONFIGS, ConfigDef};
use crate::config::local_email_policy;
use crate::config::operations;
use crate::config::site_url;
use crate::entities::system_config;
use crate::errors::{AsterError, Result};
use crate::types::{SystemConfigSource, SystemConfigValueType};

pub trait SystemConfigValueLookup {
    fn get_config_value(&self, key: &str) -> Option<String>;
}

impl SystemConfigValueLookup for RuntimeConfig {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key)
    }
}

impl<T> SystemConfigValueLookup for Arc<T>
where
    T: SystemConfigValueLookup + ?Sized,
{
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.as_ref().get_config_value(key)
    }
}

impl SystemConfigValueLookup for HashMap<String, String> {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

impl SystemConfigValueLookup for BTreeMap<String, String> {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

pub fn get_definition(key: &str) -> Option<&'static ConfigDef> {
    ALL_CONFIGS.iter().find(|def| def.key == key)
}

pub fn validate_value_type(value_type: SystemConfigValueType, value: &str) -> Result<()> {
    let trimmed = value.trim();
    match value_type {
        SystemConfigValueType::Boolean => {
            if trimmed != "true" && trimmed != "false" {
                return Err(AsterError::validation_error(
                    "boolean config must be 'true' or 'false'",
                ));
            }
        }
        SystemConfigValueType::Number => {
            if trimmed.parse::<f64>().is_err() {
                return Err(AsterError::validation_error(
                    "number config must be a valid number",
                ));
            }
        }
        SystemConfigValueType::StringArray | SystemConfigValueType::StringEnumSet => {
            serde_json::from_str::<Vec<String>>(trimmed).map_err(|error| {
                AsterError::validation_error(format!(
                    "{} config must be a JSON array of strings: {error}",
                    value_type.as_str()
                ))
            })?;
        }
        SystemConfigValueType::String | SystemConfigValueType::Multiline => {}
    }
    Ok(())
}

pub fn normalize_system_value<L>(lookup: &L, key: &str, value: &str) -> Result<String>
where
    L: SystemConfigValueLookup + ?Sized,
{
    match key {
        audit::AUDIT_LOG_RECORDED_ACTIONS_KEY => {
            audit::normalize_recorded_actions_config_value(value)
        }
        auth_runtime::AUTH_COOKIE_SECURE_KEY => {
            auth_runtime::normalize_cookie_secure_config_value(value)
        }
        auth_runtime::AUTH_ALLOW_USER_REGISTRATION_KEY => {
            auth_runtime::normalize_allow_user_registration_config_value(value)
        }
        auth_runtime::AUTH_REGISTER_ACTIVATION_ENABLED_KEY => {
            auth_runtime::normalize_register_activation_enabled_config_value(value)
        }
        auth_runtime::AUTH_EMAIL_CODE_LOGIN_ENABLED_KEY
        | auth_runtime::AUTH_EMAIL_CODE_LOGIN_ALLOW_TOTP_FALLBACK_KEY
        | auth_runtime::AUTH_PASSKEY_LOGIN_ENABLED_KEY => {
            auth_runtime::normalize_email_code_login_bool_config_value(key, value)
        }
        auth_runtime::AUTH_ACCESS_TOKEN_TTL_SECS_KEY
        | auth_runtime::AUTH_REFRESH_TOKEN_TTL_SECS_KEY
        | auth_runtime::AUTH_REGISTER_ACTIVATION_TTL_SECS_KEY
        | auth_runtime::AUTH_CONTACT_CHANGE_TTL_SECS_KEY
        | auth_runtime::AUTH_PASSWORD_RESET_TTL_SECS_KEY
        | auth_runtime::AUTH_EMAIL_CODE_LOGIN_TTL_SECS_KEY
        | auth_runtime::AUTH_EMAIL_CODE_LOGIN_RESEND_COOLDOWN_SECS_KEY
        | auth_runtime::AUTH_PASSWORD_RESET_REQUEST_COOLDOWN_SECS_KEY
        | auth_runtime::AUTH_CONTACT_VERIFICATION_RESEND_COOLDOWN_SECS_KEY => {
            auth_runtime::normalize_token_ttl_config_value(key, value)
        }
        local_email_policy::AUTH_LOCAL_EMAIL_ALLOWLIST_KEY
        | local_email_policy::AUTH_LOCAL_EMAIL_BLOCKLIST_KEY => {
            local_email_policy::normalize_local_email_policy_config_value(key, value)
        }
        operations::BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY
        | operations::BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY
        | operations::BACKGROUND_TASK_MAX_CONCURRENCY_KEY
        | operations::BACKGROUND_TASK_MAX_ATTEMPTS_KEY
        | operations::TASK_RETENTION_HOURS_KEY
        | operations::TASK_LIST_MAX_LIMIT_KEY
        | operations::MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY => {
            operations::normalize_interval_config_value(key, value)
        }
        cors::CORS_ENABLED_KEY => cors::normalize_enabled_config_value(value),
        cors::CORS_ALLOWED_ORIGINS_KEY => {
            let normalized = cors::normalize_allowed_origins_config_value(value)?;
            let parsed = cors::parse_allowed_origins_value(&normalized)?;
            let allow_credentials = lookup
                .get_config_value(cors::CORS_ALLOW_CREDENTIALS_KEY)
                .and_then(|raw| parse_bool_like(&raw))
                .unwrap_or(cors::DEFAULT_CORS_ALLOW_CREDENTIALS);
            cors::validate_runtime_cors_combination(&parsed, allow_credentials)?;
            Ok(normalized)
        }
        cors::CORS_ALLOW_CREDENTIALS_KEY => {
            let normalized = cors::normalize_allow_credentials_config_value(value)?;
            let allow_credentials = normalized == "true";
            let current_origins = lookup
                .get_config_value(cors::CORS_ALLOWED_ORIGINS_KEY)
                .unwrap_or_default();
            let parsed = cors::parse_allowed_origins_value(&current_origins)?;
            cors::validate_runtime_cors_combination(&parsed, allow_credentials)?;
            Ok(normalized)
        }
        cors::CORS_MAX_AGE_SECS_KEY => cors::normalize_max_age_config_value(value),
        site_url::PUBLIC_SITE_URL_KEY => site_url::normalize_public_site_url_config_value(value),
        branding::BRANDING_TITLE_KEY => branding::normalize_title_config_value(value),
        branding::BRANDING_DESCRIPTION_KEY => branding::normalize_description_config_value(value),
        branding::BRANDING_FAVICON_URL_KEY => branding::normalize_favicon_url_config_value(value),
        branding::BRANDING_WORDMARK_DARK_URL_KEY => {
            branding::normalize_wordmark_dark_url_config_value(value)
        }
        branding::BRANDING_WORDMARK_LIGHT_URL_KEY => {
            branding::normalize_wordmark_light_url_config_value(value)
        }
        _ => Ok(value.to_string()),
    }
}

pub fn apply_definition(mut config: system_config::Model) -> system_config::Model {
    if config.source != SystemConfigSource::System {
        return config;
    }

    let Some(def) = get_definition(&config.key) else {
        return config;
    };

    config.value_type = def.value_type;
    config.requires_restart = def.requires_restart;
    config.is_sensitive = def.is_sensitive;
    config.category = def.category.to_string();
    config.description = def.description.to_string();
    config
}

#[cfg(test)]
mod tests {
    use super::{apply_definition, normalize_system_value, validate_value_type};
    use crate::config::definitions::{CONFIG_CATEGORY_SITE, PUBLIC_SITE_URL_KEY};
    use crate::config::{audit, cors, operations};
    use crate::entities::system_config;
    use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
    use chrono::Utc;
    use std::collections::HashMap;

    fn model(key: &str, value: &str, source: SystemConfigSource) -> system_config::Model {
        system_config::Model {
            id: 1,
            key: key.to_string(),
            value: value.to_string(),
            value_type: SystemConfigValueType::String,
            requires_restart: false,
            is_sensitive: false,
            source,
            visibility: SystemConfigVisibility::Private,
            namespace: String::new(),
            category: String::new(),
            description: String::new(),
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    #[test]
    fn validate_value_type_enforces_declared_types() {
        assert!(validate_value_type(SystemConfigValueType::Boolean, "true").is_ok());
        assert!(validate_value_type(SystemConfigValueType::Boolean, "false").is_ok());
        assert!(validate_value_type(SystemConfigValueType::Boolean, " yes ").is_err());

        assert!(validate_value_type(SystemConfigValueType::Number, "42").is_ok());
        assert!(validate_value_type(SystemConfigValueType::Number, "1.5").is_ok());
        assert!(validate_value_type(SystemConfigValueType::Number, "nope").is_err());

        assert!(validate_value_type(SystemConfigValueType::StringArray, r#"["a"]"#).is_ok());
        assert!(validate_value_type(SystemConfigValueType::StringArray, r#""a""#).is_err());
        assert!(validate_value_type(SystemConfigValueType::StringEnumSet, r#"["a"]"#).is_ok());
        assert!(validate_value_type(SystemConfigValueType::StringEnumSet, r#""a""#).is_err());
        assert!(validate_value_type(SystemConfigValueType::String, "anything").is_ok());
        assert!(validate_value_type(SystemConfigValueType::Multiline, "line\nline").is_ok());
    }

    #[test]
    fn normalize_system_value_validates_audit_action_scope() {
        let lookup = HashMap::new();

        assert_eq!(
            normalize_system_value(
                &lookup,
                audit::AUDIT_LOG_RECORDED_ACTIONS_KEY,
                r#"["user_login","config_update"]"#,
            )
            .unwrap(),
            r#"["config_update","user_login"]"#
        );
        assert!(
            normalize_system_value(
                &lookup,
                audit::AUDIT_LOG_RECORDED_ACTIONS_KEY,
                r#"["unknown_action"]"#,
            )
            .is_err()
        );
        assert!(
            normalize_system_value(
                &lookup,
                audit::AUDIT_LOG_RECORDED_ACTIONS_KEY,
                r#"["user_login","user_login"]"#,
            )
            .is_err()
        );
        assert_eq!(
            normalize_system_value(&lookup, audit::AUDIT_LOG_RECORDED_ACTIONS_KEY, "[]").unwrap(),
            "[]"
        );
    }

    #[test]
    fn normalize_system_value_uses_lookup_for_cors_cross_field_validation() {
        let lookup = HashMap::from([(
            cors::CORS_ALLOW_CREDENTIALS_KEY.to_string(),
            "true".to_string(),
        )]);

        let err = normalize_system_value(&lookup, cors::CORS_ALLOWED_ORIGINS_KEY, "*").unwrap_err();
        assert!(
            err.message()
                .contains("cors_allow_credentials cannot be true when cors_allowed_origins is '*'")
        );

        let lookup = HashMap::from([(cors::CORS_ALLOWED_ORIGINS_KEY.to_string(), "*".to_string())]);
        let err =
            normalize_system_value(&lookup, cors::CORS_ALLOW_CREDENTIALS_KEY, "true").unwrap_err();
        assert!(
            err.message()
                .contains("cors_allow_credentials cannot be true when cors_allowed_origins is '*'")
        );
    }

    #[test]
    fn normalize_system_value_routes_generic_operation_and_site_keys() {
        let lookup = HashMap::new();

        assert_eq!(
            normalize_system_value(
                &lookup,
                operations::BACKGROUND_TASK_MAX_CONCURRENCY_KEY,
                " 8 ",
            )
            .unwrap(),
            "8"
        );
        assert!(
            normalize_system_value(
                &lookup,
                operations::BACKGROUND_TASK_MAX_CONCURRENCY_KEY,
                "0",
            )
            .is_err()
        );
        assert_eq!(
            normalize_system_value(
                &lookup,
                PUBLIC_SITE_URL_KEY,
                r#"["https://example.com/"," https://admin.example.com "]"#,
            )
            .unwrap(),
            r#"["https://example.com","https://admin.example.com"]"#
        );
    }

    #[test]
    fn apply_definition_overlays_schema_metadata_for_system_rows() {
        let config = apply_definition(model(
            PUBLIC_SITE_URL_KEY,
            r#"["https://forge.example.com"]"#,
            SystemConfigSource::System,
        ));
        assert_eq!(config.value_type, SystemConfigValueType::StringArray);
        assert_eq!(config.category, CONFIG_CATEGORY_SITE);
        assert_eq!(
            config.description,
            "Public origins used to build externally visible application URLs"
        );

        let custom = apply_definition(model("custom.demo", "value", SystemConfigSource::Custom));
        assert_eq!(custom.category, "");
        assert_eq!(custom.description, "");
    }
}
