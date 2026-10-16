//! Clock abstraction for deterministic expiry and concurrency tests.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{AuthError, Result};

/// Time source used by token issuance and validation.
pub trait Clock: Send + Sync {
    /// Returns the current Unix timestamp in seconds.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError`](crate::AuthError) if the system clock cannot be represented.
    fn now_unix_seconds(&self) -> Result<i64>;
}

/// Production clock backed by `SystemTime`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_seconds(&self) -> Result<i64> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::InvalidPolicy)?;
        i64::try_from(duration.as_secs()).map_err(|_| AuthError::InvalidPolicy)
    }
}

/// Fixed clock for deterministic tests and product integration tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock {
    seconds: i64,
}

impl FixedClock {
    /// Creates a clock at a Unix timestamp.
    #[must_use]
    pub const fn new(seconds: i64) -> Self {
        Self { seconds }
    }

    /// Returns the configured timestamp.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }
}

impl Clock for FixedClock {
    fn now_unix_seconds(&self) -> Result<i64> {
        Ok(self.seconds)
    }
}
