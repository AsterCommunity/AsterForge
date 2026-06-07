//! Runtime system configuration service.

mod actions;
mod schema;
mod system;

pub use crate::services::mail_template::{
    TemplateVariableGroup, TemplateVariableItem, list_template_variable_groups,
};
pub use actions::{
    ConfigActionResult, ConfigActionType, ExecuteConfigActionInput, MAIL_CONFIG_ACTION_KEY,
    execute_action_with_audit,
};
pub use schema::{ConfigSchemaItem, ConfigSchemaOption, get_schema};
pub use system::{
    SystemConfig, SystemConfigValue, delete, delete_with_audit, ensure_defaults, get_by_key,
    list_paginated, set, set_with_audit, set_with_audit_and_visibility, set_with_visibility,
};
