//! Shared low-level utility helpers for Aster services.
//!
//! This crate holds small, dependency-light helpers that do not belong to a single domain module:
//! boolean-like string parsing, checked numeric conversions, path rendering helpers, loopback host
//! detection, UUID/token helpers, and RAII cleanup guards. The shared error type is intentionally
//! simple so callers can map it into richer product errors.
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

pub mod avatar;
pub mod backoff;
pub mod bool_like;
pub mod fs;
pub mod html;
pub mod http_validators;
pub mod id;
pub mod net;
pub mod numbers;
pub mod paths;
pub mod raii;
pub mod text;
pub mod url;

/// Result type returned by utility helpers.
pub type Result<T> = std::result::Result<T, UtilsError>;

/// Error type used by generic utility helpers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UtilsError {
    /// A value failed semantic validation.
    #[error("{0}")]
    InvalidValue(String),
    /// A numeric conversion would overflow, underflow, or lose sign information.
    #[error("{0}")]
    NumericConversion(String),
}

impl UtilsError {
    /// Creates an invalid-value error.
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue(message.into())
    }

    /// Creates a numeric-conversion error.
    pub fn numeric_conversion(message: impl Into<String>) -> Self {
        Self::NumericConversion(message.into())
    }
}
