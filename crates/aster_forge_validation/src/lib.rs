//! Shared validation helpers for Aster services.
//!
//! The crate collects validation routines that were previously repeated in service code, starting
//! with email and filename handling. It keeps validation errors as plain messages so API layers and
//! domain services can decide how to present or translate them.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

/// Email validation and normalization helpers.
pub mod email;
/// File and folder name validation helpers.
pub mod filename;

/// Result type returned by validation helpers.
pub type Result<T> = std::result::Result<T, ValidationError>;

/// Error returned when validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ValidationError {
    message: String,
}

impl ValidationError {
    /// Creates a validation error with a user-facing message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the validation failure message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::ValidationError;

    #[test]
    fn validation_error_preserves_and_displays_message() {
        let error = ValidationError::new("invalid value");

        assert_eq!(error.message(), "invalid value");
        assert_eq!(error.to_string(), "invalid value");
    }
}
