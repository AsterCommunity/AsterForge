//! Error type shared by the configuration core.
//!
//! Product services map these errors into their own public API error codes at
//! the boundary. The core deliberately keeps variants small and transport
//! neutral so it can be reused by HTTP services, CLI tools, workers, and tests.

/// Result type returned by configuration-core operations.
pub type Result<T> = std::result::Result<T, ConfigCoreError>;

/// Error returned by configuration-core helpers.
#[derive(Debug, thiserror::Error)]
pub enum ConfigCoreError {
    /// A value failed semantic validation.
    #[error("{0}")]
    InvalidValue(String),
    /// A requested key was not present in the registry.
    #[error("unknown config key '{0}'")]
    UnknownKey(String),
    /// A storage backend operation failed.
    #[error("config store operation failed: {0}")]
    Store(String),
    /// A notification backend operation failed.
    #[error("config notification failed: {0}")]
    Notification(String),
    /// JSON serialization or deserialization failed.
    #[error("config JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl ConfigCoreError {
    /// Creates an invalid-value error.
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue(message.into())
    }

    /// Creates a storage-backend error.
    pub fn store(message: impl Into<String>) -> Self {
        Self::Store(message.into())
    }

    /// Creates a notification-backend error.
    pub fn notification(message: impl Into<String>) -> Self {
        Self::Notification(message.into())
    }
}
