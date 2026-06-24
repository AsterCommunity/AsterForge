//! Shared Actix Web middleware for Aster services.
//!
//! This crate contains HTTP middleware that is tied to Actix Web rather than to
//! a product domain. It keeps framework-specific code out of `aster_forge_api`,
//! which remains focused on framework-neutral response and pagination helpers.
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

pub mod csrf;
pub mod metrics;
pub mod request_id;
pub mod security_headers;
