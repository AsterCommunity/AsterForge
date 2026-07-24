//! Shared tracing subscriber setup for Aster services.
//!
//! This crate provides the complete logging initialization behavior used by the application
//! repositories: stdout or file output, optional daily rotation, bounded retained log files,
//! `RUST_LOG` precedence over configured levels, text or JSON formatting, debug-build file and line
//! annotations, and a non-blocking writer guard. Applications can use [`LoggingConfig`] directly
//! in their deployment configuration schema.
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
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Tracing filter directive used when `RUST_LOG` is not set.
    #[serde(default = "LoggingConfig::default_level")]
    pub level: String,
    /// Output format. `"json"` enables JSON logs; every other value uses text formatting.
    #[serde(default = "LoggingConfig::default_format")]
    pub format: String,
    /// Log file path. Empty values write to stdout.
    #[serde(default)]
    pub file: String,
    /// Enables daily rolling files when `file` is non-empty.
    #[serde(default = "LoggingConfig::default_enable_rotation")]
    pub enable_rotation: bool,
    /// Maximum number of rotated log files to retain.
    #[serde(default = "LoggingConfig::default_max_backups")]
    pub max_backups: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: Self::default_level(),
            format: Self::default_format(),
            file: String::new(),
            enable_rotation: Self::default_enable_rotation(),
            max_backups: Self::default_max_backups(),
        }
    }
}

impl LoggingConfig {
    fn default_level() -> String {
        "info".to_string()
    }

    fn default_format() -> String {
        DEFAULT_TEXT_FORMAT.to_string()
    }

    const fn default_enable_rotation() -> bool {
        true
    }

    const fn default_max_backups() -> u32 {
        5
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
/// logs can surface which setting actually won; an invalid `RUST_LOG` value warns and falls back to
/// the configured level instead of being silently ignored. Installing a second global subscriber
/// (embedded runtimes, shared test processes) keeps the existing subscriber and warns rather than
/// panicking.
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

    let init_result = if config.format == DEFAULT_JSON_FORMAT {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    if let Err(error) = init_result {
        // A global subscriber already exists (embedded runtimes, tests sharing
        // a process). Panicking would break the crate's graceful-degradation
        // contract, so degrade to a startup warning like the other fallbacks.
        push_warning(
            &mut warning,
            format!(
                "Global tracing subscriber was already installed; keeping the existing subscriber: {error}"
            ),
        );
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
    // Probe RUST_LOG explicitly: `EnvFilter::try_from_default_env` cannot
    // distinguish "unset" from "invalid", and silently falling back to the
    // config level would leave operators believing their override worked.
    match std::env::var("RUST_LOG") {
        Ok(value) => match EnvFilter::try_new(&value) {
            Ok(filter) => {
                push_warning(
                    warning,
                    format!(
                        "RUST_LOG environment variable detected; config.toml logging.level='{level}' is overridden by RUST_LOG"
                    ),
                );
                filter
            }
            Err(error) => {
                push_warning(
                    warning,
                    format!(
                        "Invalid RUST_LOG value '{value}': {error}. Falling back to logging.level='{level}'."
                    ),
                );
                build_config_filter(level, warning)
            }
        },
        Err(std::env::VarError::NotPresent) => build_config_filter(level, warning),
        Err(std::env::VarError::NotUnicode(value)) => {
            push_warning(
                warning,
                format!(
                    "Invalid RUST_LOG value '{}': not valid Unicode. Falling back to logging.level='{level}'.",
                    value.to_string_lossy()
                ),
            );
            build_config_filter(level, warning)
        }
    }
}

fn build_config_filter(level: &str, warning: &mut Option<String>) -> EnvFilter {
    match EnvFilter::try_new(level) {
        Ok(filter) => filter,
        Err(error) => {
            push_warning(
                warning,
                format!("Invalid logging.level '{level}': {error}. Falling back to 'info'."),
            );
            EnvFilter::new("info")
        }
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
    use super::{
        DEFAULT_JSON_FORMAT, LoggingConfig, build_file_writer, build_filter, build_rolling_writer,
        build_writer, init_logging,
    };
    use aster_forge_test::temp::TestTempDir;
    use std::io::Write;

    #[test]
    fn build_writer_uses_stdout_for_empty_file() {
        let (_writer, warning) = build_writer(&LoggingConfig {
            file: String::new(),
            ..LoggingConfig::default()
        });

        assert!(warning.is_none());
    }

    #[test]
    fn logging_config_deserializes_missing_fields_with_defaults() {
        let config: LoggingConfig =
            serde_json::from_str("{}").expect("empty logging config should use field defaults");

        assert_eq!(config, LoggingConfig::default());
    }

    #[test]
    fn build_writer_reports_invalid_file_and_falls_back_to_stdout() {
        let directory = TestTempDir::new("logging-invalid-file");
        let parent_file = directory.join("not-a-directory");
        std::fs::write(&parent_file, "fixture").expect("parent fixture should be writable");
        let (_writer, warning) = build_writer(&LoggingConfig {
            file: parent_file.join("aster.log").to_string_lossy().into_owned(),
            enable_rotation: false,
            ..LoggingConfig::default()
        });

        let warning = warning.expect("invalid file path should report warning");
        assert!(warning.contains("Failed to open log file"));
        assert!(warning.contains("Falling back to stdout"));
    }

    #[test]
    fn build_file_writer_creates_file_and_appends_bytes() {
        let directory = TestTempDir::new("logging-file-writer");
        let path = directory.join("logs").join("aster.log");
        std::fs::create_dir_all(path.parent().expect("test path has parent"))
            .expect("fixture parent should be created");

        let (mut writer, warning) =
            build_file_writer(path.to_str().expect("test path should be utf-8"));
        assert!(warning.is_none());
        writer
            .write_all(b"first line\n")
            .expect("file writer should accept first write");
        drop(writer);

        let (mut writer, warning) =
            build_file_writer(path.to_str().expect("test path should be utf-8"));
        assert!(warning.is_none());
        writer
            .write_all(b"second line\n")
            .expect("file writer should append second write");
        drop(writer);

        let contents =
            std::fs::read_to_string(&path).expect("file writer output should be readable");
        assert_eq!(contents, "first line\nsecond line\n");
    }

    #[test]
    fn build_rolling_writer_creates_daily_appender_for_valid_directory() {
        let directory = TestTempDir::new("logging-rolling-writer");
        let file = directory.join("service.log");

        let (mut writer, warning) = build_rolling_writer(&LoggingConfig {
            file: file.to_string_lossy().into_owned(),
            max_backups: 2,
            ..LoggingConfig::default()
        });

        assert!(warning.is_none());
        writer
            .write_all(b"rolling line\n")
            .expect("rolling writer should accept writes");
    }

    #[test]
    fn build_rolling_writer_reports_invalid_directory_and_falls_back_to_stdout() {
        let directory = TestTempDir::new("logging-invalid-rolling-directory");
        let parent_file = directory.join("not-a-directory");
        std::fs::write(&parent_file, "not a directory")
            .expect("parent-file fixture should be writable");
        let file = parent_file.join("aster.log");

        let (_writer, warning) = build_rolling_writer(&LoggingConfig {
            file: file.to_string_lossy().into_owned(),
            ..LoggingConfig::default()
        });

        let warning = warning.expect("invalid rolling log path should report warning");
        assert!(warning.contains("Failed to create rolling log appender"));
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

    #[test]
    fn init_logging_warns_instead_of_panicking_when_subscriber_already_installed() {
        // The first install wins (this is the only in-process global init in
        // the test binary; the other init test uses a child process).
        let _first = init_logging(&LoggingConfig::default());

        let second = init_logging(&LoggingConfig::default());
        let warning = second
            .warning
            .expect("re-initializing should report a warning instead of panicking");
        assert!(
            warning.contains("already installed"),
            "unexpected warning: {warning}"
        );
    }

    #[test]
    fn build_filter_warns_when_rust_log_is_invalid_in_child_process() {
        if std::env::var("ASTER_FORGE_LOGGING_FILTER_CHILD").is_ok() {
            run_build_filter_child();
            return;
        }

        let current_exe = std::env::current_exe().expect("current test executable should resolve");
        let output = std::process::Command::new(current_exe)
            .arg("--exact")
            .arg("tests::build_filter_warns_when_rust_log_is_invalid_in_child_process")
            .arg("--nocapture")
            .env("ASTER_FORGE_LOGGING_FILTER_CHILD", "1")
            .env("RUST_LOG", "aster=not-a-level")
            .output()
            .expect("build_filter child process should run");

        assert!(
            output.status.success(),
            "child process failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_build_filter_child() {
        let mut warning = None;
        let _filter = build_filter("debug", &mut warning);

        let warning = warning.expect("invalid RUST_LOG should report warning");
        assert!(
            warning.contains("Invalid RUST_LOG"),
            "unexpected warning: {warning}"
        );
        assert!(
            warning.contains("logging.level='debug'"),
            "fallback should name the config level: {warning}"
        );
    }

    #[test]
    fn init_logging_can_initialize_global_subscriber_in_child_process() {
        if std::env::var("ASTER_FORGE_LOGGING_INIT_CHILD").is_ok() {
            run_init_logging_child();
            return;
        }

        let current_exe = std::env::current_exe().expect("current test executable should resolve");
        let output = std::process::Command::new(current_exe)
            .arg("--exact")
            .arg("tests::init_logging_can_initialize_global_subscriber_in_child_process")
            .arg("--nocapture")
            .env("ASTER_FORGE_LOGGING_INIT_CHILD", "1")
            .env_remove("RUST_LOG")
            .output()
            .expect("init_logging child process should run");

        assert!(
            output.status.success(),
            "child process failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_init_logging_child() {
        let directory = TestTempDir::new("logging-init-child");
        let log_path = directory.join("logs").join("aster.log");
        std::fs::create_dir_all(log_path.parent().expect("test path has parent"))
            .expect("fixture parent should be created");

        let result = init_logging(&LoggingConfig {
            level: "aster=not-a-level".to_string(),
            format: DEFAULT_JSON_FORMAT.to_string(),
            file: log_path.to_string_lossy().into_owned(),
            enable_rotation: false,
            max_backups: 1,
        });

        let warning = result
            .warning
            .expect("invalid logging level should report startup warning");
        assert!(warning.contains("Invalid logging.level"));
        drop(result.guard);
    }
}
