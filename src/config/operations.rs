//! Generic runtime operation settings.

use crate::config::RuntimeConfig;
use crate::errors::{AsterError, Result};

pub use crate::config::definitions::{
    BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY,
    BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY, BACKGROUND_TASK_MAX_ATTEMPTS_KEY,
    BACKGROUND_TASK_MAX_CONCURRENCY_KEY, MAIL_OUTBOX_DISPATCH_INTERVAL_SECS_KEY,
    MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY, TASK_LIST_MAX_LIMIT_KEY, TASK_RETENTION_HOURS_KEY,
};

pub const DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY: usize = 4;
pub const DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS: i32 = 3;
pub const DEFAULT_TASK_LIST_MAX_LIMIT: u64 = 100;
pub const DEFAULT_MAINTENANCE_CLEANUP_INTERVAL_SECS: u64 = 3600;
pub const DEFAULT_MAIL_OUTBOX_DISPATCH_INTERVAL_SECS: u64 = 5;

pub fn normalize_interval_config_value(key: &str, value: &str) -> Result<String> {
    let parsed = parse_positive_u64(value)
        .ok_or_else(|| AsterError::validation_error(format!("{key} must be a positive integer")))?;
    Ok(parsed.to_string())
}

pub fn background_task_dispatch_interval_secs(runtime_config: &RuntimeConfig) -> u64 {
    read_positive_u64(
        runtime_config,
        BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY,
        DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS,
    )
}

pub fn background_task_dispatch_idle_max_interval_secs(runtime_config: &RuntimeConfig) -> u64 {
    read_positive_u64(
        runtime_config,
        BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY,
        DEFAULT_BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS,
    )
}

pub fn background_task_max_concurrency(runtime_config: &RuntimeConfig) -> usize {
    match read_positive_u64(
        runtime_config,
        BACKGROUND_TASK_MAX_CONCURRENCY_KEY,
        u64::try_from(DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY).unwrap_or(4),
    )
    .try_into()
    {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!("background_task_max_concurrency exceeds usize; using default");
            DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY
        }
    }
}

pub fn background_task_max_attempts(runtime_config: &RuntimeConfig) -> i32 {
    match read_positive_u64(
        runtime_config,
        BACKGROUND_TASK_MAX_ATTEMPTS_KEY,
        u64::try_from(DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS).unwrap_or(3),
    )
    .try_into()
    {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!("background_task_max_attempts exceeds i32; using default");
            DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS
        }
    }
}

pub fn task_list_max_limit(runtime_config: &RuntimeConfig) -> u64 {
    read_positive_u64(
        runtime_config,
        TASK_LIST_MAX_LIMIT_KEY,
        DEFAULT_TASK_LIST_MAX_LIMIT,
    )
}

pub fn maintenance_cleanup_interval_secs(runtime_config: &RuntimeConfig) -> u64 {
    read_positive_u64(
        runtime_config,
        MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY,
        DEFAULT_MAINTENANCE_CLEANUP_INTERVAL_SECS,
    )
}

pub fn mail_outbox_dispatch_interval_secs(runtime_config: &RuntimeConfig) -> u64 {
    read_positive_u64(
        runtime_config,
        MAIL_OUTBOX_DISPATCH_INTERVAL_SECS_KEY,
        DEFAULT_MAIL_OUTBOX_DISPATCH_INTERVAL_SECS,
    )
}

fn parse_positive_u64(value: &str) -> Option<u64> {
    let parsed = value.trim().parse::<u64>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn read_positive_u64(runtime_config: &RuntimeConfig, key: &str, default: u64) -> u64 {
    match runtime_config.get(key) {
        Some(raw) => match parse_positive_u64(&raw) {
            Some(value) => value,
            None => {
                tracing::warn!(key, value = %raw, "invalid runtime operations config; using default");
                default
            }
        },
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY,
        BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY, BACKGROUND_TASK_MAX_ATTEMPTS_KEY,
        BACKGROUND_TASK_MAX_CONCURRENCY_KEY,
        DEFAULT_BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS,
        DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS, DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS,
        DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY, DEFAULT_MAINTENANCE_CLEANUP_INTERVAL_SECS,
        DEFAULT_TASK_LIST_MAX_LIMIT, MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY,
        TASK_LIST_MAX_LIMIT_KEY, background_task_dispatch_idle_max_interval_secs,
        background_task_dispatch_interval_secs, background_task_max_attempts,
        background_task_max_concurrency, maintenance_cleanup_interval_secs,
        normalize_interval_config_value, task_list_max_limit,
    };
    use crate::config::{RuntimeConfig, definitions::CONFIG_CATEGORY_RUNTIME};
    use crate::entities::system_config;
    use crate::types::{SystemConfigSource, SystemConfigValueType, SystemConfigVisibility};
    use chrono::Utc;

    fn model(key: &str, value: &str) -> system_config::Model {
        system_config::Model {
            id: 1,
            key: key.to_string(),
            value: value.to_string(),
            value_type: SystemConfigValueType::String,
            requires_restart: false,
            is_sensitive: false,
            source: SystemConfigSource::System,
            visibility: SystemConfigVisibility::Private,
            namespace: String::new(),
            category: CONFIG_CATEGORY_RUNTIME.to_string(),
            description: "test runtime operation config".to_string(),
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    fn runtime_config(values: &[(&str, &str)]) -> RuntimeConfig {
        let runtime_config = RuntimeConfig::new();
        for (key, value) in values {
            runtime_config.apply(model(key, value));
        }
        runtime_config
    }

    #[test]
    fn normalize_interval_accepts_positive_integer() {
        assert_eq!(
            normalize_interval_config_value("test_interval", " 60 ").unwrap(),
            "60"
        );
    }

    #[test]
    fn normalize_interval_rejects_zero_and_non_numbers() {
        assert!(normalize_interval_config_value("test_interval", "0").is_err());
        assert!(normalize_interval_config_value("test_interval", "abc").is_err());
    }

    #[test]
    fn runtime_operation_readers_use_defaults_when_values_are_missing() {
        let runtime_config = RuntimeConfig::new();

        assert_eq!(
            background_task_dispatch_interval_secs(&runtime_config),
            DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS
        );
        assert_eq!(
            background_task_dispatch_idle_max_interval_secs(&runtime_config),
            DEFAULT_BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS
        );
        assert_eq!(
            background_task_max_concurrency(&runtime_config),
            DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY
        );
        assert_eq!(
            background_task_max_attempts(&runtime_config),
            DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS
        );
        assert_eq!(
            task_list_max_limit(&runtime_config),
            DEFAULT_TASK_LIST_MAX_LIMIT
        );
        assert_eq!(
            maintenance_cleanup_interval_secs(&runtime_config),
            DEFAULT_MAINTENANCE_CLEANUP_INTERVAL_SECS
        );
    }

    #[test]
    fn runtime_operation_readers_accept_positive_values() {
        let runtime_config = runtime_config(&[
            (BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY, "7"),
            (BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY, "90"),
            (BACKGROUND_TASK_MAX_CONCURRENCY_KEY, "8"),
            (BACKGROUND_TASK_MAX_ATTEMPTS_KEY, "5"),
            (TASK_LIST_MAX_LIMIT_KEY, "250"),
            (MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY, "7200"),
        ]);

        assert_eq!(background_task_dispatch_interval_secs(&runtime_config), 7);
        assert_eq!(
            background_task_dispatch_idle_max_interval_secs(&runtime_config),
            90
        );
        assert_eq!(background_task_max_concurrency(&runtime_config), 8);
        assert_eq!(background_task_max_attempts(&runtime_config), 5);
        assert_eq!(task_list_max_limit(&runtime_config), 250);
        assert_eq!(maintenance_cleanup_interval_secs(&runtime_config), 7200);
    }

    #[test]
    fn runtime_operation_readers_fall_back_for_invalid_or_non_positive_values() {
        let runtime_config = runtime_config(&[
            (BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY, "0"),
            (BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS_KEY, "-1"),
            (BACKGROUND_TASK_MAX_CONCURRENCY_KEY, "not-a-number"),
            (BACKGROUND_TASK_MAX_ATTEMPTS_KEY, "0"),
            (TASK_LIST_MAX_LIMIT_KEY, ""),
            (MAINTENANCE_CLEANUP_INTERVAL_SECS_KEY, "1.5"),
        ]);

        assert_eq!(
            background_task_dispatch_interval_secs(&runtime_config),
            DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS
        );
        assert_eq!(
            background_task_dispatch_idle_max_interval_secs(&runtime_config),
            DEFAULT_BACKGROUND_TASK_DISPATCH_IDLE_MAX_INTERVAL_SECS
        );
        assert_eq!(
            background_task_max_concurrency(&runtime_config),
            DEFAULT_BACKGROUND_TASK_MAX_CONCURRENCY
        );
        assert_eq!(
            background_task_max_attempts(&runtime_config),
            DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS
        );
        assert_eq!(
            task_list_max_limit(&runtime_config),
            DEFAULT_TASK_LIST_MAX_LIMIT
        );
        assert_eq!(
            maintenance_cleanup_interval_secs(&runtime_config),
            DEFAULT_MAINTENANCE_CLEANUP_INTERVAL_SECS
        );
    }

    #[test]
    fn background_task_max_attempts_falls_back_when_value_exceeds_i32() {
        let runtime_config = runtime_config(&[(BACKGROUND_TASK_MAX_ATTEMPTS_KEY, "2147483648")]);

        assert_eq!(
            background_task_max_attempts(&runtime_config),
            DEFAULT_BACKGROUND_TASK_MAX_ATTEMPTS
        );
    }
}
