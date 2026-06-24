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

pub mod connection;
pub mod pagination;
pub mod retry;
pub mod search_query;
pub mod sort;
pub mod transaction;

pub use connection::{
    DatabaseConfig, DbHandles, DbMetricsRecorder, NoopDbMetrics, SharedDbMetricsRecorder, connect,
    connect_reader_for_writer, connect_reader_for_writer_with_metrics, connect_with_metrics,
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
