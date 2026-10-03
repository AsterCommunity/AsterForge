//! Shared HTTP middleware for Aster services.
//!
//! This crate contains product-neutral middleware mechanics and Actix/Axum adapters.
//! It keeps framework-specific code out of `aster_forge_api`,
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

#[cfg(feature = "actix")]
pub mod actix;
#[cfg(feature = "axum")]
pub mod axum;

/// Framework-neutral middleware kernels shared by both transports.
pub mod shared;

#[cfg(all(test, feature = "metrics", any(feature = "actix", feature = "axum")))]
mod test_support;

#[cfg(all(test, feature = "actix", feature = "axum"))]
mod contract_tests;
