//! Termination signal helpers.
//!
//! This module only waits for process termination signals. Product crates remain
//! responsible for recording audit events, stopping background tasks, flushing
//! buffers, and closing database or network handles in their preferred order.

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

/// Waits until the process receives a termination signal.
pub async fn wait_for_termination_signal() -> Result<TerminationSignal, RuntimeSignalError> {
    let signal = wait_for_signal_impl().await?;
    tracing::info!(
        signal = signal.as_str(),
        "received termination signal, shutting down gracefully..."
    );
    Ok(signal)
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
    use super::TerminationSignal;

    #[test]
    fn termination_signal_reports_stable_labels() {
        assert_eq!(TerminationSignal::Interrupt.as_str(), "SIGINT");
        assert_eq!(TerminationSignal::Terminate.as_str(), "SIGTERM");
    }
}
