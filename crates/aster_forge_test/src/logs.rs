//! Per-call text-log capture for redaction and record-boundary regression tests.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Captures tracing text records without installing a global subscriber.
#[derive(Default)]
pub struct CapturedLogs {
    capture: Capture,
}

impl CapturedLogs {
    fn dispatch(&self) -> tracing::Dispatch {
        let capture = self.capture.clone();
        tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || capture.clone())
                .finish(),
        )
    }

    /// Runs a synchronous action under this capture's subscriber.
    pub fn capture<T>(&self, action: impl FnOnce() -> T) -> T {
        tracing::dispatcher::with_default(&self.dispatch(), action)
    }

    /// Captures every poll of an async action, including after executor thread changes.
    pub async fn capture_future<F: std::future::Future>(&self, future: F) -> F::Output {
        future.with_subscriber(self.dispatch()).await
    }

    /// Returns the captured UTF-8 text, recovering any poisoned test lock.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .capture
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}
