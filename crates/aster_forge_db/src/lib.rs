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
mod index;
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
    delete_audit_logs_before, drop_audit_logs_table, find_audit_logs_with_filters_cursor,
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
pub use index::{drop_index_if_exists, rename_mysql_index_if_exists};
#[cfg(feature = "mail-outbox")]
pub use mail_outbox::{
    MAIL_OUTBOX_ATTEMPT_COUNT_COLUMN, MAIL_OUTBOX_CREATED_AT_COLUMN, MAIL_OUTBOX_DUE_INDEX,
    MAIL_OUTBOX_ID_COLUMN, MAIL_OUTBOX_LAST_ERROR_COLUMN, MAIL_OUTBOX_NEXT_ATTEMPT_AT_COLUMN,
    MAIL_OUTBOX_PAYLOAD_JSON_COLUMN, MAIL_OUTBOX_PROCESSING_INDEX,
    MAIL_OUTBOX_PROCESSING_STARTED_AT_COLUMN, MAIL_OUTBOX_SENT_AT_COLUMN,
    MAIL_OUTBOX_SENT_AT_INDEX, MAIL_OUTBOX_STATUS_COLUMN, MAIL_OUTBOX_TABLE,
    MAIL_OUTBOX_TEMPLATE_CODE_COLUMN, MAIL_OUTBOX_TO_ADDRESS_COLUMN, MAIL_OUTBOX_TO_NAME_COLUMN,
    MAIL_OUTBOX_UPDATED_AT_COLUMN, MailOutboxCreate, MailOutboxDbStore,
    create_mail_outbox_due_index, create_mail_outbox_processing_index, create_mail_outbox_row,
    create_mail_outbox_sent_at_index, create_mail_outbox_table, drop_mail_outbox_table,
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
    SCHEDULED_TASK_NAME_COLUMN, SCHEDULED_TASK_NAMESPACE_COLUMN,
    SCHEDULED_TASK_NAMESPACE_NAME_UNIQUE_INDEX, SCHEDULED_TASK_NEXT_RUN_AT_COLUMN,
    SCHEDULED_TASK_NEXT_RUN_INDEX, SCHEDULED_TASK_UPDATED_AT_COLUMN, SCHEDULED_TASKS_TABLE,
    ScheduledTaskDbStore, create_scheduled_tasks_namespace_name_unique_index,
    create_scheduled_tasks_next_run_index, create_scheduled_tasks_table,
    drop_scheduled_tasks_table,
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
    create_system_config_table, drop_system_config_table,
};

/// Result type returned by database helpers.
pub type Result<T> = std::result::Result<T, DbError>;

/// Database failure classes that are stable enough for infrastructure retry decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseErrorKind {
    /// The database aborted the current transaction because of a deadlock.
    Deadlock,
    /// The database aborted the transaction because its serialization snapshot could not commit.
    SerializationFailure,
    /// The database rejected an operation after a lock wait timeout, or reported the
    /// database as busy/locked (SQLite).
    LockTimeout,
    /// A unique or primary-key constraint rejected the operation.
    UniqueConstraint,
    /// A foreign-key constraint rejected the operation.
    ForeignKeyConstraint,
}

impl DatabaseErrorKind {
    /// Returns whether the failure class is a transient locking conflict that a bounded
    /// retry at the correct boundary can resolve.
    pub fn is_transient_locking(self) -> bool {
        matches!(
            self,
            Self::Deadlock | Self::SerializationFailure | Self::LockTimeout
        )
    }
}

/// Classifies driver-native database errors without relying on localized messages.
pub fn database_error_kind(error: &sea_orm::DbErr) -> Option<DatabaseErrorKind> {
    use sea_orm::{DbErr, RuntimeErr};

    let sqlx_error = match error {
        DbErr::Exec(RuntimeErr::SqlxError(error)) | DbErr::Query(RuntimeErr::SqlxError(error)) => {
            error.as_ref()
        }
        _ => return None,
    };
    let sea_orm::sqlx::Error::Database(database_error) = sqlx_error else {
        return None;
    };

    let mysql_number = database_error
        .try_downcast_ref::<sea_orm::sqlx::mysql::MySqlDatabaseError>()
        .map(sea_orm::sqlx::mysql::MySqlDatabaseError::number);
    let postgres_code = database_error
        .try_downcast_ref::<sea_orm::sqlx::postgres::PgDatabaseError>()
        .map(sea_orm::sqlx::postgres::PgDatabaseError::code);
    let sqlite_code = database_error
        .try_downcast_ref::<sea_orm::sqlx::sqlite::SqliteError>()
        .and_then(|error| {
            use sea_orm::sqlx::error::DatabaseError;

            error.code()
        })
        .and_then(|code| code.parse::<i32>().ok());
    database_error_kind_from_signals(
        database_error.kind(),
        mysql_number,
        postgres_code,
        sqlite_code,
    )
}

fn database_error_kind_from_signals(
    driver_kind: sea_orm::sqlx::error::ErrorKind,
    mysql_number: Option<u16>,
    postgres_code: Option<&str>,
    sqlite_code: Option<i32>,
) -> Option<DatabaseErrorKind> {
    use sea_orm::sqlx::error::ErrorKind;

    match driver_kind {
        ErrorKind::UniqueViolation => return Some(DatabaseErrorKind::UniqueConstraint),
        ErrorKind::ForeignKeyViolation => return Some(DatabaseErrorKind::ForeignKeyConstraint),
        _ => {}
    }
    if let Some(number) = mysql_number {
        match number {
            1205 => return Some(DatabaseErrorKind::LockTimeout),
            1213 => return Some(DatabaseErrorKind::Deadlock),
            _ => {}
        }
    }
    if let Some(code) = postgres_code {
        match code {
            "40P01" => return Some(DatabaseErrorKind::Deadlock),
            "40001" => return Some(DatabaseErrorKind::SerializationFailure),
            "55P03" => return Some(DatabaseErrorKind::LockTimeout),
            _ => {}
        }
    }
    // SQLite reports lock contention through the extended result code; the primary code
    // lives in the low byte (e.g. SQLITE_BUSY_SNAPSHOT = 517 belongs to the SQLITE_BUSY = 5
    // family), so match on the masked value to cover the extended variants.
    match sqlite_code.map(|code| code & 0xFF) {
        Some(5 | 6) => Some(DatabaseErrorKind::LockTimeout),
        _ => None,
    }
}

/// Errors returned by database helpers.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A database connection could not be established.
    #[error("database connection error: {0}")]
    DatabaseConnection(String),
    /// A database query, transaction, or setup operation failed.
    #[error("database operation error: {0}")]
    DatabaseOperation(String),
    /// A database operation error with a driver-native classification.
    #[error("database operation error: {message}")]
    DatabaseOperationClassified {
        message: String,
        kind: DatabaseErrorKind,
    },
    /// The commit response was lost after the transaction may have been committed.
    #[error("database commit outcome unknown: {message}")]
    CommitOutcomeUnknown {
        message: String,
        kind: Option<DatabaseErrorKind>,
    },
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

    /// Creates a database-operation error while preserving its driver-native classification.
    pub fn database_operation_classified(
        error: impl std::fmt::Display,
        kind: DatabaseErrorKind,
    ) -> Self {
        Self::DatabaseOperationClassified {
            message: error.to_string(),
            kind,
        }
    }

    /// Creates an error for a commit whose final server-side outcome is unknown.
    pub fn commit_outcome_unknown(
        error: impl std::fmt::Display,
        kind: Option<DatabaseErrorKind>,
    ) -> Self {
        Self::CommitOutcomeUnknown {
            message: error.to_string(),
            kind,
        }
    }

    /// Returns the driver-native classification, when one was available.
    pub fn database_error_kind(&self) -> Option<DatabaseErrorKind> {
        match self {
            Self::DatabaseOperationClassified { kind, .. } => Some(*kind),
            Self::CommitOutcomeUnknown { kind, .. } => *kind,
            _ => None,
        }
    }

    /// Returns whether this error came from a commit with an unknown final outcome.
    pub fn commit_outcome_is_unknown(&self) -> bool {
        matches!(self, Self::CommitOutcomeUnknown { .. })
    }

    /// Creates a non-retryable error from a displayable error.
    pub fn non_retryable(error: impl std::fmt::Display) -> Self {
        Self::NonRetryable(error.to_string())
    }

    /// Returns whether the error is considered retryable by `retry::with_retry`.
    ///
    /// Only connection failures and driver-classified transient locking conflicts
    /// (deadlock, serialization failure, lock timeout) qualify. Unclassified operation
    /// errors are not retried: without a driver classification there is no evidence the
    /// operation failed in a retry-safe way, so callers see the failure immediately.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::DatabaseConnection(_) => true,
            Self::DatabaseOperationClassified { kind, .. } => kind.is_transient_locking(),
            _ => false,
        }
    }
}

impl From<sea_orm::DbErr> for DbError {
    fn from(value: sea_orm::DbErr) -> Self {
        match database_error_kind(&value) {
            Some(kind) => Self::database_operation_classified(value, kind),
            None => Self::database_operation(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseErrorKind, DbError, database_error_kind_from_signals};
    use sea_orm::sqlx::error::ErrorKind;

    #[test]
    fn database_error_kind_covers_common_driver_signals() {
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, Some(1213), None, None),
            Some(DatabaseErrorKind::Deadlock)
        );
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, Some(1205), None, None),
            Some(DatabaseErrorKind::LockTimeout)
        );
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, None, Some("40P01"), None),
            Some(DatabaseErrorKind::Deadlock)
        );
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, None, Some("40001"), None),
            Some(DatabaseErrorKind::SerializationFailure)
        );
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, None, Some("55P03"), None),
            Some(DatabaseErrorKind::LockTimeout)
        );
    }

    #[test]
    fn database_error_kind_covers_sqlite_busy_and_locked_families() {
        // SQLITE_BUSY = 5 and SQLITE_LOCKED = 6, including extended variants whose
        // high byte carries extra context (e.g. SQLITE_BUSY_SNAPSHOT = 517).
        for code in [5, 6, 261, 517, 262] {
            assert_eq!(
                database_error_kind_from_signals(ErrorKind::Other, None, None, Some(code)),
                Some(DatabaseErrorKind::LockTimeout),
                "sqlite code {code} should classify as a lock timeout"
            );
        }
        // SQLITE_ERROR = 1 and SQLITE_CONSTRAINT = 19 carry no retryable locking meaning.
        for code in [1, 19, 0] {
            assert_eq!(
                database_error_kind_from_signals(ErrorKind::Other, None, None, Some(code)),
                None,
                "sqlite code {code} should stay unclassified"
            );
        }
    }

    #[test]
    fn database_error_kind_prefers_cross_backend_constraint_kind() {
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::UniqueViolation, Some(1213), None, None),
            Some(DatabaseErrorKind::UniqueConstraint)
        );
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::ForeignKeyViolation, None, None, None),
            Some(DatabaseErrorKind::ForeignKeyConstraint)
        );
        // A SQLite locking code must not override a cross-backend constraint kind.
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::UniqueViolation, None, None, Some(5)),
            Some(DatabaseErrorKind::UniqueConstraint)
        );
    }

    #[test]
    fn database_error_kind_ignores_unknown_or_non_driver_signals() {
        assert_eq!(
            database_error_kind_from_signals(ErrorKind::Other, Some(9999), Some("99999"), None),
            None
        );
        assert_eq!(
            super::database_error_kind(&sea_orm::DbErr::Custom("not a driver error".to_string())),
            None
        );
    }

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
        for kind in [
            DatabaseErrorKind::Deadlock,
            DatabaseErrorKind::SerializationFailure,
            DatabaseErrorKind::LockTimeout,
        ] {
            assert!(
                DbError::database_operation_classified("conflict", kind).is_retryable(),
                "{kind:?} should be retryable"
            );
        }
        // Unclassified operation errors carry no evidence of retry safety.
        assert!(!DbError::database_operation("locked").is_retryable());
        for kind in [
            DatabaseErrorKind::UniqueConstraint,
            DatabaseErrorKind::ForeignKeyConstraint,
        ] {
            assert!(
                !DbError::database_operation_classified("constraint", kind).is_retryable(),
                "{kind:?} should not be retryable"
            );
        }
        assert!(!DbError::RetryExhausted.is_retryable());
        assert!(!DbError::non_retryable("invalid config").is_retryable());
    }

    #[test]
    fn commit_outcome_unknown_preserves_kind_and_marker() {
        let error = DbError::commit_outcome_unknown(
            "connection lost after COMMIT",
            Some(DatabaseErrorKind::Deadlock),
        );
        assert!(error.commit_outcome_is_unknown());
        assert_eq!(
            error.database_error_kind(),
            Some(DatabaseErrorKind::Deadlock)
        );
        assert!(!error.is_retryable());
    }

    #[test]
    fn sea_orm_errors_are_mapped_to_operation_errors() {
        let error = DbError::from(sea_orm::DbErr::Custom("custom failure".to_string()));

        assert!(matches!(error, DbError::DatabaseOperation(_)));
        assert!(error.to_string().contains("custom failure"));
    }
}
