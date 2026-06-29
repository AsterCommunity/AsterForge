//! Static configuration schema.
//!
//! Values in this module are loaded before the runtime component graph starts. These conservative
//! defaults keep the generated service runnable; `config.toml` can override any subset of them.

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
                backend: "memory".to_string(),
                endpoint: String::new(),
                default_ttl: 3600,
            },
            config_sync: aster_forge_config::ConfigSyncConfig {
                backend: "disabled".to_string(),
                endpoint: String::new(),
                topic: "{{project-name}}.config_reload".to_string(),
            },
            logging: aster_forge_logging::LoggingConfig {
                level: "info".to_string(),
                format: "text".to_string(),
                file: String::new(),
                enable_rotation: false,
                max_backups: 5,
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
            host: "127.0.0.1".to_string(),
            port: {{server_port}},
            workers: 0,
            temp_dir: ".tmp".to_string(),
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
            url: "sqlite://{{project-name}}.db?mode=rwc".to_string(),
            pool_size: 10,
            retry_count: 3,
        }
    }
}
