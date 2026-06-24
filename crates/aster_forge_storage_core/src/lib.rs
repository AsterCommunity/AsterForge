//! Shared storage utility helpers for Aster services.
//!
//! The crate contains storage-adjacent primitives that are independent of concrete drivers:
//! safe relative object-key handling and S3-compatible endpoint normalization. Driver traits,
//! connector wiring, and credential storage remain in application crates where policy decisions
//! belong.
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

pub mod object_key;
pub mod s3_config;

pub use object_key::{join_key_prefix, normalize_relative_key, strip_key_prefix};
pub use s3_config::{NormalizedS3Config, S3ConfigError, normalize_s3_endpoint_and_bucket};

/// Result type returned by storage core helpers.
pub type Result<T> = std::result::Result<T, StorageCoreError>;

/// Errors produced by storage core helpers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageCoreError {
    /// The object key is not a safe relative storage path.
    #[error("{0}")]
    InvalidObjectKey(String),
}
