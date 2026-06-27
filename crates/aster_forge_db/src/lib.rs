//! Shared database utilities for Aster services.
//!
//! This crate contains framework-neutral SeaORM helpers: connection setup, retry policy, offset
//! pagination, full-text search query helpers, whitelisted sorting, and transaction wrappers.
//! Product migrations, entities, and repository-specific query logic intentionally remain outside
//! this crate.
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

#[cfg(feature = "audit-log")]
pub mod audit_log;
#[cfg(feature = "runtime-component")]
mod component;
pub mod connection;
#[cfg(feature = "mail-outbox")]
pub mod mail_outbox;
pub mod pagination;
pub mod retry;
#[cfg(feature = "runtime-lease")]
pub mod runtime_lease;
#[cfg(feature = "scheduled-task")]
pub mod scheduled_task;
pub mod search_query;
pub mod sort;
#[cfg(feature = "system-config")]
pub mod system_config;
pub mod transaction;

#[cfg(feature = "audit-log")]
pub use audit_log::{
    AUDIT_LOG_ACTION_COLUMN, AUDIT_LOG_ACTION_CREATED_ID_INDEX,
    AUDIT_LOG_ACTION_CREATED_USER_INDEX, AUDIT_LOG_ACTION_INDEX, AUDIT_LOG_CREATED_AT_COLUMN,
    AUDIT_LOG_CREATED_AT_INDEX, AUDIT_LOG_CREATED_ID_INDEX, AUDIT_LOG_DETAILS_COLUMN,
    AUDIT_LOG_ENTITY_ID_COLUMN, AUDIT_LOG_ENTITY_NAME_COLUMN, AUDIT_LOG_ENTITY_TYPE_COLUMN,
    AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX, AUDIT_LOG_ID_COLUMN, AUDIT_LOG_IP_ADDRESS_COLUMN,
    AUDIT_LOG_USER_AGENT_COLUMN, AUDIT_LOG_USER_CREATED_ID_INDEX, AUDIT_LOG_USER_ID_COLUMN,
    AUDIT_LOG_USER_ID_INDEX, AUDIT_LOGS_TABLE, AuditLogCreate, AuditLogCursorSlice,
    AuditLogDbStore, AuditLogQuery, count_audit_logs_created_between,
    count_audit_logs_created_between_with_actions,
    count_distinct_audit_log_users_created_between_with_actions, create_audit_log_requests,
    create_audit_log_row, create_audit_log_rows, create_audit_logs_action_created_id_index,
    create_audit_logs_action_created_user_index, create_audit_logs_action_index,
    create_audit_logs_base_indexes, create_audit_logs_created_at_index,
    create_audit_logs_created_id_index, create_audit_logs_entity_type_created_id_index,
    create_audit_logs_query_indexes, create_audit_logs_table,
    create_audit_logs_user_created_id_index, create_audit_logs_user_id_index,
    delete_audit_logs_before, drop_audit_logs_action_created_id_index,
    drop_audit_logs_action_created_user_index, drop_audit_logs_action_index,
    drop_audit_logs_created_at_index, drop_audit_logs_created_id_index,
    drop_audit_logs_entity_type_created_id_index, drop_audit_logs_index, drop_audit_logs_table,
    drop_audit_logs_user_created_id_index, drop_audit_logs_user_id_index,
    find_audit_logs_with_filters_cursor,
};
#[cfg(feature = "runtime-component")]
pub use component::{
    DATABASE_COMPONENT, DATABASE_CONNECTIONS_SHUTDOWN_PHASE, DATABASE_HEALTH_CHECK,
    DATABASE_HEALTH_CHECK_TIMEOUT, DatabaseHealthComponent, DatabaseRuntimeComponent,
    check_database_component, database_component, database_component_after,
    database_health_component, database_health_options, ping_database,
};
pub use connection::{
    DatabaseConfig, DbHandles, connect, connect_reader_for_writer,
    connect_reader_for_writer_with_metrics, connect_with_metrics,
};
#[cfg(feature = "mail-outbox")]
pub use mail_outbox::{
    MAIL_OUTBOX_ATTEMPT_COUNT_COLUMN, MAIL_OUTBOX_CREATED_AT_COLUMN, MAIL_OUTBOX_ID_COLUMN,
    MAIL_OUTBOX_LAST_ERROR_COLUMN, MAIL_OUTBOX_NEXT_ATTEMPT_AT_COLUMN,
    MAIL_OUTBOX_PAYLOAD_JSON_COLUMN, MAIL_OUTBOX_PROCESSING_STARTED_AT_COLUMN,
    MAIL_OUTBOX_SENT_AT_COLUMN, MAIL_OUTBOX_STATUS_COLUMN, MAIL_OUTBOX_TABLE,
    MAIL_OUTBOX_TEMPLATE_CODE_COLUMN, MAIL_OUTBOX_TO_ADDRESS_COLUMN, MAIL_OUTBOX_TO_NAME_COLUMN,
    MAIL_OUTBOX_UPDATED_AT_COLUMN, MailOutboxCreate, MailOutboxDbStore,
    create_mail_outbox_due_index, create_mail_outbox_processing_index, create_mail_outbox_row,
    create_mail_outbox_sent_at_index, create_mail_outbox_table, drop_mail_outbox_due_index,
    drop_mail_outbox_processing_index, drop_mail_outbox_sent_at_index, drop_mail_outbox_table,
};
#[cfg(feature = "runtime-lease")]
pub use runtime_lease::{
    RUNTIME_LEASE_CREATED_AT_COLUMN, RUNTIME_LEASE_EXPIRES_AT_COLUMN, RUNTIME_LEASE_ID_COLUMN,
    RUNTIME_LEASE_LAST_RENEWED_AT_COLUMN, RUNTIME_LEASE_OWNER_ID_COLUMN,
    RUNTIME_LEASE_UPDATED_AT_COLUMN, RUNTIME_LEASES_TABLE, RuntimeLeaseDbStore,
    create_runtime_leases_table, drop_runtime_leases_table,
};
#[cfg(feature = "scheduled-task")]
pub use scheduled_task::{
    SCHEDULED_TASK_CLAIM_EXPIRES_AT_COLUMN, SCHEDULED_TASK_CLAIM_OWNER_ID_COLUMN,
    SCHEDULED_TASK_CREATED_AT_COLUMN, SCHEDULED_TASK_DISPLAY_NAME_COLUMN, SCHEDULED_TASK_ID_COLUMN,
    SCHEDULED_TASK_LAST_CLAIMED_AT_COLUMN, SCHEDULED_TASK_LAST_FINISHED_AT_COLUMN,
    SCHEDULED_TASK_NAME_COLUMN, SCHEDULED_TASK_NAMESPACE_COLUMN, SCHEDULED_TASK_NEXT_RUN_AT_COLUMN,
    SCHEDULED_TASK_UPDATED_AT_COLUMN, SCHEDULED_TASKS_TABLE, ScheduledTaskDbStore,
    create_scheduled_tasks_namespace_name_unique_index, create_scheduled_tasks_next_run_index,
    create_scheduled_tasks_table, drop_scheduled_tasks_namespace_name_unique_index,
    drop_scheduled_tasks_next_run_index, drop_scheduled_tasks_table,
};
#[cfg(feature = "system-config")]
pub use system_config::{
    PresentedSystemConfig, SystemConfigCursorSlice, SystemConfigDbBinding, SystemConfigDbStore,
    SystemConfigUpsert, present_system_config,
};
#[cfg(feature = "system-config")]
pub use system_config::{
    SYSTEM_CONFIG_CATEGORY_COLUMN, SYSTEM_CONFIG_DESCRIPTION_COLUMN, SYSTEM_CONFIG_ID_COLUMN,
    SYSTEM_CONFIG_IS_SENSITIVE_COLUMN, SYSTEM_CONFIG_KEY_COLUMN, SYSTEM_CONFIG_KEY_UNIQUE_INDEX,
    SYSTEM_CONFIG_NAMESPACE_COLUMN, SYSTEM_CONFIG_REQUIRES_RESTART_COLUMN,
    SYSTEM_CONFIG_SOURCE_COLUMN, SYSTEM_CONFIG_TABLE, SYSTEM_CONFIG_UPDATED_AT_COLUMN,
    SYSTEM_CONFIG_UPDATED_BY_COLUMN, SYSTEM_CONFIG_VALUE_COLUMN, SYSTEM_CONFIG_VALUE_TYPE_COLUMN,
    SYSTEM_CONFIG_VISIBILITY_COLUMN, create_system_config_key_unique_index,
    create_system_config_table, drop_system_config_key_unique_index, drop_system_config_table,
};

/// Result type returned by database helpers.
pub type Result<T> = std::result::Result<T, DbError>;

/// Errors returned by database helpers.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A database connection could not be established.
    #[error("database connection error: {0}")]
    DatabaseConnection(String),
    /// A database query, transaction, or setup operation failed.
    #[error("database operation error: {0}")]
    DatabaseOperation(String),
    /// Retry loop exhausted without a final operation error.
    #[error("retry exhausted")]
    RetryExhausted,
    /// Operation failed with an error that should not be retried.
    #[error("non-retryable error: {0}")]
    NonRetryable(String),
}

impl DbError {
    /// Creates a database-connection error from a displayable error.
    pub fn database_connection(error: impl std::fmt::Display) -> Self {
        Self::DatabaseConnection(error.to_string())
    }

    /// Creates a database-operation error from a displayable error.
    pub fn database_operation(error: impl std::fmt::Display) -> Self {
        Self::DatabaseOperation(error.to_string())
    }

    /// Creates a non-retryable error from a displayable error.
    pub fn non_retryable(error: impl std::fmt::Display) -> Self {
        Self::NonRetryable(error.to_string())
    }

    /// Returns whether the error is considered retryable by `retry::with_retry`.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::DatabaseOperation(_) | Self::DatabaseConnection(_)
        )
    }
}

impl From<sea_orm::DbErr> for DbError {
    fn from(value: sea_orm::DbErr) -> Self {
        Self::database_operation(value)
    }
}

#[cfg(test)]
mod tests {
    use super::DbError;

    #[test]
    fn db_error_constructors_preserve_messages() {
        assert_eq!(
            DbError::database_connection("offline").to_string(),
            "database connection error: offline"
        );
        assert_eq!(
            DbError::database_operation("bad query").to_string(),
            "database operation error: bad query"
        );
        assert_eq!(
            DbError::non_retryable("invalid config").to_string(),
            "non-retryable error: invalid config"
        );
        assert_eq!(DbError::RetryExhausted.to_string(), "retry exhausted");
    }

    #[test]
    fn retryable_classification_matches_error_kind() {
        assert!(DbError::database_connection("offline").is_retryable());
        assert!(DbError::database_operation("locked").is_retryable());
        assert!(!DbError::RetryExhausted.is_retryable());
        assert!(!DbError::non_retryable("invalid config").is_retryable());
    }

    #[test]
    fn sea_orm_errors_are_mapped_to_operation_errors() {
        let error = DbError::from(sea_orm::DbErr::Custom("custom failure".to_string()));

        assert!(matches!(error, DbError::DatabaseOperation(_)));
        assert!(error.to_string().contains("custom failure"));
    }
}
