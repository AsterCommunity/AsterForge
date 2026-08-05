//! Shared cryptographic helpers for Aster services.
//!
//! The crate exposes password hashing, digest utilities, and a versioned authenticated secret
//! envelope shared by Aster products. It keeps the error surface narrow so services can map
//! cryptographic failures into their own API or domain errors without depending on
//! implementation-specific error types.
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
pub mod secret_envelope;

pub use hash::{
    PasswordHashPolicy, PasswordHashVerification, PasswordHashVerificationLimits,
    PasswordHashWorkFactor, bytes_to_hex, hash_password, hash_password_with_policy,
    hmac_sha256_hex, new_sha256, sha256_digest_to_hex, sha256_hex, verify_password,
    verify_password_with_policy,
};
pub use secret_envelope::{decrypt_secret, encrypt_secret};

/// Result type returned by `aster_forge_crypto` helpers.
pub type Result<T> = std::result::Result<T, CryptoError>;

/// Errors produced by cryptographic helper functions.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Password hashing, parsing, or verification failed.
    #[error("password hash error: {0}")]
    PasswordHash(String),

    /// A password-hash policy is internally inconsistent or unsupported.
    #[error("password hash policy error: {0}")]
    PasswordHashPolicy(String),

    /// A stored password hash exceeds the configured verification resource budget.
    #[error("password hash parameter {parameter}={actual} exceeds verification limit {maximum}")]
    PasswordHashVerificationLimit {
        /// PHC parameter or derived value that exceeded the limit.
        parameter: &'static str,
        /// Value read from the stored password hash.
        actual: u64,
        /// Maximum value accepted by the verification policy.
        maximum: u64,
    },

    /// Keyed message authentication initialization failed.
    #[error("message authentication error: {0}")]
    MessageAuthentication(String),

    /// The caller supplied an invalid product-owned secret-envelope context or policy value.
    #[error("invalid secret envelope policy")]
    InvalidSecretEnvelopePolicy,

    /// The stored secret envelope is malformed.
    #[error("invalid secret envelope")]
    InvalidSecretEnvelope,

    /// The stored secret envelope uses an unsupported version.
    #[error("unsupported secret envelope version")]
    UnsupportedSecretEnvelopeVersion,

    /// The encryption key could not be derived for the supplied context.
    #[error("secret envelope key derivation failed")]
    SecretEnvelopeKeyDerivation,

    /// Authenticated encryption failed.
    #[error("secret envelope encryption failed")]
    SecretEnvelopeEncryption,

    /// Authentication or decryption failed.
    #[error("secret envelope authentication failed")]
    SecretEnvelopeAuthentication,
}

impl CryptoError {
    /// Creates a password-hash error from any displayable error value.
    pub fn password_hash(error: impl std::fmt::Display) -> Self {
        Self::PasswordHash(error.to_string())
    }

    /// Creates a password-hash policy error.
    pub fn password_hash_policy(error: impl std::fmt::Display) -> Self {
        Self::PasswordHashPolicy(error.to_string())
    }

    /// Creates a keyed message-authentication error.
    pub fn message_authentication(error: impl std::fmt::Display) -> Self {
        Self::MessageAuthentication(error.to_string())
    }
}
