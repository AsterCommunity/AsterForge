//! Shared panic hook and crash report writer for Aster services.
//!
//! This crate implements the full crash-reporting behavior used by Aster
//! services: a process-wide panic hook, a lazily opened crash log, backtrace
//! capture for developer diagnostics, user-facing stderr notices, and a
//! repository issue target. Product crates provide names, versions, repository
//! URLs, and crash log paths through [`PanicHookConfig`].
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

use std::any::Any;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Default crash log path used by Aster services.
pub const DEFAULT_CRASH_LOG_PATH: &str = "data/crash.log";
/// Default repository issue template path used in panic notices.
pub const DEFAULT_ISSUE_TEMPLATE: &str = "issues/new?template=bug_report.yml";

static CRASH_LOG: OnceLock<Result<Mutex<std::fs::File>, String>> = OnceLock::new();
static PANIC_HOOK_CONFIG: OnceLock<PanicHookConfig> = OnceLock::new();

/// Configuration used by the shared panic hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicHookConfig {
    /// Human-facing service name shown in crash reports.
    pub app_name: String,
    /// Service version shown in crash reports.
    pub version: String,
    /// Repository URL used to build issue-report targets.
    pub repository: String,
    /// Path to the crash log file.
    pub crash_log_path: PathBuf,
    /// Repository-relative issue template path.
    pub issue_template: String,
}

impl PanicHookConfig {
    /// Creates a panic hook config with Aster defaults for path and issue template.
    pub fn new(
        app_name: impl Into<String>,
        version: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            app_name: app_name.into(),
            version: version.into(),
            repository: repository.into(),
            crash_log_path: PathBuf::from(DEFAULT_CRASH_LOG_PATH),
            issue_template: DEFAULT_ISSUE_TEMPLATE.to_string(),
        }
    }

    /// Overrides the crash log path.
    pub fn with_crash_log_path(mut self, crash_log_path: impl Into<PathBuf>) -> Self {
        self.crash_log_path = crash_log_path.into();
        self
    }

    /// Overrides the repository-relative issue template path.
    pub fn with_issue_template(mut self, issue_template: impl Into<String>) -> Self {
        self.issue_template = issue_template.into();
        self
    }
}

#[derive(Debug, Clone)]
struct PanicContext {
    app_name: String,
    version: String,
    platform: &'static str,
    repository: String,
    issue_template: String,
    timestamp: String,
    thread_name: String,
    location: String,
    message: String,
}

#[derive(Debug, Clone)]
struct CrashReportWriteFailure {
    reason: String,
    report: String,
}

impl CrashReportWriteFailure {
    fn new(reason: String, context: &PanicContext) -> Self {
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        Self {
            reason,
            report: render_crash_report(context, &backtrace),
        }
    }
}

/// Installs the shared panic hook for the current process.
///
/// The first configuration installed in a process is retained. This matches the
/// process-wide nature of Rust panic hooks and avoids swapping crash-log targets
/// after a hook has already been installed.
pub fn install_panic_hook(config: PanicHookConfig) {
    let _config_already_installed = PANIC_HOOK_CONFIG.set(config.clone()).is_err();
    panic::set_hook(Box::new(move |info| {
        let config = PANIC_HOOK_CONFIG.get().unwrap_or(&config);
        let thread = std::thread::current();
        let context = PanicContext {
            app_name: config.app_name.clone(),
            version: config.version.clone(),
            platform: std::env::consts::OS,
            repository: config.repository.clone(),
            issue_template: config.issue_template.clone(),
            timestamp: chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            thread_name: thread.name().unwrap_or("<unnamed>").to_string(),
            location: info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown>".to_string()),
            message: panic_payload_message(info.payload()),
        };

        let crash_log_path = crash_log_display_path(&config.crash_log_path);
        let crash_log_result = write_crash_report(&config.crash_log_path, &context);
        let failure_report = crash_log_result
            .as_ref()
            .err()
            .map(|failure| failure.report.trim_end());
        let notice = render_user_panic_notice(&context, &crash_log_path, crash_log_result.as_ref());
        write_stderr_diagnostics(failure_report, &notice);
    }));
}

/// Prints panic diagnostics to stderr on a best-effort basis.
fn write_stderr_diagnostics(failure_report: Option<&str>, notice: &str) {
    write_diagnostics(&mut std::io::stderr().lock(), failure_report, notice);
}

/// Writes the crash report (when the log file failed) followed by the user
/// notice, ignoring IO errors.
///
/// stderr may be closed or a broken pipe; a panicking write (`eprintln!`)
/// inside the panic hook would abort the process via double panic and destroy
/// the very diagnostics this hook exists to capture.
fn write_diagnostics(mut writer: impl Write, failure_report: Option<&str>, notice: &str) {
    if let Some(report) = failure_report {
        let _ = writeln!(writer, "{report}");
    }
    let _ = writeln!(writer, "{notice}");
}

fn write_crash_report(
    crash_log_path: &Path,
    context: &PanicContext,
) -> Result<(), CrashReportWriteFailure> {
    let file_mutex = crash_log_file(crash_log_path)
        .map_err(|reason| CrashReportWriteFailure::new(reason, context))?;
    write_crash_report_to_file(file_mutex, crash_log_path, context)
}

fn write_crash_report_to_file(
    file_mutex: &Mutex<std::fs::File>,
    crash_log_path: &Path,
    context: &PanicContext,
) -> Result<(), CrashReportWriteFailure> {
    let mut guard = file_mutex.try_lock().map_err(|_| {
        CrashReportWriteFailure::new(
            "crash log is locked by another panic writer".to_string(),
            context,
        )
    })?;

    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let crash_report = render_crash_report(context, &backtrace);
    guard
        .write_all(crash_report.as_bytes())
        .map_err(|error| CrashReportWriteFailure {
            reason: format!("failed to write {}: {error}", crash_log_path.display()),
            report: crash_report,
        })
}

fn crash_log_file(crash_log_path: &Path) -> Result<&'static Mutex<std::fs::File>, String> {
    CRASH_LOG
        .get_or_init(|| open_crash_log_file(crash_log_path))
        .as_ref()
        .map_err(Clone::clone)
}

fn open_crash_log_file(crash_log_path: &Path) -> Result<Mutex<std::fs::File>, String> {
    if let Some(parent) = crash_log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create crash log dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(crash_log_path)
        .map(Mutex::new)
        .map_err(|error| format!("failed to open {}: {error}", crash_log_path.display()))
}

fn crash_log_display_path(crash_log_path: &Path) -> PathBuf {
    std::env::current_dir()
        .map(|dir| dir.join(crash_log_path))
        .unwrap_or_else(|_| crash_log_path.to_path_buf())
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn issue_report_target(repository: &str, issue_template: &str) -> String {
    let repository = repository.trim_end_matches('/');
    let issue_template = issue_template.trim_start_matches('/');
    if repository.is_empty() {
        "the project issue tracker".to_string()
    } else if issue_template.is_empty() {
        repository.to_string()
    } else {
        format!("{repository}/{issue_template}")
    }
}

fn render_crash_report(context: &PanicContext, backtrace: &str) -> String {
    let report_target = issue_report_target(&context.repository, &context.issue_template);
    format!(
        "=== {} Panic Report ===\n\
         Version:   {}\n\
         Platform:  {}\n\
         Timestamp: {}\n\
         Thread:    {}\n\
         Location:  {}\n\
         Message:   {}\n\
         Report:    {}\n\
         Backtrace:\n{}\n\
         ===============================\n\n",
        context.app_name,
        context.version,
        context.platform,
        context.timestamp,
        context.thread_name,
        context.location,
        context.message,
        report_target,
        backtrace.trim_end()
    )
}

fn render_user_panic_notice(
    context: &PanicContext,
    crash_log_path: &Path,
    crash_log_result: Result<&(), &CrashReportWriteFailure>,
) -> String {
    let report_target = issue_report_target(&context.repository, &context.issue_template);
    let diagnostic_line = match crash_log_result {
        Ok(()) => format!(
            "A diagnostic report was written to {}.",
            crash_log_path.display()
        ),
        Err(failure) => format!(
            "A diagnostic report could not be written to {}: {}.",
            crash_log_path.display(),
            failure.reason
        ),
    };

    let fallback_line = match crash_log_result {
        Ok(()) => String::new(),
        Err(_) => " The diagnostic report was printed to stderr instead.".to_string(),
    };

    format!(
        "{} encountered an unexpected internal error.\n\
         {diagnostic_line}{fallback_line}\n\
         Timestamp: {}\n\
         If the process exits, restart {} and report the diagnostic report at:\n\
         {report_target}",
        context.app_name, context.timestamp, context.app_name
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CrashReportWriteFailure, PanicContext, PanicHookConfig, issue_report_target,
        open_crash_log_file, panic_payload_message, render_crash_report, render_user_panic_notice,
        write_crash_report_to_file,
    };
    use aster_forge_test::temp::TestTempDir;
    use std::sync::{Mutex, OnceLock};

    const PANIC_HOOK_CHILD_ENV: &str = "ASTER_FORGE_PANIC_HOOK_CHILD";
    const PANIC_HOOK_PATH_ENV: &str = "ASTER_FORGE_PANIC_HOOK_PATH";

    fn write_crash_report_with_log(
        crash_log: &OnceLock<Result<Mutex<std::fs::File>, String>>,
        crash_log_path: &std::path::Path,
        context: &PanicContext,
    ) -> Result<(), CrashReportWriteFailure> {
        let file_mutex = crash_log_file_from(crash_log, crash_log_path)
            .map_err(|reason| CrashReportWriteFailure::new(reason, context))?;
        write_crash_report_to_file(file_mutex, crash_log_path, context)
    }

    fn crash_log_file_from<'a>(
        crash_log: &'a OnceLock<Result<Mutex<std::fs::File>, String>>,
        crash_log_path: &std::path::Path,
    ) -> Result<&'a Mutex<std::fs::File>, String> {
        crash_log
            .get_or_init(|| open_crash_log_file(crash_log_path))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn test_context() -> PanicContext {
        PanicContext {
            app_name: "AsterDrive".to_string(),
            version: "0.1.0-test".to_string(),
            platform: "test-os",
            repository: "https://example.test/asterdrive/".to_string(),
            issue_template: super::DEFAULT_ISSUE_TEMPLATE.to_string(),
            timestamp: "2026-05-05 12:34:56.789".to_string(),
            thread_name: "test-thread".to_string(),
            location: "src/main.rs:42:9".to_string(),
            message: "secret panic payload".to_string(),
        }
    }

    fn crash_log_fixture(scope: &str) -> (TestTempDir, std::path::PathBuf) {
        let directory = TestTempDir::new(scope);
        let path = directory.join("crash.log");
        (directory, path)
    }

    fn write_parent_file_fixture(path: &std::path::Path) {
        let parent = path
            .parent()
            .expect("parent-file fixture should have parent");
        std::fs::create_dir_all(parent).expect("parent-file fixture dir should be writable");
        std::fs::write(path, "not a directory").expect("parent-file fixture should be writable");
    }

    #[test]
    fn user_notice_is_short_and_omits_developer_diagnostics() {
        let context = test_context();
        let notice = render_user_panic_notice(
            &context,
            std::path::Path::new("/tmp/asterdrive/data/crash.log"),
            Ok(&()),
        );

        assert!(notice.contains("AsterDrive encountered an unexpected internal error."));
        assert!(notice.contains("/tmp/asterdrive/data/crash.log"));
        assert!(notice.contains("2026-05-05 12:34:56.789"));
        assert!(
            notice.contains("https://example.test/asterdrive/issues/new?template=bug_report.yml")
        );
        assert!(!notice.contains("src/main.rs:42:9"));
        assert!(!notice.contains("secret panic payload"));
        assert!(!notice.contains("Backtrace"));
    }

    #[test]
    fn user_notice_reports_when_crash_log_could_not_be_written() {
        let context = test_context();
        let failure = CrashReportWriteFailure {
            reason: "permission denied".to_string(),
            report: render_crash_report(&context, "frame 1"),
        };
        let notice = render_user_panic_notice(
            &context,
            std::path::Path::new("data/crash.log"),
            Err(&failure),
        );

        assert!(notice.contains("could not be written"));
        assert!(notice.contains("data/crash.log"));
        assert!(notice.contains("permission denied"));
        assert!(notice.contains("printed to stderr"));
    }

    #[test]
    fn crash_report_keeps_developer_diagnostics() {
        let context = test_context();
        let report = render_crash_report(&context, "frame 1\nframe 2\n");

        assert!(report.contains("=== AsterDrive Panic Report ==="));
        assert!(report.contains("Version:   0.1.0-test"));
        assert!(report.contains("Platform:  test-os"));
        assert!(report.contains("Thread:    test-thread"));
        assert!(report.contains("Location:  src/main.rs:42:9"));
        assert!(report.contains("Message:   secret panic payload"));
        assert!(report.contains(
            "Report:    https://example.test/asterdrive/issues/new?template=bug_report.yml"
        ));
        assert!(report.contains("Backtrace:\nframe 1\nframe 2"));
    }

    #[test]
    fn crash_report_write_failure_new_renders_report_with_reason() {
        let context = test_context();
        let failure = CrashReportWriteFailure::new("permission denied".to_string(), &context);

        assert_eq!(failure.reason, "permission denied");
        assert!(failure.report.contains("=== AsterDrive Panic Report ==="));
        assert!(failure.report.contains("Message:   secret panic payload"));
        assert!(failure.report.contains("Backtrace:"));
    }

    #[test]
    fn crash_log_file_creates_parent_directory_and_reuses_file() {
        let (_directory, path) = crash_log_fixture("panic-reused-file");
        let crash_log = OnceLock::new();

        let first = crash_log_file_from(&crash_log, &path).expect("crash log should open");
        let second = crash_log_file_from(&crash_log, &path).expect("crash log should be reused");

        assert!(path.parent().expect("test path has parent").exists());
        assert!(path.exists());
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn crash_log_file_returns_cached_initialization_error() {
        let (_directory, path) = crash_log_fixture("panic-cached-open-error");
        let crash_log = OnceLock::new();
        write_parent_file_fixture(&path);
        let nested_log = path.join("crash.log");

        let first_error = crash_log_file_from(&crash_log, &nested_log)
            .expect_err("file parent should not be usable as directory");
        let second_error = crash_log_file_from(&crash_log, &nested_log)
            .expect_err("cached initialization error should be returned");

        assert!(first_error.contains("failed to create crash log dir"));
        assert_eq!(first_error, second_error);
    }

    #[test]
    fn write_crash_report_appends_developer_report() {
        let (_directory, path) = crash_log_fixture("panic-appended-report");
        let crash_log = OnceLock::new();
        let context = test_context();

        write_crash_report_with_log(&crash_log, &path, &context)
            .expect("crash report should be written");
        write_crash_report_with_log(&crash_log, &path, &context)
            .expect("second crash report should append");

        let contents =
            std::fs::read_to_string(&path).expect("crash report should be readable from fixture");
        assert_eq!(
            contents.matches("=== AsterDrive Panic Report ===").count(),
            2
        );
        assert!(contents.contains("Location:  src/main.rs:42:9"));
        assert!(contents.contains("Message:   secret panic payload"));
    }

    #[test]
    fn write_crash_report_returns_rendered_failure_when_log_is_locked() {
        let (_directory, path) = crash_log_fixture("panic-locked-log");
        let crash_log = OnceLock::new();
        let context = test_context();
        let file_mutex = crash_log_file_from(&crash_log, &path).expect("crash log should open");
        let _locked = file_mutex.lock().expect("fixture lock should be available");

        let failure = write_crash_report_with_log(&crash_log, &path, &context)
            .expect_err("locked crash log should report failure");

        assert_eq!(
            failure.reason,
            "crash log is locked by another panic writer"
        );
        assert!(failure.report.contains("=== AsterDrive Panic Report ==="));
        assert!(failure.report.contains("Message:   secret panic payload"));
    }

    #[test]
    fn write_crash_report_returns_rendered_failure_when_log_cannot_open() {
        let (_directory, path) = crash_log_fixture("panic-open-failure");
        let crash_log = OnceLock::new();
        write_parent_file_fixture(&path);
        let nested_log = path.join("crash.log");
        let context = test_context();

        let failure = write_crash_report_with_log(&crash_log, &nested_log, &context)
            .expect_err("invalid crash log path should report failure");

        assert!(failure.reason.contains("failed to create crash log dir"));
        assert!(failure.report.contains("=== AsterDrive Panic Report ==="));
        assert!(failure.report.contains("Backtrace:"));
    }

    #[test]
    fn install_panic_hook_writes_report_for_caught_thread_panic() {
        if std::env::var_os(PANIC_HOOK_CHILD_ENV).is_some() {
            run_panic_hook_child();
            return;
        }

        let (_directory, path) = crash_log_fixture("panic-installed-hook");
        let current_exe = std::env::current_exe().expect("current test executable should resolve");
        let output = std::process::Command::new(current_exe)
            .arg("--exact")
            .arg("tests::install_panic_hook_writes_report_for_caught_thread_panic")
            .arg("--nocapture")
            .env(PANIC_HOOK_CHILD_ENV, "1")
            .env(PANIC_HOOK_PATH_ENV, &path)
            .output()
            .expect("panic hook child process should run");

        assert!(
            output.status.success(),
            "panic hook child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let contents =
            std::fs::read_to_string(&path).expect("panic hook should write crash report");
        assert!(contents.contains("=== HookTest Panic Report ==="));
        assert!(contents.contains("Version:   9.9.9-test"));
        assert!(contents.contains("Thread:    panic-hook-fixture"));
        assert!(contents.contains("Message:   hook panic payload"));
        assert!(
            contents.contains("Report:    https://example.test/hook/issues/new?template=panic.yml")
        );
    }

    fn run_panic_hook_child() {
        let path = std::env::var_os(PANIC_HOOK_PATH_ENV)
            .map(std::path::PathBuf::from)
            .expect("panic hook child path should be provided");
        let config = PanicHookConfig::new("HookTest", "9.9.9-test", "https://example.test/hook")
            .with_crash_log_path(path)
            .with_issue_template("issues/new?template=panic.yml");

        super::install_panic_hook(config);

        let result = std::thread::Builder::new()
            .name("panic-hook-fixture".to_string())
            .spawn(|| panic!("hook panic payload"))
            .expect("panic fixture thread should spawn")
            .join();

        assert!(result.is_err());
    }

    struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stderr closed",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stderr closed",
            ))
        }
    }

    #[test]
    fn write_diagnostics_ignores_broken_stderr_and_captures_content() {
        // A closed or broken-pipe stderr must not panic the panic hook (the
        // old `eprintln!` would, aborting the process via double panic).
        super::write_diagnostics(FailingWriter, Some("report-body"), "notice-body");

        let mut captured = Vec::new();
        super::write_diagnostics(&mut captured, Some("report-body"), "notice-body");
        let output = String::from_utf8(captured).expect("diagnostics should be utf-8");
        assert_eq!(output, "report-body\nnotice-body\n");

        let mut captured = Vec::new();
        super::write_diagnostics(&mut captured, None, "notice-body");
        let output = String::from_utf8(captured).expect("diagnostics should be utf-8");
        assert_eq!(output, "notice-body\n");
    }

    #[test]
    fn panic_payload_message_handles_common_payload_types() {
        let owned = "owned panic".to_string();

        assert_eq!(panic_payload_message(&"static panic"), "static panic");
        assert_eq!(panic_payload_message(&owned), "owned panic");
        assert_eq!(
            panic_payload_message(&123_i32),
            "<non-string panic payload>"
        );
    }

    #[test]
    fn issue_report_target_tolerates_empty_repository() {
        assert_eq!(
            issue_report_target(
                "https://example.test/project/",
                super::DEFAULT_ISSUE_TEMPLATE
            ),
            "https://example.test/project/issues/new?template=bug_report.yml"
        );
        assert_eq!(
            issue_report_target("", super::DEFAULT_ISSUE_TEMPLATE),
            "the project issue tracker"
        );
        assert_eq!(
            issue_report_target("https://example.test/project", ""),
            "https://example.test/project"
        );
    }
}
