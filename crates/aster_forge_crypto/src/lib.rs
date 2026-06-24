//! Shared cryptographic helpers for Aster services.
//!
//! The crate currently exposes password hashing and digest utilities that were duplicated across
//! application code. It keeps the error surface narrow so services can map cryptographic failures
//! into their own API or domain errors without depending on implementation-specific error types.
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

pub mod hash;

pub use hash::{
    bytes_to_hex, hash_password, new_sha256, sha256_digest_to_hex, sha256_hex, verify_password,
};

/// Result type returned by `aster_forge_crypto` helpers.
pub type Result<T> = std::result::Result<T, CryptoError>;

/// Errors produced by cryptographic helper functions.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Password hashing, parsing, or verification failed.
    #[error("password hash error: {0}")]
    PasswordHash(String),
}

impl CryptoError {
    /// Creates a password-hash error from any displayable error value.
    pub fn password_hash(error: impl std::fmt::Display) -> Self {
        Self::PasswordHash(error.to_string())
    }
}
