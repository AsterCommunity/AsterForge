//! Service lifecycle runner.
//!
//! This module contains the shared entrypoint mechanics that are common across
//! Actix-based Aster services without depending on Actix itself. Product crates
//! still build their HTTP server, own application state, spawn background
//! workers, and decide the business shutdown order.

use std::future::Future;

use tokio_util::sync::CancellationToken;

use crate::spawn_termination_signal_handler;

/// Runs one service future with shared Aster shutdown mechanics.
///
/// The lifecycle runner deliberately does not know about Actix, `AppState`,
/// database handles, audit events, or background task types. It only owns the
/// repeated process-level pattern: install a termination-signal task, cancel the
/// shared shutdown token, invoke the product-provided stop callback, wait for
/// the service future, then run product-provided after-stop cleanup.
pub struct ServiceLifecycle<S> {
    server: S,
    shutdown_token: CancellationToken,
}

impl<S> ServiceLifecycle<S> {
    /// Creates a lifecycle runner for `server` and `shutdown_token`.
    pub fn new(server: S, shutdown_token: CancellationToken) -> Self {
        Self {
            server,
            shutdown_token,
        }
    }
}

impl<S> ServiceLifecycle<S>
where
    S: Future,
{
    /// Runs the service future and product cleanup hooks.
    pub async fn run<Stop, StopFut, AfterStop, AfterStopFut>(
        self,
        stop_on_signal: Stop,
        after_stop: AfterStop,
    ) -> S::Output
    where
        Stop: FnOnce() -> StopFut + Send + 'static,
        StopFut: Future<Output = ()> + Send + 'static,
        AfterStop: FnOnce() -> AfterStopFut,
        AfterStopFut: Future<Output = ()>,
    {
        let _signal_task = spawn_termination_signal_handler(self.shutdown_token, stop_on_signal);

        let server_result = self.server.await;
        tracing::info!("server stopped");
        after_stop().await;
        server_result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::ServiceLifecycle;

    #[tokio::test]
    async fn lifecycle_runs_after_stop_and_returns_server_result() {
        let after_stop_ran = Arc::new(AtomicBool::new(false));
        let observed_after_stop = Arc::clone(&after_stop_ran);

        let result = ServiceLifecycle::new(async { Ok::<_, &'static str>(42) }, Default::default())
            .run(
                || async {},
                move || {
                    let observed_after_stop = Arc::clone(&observed_after_stop);
                    async move {
                        observed_after_stop.store(true, Ordering::SeqCst);
                    }
                },
            )
            .await;

        assert_eq!(result, Ok(42));
        assert!(after_stop_ran.load(Ordering::SeqCst));
    }
}
