//! Product-neutral backend error classifications and retry advice.

use std::time::Duration;

/// Result returned by product-owned cloud backend adapters.
pub type BackendResult<T> = std::result::Result<T, CloudBackendError>;

/// Stable backend failure categories consumed by cloud-files mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudBackendErrorKind {
    /// The requested item or parent does not exist.
    NotFound,
    /// Credentials are missing, expired, or otherwise require user authentication.
    AuthenticationRequired,
    /// The authenticated principal lacks permission.
    PermissionDenied,
    /// Current remote state conflicts with the requested operation.
    Conflict,
    /// An expected metadata/content revision no longer matches.
    PreconditionFailed,
    /// The caller supplied a range, cursor, key, or request shape rejected by the backend.
    InvalidRequest,
    /// The backend throttled the request.
    RateLimited,
    /// A transient transport or service outage prevented completion.
    TemporarilyUnavailable,
    /// The backend does not implement the requested optional operation.
    Unsupported,
    /// The adapter received a response that violated the Forge contract.
    InvalidResponse,
    /// An uncategorized backend or adapter failure occurred.
    Internal,
}

/// Product-neutral retry guidance attached to a backend error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAdvice {
    /// Retrying the same request without a state change is not advised.
    Never,
    /// The request may be retried using the coordinator's backoff policy.
    Retry,
    /// The backend supplied a minimum retry delay.
    RetryAfter(Duration),
}

/// Product-neutral failure returned by a product-owned backend adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cloud-files backend operation failed: {kind:?}")]
pub struct CloudBackendError {
    kind: CloudBackendErrorKind,
    retry: RetryAdvice,
}

impl CloudBackendError {
    /// Creates a non-retryable backend error.
    pub const fn new(kind: CloudBackendErrorKind) -> Self {
        Self {
            kind,
            retry: RetryAdvice::Never,
        }
    }

    /// Creates a retryable backend error using coordinator-controlled backoff.
    pub const fn retryable(kind: CloudBackendErrorKind) -> Self {
        Self {
            kind,
            retry: RetryAdvice::Retry,
        }
    }

    /// Creates a retryable backend error with a minimum delay.
    pub const fn retry_after(kind: CloudBackendErrorKind, delay: Duration) -> Self {
        Self {
            kind,
            retry: RetryAdvice::RetryAfter(delay),
        }
    }

    /// Returns the stable failure classification.
    pub const fn kind(&self) -> CloudBackendErrorKind {
        self.kind
    }

    /// Returns the backend's retry guidance.
    pub const fn retry_advice(&self) -> RetryAdvice {
        self.retry
    }

    /// Returns whether a coordinator may retry the request.
    pub const fn is_retryable(&self) -> bool {
        !matches!(self.retry, RetryAdvice::Never)
    }
}
