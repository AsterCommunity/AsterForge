//! Framework-neutral API helper errors.
/// Result type returned by API helper functions.
pub type Result<T> = std::result::Result<T, ApiError>;

/// Error type for generic API helper failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    message: String,
}

impl ApiError {
    /// Creates an API helper error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the stored error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}
