//! Shared runtime primitives for Aster services.
//!
//! This crate contains small process/runtime building blocks that are not tied
//! to a concrete product domain: health report aggregation and termination
//! signal handling. Product crates still own runtime state, audit events,
//! background task shutdown order, database handles, and service-specific
//! readiness checks.
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

pub mod health;
pub mod shutdown;

pub use health::{HealthComponentReport, HealthStatus, SystemHealthReport};
pub use shutdown::{RuntimeSignalError, TerminationSignal, wait_for_termination_signal};
