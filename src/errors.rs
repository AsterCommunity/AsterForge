//! Unified error type.

use crate::api::error_code::AsterErrorCode;
use crate::api::response::ApiResponse;
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use std::any::Any;

pub type Result<T> = std::result::Result<T, AsterError>;

#[derive(Debug, Clone)]
pub enum AsterError {
    DatabaseConnection(String),
    DatabaseOperation(String),
    ConfigError(String),
    ValidationError(String),
    AuthInvalidCredentials(String),
    AuthTokenInvalid(String),
    AuthTokenExpired(String),
    AuthForbidden(String),
    RecordNotFound(String),
    ExternalAuthError(String),
    MailNotConfigured(String),
    MailDeliveryFailed(String),
    InternalError(String),
}

impl AsterError {
    pub fn database_connection(message: impl Into<String>) -> Self {
        Self::DatabaseConnection(message.into())
    }

    pub fn database_operation(message: impl Into<String>) -> Self {
        Self::DatabaseOperation(message.into())
    }

    pub fn config_error(message: impl Into<String>) -> Self {
        Self::ConfigError(message.into())
    }

    pub fn validation_error(message: impl Into<String>) -> Self {
        Self::ValidationError(message.into())
    }

    pub fn auth_invalid_credentials(message: impl Into<String>) -> Self {
        Self::AuthInvalidCredentials(message.into())
    }

    pub fn auth_token_invalid(message: impl Into<String>) -> Self {
        Self::AuthTokenInvalid(message.into())
    }

    pub fn auth_token_expired(message: impl Into<String>) -> Self {
        Self::AuthTokenExpired(message.into())
    }

    pub fn auth_forbidden(message: impl Into<String>) -> Self {
        Self::AuthForbidden(message.into())
    }

    pub fn record_not_found(message: impl Into<String>) -> Self {
        Self::RecordNotFound(message.into())
    }

    pub fn external_auth_error(message: impl Into<String>) -> Self {
        Self::ExternalAuthError(message.into())
    }

    pub fn mail_not_configured(message: impl Into<String>) -> Self {
        Self::MailNotConfigured(message.into())
    }

    pub fn mail_delivery_failed(message: impl Into<String>) -> Self {
        Self::MailDeliveryFailed(message.into())
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::InternalError(message.into())
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::DatabaseConnection(_) => "E001",
            Self::DatabaseOperation(_) => "E002",
            Self::ConfigError(_) => "E003",
            Self::InternalError(_) => "E004",
            Self::ValidationError(_) => "E005",
            Self::RecordNotFound(_) => "E006",
            Self::AuthInvalidCredentials(_) => "E010",
            Self::AuthTokenExpired(_) => "E011",
            Self::AuthTokenInvalid(_) => "E012",
            Self::AuthForbidden(_) => "E013",
            Self::ExternalAuthError(_) => "E020",
            Self::MailNotConfigured(_) => "E030",
            Self::MailDeliveryFailed(_) => "E031",
        }
    }

    pub fn api_error_code(&self) -> AsterErrorCode {
        match self {
            Self::DatabaseConnection(_) | Self::DatabaseOperation(_) => {
                AsterErrorCode::DatabaseError
            }
            Self::ConfigError(_) => AsterErrorCode::ConfigError,
            Self::ValidationError(_) => AsterErrorCode::BadRequest,
            Self::AuthInvalidCredentials(_) => AsterErrorCode::AuthCredentialsFailed,
            Self::AuthTokenInvalid(_) => AsterErrorCode::AuthTokenInvalid,
            Self::AuthTokenExpired(_) => AsterErrorCode::AuthTokenExpired,
            Self::AuthForbidden(_) => AsterErrorCode::Forbidden,
            Self::RecordNotFound(_) => AsterErrorCode::NotFound,
            Self::ExternalAuthError(_) => AsterErrorCode::ExternalAuthError,
            Self::MailNotConfigured(_) => AsterErrorCode::MailNotConfigured,
            Self::MailDeliveryFailed(_) => AsterErrorCode::MailDeliveryFailed,
            Self::InternalError(_) => AsterErrorCode::InternalServerError,
        }
    }

    pub fn retryable(&self) -> Option<bool> {
        match self {
            Self::DatabaseConnection(_) | Self::DatabaseOperation(_) => Some(true),
            Self::MailDeliveryFailed(_) => Some(true),
            Self::MailNotConfigured(_) => Some(false),
            _ => None,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::DatabaseConnection(message)
            | Self::DatabaseOperation(message)
            | Self::ConfigError(message)
            | Self::ValidationError(message)
            | Self::AuthInvalidCredentials(message)
            | Self::AuthTokenInvalid(message)
            | Self::AuthTokenExpired(message)
            | Self::AuthForbidden(message)
            | Self::RecordNotFound(message)
            | Self::ExternalAuthError(message)
            | Self::MailNotConfigured(message)
            | Self::MailDeliveryFailed(message)
            | Self::InternalError(message) => message,
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::ValidationError(_) => StatusCode::BAD_REQUEST,
            Self::AuthInvalidCredentials(_)
            | Self::AuthTokenInvalid(_)
            | Self::AuthTokenExpired(_) => StatusCode::UNAUTHORIZED,
            Self::AuthForbidden(_) => StatusCode::FORBIDDEN,
            Self::RecordNotFound(_) => StatusCode::NOT_FOUND,
            Self::ExternalAuthError(_) => StatusCode::BAD_REQUEST,
            Self::MailNotConfigured(_) | Self::MailDeliveryFailed(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::DatabaseConnection(_)
            | Self::DatabaseOperation(_)
            | Self::ConfigError(_)
            | Self::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl std::fmt::Display for AsterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message())
    }
}

impl std::error::Error for AsterError {}

impl ResponseError for AsterError {
    fn status_code(&self) -> StatusCode {
        self.status_code()
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        if status.is_server_error() {
            tracing::error!(
                error = %self,
                internal_code = self.code(),
                api_code = %self.api_error_code(),
                "request failed"
            );
        } else {
            tracing::warn!(
                error = %self,
                internal_code = self.code(),
                api_code = %self.api_error_code(),
                "request failed"
            );
        }
        HttpResponse::build(status).json(ApiResponse::<()>::error_body(
            self.api_error_code(),
            self.message(),
            self.retryable(),
        ))
    }
}

impl From<sea_orm::DbErr> for AsterError {
    fn from(value: sea_orm::DbErr) -> Self {
        Self::database_operation(value.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for AsterError {
    fn from(value: jsonwebtoken::errors::Error) -> Self {
        match value.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                Self::auth_token_expired("token expired")
            }
            _ => Self::auth_token_invalid(value.to_string()),
        }
    }
}

fn map_display_error<E: std::fmt::Display + 'static>(
    error: E,
    context: Option<&str>,
    mapper: impl FnOnce(String) -> AsterError,
) -> AsterError {
    if let Some(db_error) = (&error as &dyn Any).downcast_ref::<sea_orm::DbErr>()
        && let sea_orm::DbErr::RecordNotFound(message) = db_error
    {
        let message = match context {
            Some(context) => format!("{context}: {message}"),
            None => message.clone(),
        };
        return AsterError::record_not_found(message);
    }

    let message = match context {
        Some(context) => format!("{context}: {error}"),
        None => error.to_string(),
    };
    mapper(message)
}

pub trait MapAsterErr<T> {
    fn map_aster_err(self, mapper: impl FnOnce(String) -> AsterError) -> Result<T>;
    fn map_aster_err_ctx(
        self,
        context: &str,
        mapper: impl FnOnce(String) -> AsterError,
    ) -> Result<T>;
    fn map_aster_err_with(self, mapper: impl FnOnce() -> AsterError) -> Result<T>;
}

impl<T, E: std::fmt::Display + 'static> MapAsterErr<T> for std::result::Result<T, E> {
    fn map_aster_err(self, mapper: impl FnOnce(String) -> AsterError) -> Result<T> {
        self.map_err(|error| map_display_error(error, None, mapper))
    }

    fn map_aster_err_ctx(
        self,
        context: &str,
        mapper: impl FnOnce(String) -> AsterError,
    ) -> Result<T> {
        self.map_err(|error| map_display_error(error, Some(context), mapper))
    }

    fn map_aster_err_with(self, mapper: impl FnOnce() -> AsterError) -> Result<T> {
        self.map_err(|_| mapper())
    }
}

pub fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{AsterError, MapAsterErr, display_error};
    use crate::api::error_code::AsterErrorCode;
    use actix_web::ResponseError;
    use actix_web::body::to_bytes;
    use actix_web::http::StatusCode;

    #[test]
    fn error_variants_map_to_public_codes_statuses_and_retryability() {
        let cases = [
            (
                AsterError::database_connection("db connect"),
                "E001",
                AsterErrorCode::DatabaseError,
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(true),
            ),
            (
                AsterError::database_operation("db op"),
                "E002",
                AsterErrorCode::DatabaseError,
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(true),
            ),
            (
                AsterError::config_error("config"),
                "E003",
                AsterErrorCode::ConfigError,
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
            ),
            (
                AsterError::internal_error("internal"),
                "E004",
                AsterErrorCode::InternalServerError,
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
            ),
            (
                AsterError::validation_error("bad"),
                "E005",
                AsterErrorCode::BadRequest,
                StatusCode::BAD_REQUEST,
                None,
            ),
            (
                AsterError::record_not_found("missing"),
                "E006",
                AsterErrorCode::NotFound,
                StatusCode::NOT_FOUND,
                None,
            ),
            (
                AsterError::auth_invalid_credentials("invalid credentials"),
                "E010",
                AsterErrorCode::AuthCredentialsFailed,
                StatusCode::UNAUTHORIZED,
                None,
            ),
            (
                AsterError::auth_token_expired("expired"),
                "E011",
                AsterErrorCode::AuthTokenExpired,
                StatusCode::UNAUTHORIZED,
                None,
            ),
            (
                AsterError::auth_token_invalid("invalid"),
                "E012",
                AsterErrorCode::AuthTokenInvalid,
                StatusCode::UNAUTHORIZED,
                None,
            ),
            (
                AsterError::auth_forbidden("forbidden"),
                "E013",
                AsterErrorCode::Forbidden,
                StatusCode::FORBIDDEN,
                None,
            ),
            (
                AsterError::external_auth_error("external"),
                "E020",
                AsterErrorCode::ExternalAuthError,
                StatusCode::BAD_REQUEST,
                None,
            ),
        ];

        for (error, internal_code, public_code, status, retryable) in cases {
            assert_eq!(error.code(), internal_code);
            assert_eq!(error.api_error_code(), public_code);
            assert_eq!(error.status_code(), status);
            assert_eq!(ResponseError::status_code(&error), status);
            assert_eq!(error.retryable(), retryable);
            assert_eq!(error.to_string(), error.message());
        }
    }

    #[actix_web::test]
    async fn error_response_exposes_public_code_without_internal_code() {
        let response = AsterError::auth_invalid_credentials("invalid credentials").error_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body()).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["code"],
            AsterErrorCode::AuthCredentialsFailed.as_str()
        );
        assert_eq!(
            value["error"]["code"],
            AsterErrorCode::AuthCredentialsFailed.as_str()
        );
        assert!(value.get("internal_code").is_none());
        assert!(value["error"].get("internal_code").is_none());
    }

    #[test]
    fn jsonwebtoken_errors_map_expired_signature_to_expired_token() {
        let token_error = jsonwebtoken::decode::<serde_json::Value>(
            "not-a-jwt",
            &jsonwebtoken::DecodingKey::from_secret(b"secret"),
            &jsonwebtoken::Validation::default(),
        )
        .unwrap_err();
        let error = AsterError::from(token_error);

        assert_eq!(error.api_error_code(), AsterErrorCode::AuthTokenInvalid);
        assert_eq!(error.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn map_aster_err_helpers_preserve_record_not_found_special_case() {
        let db_error: std::result::Result<(), sea_orm::DbErr> =
            Err(sea_orm::DbErr::RecordNotFound("row missing".to_string()));
        let mapped = db_error
            .map_aster_err_ctx("load user", AsterError::database_operation)
            .unwrap_err();

        assert_eq!(mapped.api_error_code(), AsterErrorCode::NotFound);
        assert_eq!(mapped.message(), "load user: row missing");

        let display_error_result: std::result::Result<(), &str> = Err("plain error");
        let mapped = display_error_result
            .map_aster_err(AsterError::config_error)
            .unwrap_err();
        assert_eq!(mapped.api_error_code(), AsterErrorCode::ConfigError);
        assert_eq!(mapped.message(), "plain error");

        let fixed_error_result: std::result::Result<(), &str> = Err("ignored");
        let mapped = fixed_error_result
            .map_aster_err_with(|| AsterError::internal_error("fixed"))
            .unwrap_err();
        assert_eq!(mapped.api_error_code(), AsterErrorCode::InternalServerError);
        assert_eq!(mapped.message(), "fixed");
    }

    #[test]
    fn display_error_uses_display_representation() {
        assert_eq!(display_error("plain"), "plain");
        assert_eq!(display_error(42), "42");
    }
}
