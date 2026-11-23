//! Shared integration-test infrastructure for Aster services.
//!
//! This crate packages the container machinery that Aster products previously duplicated in
//! their `tests/common` modules: shared reusable containers serialized by filesystem locks,
//! per-process resource registries with stale-process pruning, and readiness wait helpers.
//!
//! Everything here is test support code. Failures panic with descriptive messages instead of
//! returning recoverable errors, because a broken test environment should fail fast and loud.

#[cfg(feature = "logs")]
pub mod logs;
pub mod temp;

#[cfg(feature = "containers")]
pub mod fixture;
#[cfg(feature = "containers")]
pub mod state;
#[cfg(feature = "containers")]
pub mod suite;
#[cfg(feature = "containers")]
pub mod wait;

#[cfg(any(
    any(feature = "mysql", feature = "mysql-native-tls"),
    any(feature = "postgres", feature = "postgres-native-tls")
))]
mod database;

#[cfg(feature = "process")]
pub mod process;

#[cfg(feature = "smtp")]
pub mod smtp;

#[cfg(any(feature = "mysql", feature = "mysql-native-tls"))]
pub mod mysql;
#[cfg(any(feature = "postgres", feature = "postgres-native-tls"))]
pub mod postgres;
#[cfg(any(feature = "redis", feature = "redis-native-tls"))]
pub mod redis;
