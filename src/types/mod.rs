//! Shared domain types.
//!
//! Concrete submodules make type ownership explicit. The root facade preserves
//! the stable `crate::types::{...}` compatibility entry for cross-boundary
//! imports.

pub mod audit;
pub mod config;
pub mod external_auth;
mod facade;
pub mod mail;
pub mod task;
pub mod user;

pub use facade::*;
