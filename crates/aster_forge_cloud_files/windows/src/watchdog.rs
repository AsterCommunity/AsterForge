//! Deterministic callback watchdog primitives.

use std::time::{Duration, Instant};

use crate::{Result, WindowsCloudFilesError};

/// The fixed callback timeout documented by Windows CFAPI.
pub const WINDOWS_CFAPI_CALLBACK_TIMEOUT: Duration = Duration::from_mins(1);

/// Host-controlled watchdog configuration for one pending fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsFetchDataWatchdogConfig {
    timeout: Duration,
}

impl WindowsFetchDataWatchdogConfig {
    /// Creates a positive timeout no longer than the platform callback contract.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(WindowsCloudFilesError::InvalidWatchdogTimeout {
                reason: "timeout must be positive",
            });
        }
        if timeout > WINDOWS_CFAPI_CALLBACK_TIMEOUT {
            return Err(WindowsCloudFilesError::InvalidWatchdogTimeout {
                reason: "timeout exceeds the fixed CFAPI callback timeout",
            });
        }
        Ok(Self { timeout })
    }

    /// Returns the configured timeout.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for WindowsFetchDataWatchdogConfig {
    fn default() -> Self {
        Self {
            timeout: WINDOWS_CFAPI_CALLBACK_TIMEOUT,
        }
    }
}

/// Deadline state for one pending fetch. The host supplies `Instant` values, making tests
/// deterministic without a Tokio timer or a background thread in the native callback path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsFetchDataWatchdog {
    config: WindowsFetchDataWatchdogConfig,
    last_activity: Instant,
}

impl WindowsFetchDataWatchdog {
    /// Starts a watchdog at `now`.
    #[must_use]
    pub const fn started(config: WindowsFetchDataWatchdogConfig, now: Instant) -> Self {
        Self {
            config,
            last_activity: now,
        }
    }

    /// Returns the next deadline, or `None` if the host clock cannot represent it.
    #[must_use]
    pub fn deadline(self) -> Option<Instant> {
        self.last_activity.checked_add(self.config.timeout)
    }

    /// Records valid provider progress or another host operation and resets the deadline.
    pub fn touch(&mut self, now: Instant) {
        self.last_activity = now;
    }

    /// Returns whether the watchdog is due at `now`.
    #[must_use]
    pub fn is_due(self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_rejects_zero_and_longer_than_platform_deadline() {
        assert!(WindowsFetchDataWatchdogConfig::new(Duration::ZERO).is_err());
        assert!(
            WindowsFetchDataWatchdogConfig::new(
                WINDOWS_CFAPI_CALLBACK_TIMEOUT + Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn watchdog_deadline_is_refreshed_by_activity() {
        let config = WindowsFetchDataWatchdogConfig::new(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        let mut watchdog = WindowsFetchDataWatchdog::started(config, start);
        assert!(!watchdog.is_due(start + Duration::from_secs(4)));
        watchdog.touch(start + Duration::from_secs(4));
        assert!(!watchdog.is_due(start + Duration::from_secs(8)));
        assert!(watchdog.is_due(start + Duration::from_secs(9)));
    }
}
