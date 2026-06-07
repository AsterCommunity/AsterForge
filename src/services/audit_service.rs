//! Audit log service.

mod context;
mod details;
mod filters;
mod manager;
mod models;
mod presentation;
mod query;

pub use crate::types::{AuditAction, AuditEntityType};
pub use context::{AuditContext, AuditRequestInfo};
pub use details::{
    AdminTaskCleanupAuditDetails, ConfigActionDetails, ConfigUpdateDetails,
    ExternalAuthProviderTestParamsAuditDetails, LoginAuditDetails, MailAuditDetails,
    TaskRetryAuditDetails, details,
};
pub use filters::{AuditLogFilterQuery, AuditLogFilters};
pub use manager::{
    flush_global_audit_log_manager, init_global_audit_log_manager, log, log_with_db_and_config,
    log_with_details, should_record, should_record_with_config, shutdown_global_audit_log_manager,
};
pub use models::{AuditLogEntry, AuditPresentation, AuditPresentationMessage, AuditUserSummary};
pub use query::{cleanup_expired, query};
