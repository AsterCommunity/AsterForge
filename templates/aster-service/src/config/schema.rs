//! Static configuration schema.
//!
//! Values in this module are loaded before the runtime component graph starts. The generated
//! defaults come from `cargo generate`; `config.toml` can override any subset of these fields.

/// Static service configuration used by the generated skeleton.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AppConfig {
    /// HTTP server settings.
    pub server: ServerConfig,
    /// Database settings.
    pub database: DatabaseConfig,
    /// Cache backend settings.
    pub cache: aster_forge_cache::CacheConfig,
    /// Cross-process runtime configuration reload settings.
    pub config_sync: aster_forge_config::ConfigSyncConfig,
    /// Logging settings.
    pub logging: aster_forge_logging::LoggingConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            cache: aster_forge_cache::CacheConfig {
                backend: "{{cache_backend}}".to_string(),
                endpoint: "{{cache_endpoint}}".to_string(),
                default_ttl: {{cache_default_ttl}},
            },
            config_sync: aster_forge_config::ConfigSyncConfig {
                backend: "{{config_sync_backend}}".to_string(),
                endpoint: "{{config_sync_endpoint}}".to_string(),
                topic: "{{config_sync_topic}}".to_string(),
            },
            logging: aster_forge_logging::LoggingConfig {
                level: "{{logging_level}}".to_string(),
                format: "{{logging_format}}".to_string(),
                file: "{{logging_file}}".to_string(),
                enable_rotation: {{logging_enable_rotation}},
                max_backups: {{logging_max_backups}},
            },
        }
    }
}

/// HTTP server settings.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ServerConfig {
    /// HTTP bind host.
    pub host: String,
    /// HTTP bind port.
    pub port: u16,
    /// Actix worker count. `0` keeps Actix's default worker count.
    pub workers: usize,
    /// Temporary directory used by product-specific upload or processing paths.
    pub temp_dir: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "{{server_host}}".to_string(),
            port: {{server_port}},
            workers: {{server_workers}},
            temp_dir: "{{server_temp_dir}}".to_string(),
        }
    }
}

/// Database connection settings.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// SeaORM database URL.
    pub url: String,
    /// Maximum pool size for non-SQLite pools and SQLite reader pools.
    pub pool_size: u32,
    /// Connection retry count.
    pub retry_count: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "{{database_url}}".to_string(),
            pool_size: {{database_pool_size}},
            retry_count: {{database_retry_count}},
        }
    }
}
