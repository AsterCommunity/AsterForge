//! AsterForge backend crate.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod alloc;
pub mod api;
pub mod cache;
pub mod config;
pub mod db;
pub mod entities;
pub mod errors;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod metrics_core;
pub mod runtime;
pub mod services;
pub mod types;
pub mod utils;
