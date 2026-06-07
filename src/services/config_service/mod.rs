//! Runtime system configuration service.

mod schema;
mod system;

pub use schema::{ConfigSchemaItem, ConfigSchemaOption, get_schema};
pub use system::{
    SystemConfig, SystemConfigValue, delete, delete_with_audit, ensure_defaults, get_by_key,
    list_paginated, set, set_with_audit, set_with_audit_and_visibility, set_with_visibility,
};
