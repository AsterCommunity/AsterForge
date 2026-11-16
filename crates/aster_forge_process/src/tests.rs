//! In-memory lifecycle and pipe tests shared by native Rust and Miri runs.
//!
//! These fixtures avoid spawning, timers, and platform FFI. Real process-tree
//! ownership remains covered by the native integration test binary.

use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncWrite, ReadBuf};

/// Poll an immediately ready in-memory operation without an OS-backed runtime.
pub(crate) fn ready<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory fixture unexpectedly blocked"),
    }
}

/// Supply bounded chunks, then either EOF or a deterministic pipe error.
struct Reader {
    bytes: Vec<u8>,
    position: usize,
    chunk_size: usize,
    error: Option<io::ErrorKind>,
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.position == self.bytes.len() {
            return Poll::Ready(match self.error {
                Some(kind) => Err(kind.into()),
                None => Ok(()),
            });
        }
        let size = self
            .chunk_size
            .min(buffer.remaining())
            .min(self.bytes.len() - self.position);
        buffer.put_slice(&self.bytes[self.position..self.position + size]);
        self.position += size;
        Poll::Ready(Ok(()))
    }
}

#[test]
fn capture_drains_past_limits_and_only_callbacks_retained_bytes() {
    // Cross both the 4096-byte internal buffer boundary and short reads.
    let input: Vec<u8> = (0..=255).cycle().take(8193).collect();
    for chunk_size in [7, 4096] {
        for limit in [0, 1, 4095, 4096, 4097, 8193, 8194] {
            let mut reader = Reader {
                bytes: input.clone(),
                position: 0,
                chunk_size,
                error: None,
            };
            let mut saved = CapturedOutput::default();
            let chunks = Mutex::new(Vec::new());
            ready(capture(
                Some(&mut reader),
                &mut saved,
                limit,
                ProcessStream::Stderr,
                &|stream, bytes| {
                    assert_eq!(stream, ProcessStream::Stderr);
                    assert!(!bytes.is_empty());
                    chunks.lock().unwrap().extend_from_slice(bytes);
                },
            ));
            let retained = &input[..limit.min(input.len())];
            assert_eq!(saved.bytes, retained);
            assert_eq!(*chunks.lock().unwrap(), retained);
            assert_eq!(saved.truncated, input.len() > limit);
            assert!(saved.read_error.is_none());
            assert_eq!(reader.position, input.len(), "excess output must drain");
        }
    }
}

#[test]
fn capture_preserves_partial_output_and_reports_read_failure() {
    for limit in [0, 2, 8] {
        let reader = Reader {
            bytes: b"partial".to_vec(),
            position: 0,
            chunk_size: 3,
            error: Some(io::ErrorKind::ConnectionReset),
        };
        let mut saved = CapturedOutput::default();
        ready(capture(
            Some(reader),
            &mut saved,
            limit,
            ProcessStream::Stdout,
            &|_, _| {},
        ));
        assert_eq!(saved.bytes, &b"partial"[..limit.min(7)]);
        assert_eq!(saved.truncated, limit < 7);
        assert_eq!(
            saved.read_error,
            Some(io::Error::from(io::ErrorKind::ConnectionReset).to_string())
        );
    }
}

#[test]
fn missing_or_empty_output_does_not_callback_or_report_truncation() {
    for stream in [None, Some(&b""[..])] {
        let mut saved = CapturedOutput::default();
        ready(capture(
            stream,
            &mut saved,
            0,
            ProcessStream::Stdout,
            &|_, _| panic!("empty output must not callback"),
        ));
        assert_eq!(saved, CapturedOutput::default());
    }
}

/// Accept short writes or fail deterministically in writing/shutdown.
#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    max_write: usize,
    write_error: Option<io::ErrorKind>,
    shutdown_error: Option<io::ErrorKind>,
    shutdown: bool,
}

impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(kind) = self.write_error {
            return Poll::Ready(Err(kind.into()));
        }
        let size = bytes.len().min(self.max_write);
        self.bytes.extend_from_slice(&bytes[..size]);
        Poll::Ready(Ok(size))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shutdown = true;
        Poll::Ready(match self.shutdown_error {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        })
    }
}

#[test]
fn stdin_retries_short_writes_and_shuts_down_after_empty_or_full_input() {
    for input in [Vec::new(), vec![0, 255, 128, 1, 7]] {
        let mut writer = Writer {
            max_write: 2,
            ..Writer::default()
        };
        ready(write_stdin(Some(&mut writer), Some(input.clone()))).unwrap();
        assert_eq!(writer.bytes, input);
        assert!(writer.shutdown);
    }
}

#[test]
fn stdin_reports_write_zero_broken_pipe_and_shutdown_failures() {
    for kind in [io::ErrorKind::WriteZero, io::ErrorKind::BrokenPipe] {
        let mut writer = Writer {
            write_error: (kind == io::ErrorKind::BrokenPipe).then_some(kind),
            ..Writer::default()
        };
        let error = ready(write_stdin(Some(&mut writer), Some(vec![1]))).unwrap_err();
        assert_eq!(error.kind(), kind);
        assert!(!writer.shutdown);
    }
    let mut writer = Writer {
        max_write: 2,
        shutdown_error: Some(io::ErrorKind::BrokenPipe),
        ..Writer::default()
    };
    let error = ready(write_stdin(Some(&mut writer), Some(vec![1, 2, 3]))).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(writer.bytes, [1, 2, 3]);
    assert!(writer.shutdown);
}

#[test]
fn missing_input_or_pipe_is_a_noop() {
    let mut writer = Writer::default();
    ready(write_stdin(Some(&mut writer), None)).unwrap();
    assert!(writer.bytes.is_empty());
    assert!(!writer.shutdown);
    ready(write_stdin(None::<Writer>, Some(vec![1]))).unwrap();
}

#[test]
fn preflight_cancellation_precedes_zero_deadline_without_spawning() {
    for (cancelled, timeout) in [
        (true, Duration::ZERO),
        (true, Duration::from_secs(1)),
        (false, Duration::ZERO),
    ] {
        let cancellation = CancellationToken::new();
        if cancelled {
            cancellation.cancel();
        }
        let outcome = ready(run(
            ProcessSpec::new("must-not-spawn", timeout),
            &cancellation,
            |_, _| panic!("preflight must not emit output"),
        ));
        if cancelled {
            assert!(matches!(
                outcome.status,
                Err(ProcessFailure::CancelledBeforeStart)
            ));
        } else {
            assert!(matches!(outcome.status, Err(ProcessFailure::TimedOut)));
        }
        assert!(!outcome.started);
        assert_eq!(outcome.stdout, CapturedOutput::default());
        assert_eq!(outcome.stderr, CapturedOutput::default());
        assert!(outcome.stdin_error.is_none());
        assert!(outcome.cleanup_error.is_none());
    }
}
