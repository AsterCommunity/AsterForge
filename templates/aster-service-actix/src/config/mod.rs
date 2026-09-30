//! Configuration loading and shared runtime access.
//!
//! The concrete static schema lives in `schema`, matching the structure used by Aster services
//! such as AsterYggdrasil. Product-specific runtime-editable configuration can grow into sibling
//! modules later without forcing callers to change imports.

mod loader;
mod schema;

pub use loader::{CONFIG_ENV_VAR, DEFAULT_CONFIG_PATH, load};
pub use schema::{AppConfig, DatabaseConfig};
