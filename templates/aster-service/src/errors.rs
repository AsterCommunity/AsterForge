//! Product error boundary.
//!
//! Keep product-facing status codes, response envelopes, localization, and audit wording outside
//! Forge. This template only maps shared infrastructure errors into a small product error enum.

/// Product result type.
pub type Result<T> = std::result::Result<T, AppError>;

/// Product error type used by the generated service skeleton.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Database or shared persistence failure.
    #[error("database error: {0}")]
    Database(String),
    /// Runtime assembly or lifecycle failure.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// Runtime configuration or config-sync failure.
    #[error("config error: {0}")]
    Config(String),
    /// Mail delivery failure.
    #[error("mail delivery error: {0}")]
    Mail(String),
    /// Runtime data directory preparation failure.
    #[error("io error: {0}")]
    Io(String),
}

impl From<aster_forge_db::DbError> for AppError {
    fn from(error: aster_forge_db::DbError) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<sea_orm::DbErr> for AppError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<aster_forge_runtime::AsterRuntimeError> for AppError {
    fn from(error: aster_forge_runtime::AsterRuntimeError) -> Self {
        Self::Runtime(error.to_string())
    }
}

impl From<aster_forge_config::ConfigCoreError> for AppError {
    fn from(error: aster_forge_config::ConfigCoreError) -> Self {
        Self::Config(error.to_string())
    }
}

impl From<config::ConfigError> for AppError {
    fn from(error: config::ConfigError) -> Self {
        Self::Config(error.to_string())
    }
}

impl From<aster_forge_mail::MailDeliveryError> for AppError {
    fn from(error: aster_forge_mail::MailDeliveryError) -> Self {
        Self::Mail(error.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
