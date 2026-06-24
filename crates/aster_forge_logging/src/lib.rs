//! Shared tracing subscriber setup for Aster services.
//!
//! This crate provides the complete logging initialization behavior used by the application
//! repositories: stdout or file output, optional daily rotation, bounded retained log files,
//! `RUST_LOG` precedence over configured levels, text or JSON formatting, debug-build file and line
//! annotations, and a non-blocking writer guard. Applications keep their own configuration structs
//! and map them into [`LoggingConfig`].
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;

const DEFAULT_TEXT_FORMAT: &str = "text";
const DEFAULT_JSON_FORMAT: &str = "json";
const DEFAULT_LOG_FILENAME: &str = "aster.log";

/// Logging initialization options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Tracing filter directive used when `RUST_LOG` is not set.
    pub level: String,
    /// Output format. `"json"` enables JSON logs; every other value uses text formatting.
    pub format: String,
    /// Log file path. Empty values write to stdout.
    pub file: String,
    /// Enables daily rolling files when `file` is non-empty.
    pub enable_rotation: bool,
    /// Maximum number of rotated log files to retain.
    pub max_backups: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: DEFAULT_TEXT_FORMAT.to_string(),
            file: String::new(),
            enable_rotation: true,
            max_backups: 5,
        }
    }
}

/// Result returned after installing the tracing subscriber.
pub struct LoggingInitResult {
    /// Guard that keeps the non-blocking logging worker alive.
    pub guard: WorkerGuard,
    /// Startup warnings produced while selecting writer or filter settings.
    pub warning: Option<String>,
}

/// Initializes global tracing subscriber using Aster's standard runtime logging behavior.
///
/// File writer failures fall back to stdout and are reported through [`LoggingInitResult::warning`].
/// `RUST_LOG` takes precedence over [`LoggingConfig::level`] and also produces a warning so startup
/// logs can surface which setting actually won.
pub fn init_logging(config: &LoggingConfig) -> LoggingInitResult {
    let (writer, warning) = build_writer(config);
    let (non_blocking_writer, guard) = tracing_appender::non_blocking(writer);

    let mut warning = warning;
    let filter = build_filter(&config.level, &mut warning);
    let is_stdout = config.file.is_empty();

    let builder = tracing_subscriber::fmt()
        .with_writer(non_blocking_writer)
        .with_env_filter(filter)
        .with_level(true)
        .with_ansi(is_stdout);

    #[cfg(debug_assertions)]
    let builder = builder.with_file(true).with_line_number(true);

    if config.format == DEFAULT_JSON_FORMAT {
        builder.json().init();
    } else {
        builder.init();
    }

    LoggingInitResult { guard, warning }
}

fn build_writer(config: &LoggingConfig) -> (Box<dyn Write + Send + Sync>, Option<String>) {
    if config.file.is_empty() {
        return (Box::new(std::io::stdout()), None);
    }

    if config.enable_rotation {
        build_rolling_writer(config)
    } else {
        build_file_writer(&config.file)
    }
}

fn build_rolling_writer(config: &LoggingConfig) -> (Box<dyn Write + Send + Sync>, Option<String>) {
    let path = Path::new(&config.file);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(DEFAULT_LOG_FILENAME));
    let filename = filename.to_str().unwrap_or(DEFAULT_LOG_FILENAME);
    let max_log_files = usize::try_from(config.max_backups).unwrap_or(usize::MAX);

    match rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_prefix(filename.trim_end_matches(".log"))
        .filename_suffix("log")
        .max_log_files(max_log_files)
        .build(dir)
    {
        Ok(appender) => (Box::new(appender), None),
        Err(error) => (
            Box::new(std::io::stdout()),
            Some(format!(
                "Failed to create rolling log appender for '{}': {}. Falling back to stdout.",
                config.file, error
            )),
        ),
    }
}

fn build_file_writer(file: &str) -> (Box<dyn Write + Send + Sync>, Option<String>) {
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        Ok(file) => (Box::new(file), None),
        Err(error) => (
            Box::new(std::io::stdout()),
            Some(format!(
                "Failed to open log file '{}': {}. Falling back to stdout.",
                file, error
            )),
        ),
    }
}

fn build_filter(level: &str, warning: &mut Option<String>) -> EnvFilter {
    match EnvFilter::try_from_default_env() {
        Ok(filter) => {
            push_warning(
                warning,
                format!(
                    "RUST_LOG environment variable detected; config.toml logging.level='{level}' is overridden by RUST_LOG"
                ),
            );
            filter
        }
        Err(_) => match EnvFilter::try_new(level) {
            Ok(filter) => filter,
            Err(error) => {
                push_warning(
                    warning,
                    format!("Invalid logging.level '{level}': {error}. Falling back to 'info'."),
                );
                EnvFilter::new("info")
            }
        },
    }
}

fn push_warning(warning: &mut Option<String>, message: String) {
    if let Some(existing) = warning.as_mut() {
        existing.push(' ');
        existing.push_str(&message);
    } else {
        *warning = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::{LoggingConfig, build_filter, build_writer};

    #[test]
    fn build_writer_uses_stdout_for_empty_file() {
        let (_writer, warning) = build_writer(&LoggingConfig {
            file: String::new(),
            ..LoggingConfig::default()
        });

        assert!(warning.is_none());
    }

    #[test]
    fn build_writer_reports_invalid_file_and_falls_back_to_stdout() {
        let (_writer, warning) = build_writer(&LoggingConfig {
            file: "/definitely-missing-parent/aster.log".to_string(),
            enable_rotation: false,
            ..LoggingConfig::default()
        });

        let warning = warning.expect("invalid file path should report warning");
        assert!(warning.contains("Failed to open log file"));
        assert!(warning.contains("Falling back to stdout"));
    }

    #[test]
    fn build_filter_reports_invalid_level_warning() {
        let mut warning = None;
        let _filter = build_filter("aster=not-a-level", &mut warning);

        let warning = warning.expect("invalid level should report warning");
        assert!(
            warning.contains("Invalid logging.level")
                || warning.contains("RUST_LOG environment variable detected"),
            "{warning}"
        );
    }
}
