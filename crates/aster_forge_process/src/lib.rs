//! Bounded child-process ownership, cancellation, timeout and output capture.

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

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

/// Which captured pipe produced a byte chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStream {
    /// Child standard output.
    Stdout,
    /// Child standard error.
    Stderr,
}

/// One directly executed child process and its resource limits.
#[derive(Clone)]
pub struct ProcessSpec {
    /// Absolute executable path.
    pub program: PathBuf,
    /// Exact arguments; never interpreted by a shell.
    pub args: Vec<OsString>,
    /// Optional bytes written to standard input.
    pub stdin: Option<Vec<u8>>,
    /// Invocation deadline from process start.
    pub timeout: Duration,
    /// Maximum retained stdout bytes. Excess output is drained but not stored.
    pub stdout_limit: usize,
    /// Maximum retained stderr bytes. Excess output is drained but not stored.
    pub stderr_limit: usize,
    /// Remove inherited variables before applying `environment`.
    pub clear_environment: bool,
    /// Explicit environment additions, if any.
    pub environment: Vec<(OsString, OsString)>,
}

impl ProcessSpec {
    /// Construct a process spec with inherited environment and bounded captures.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            stdin: None,
            timeout,
            stdout_limit: 64 * 1024,
            stderr_limit: 64 * 1024,
            clear_environment: false,
            environment: Vec::new(),
        }
    }

    /// Set an exact environment value without involving a shell.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.environment
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }
}

/// A bounded capture of one child output pipe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    /// Retained bytes up to the configured bound.
    pub bytes: Vec<u8>,
    /// True when additional bytes were drained but not retained.
    pub truncated: bool,
    /// Read error, if the pipe failed before EOF.
    pub read_error: Option<String>,
}

/// Why a process did not produce an observed exit status.
#[derive(Debug)]
pub enum ProcessFailure {
    /// The process could not be started.
    Spawn(io::Error),
    /// Cancellation arrived before startup.
    CancelledBeforeStart,
    /// Cancellation arrived after startup; termination was requested for the owned process tree.
    Cancelled,
    /// The process exceeded its time budget; termination was requested for the owned process tree.
    TimedOut,
    /// Waiting for the immediate process failed.
    Wait(io::Error),
}

/// Result after the immediate child exits and owned process-tree cleanup completes.
#[derive(Debug)]
pub struct ProcessOutcome {
    /// Observed process exit status, or the reason no status was observed.
    pub status: Result<ExitStatus, ProcessFailure>,
    /// Whether process creation succeeded.
    pub started: bool,
    /// Captured standard output.
    pub stdout: CapturedOutput,
    /// Captured standard error.
    pub stderr: CapturedOutput,
    /// Failure to write/close the input pipe; input bytes are never included.
    pub stdin_error: Option<io::ErrorKind>,
    /// Error while waiting for the owned process group/job to finish cleanup.
    pub cleanup_error: Option<io::Error>,
}

struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    process_id: u32,
}

impl ProcessTree {
    fn start_kill(&mut self) -> io::Result<()> {
        self.child.start_kill()
    }

    async fn wait_direct(&mut self) -> io::Result<ExitStatus> {
        // SAFETY: this only observes the native child's status; the wrapper
        // remains responsible for group/job cleanup below.
        unsafe { self.child.try_inner_child_mut() }
            .ok_or_else(|| io::Error::other("process wrapper has no native child"))?
            .wait()
            .await
    }

    async fn wait_tree(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        wait_for_empty_process_group(self.process_id).await?;
        #[cfg(windows)]
        wait_for_empty_process_tree(self.process_id).await?;
        Ok(())
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Run a child with owned process-tree cleanup, bounded output and cancellation.
///
/// `on_chunk` receives retained bytes only, within the configured output
/// limits. Callers must not persist or display sensitive streams. It runs on the
/// Tokio task and must return promptly.
pub async fn run(
    spec: ProcessSpec,
    cancellation: &CancellationToken,
    on_chunk: impl Fn(ProcessStream, &[u8]) + Send + Sync,
) -> ProcessOutcome {
    if cancellation.is_cancelled() || spec.timeout.is_zero() {
        return ProcessOutcome {
            status: Err(if cancellation.is_cancelled() {
                ProcessFailure::CancelledBeforeStart
            } else {
                ProcessFailure::TimedOut
            }),
            started: false,
            stdout: CapturedOutput::default(),
            stderr: CapturedOutput::default(),
            stdin_error: None,
            cleanup_error: None,
        };
    }

    let child = match spawn(&spec) {
        Ok(child) => child,
        Err(error) => {
            return ProcessOutcome {
                status: Err(ProcessFailure::Spawn(error)),
                started: false,
                stdout: CapturedOutput::default(),
                stderr: CapturedOutput::default(),
                stdin_error: None,
                cleanup_error: None,
            };
        }
    };
    let process_id = child.id().unwrap_or_default();
    let mut child = ProcessTree { child, process_id };
    let stdout = child.child.stdout().take();
    let stderr = child.child.stderr().take();
    let stdin = child.child.stdin().take();
    let mut out = CapturedOutput::default();
    let mut err = CapturedOutput::default();
    let write_input = spec.stdin;
    let mut stdin_error = None;
    let completion = tokio::time::timeout(spec.timeout, async {
        let (status, (), (), ()) = tokio::join!(
            async { child.wait_direct().await },
            async {
                let result = write_stdin(stdin, write_input).await;
                stdin_error = result.as_ref().err().map(io::Error::kind);
            },
            capture(
                stdout,
                &mut out,
                spec.stdout_limit,
                ProcessStream::Stdout,
                &on_chunk,
            ),
            capture(
                stderr,
                &mut err,
                spec.stderr_limit,
                ProcessStream::Stderr,
                &on_chunk,
            )
        );
        status
    });
    let observed = tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(ProcessFailure::Cancelled),
        status = completion => match status {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(error)) => Err(ProcessFailure::Wait(error)),
            Err(_) => Err(ProcessFailure::TimedOut),
        },
    };

    let kill_error = child.start_kill().err().filter(|error| {
        #[cfg(unix)]
        {
            error.raw_os_error() != Some(libc::ESRCH)
        }
        #[cfg(not(unix))]
        {
            let _ = error;
            true
        }
    });
    let cleanup_error = child.wait_tree().await.err().or(kill_error);
    ProcessOutcome {
        status: observed,
        started: true,
        stdout: out,
        stderr: err,
        stdin_error,
        cleanup_error,
    }
}

fn spawn(spec: &ProcessSpec) -> io::Result<Box<dyn ChildWrapper>> {
    let mut wrapped = CommandWrap::with_new(&spec.program, |command| {
        command
            .args(&spec.args)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if spec.clear_environment {
            command.env_clear();
        }
        command.envs(spec.environment.iter().map(|(key, value)| (key, value)));
    });
    wrapped.wrap(KillOnDrop);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::tokio::JobObject);

    wrapped.spawn()
}

#[cfg(unix)]
async fn wait_for_empty_process_group(process_id: u32) -> io::Result<()> {
    loop {
        // The process group is created with the direct child's PID as its PGID.
        // A zero signal observes group membership without introducing another
        // kill race; the group is empty only once the kernel reports ESRCH.
        let group_id = i32::try_from(process_id).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "process ID exceeds Unix PID range",
            )
        })?;
        // SAFETY: signal 0 only probes the kernel-owned process group and sends no signal.
        let result = unsafe { libc::kill(-group_id, 0) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(windows)]
async fn wait_for_empty_process_tree(process_id: u32) -> io::Result<()> {
    loop {
        if !windows_process_tree_alive(process_id)? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(windows)]
fn windows_process_tree_alive(root: u32) -> io::Result<bool> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };

    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(io::Error::other)?;
    let mut entries = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let first = unsafe { Process32FirstW(snapshot, &mut entry) };
    if first.is_err() {
        unsafe { CloseHandle(snapshot) }.ok();
        return Err(io::Error::last_os_error());
    }
    loop {
        entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
        if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
            break;
        }
    }
    unsafe { CloseHandle(snapshot) }.ok();
    Ok(entries.iter().any(|(pid, _)| {
        let mut current = *pid;
        for _ in 0..entries.len() {
            if current == root {
                return true;
            }
            let Some((_, parent)) = entries.iter().find(|(candidate, _)| *candidate == current)
            else {
                return false;
            };
            if *parent == current {
                return false;
            }
            current = *parent;
        }
        false
    }))
}

async fn write_stdin(
    stdin: Option<impl tokio::io::AsyncWrite + Unpin>,
    input: Option<Vec<u8>>,
) -> io::Result<()> {
    let (Some(mut stdin), Some(input)) = (stdin, input) else {
        return Ok(());
    };
    stdin.write_all(&input).await?;
    stdin.shutdown().await
}

async fn capture(
    stream: Option<impl AsyncRead + Unpin>,
    saved: &mut CapturedOutput,
    limit: usize,
    kind: ProcessStream,
    on_chunk: &(impl Fn(ProcessStream, &[u8]) + Send + Sync),
) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buffer = [0_u8; 4096];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => break,
            Ok(size) => {
                let keep = size.min(limit.saturating_sub(saved.bytes.len()));
                if keep > 0 {
                    on_chunk(kind, &buffer[..keep]);
                }
                saved.bytes.extend_from_slice(&buffer[..keep]);
                saved.truncated |= keep < size;
            }
            Err(error) => {
                saved.read_error = Some(error.to_string());
                break;
            }
        }
    }
}

impl std::fmt::Debug for ProcessSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("program", &self.program)
            .field("stdin", &"<redacted>")
            .field("environment", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}
