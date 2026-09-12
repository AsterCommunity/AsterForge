//! HTTP observability endpoints for Aster services.
//!
//! This crate owns route-level observability glue with Actix and Axum adapters, such as the
//! Prometheus text exposition endpoint. Metrics recording traits and concrete backend state remain
//! in `aster_forge_metrics`; product route modules can call these helpers without carrying
//! backend-specific `#[cfg]` blocks.
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
