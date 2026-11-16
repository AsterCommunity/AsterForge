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

/// Result after the immediate child exits and cleanup is attempted for owned descendants.
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
}

struct ProcessTree(Box<dyn ChildWrapper>);

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
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
            };
        }
    };
    let mut child = ProcessTree(child);
    let stdout = child.0.stdout().take();
    let stderr = child.0.stderr().take();
    let stdin = child.0.stdin().take();
    let mut out = CapturedOutput::default();
    let mut err = CapturedOutput::default();
    let write_input = spec.stdin;
    let mut stdin_error = None;
    let completion = tokio::time::timeout(spec.timeout, async {
        let (status, input_result, (), ()) = tokio::join!(
            async {
                let status = child.0.inner_mut().wait().await;
                let _ = child.0.start_kill();
                status
            },
            write_stdin(stdin, write_input),
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
        stdin_error = input_result.err().map(|error| error.kind());
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

    let _ = child.0.start_kill();
    if observed.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(2), child.0.wait()).await;
    }
    ProcessOutcome {
        status: observed,
        started: true,
        stdout: out,
        stderr: err,
        stdin_error,
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
