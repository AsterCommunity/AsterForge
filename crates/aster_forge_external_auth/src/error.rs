//! Error categories used by shared external authentication drivers.
//!
//! The variants describe where a failure belongs without prescribing an application's HTTP status
//! code or response body. Application crates can preserve the category when converting this error
//! into their own domain error type, which keeps call sites from repeating ad hoc string matching.

/// Result type returned by external authentication helpers.
pub type Result<T> = std::result::Result<T, ExternalAuthError>;

/// Error returned by shared external authentication providers.
#[derive(Debug, thiserror::Error)]
pub enum ExternalAuthError {
    /// Stored or submitted provider configuration is invalid.
    #[error("{0}")]
    Validation(String),
    /// Runtime configuration is missing or unsupported.
    #[error("{0}")]
    Config(String),
    /// The provider rejected credentials, returned an invalid token, or exposed invalid profile
    /// data during an authentication flow.
    #[error("{0}")]
    InvalidCredentials(String),
    /// Stored login-flow state is incomplete or corrupted.
    #[error("{0}")]
    State(String),
    /// Internal client setup or infrastructure failed.
    #[error("{0}")]
    Internal(String),
}

impl ExternalAuthError {
    /// Creates a validation error.
    pub fn validation_error(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    /// Creates a configuration error.
    pub fn config_error(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    /// Creates an invalid-credentials error.
    pub fn auth_invalid_credentials(message: impl Into<String>) -> Self {
        Self::InvalidCredentials(message.into())
    }

    /// Creates a stored-state error.
    pub fn state_error(message: impl Into<String>) -> Self {
        Self::State(message.into())
    }

    /// Compatibility constructor for code moved from product crates where missing login-flow state
    /// was reported through the database category.
    pub fn database_operation(message: impl Into<String>) -> Self {
        Self::State(message.into())
    }

    /// Creates an internal error.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<aster_forge_validation::ValidationError> for ExternalAuthError {
    fn from(error: aster_forge_validation::ValidationError) -> Self {
        Self::validation_error(error.to_string())
    }
}

/// Extension trait for mapping lower-level errors into external auth errors with context.
pub trait MapExternalAuthErr<T> {
    /// Maps an error into [`ExternalAuthError`] by passing a context string to `error_fn`.
    fn map_external_auth_err_ctx(
        self,
        context: impl Into<String>,
        error_fn: fn(String) -> ExternalAuthError,
    ) -> Result<T>;
}

impl<T, E> MapExternalAuthErr<T> for std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    fn map_external_auth_err_ctx(
        self,
        context: impl Into<String>,
        error_fn: fn(String) -> ExternalAuthError,
    ) -> Result<T> {
        let context = context.into();
        self.map_err(|error| error_fn(format!("{context}: {error}")))
    }
}
