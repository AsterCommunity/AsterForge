//! Real child-process fixtures for integration and end-to-end tests.

use std::fs::File;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::temp::TestTempDir;

/// Reserves an ephemeral loopback port and releases it for a child process to bind.
pub fn available_loopback_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("failed to reserve local test port")
        .local_addr()
        .expect("failed to resolve local test port")
        .port()
}

/// Child process with isolated working directory, captured logs, and kill-on-drop cleanup.
pub struct TestProcess {
    name: String,
    child: Option<Child>,
    runtime_dir: TestTempDir,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl TestProcess {
    /// Spawns `command` in a fresh temporary directory and captures stdout/stderr to files.
    ///
    /// Callers own product-specific arguments and environment variables. This helper owns only
    /// process lifecycle and diagnostics.
    pub fn spawn(name: &str, command: &mut Command) -> Self {
        assert_valid_process_name(name);
        let runtime_dir = TestTempDir::new(&format!("process-{name}"));
        let stdout_log = runtime_dir.join("stdout.log");
        let stderr_log = runtime_dir.join("stderr.log");
        let stdout = File::create(&stdout_log)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", stdout_log.display()));
        let stderr = File::create(&stderr_log)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", stderr_log.display()));

        let child = command
            .current_dir(runtime_dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("failed to spawn test process {name}: {error}"));

        Self {
            name: name.to_string(),
            child: Some(child),
            runtime_dir,
            stdout_log,
            stderr_log,
        }
    }

    /// Returns the fixture name used in diagnostics.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the isolated working directory.
    pub fn runtime_dir(&self) -> &Path {
        self.runtime_dir.path()
    }

    /// Kills the child process and waits for it to exit. Repeated calls are harmless.
    pub fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Sends SIGTERM and waits for the child to exit within `timeout`.
    #[cfg(unix)]
    pub fn terminate_gracefully(&mut self, timeout: Duration) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        let status = Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap_or_else(|error| panic!("failed to send SIGTERM to {}: {error}", self.name));
        assert!(
            status.success(),
            "failed to send SIGTERM to {}: {status}",
            self.name
        );

        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.child.take();
                    return true;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => return false,
                Err(error) => panic!("failed to wait for {} after SIGTERM: {error}", self.name),
            }
        }
    }

    /// Panics with captured logs when the child has exited unexpectedly.
    pub fn assert_running(&mut self) {
        let Some(child) = self.child.as_mut() else {
            panic!("test process {} has already stopped", self.name);
        };
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to query process {}: {error}", self.name))
        {
            panic!(
                "test process {} exited with {status}\n{}",
                self.name,
                self.diagnostics()
            );
        }
    }

    /// Returns the tail of stdout and stderr for failure reporting.
    pub fn diagnostics(&self) -> String {
        format!(
            "--- {} stdout ---\n{}\n--- {} stderr ---\n{}",
            self.name,
            read_log_tail(&self.stdout_log),
            self.name,
            read_log_tail(&self.stderr_log)
        )
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn assert_valid_process_name(name: &str) {
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "test process name must be non-empty ascii alphanumeric or '-': {name:?}"
    );
}

fn read_log_tail(path: &Path) -> String {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return format!("<failed to open log: {error}>"),
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        return format!("<failed to read log: {error}>");
    }
    let start = bytes.len().saturating_sub(16 * 1024);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{TestProcess, available_loopback_port};
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn available_port_is_nonzero() {
        assert_ne!(available_loopback_port(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn process_captures_logs_and_terminates_on_request() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo ready; echo warning >&2; sleep 30"]);
        let mut process = TestProcess::spawn("capture", &mut command);

        process.assert_running();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let diagnostics = process.diagnostics();
        assert!(diagnostics.contains("ready"));
        assert!(diagnostics.contains("warning"));

        process.terminate();
    }

    #[test]
    fn process_rejects_unsafe_fixture_names() {
        let result = std::panic::catch_unwind(|| {
            let mut command = Command::new("unused");
            TestProcess::spawn("../escape", &mut command)
        });
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn process_supports_graceful_termination() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap 'exit 0' TERM; while true; do sleep 1; done"]);
        let mut process = TestProcess::spawn("graceful", &mut command);

        assert!(process.terminate_gracefully(Duration::from_secs(2)));
    }
}
