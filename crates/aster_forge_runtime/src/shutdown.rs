//! Termination signal helpers.
//!
//! This module only waits for process termination signals. Product crates remain
//! responsible for recording audit events, stopping background tasks, flushing
//! buffers, and closing database or network handles in their preferred order.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

type ShutdownFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type ShutdownPhaseFn = dyn FnMut() -> ShutdownFuture + Send;

/// Termination signal observed by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationSignal {
    /// Unix SIGINT or cross-platform Ctrl+C.
    Interrupt,
    /// Unix SIGTERM.
    Terminate,
}

impl TerminationSignal {
    /// Returns a stable label for logging and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }
}

/// Errors returned while installing signal listeners.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSignalError {
    /// Failed to install or await a process signal handler.
    #[error("failed to install termination signal handler: {0}")]
    Install(String),
}

/// Final status for one shutdown phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownPhaseStatus {
    /// The phase completed successfully.
    Succeeded,
    /// The phase returned an error string.
    Failed(String),
    /// The phase exceeded its timeout.
    TimedOut,
}

impl ShutdownPhaseStatus {
    /// Returns whether this phase did not complete successfully.
    pub const fn is_failure(&self) -> bool {
        !matches!(self, Self::Succeeded)
    }
}

/// Report for one executed shutdown phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownPhaseReport {
    /// Stable phase name.
    pub name: &'static str,
    /// Phase result.
    pub status: ShutdownPhaseStatus,
    /// Execution duration.
    pub duration: Duration,
}

/// Aggregate report for a shutdown run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Phase reports in execution order.
    pub phases: Vec<ShutdownPhaseReport>,
}

impl ShutdownReport {
    /// Returns a report from phase entries.
    pub fn new(phases: Vec<ShutdownPhaseReport>) -> Self {
        Self { phases }
    }

    /// Returns whether any phase failed or timed out.
    pub fn has_failures(&self) -> bool {
        self.phases.iter().any(|phase| phase.status.is_failure())
    }
}

/// Logs the aggregate result of a shutdown run.
pub fn log_shutdown_report(report: &ShutdownReport) {
    if report.has_failures() {
        tracing::warn!("shutdown completed with one or more failed phases");
    } else {
        tracing::info!("shutdown complete");
    }
}

struct RegisteredShutdownPhase {
    name: &'static str,
    timeout: Option<Duration>,
    phase: Box<ShutdownPhaseFn>,
}

/// Ordered shutdown phase coordinator.
///
/// The coordinator owns phase ordering, timeout handling, duration collection,
/// and error aggregation. Product crates provide the actual phase closures.
/// Phases are `FnMut` so shutdown code can move owned handles into the
/// coordinator and consume them exactly once during the shutdown run.
#[derive(Default)]
pub struct ShutdownCoordinator {
    phases: Vec<RegisteredShutdownPhase>,
}

impl ShutdownCoordinator {
    /// Creates an empty shutdown coordinator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a shutdown phase.
    pub fn phase<F, Fut>(
        &mut self,
        name: &'static str,
        timeout: Option<Duration>,
        mut phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.phases.push(RegisteredShutdownPhase {
            name,
            timeout,
            phase: Box::new(move || Box::pin(phase())),
        });
        self
    }

    /// Runs phases sequentially and returns a report.
    ///
    /// Later phases still run when an earlier phase fails. This lets product
    /// shutdown code make best-effort progress through independent resources.
    pub async fn run(&mut self) -> ShutdownReport {
        let mut reports = Vec::with_capacity(self.phases.len());

        for phase in &mut self.phases {
            tracing::info!(phase = phase.name, "starting shutdown phase");
            let started_at = Instant::now();
            let future = (phase.phase)();
            let status = match phase.timeout {
                Some(timeout) => match tokio::time::timeout(timeout, future).await {
                    Ok(Ok(())) => ShutdownPhaseStatus::Succeeded,
                    Ok(Err(error)) => ShutdownPhaseStatus::Failed(error),
                    Err(_) => ShutdownPhaseStatus::TimedOut,
                },
                None => match future.await {
                    Ok(()) => ShutdownPhaseStatus::Succeeded,
                    Err(error) => ShutdownPhaseStatus::Failed(error),
                },
            };
            let duration = started_at.elapsed();
            match &status {
                ShutdownPhaseStatus::Succeeded => {
                    tracing::info!(phase = phase.name, ?duration, "shutdown phase completed");
                }
                ShutdownPhaseStatus::Failed(error) => {
                    tracing::error!(phase = phase.name, ?duration, %error, "shutdown phase failed");
                }
                ShutdownPhaseStatus::TimedOut => {
                    tracing::error!(phase = phase.name, ?duration, "shutdown phase timed out");
                }
            }
            reports.push(ShutdownPhaseReport {
                name: phase.name,
                status,
                duration,
            });
        }

        ShutdownReport::new(reports)
    }

    /// Returns how many phases are registered.
    pub fn len(&self) -> usize {
        self.phases.len()
    }

    /// Returns whether no phases are registered.
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }
}

/// Waits until the process receives a termination signal.
pub async fn wait_for_termination_signal() -> Result<TerminationSignal, RuntimeSignalError> {
    let signal = wait_for_signal_impl().await?;
    tracing::info!(
        signal = signal.as_str(),
        "received termination signal, shutting down gracefully..."
    );
    Ok(signal)
}

/// Spawns a task that waits for a termination signal, cancels `shutdown_token`,
/// and then runs `on_signal`.
///
/// This keeps product entrypoints from duplicating the same signal-listener
/// task while leaving the actual server stop primitive product-specific.
pub fn spawn_termination_signal_handler<F, Fut>(
    shutdown_token: CancellationToken,
    on_signal: F,
) -> JoinHandle<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(error) = wait_for_termination_signal().await {
            tracing::error!(%error, "shutdown signal listener failed");
        }
        shutdown_token.cancel();
        on_signal().await;
    })
}

#[cfg(unix)]
async fn wait_for_signal_impl() -> Result<TerminationSignal, RuntimeSignalError> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|error| RuntimeSignalError::Install(error.to_string()))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|error| RuntimeSignalError::Install(error.to_string()))?;

    tokio::select! {
        _ = sigint.recv() => Ok(TerminationSignal::Interrupt),
        _ = sigterm.recv() => Ok(TerminationSignal::Terminate),
    }
}

#[cfg(not(unix))]
async fn wait_for_signal_impl() -> Result<TerminationSignal, RuntimeSignalError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeSignalError::Install(error.to_string()))?;
    Ok(TerminationSignal::Interrupt)
}

#[cfg(test)]
mod tests {
    use super::{ShutdownCoordinator, ShutdownPhaseStatus, TerminationSignal};
    use std::time::Duration;

    #[test]
    fn termination_signal_reports_stable_labels() {
        assert_eq!(TerminationSignal::Interrupt.as_str(), "SIGINT");
        assert_eq!(TerminationSignal::Terminate.as_str(), "SIGTERM");
    }

    #[tokio::test]
    async fn shutdown_coordinator_runs_all_phases_in_order() {
        let mut coordinator = ShutdownCoordinator::new();
        coordinator
            .phase("tasks", None, || async { Ok(()) })
            .phase("audit", None, || async { Err("flush failed".to_string()) })
            .phase("db", None, || async { Ok(()) });

        let report = coordinator.run().await;

        assert_eq!(coordinator.len(), 3);
        assert!(report.has_failures());
        assert_eq!(report.phases[0].name, "tasks");
        assert_eq!(report.phases[0].status, ShutdownPhaseStatus::Succeeded);
        assert_eq!(
            report.phases[1].status,
            ShutdownPhaseStatus::Failed("flush failed".to_string())
        );
        assert_eq!(report.phases[2].status, ShutdownPhaseStatus::Succeeded);
    }

    #[test]
    fn shutdown_report_logger_accepts_success_and_failure_reports() {
        super::log_shutdown_report(&super::ShutdownReport::new(vec![
            super::ShutdownPhaseReport {
                name: "tasks",
                status: ShutdownPhaseStatus::Succeeded,
                duration: Duration::from_millis(1),
            },
        ]));

        super::log_shutdown_report(&super::ShutdownReport::new(vec![
            super::ShutdownPhaseReport {
                name: "database",
                status: ShutdownPhaseStatus::Failed("close failed".to_string()),
                duration: Duration::from_millis(1),
            },
        ]));
    }

    #[tokio::test]
    async fn shutdown_coordinator_reports_timeouts() {
        let mut coordinator = ShutdownCoordinator::new();
        coordinator.phase("slow", Some(Duration::from_millis(1)), || async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        });

        let report = coordinator.run().await;

        assert!(report.has_failures());
        assert_eq!(report.phases[0].status, ShutdownPhaseStatus::TimedOut);
    }

    #[tokio::test]
    async fn shutdown_coordinator_supports_consumed_phase_handles() {
        let mut coordinator = ShutdownCoordinator::new();
        let mut owned_handle = Some("resource");
        coordinator.phase("owned", None, move || {
            let handle = owned_handle.take();
            async move {
                if handle == Some("resource") {
                    Ok(())
                } else {
                    Err("resource already consumed".to_string())
                }
            }
        });

        let report = coordinator.run().await;

        assert!(!report.has_failures());
        assert_eq!(report.phases[0].status, ShutdownPhaseStatus::Succeeded);
    }
}
