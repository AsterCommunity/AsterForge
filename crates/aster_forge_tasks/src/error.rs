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
}
