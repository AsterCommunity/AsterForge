use aster_forge_panic::{
    DiagnosticSink, InstallOutcome, PanicHookConfig, PanicOutput, PublicDiagnostic, ReportWriter,
    install_panic_hook,
};
use aster_forge_test::temp::TestTempDir;
use std::io::{self, Write};
use std::sync::{Arc, Barrier, Mutex};

struct JsonSink;
impl DiagnosticSink for JsonSink {
    fn emit(&self, event: &PublicDiagnostic) -> io::Result<()> {
        writeln!(
            io::stderr(),
            "{}",
            serde_json::json!({"code":"internal_error", "timestamp":event.timestamp, "report_written":event.report_written})
        )
    }
}
struct BrokenSink;
impl DiagnosticSink for BrokenSink {
    fn emit(&self, _: &PublicDiagnostic) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}
/// Test-only transport: isolates hook behavior from platform permission policy.
struct TestWriter(Mutex<std::fs::File>);
impl TestWriter {
    fn new(path: &std::path::Path) -> Self {
        Self(Mutex::new(std::fs::File::create(path).unwrap()))
    }
}
impl ReportWriter for TestWriter {
    fn write_report(&self, report: &str) -> io::Result<()> {
        self.0
            .try_lock()
            .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?
            .write_all(report.as_bytes())
    }
}
struct GateWriter {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
    writer: TestWriter,
}
impl ReportWriter for GateWriter {
    fn write_report(&self, report: &str) -> io::Result<()> {
        // The guard must reject installation during unwinding without panicking.
        if install_panic_hook(PanicHookConfig::new("ignored", "", ""))
            != InstallOutcome::PanickingThread
        {
            return Err(io::Error::other("unexpected installation outcome"));
        }
        self.entered.wait();
        self.release.wait();
        self.writer.write_report(report)
    }
}
struct FailedWriter;
impl ReportWriter for FailedWriter {
    fn write_report(&self, _: &str) -> io::Result<()> {
        Err(io::Error::other("secret-writer-error"))
    }
}
fn fixed_time() -> String {
    "2030-01-02T03:04:05.000Z".into()
}

#[test]
fn child() {
    let Ok(mode) = std::env::var("FORGE_PANIC_TEST_MODE") else {
        return;
    };
    let path = std::env::var_os("FORGE_PANIC_TEST_PATH").unwrap();
    // The previous hook must not run and expose developer payloads.
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(io::stderr(), "OLD-HOOK-SECRET");
    }));
    let output = match mode.as_str() {
        "human" => PanicOutput::Human,
        "silent" | "concurrent" | "busy" | "host" => PanicOutput::Silent,
        "broken" => PanicOutput::Sink(Arc::new(BrokenSink)),
        _ => PanicOutput::Sink(Arc::new(JsonSink)),
    };
    let mut config = PanicHookConfig::new("HookTest", "1", "https://example.test")
        .with_crash_log_path(path)
        .with_output(output)
        .with_timestamp(fixed_time);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    if !matches!(mode.as_str(), "failed" | "unavailable" | "default") {
        config.report_writer = Some(Arc::new(TestWriter::new(&config.crash_log_path)));
    }
    if mode == "busy" {
        let writer = TestWriter::new(&config.crash_log_path);
        config = config.with_report_writer(Arc::new(GateWriter {
            entered: entered.clone(),
            release: release.clone(),
            writer,
        }));
    }
    if mode == "failed" {
        config = config.with_report_writer(Arc::new(FailedWriter));
    }
    assert_eq!(install_panic_hook(config), InstallOutcome::Installed);
    if mode == "host" {
        std::panic::set_hook(Box::new(|_| {
            let _ = writeln!(io::stderr(), "host-hook");
        }));
    }
    assert_eq!(
        install_panic_hook(PanicHookConfig::new("Wrong", "2", "").with_output(PanicOutput::Human)),
        InstallOutcome::AlreadyInstalled
    );
    if mode == "busy" {
        // Exit instead of hanging the test runner if try_lock regresses to lock.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(10));
            std::process::exit(9);
        });
        let first = std::thread::spawn(|| panic!("secret-first"));
        entered.wait();
        assert!(
            std::thread::spawn(|| panic!("secret-skipped"))
                .join()
                .is_err()
        );
        release.wait();
        assert!(first.join().is_err());
    } else if mode == "concurrent" {
        let barrier = Arc::new(Barrier::new(9));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    panic!("secret-concurrent");
                })
            })
            .collect();
        barrier.wait();
        for thread in threads {
            assert!(thread.join().is_err());
        }
    } else {
        assert!(
            std::thread::spawn(|| panic!("secret-signature-and-credentials"))
                .join()
                .is_err()
        );
        assert!(
            std::thread::spawn(|| std::panic::panic_any(42_u32))
                .join()
                .is_err()
        );
    }
}

#[test]
fn isolated_output_policies() {
    for mode in [
        "human",
        "json",
        "silent",
        "failed",
        "unavailable",
        "default",
        "broken",
        "concurrent",
        "busy",
        "host",
    ] {
        let dir = TestTempDir::new("panic-hook-policies");
        // macOS temp path can contain OS symlinks. Resolve the trusted temp root
        // before passing a no-follow managed path to the default writer.
        let path = dir.path().canonicalize().unwrap().join("crash.log");
        if mode == "unavailable" {
            std::fs::create_dir(&path).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child", "--nocapture"])
            .env("FORGE_PANIC_TEST_MODE", mode)
            .env("FORGE_PANIC_TEST_PATH", &path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        for secret in ["secret-", "Backtrace", "OLD-HOOK", "<non-string", "Wrong"] {
            assert!(!stderr.contains(secret), "{mode}: {stderr}");
        }
        match mode {
            "human" => assert!(stderr.contains("HookTest encountered")),
            "json" | "failed" | "unavailable" | "default" => {
                let events: Vec<serde_json::Value> = stderr
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(events.len(), 2);
                assert_eq!(
                    events[0]["report_written"],
                    mode == "json" || (mode == "default" && cfg!(target_os = "linux"))
                );
                assert_eq!(events[0]["timestamp"], "2030-01-02T03:04:05.000Z");
            }
            "host" => assert_eq!(stderr.replace("\r\n", "\n"), "host-hook\nhost-hook\n"),
            _ => assert!(stderr.is_empty(), "{mode}: {stderr}"),
        }
        if mode == "failed" || (mode == "default" && !cfg!(target_os = "linux")) {
            assert!(!path.exists());
        } else if mode == "unavailable" {
            assert!(path.is_dir());
        } else if mode == "host" {
            assert!(std::fs::read_to_string(path).unwrap().is_empty());
        } else {
            let report = std::fs::read_to_string(path).unwrap();
            assert!(report.contains("Backtrace:"));
            assert!(report.contains("Timestamp: 2030-01-02T03:04:05.000Z"));
            assert!(report.contains("Thread:    <unnamed>"));
            assert!(report.contains("issues/new?template=bug_report.yml"));
            assert!(
                report
                    .replace('\\', "/")
                    .contains("Location:  crates/aster_forge_panic/tests/hook.rs:")
            );
            assert!(report.contains("secret-"));
            if mode == "busy" {
                assert!(!report.contains("secret-skipped"));
                assert_eq!(report.matches("Panic Report ===").count(), 1);
            } else if mode != "concurrent" {
                assert!(report.contains("<non-string panic payload>"));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn closed_and_broken_stderr_do_not_abort() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    for broken_pipe in [false, true] {
        let dir = TestTempDir::new("panic-stderr");
        let path = dir.path().canonicalize().unwrap().join("crash.log");
        let executable = std::env::current_exe().unwrap();
        let mut command = if broken_pipe {
            let mut command = Command::new(&executable);
            let (reader, writer) = UnixStream::pair().unwrap();
            drop(reader);
            command.stderr(Stdio::from(OwnedFd::from(writer)));
            command
        } else {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", "exec \"$@\" 2>&-", "panic-child"])
                .arg(&executable);
            command
        };
        let status = command
            .args(["--exact", "child", "--nocapture"])
            .env("FORGE_PANIC_TEST_MODE", "human")
            .env("FORGE_PANIC_TEST_PATH", &path)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("secret-signature")
        );
    }
}
