//! Process-wide best-effort panic reporting for executable entrypoints.
//!
//! Full reports can contain sensitive payloads and source paths. Only the report
//! writer receives them; public sinks receive a separate, minimal diagnostic.
//! Libraries must not install this hook. Callbacks must never panic or block:
//! Rust aborts on a panic inside a panic hook, even with `catch_unwind`.
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
use std::io::{self, Write};
use std::panic;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

pub use private_file::PrivateReportWriter;
mod private_file;

/// Default private report destination.
pub const DEFAULT_CRASH_LOG_PATH: &str = "data/crash.log";
/// Default repository issue template.
pub const DEFAULT_ISSUE_TEMPLATE: &str = "issues/new?template=bug_report.yml";
static INSTALLED: OnceLock<()> = OnceLock::new();

/// Receives sensitive developer reports. Implementations must not panic or wait
/// for locks; return an IO error when busy. Errors are never exposed publicly.
pub trait ReportWriter: Send + Sync {
    /// Writes one complete report on a best-effort basis.
    ///
    /// # Errors
    /// Returns an IO error when the destination is unavailable or busy.
    fn write_report(&self, report: &str) -> io::Result<()>;
}

/// Minimal public diagnostic, deliberately excluding payload, location,
/// backtrace, file paths and writer error text. Products own their JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicDiagnostic {
    /// Product identity.
    pub app_name: String,
    /// UTC RFC3339 timestamp (or a product-provided timestamp).
    pub timestamp: String,
    /// Whether the private report writer succeeded.
    pub report_written: bool,
}

/// Public output policy. A custom sink receives no developer report.
#[derive(Clone, Default)]
pub enum PanicOutput {
    /// Best-effort human notice on stderr.
    #[default]
    Human,
    /// No public output at all.
    Silent,
    /// Product-defined output (for example a JSON envelope).
    Sink(Arc<dyn DiagnosticSink>),
}

/// Public output adapter. Must not panic or block; errors are ignored.
pub trait DiagnosticSink: Send + Sync {
    /// Emits a public diagnostic without receiving sensitive context.
    ///
    /// # Errors
    /// Returns an IO error when public output is unavailable.
    fn emit(&self, diagnostic: &PublicDiagnostic) -> io::Result<()>;
}

/// Panic reporting configuration; installation is explicit and process-wide.
#[derive(Clone)]
pub struct PanicHookConfig {
    /// Product name in the private report and public diagnostic.
    pub app_name: String,
    /// Product version in the private report.
    pub version: String,
    /// Repository URL in the private report only.
    pub repository: String,
    /// Default writer destination; ignored when a custom writer is supplied.
    pub crash_log_path: PathBuf,
    /// Repository-relative issue template in the private report.
    pub issue_template: String,
    /// Public output policy; defaults to human stderr.
    pub output: PanicOutput,
    /// Product-owned secure writer, replacing the default file writer.
    pub report_writer: Option<Arc<dyn ReportWriter>>,
    /// Timestamp source. Defaults to UTC RFC3339 milliseconds. Must not panic.
    pub timestamp: fn() -> String,
}

impl PanicHookConfig {
    /// Creates configuration with private file output and a safe human notice.
    pub fn new(
        app_name: impl Into<String>,
        version: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            app_name: app_name.into(),
            version: version.into(),
            repository: repository.into(),
            crash_log_path: DEFAULT_CRASH_LOG_PATH.into(),
            issue_template: DEFAULT_ISSUE_TEMPLATE.into(),
            output: PanicOutput::Human,
            report_writer: None,
            timestamp: utc_timestamp,
        }
    }
    /// Overrides the default private report path.
    #[must_use]
    pub fn with_crash_log_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.crash_log_path = path.into();
        self
    }
    /// Overrides the private report's issue target.
    #[must_use]
    pub fn with_issue_template(mut self, template: impl Into<String>) -> Self {
        self.issue_template = template.into();
        self
    }
    /// Selects human, silent or product-defined public output.
    #[must_use]
    pub fn with_output(mut self, output: PanicOutput) -> Self {
        self.output = output;
        self
    }
    /// Injects the product's private file/privacy policy.
    #[must_use]
    pub fn with_report_writer(mut self, writer: Arc<dyn ReportWriter>) -> Self {
        self.report_writer = Some(writer);
        self
    }
    /// Selects a deterministic or product-defined timestamp source.
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: fn() -> String) -> Self {
        self.timestamp = timestamp;
        self
    }
}

/// Outcome of an explicit installation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Replaced the prior host hook; it is not chained to prevent raw output.
    Installed,
    /// No global changes; even a subsequently installed host hook is retained.
    AlreadyInstalled,
    /// Installation from a panicking thread is rejected without calling `set_hook`.
    PanickingThread,
}

/// Installs once, only from the executable entrypoint before spawning threads.
/// Replaces (does not chain) the existing hook. Repeated calls never reinstall,
/// even if the host later replaces our hook. Concurrent host `set_hook` calls are
/// outside this contract. This does not recover panics or select an exit code.
pub fn install_panic_hook(config: PanicHookConfig) -> InstallOutcome {
    if std::thread::panicking() {
        return InstallOutcome::PanickingThread;
    }
    let mut outcome = InstallOutcome::AlreadyInstalled;
    INSTALLED.get_or_init(|| {
        let writer = config.report_writer.clone().unwrap_or_else(|| {
            Arc::new(PrivateReportWriter::new(&config.crash_log_path)) as Arc<dyn ReportWriter>
        });
        // Serialize the complete hook without waiting. Concurrent panics are
        // dropped while a report/sink is active, without waiting for this gate.
        let gate = Mutex::new(());
        panic::set_hook(Box::new(move |info| {
            let Ok(_guard) = gate.try_lock() else {
                return;
            };
            let thread = std::thread::current();
            let context = PanicContext {
                app_name: config.app_name.clone(),
                version: config.version.clone(),
                platform: std::env::consts::OS,
                repository: config.repository.clone(),
                issue_template: config.issue_template.clone(),
                timestamp: (config.timestamp)(),
                thread_name: thread.name().unwrap_or("<unnamed>").to_string(),
                location: format_location(
                    info.location()
                        .map(|loc| (loc.file(), loc.line(), loc.column())),
                ),
                message: panic_payload_message(info.payload()),
            };
            let report = render_crash_report(
                &context,
                &std::backtrace::Backtrace::force_capture().to_string(),
            );
            let diagnostic = PublicDiagnostic {
                app_name: context.app_name,
                timestamp: context.timestamp,
                report_written: writer.write_report(&report).is_ok(),
            };
            match &config.output {
                PanicOutput::Human => {
                    let _ = writeln!(io::stderr(), "{}", human_notice(&diagnostic));
                }
                PanicOutput::Silent => {}
                PanicOutput::Sink(sink) => {
                    let _ = sink.emit(&diagnostic);
                }
            }
        }));
        outcome = InstallOutcome::Installed;
    });
    outcome
}

fn utc_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn format_location(location: Option<(&str, u32, u32)>) -> String {
    location.map_or_else(
        || "<unknown>".into(),
        |(file, line, column)| format!("{file}:{line}:{column}"),
    )
}
fn human_notice(diagnostic: &PublicDiagnostic) -> String {
    format!(
        "{} encountered an unexpected internal error. Private diagnostic report: {}. Timestamp: {}",
        diagnostic.app_name,
        if diagnostic.report_written {
            "written"
        } else {
            "unavailable"
        },
        diagnostic.timestamp
    )
}

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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_location_and_time() {
        assert_eq!(panic_payload_message(&42_u32), "<non-string panic payload>");
        assert_eq!(panic_payload_message(&"secret"), "secret");
        assert_eq!(panic_payload_message(&String::from("owned")), "owned");
        assert_eq!(format_location(None), "<unknown>");
        let timestamp = utc_timestamp();
        assert!(timestamp.ends_with('Z'));
        chrono::DateTime::parse_from_rfc3339(&timestamp).unwrap();
    }
}
