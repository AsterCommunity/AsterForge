//! Shared integration-test infrastructure for Aster services.
//!
//! This crate packages the container machinery that Aster products previously duplicated in
//! their `tests/common` modules: shared reusable containers serialized by filesystem locks,
//! per-process resource registries with stale-process pruning, and readiness wait helpers.
//!
//! Everything here is test support code. Failures panic with descriptive messages instead of
//! returning recoverable errors, because a broken test environment should fail fast and loud.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

#[cfg(feature = "containers")]
pub mod state;
#[cfg(feature = "containers")]
pub mod suite;
#[cfg(feature = "containers")]
pub mod wait;

#[cfg(feature = "process")]
pub mod process;

#[cfg(feature = "smtp")]
pub mod smtp;

#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "redis")]
pub mod redis;
