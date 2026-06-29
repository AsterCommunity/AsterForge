//! Static configuration loader.
//!
//! The loader keeps generated services convenient for local development while supporting real
//! deployment files. Like AsterDrive and AsterYggdrasil, first startup writes `data/config.toml`,
//! then loads that file and overlays `ASTER__...` environment variables.

use std::path::{Path, PathBuf};

use crate::errors::{AppError, Result};

/// Environment variable used to override the static configuration file path.
pub const CONFIG_ENV_VAR: &str = "ASTER_CONFIG";

/// Default static configuration file path.
pub const DEFAULT_CONFIG_PATH: &str = "data/config.toml";

/// Loads static configuration from `ASTER_CONFIG` or `data/config.toml`.
pub fn load() -> Result<super::AppConfig> {
    let path = config_path();
    ensure_default_config_exists(&path, &super::AppConfig::default())?;

    let mut loaded = ::config::Config::builder()
        .add_source(::config::File::from(path.as_path()).required(false))
        .add_source(
            ::config::Environment::with_prefix("ASTER")
                .separator("__")
                .try_parsing(true),
        )
        .build()?
        .try_deserialize::<super::AppConfig>()?;
    normalize_paths(&mut loaded, &path)?;
    tracing::info!(path = %path.display(), "loaded configuration file");
    Ok(loaded)
}

fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

fn ensure_default_config_exists(path: &Path, default: &super::AppConfig) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let toml = toml::to_string_pretty(default).map_err(|error| {
        AppError::Config(format!(
            "failed to serialize default configuration: {error}"
        ))
    })?;
    let content = format!(
        "# {service} configuration file\n\
         # Generated on first startup; edit as needed.\n\
         # Relative paths are resolved against the directory containing this file.\n\n\
         {toml}",
        service = env!("CARGO_PKG_NAME")
    );
    std::fs::write(path, content)?;
    Ok(())
}

fn normalize_paths(config: &mut super::AppConfig, config_path: &Path) -> Result<()> {
    let base_dir = std::env::current_dir()?;
    let raw_config_dir = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let config_dir = if raw_config_dir.is_absolute() {
        raw_config_dir.to_path_buf()
    } else {
        base_dir.join(raw_config_dir)
    };

    config.server.temp_dir = aster_forge_utils::paths::resolve_config_relative_path(
        &base_dir,
        &config_dir,
        &config.server.temp_dir,
    )
    .map_err(|error| AppError::Config(error.to_string()))?;
    config.logging.file = if config.logging.file.is_empty() {
        String::new()
    } else {
        aster_forge_utils::paths::resolve_config_relative_path(
            &base_dir,
            &config_dir,
            &config.logging.file,
        )
        .map_err(|error| AppError::Config(error.to_string()))?
    };
    config.database.url = aster_forge_utils::paths::resolve_config_relative_sqlite_url(
        &base_dir,
        &config_dir,
        &config.database.url,
    )
    .map_err(|error| AppError::Config(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_default_file_uses_config_relative_paths() {
        let path = std::env::temp_dir().join(format!(
            "{{project-name}}-loader-default-{}.toml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));

        ensure_default_config_exists(&path, &crate::config::AppConfig::default())
            .expect("write default config");
        let generated = std::fs::read_to_string(&path).expect("read default config");

        assert!(generated.contains(r#"temp_dir = ".tmp""#));
        assert!(
            generated.contains(r#"url = "sqlite://{{project-name}}.db?mode=rwc""#)
        );

        let _ = std::fs::remove_file(path);
    }
}
