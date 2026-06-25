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
    use super::{
        DEFAULT_JSON_FORMAT, LoggingConfig, build_file_writer, build_filter, build_rolling_writer,
        build_writer, init_logging,
    };
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_PATH_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_test_path(test_name: &str) -> std::path::PathBuf {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();

        std::env::temp_dir()
            .join("aster_forge_logging_tests")
            .join(format!(
                "{}-{}-{}-{}-{}",
                test_name,
                std::process::id(),
                thread_name.replace(':', "_"),
                timestamp,
                counter
            ))
    }

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
    fn build_file_writer_creates_file_and_appends_bytes() {
        let path = unique_test_path("build_file_writer_creates_file_and_appends_bytes");
        let path = path.join("logs").join("aster.log");
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
        let root = unique_test_path("build_rolling_writer_creates_daily_appender");
        std::fs::create_dir_all(&root).expect("fixture directory should be created");
        let file = root.join("service.log");

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
        let parent_file = unique_test_path("build_rolling_writer_reports_invalid_directory");
        std::fs::create_dir_all(parent_file.parent().expect("test path has parent"))
            .expect("fixture root should be created");
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
        let log_path = unique_test_path("init_logging_child")
            .join("logs")
            .join("aster.log");
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
