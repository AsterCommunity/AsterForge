//! Error type used by product-neutral task helpers.

/// Result type returned by shared task helpers.
pub type Result<T> = std::result::Result<T, TaskCoreError>;

/// Error returned by shared task helpers before product-level mapping.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskCoreError {
    /// A stored task value could not be decoded or serialized.
    #[error("{0}")]
    Codec(String),
    /// A task step or registry lookup failed semantic validation.
    #[error("{0}")]
    InvalidValue(String),
    /// Runtime filesystem work failed.
    #[error("{0}")]
    Io(String),
    /// The current worker lost its processing lease to another worker.
    #[error("background task lease lost for task #{task_id} with token {processing_token}")]
    LeaseLost {
        /// Persisted task identifier.
        task_id: i64,
        /// Processing token owned by the current worker.
        processing_token: i64,
    },
    /// The current worker did not renew its processing lease before the safety deadline.
    #[error(
        "background task lease renewal timed out for task #{task_id} with token {processing_token}"
    )]
    LeaseRenewalTimedOut {
        /// Persisted task identifier.
        task_id: i64,
        /// Processing token owned by the current worker.
        processing_token: i64,
    },
    /// The current worker observed shutdown and should release its processing lease.
    #[error(
        "background task worker shutdown requested for task #{task_id} with token {processing_token}"
    )]
    WorkerShutdownRequested {
        /// Persisted task identifier.
        task_id: i64,
        /// Processing token owned by the current worker.
        processing_token: i64,
    },
}

impl TaskCoreError {
    /// Creates a codec error.
    pub fn codec(message: impl Into<String>) -> Self {
        Self::Codec(message.into())
    }

    /// Creates an invalid-value error.
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue(message.into())
    }

    /// Creates an I/O error.
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }

    /// Returns whether this error means the current worker lost its lease.
    pub const fn is_task_lease_lost(&self) -> bool {
        matches!(self, Self::LeaseLost { .. })
    }

    /// Returns whether this error means the current worker's lease renewal timed out.
    pub const fn is_task_lease_renewal_timed_out(&self) -> bool {
        matches!(self, Self::LeaseRenewalTimedOut { .. })
    }

    /// Returns whether this error means the current worker should stop for shutdown.
    pub const fn is_task_worker_shutdown_requested(&self) -> bool {
        matches!(self, Self::WorkerShutdownRequested { .. })
    }
}
