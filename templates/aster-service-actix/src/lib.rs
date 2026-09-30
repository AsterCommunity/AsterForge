//! Generated Aster service library crate.
//!
//! The library target keeps the service testable from integration tests. Product modules are
//! public so generated OpenAPI tests, route smoke tests, and future product crates can import the
//! same API/runtime assembly that the binary entrypoint uses.
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

pub mod api;
pub mod config;
pub mod db;
pub mod errors;
pub mod metrics;
pub mod runtime;
pub mod services;
pub mod tasks;
