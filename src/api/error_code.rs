//! Stable public API error codes.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

macro_rules! define_error_codes {
    ($($variant:ident => $value:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum AsterErrorCode {
            $(
                #[serde(rename = $value)]
                $variant,
            )+
        }

        impl AsterErrorCode {
            pub const ALL: &'static [Self] = &[
                $(Self::$variant,)+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

#[cfg(all(debug_assertions, feature = "openapi"))]
impl utoipa::PartialSchema for AsterErrorCode {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .enum_values(Some(Self::ALL.iter().map(|code| code.as_str())))
            .into()
    }
}

#[cfg(all(debug_assertions, feature = "openapi"))]
impl utoipa::ToSchema for AsterErrorCode {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("AsterErrorCode")
    }
}

define_error_codes! {
    Success => "success",

    BadRequest => "bad_request",
    NotFound => "not_found",
    InternalServerError => "internal_server_error",
    DatabaseError => "database.error",
    ConfigError => "config.error",
    EndpointNotFound => "endpoint.not_found",
    RateLimited => "rate_limited",
    MailNotConfigured => "mail.not_configured",
    MailDeliveryFailed => "mail.delivery_failed",

    AuthCredentialsFailed => "auth.credentials_failed",
    AuthTokenExpired => "auth.token_expired",
    AuthTokenInvalid => "auth.token_invalid",
    Forbidden => "forbidden",

    ExternalAuthError => "external_auth.error",
}

impl AsRef<str> for AsterErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for AsterErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseAsterErrorCodeError;

impl std::fmt::Display for ParseAsterErrorCodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid Aster error code")
    }
}

impl std::error::Error for ParseAsterErrorCodeError {}

impl FromStr for AsterErrorCode {
    type Err = ParseAsterErrorCodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(ParseAsterErrorCodeError)
    }
}

#[cfg(test)]
mod tests {
    use super::{AsterErrorCode, ParseAsterErrorCodeError};
    use std::str::FromStr;

    #[test]
    fn serializes_as_stable_wire_value() {
        assert_eq!(
            serde_json::to_value(AsterErrorCode::AuthCredentialsFailed).unwrap(),
            serde_json::json!("auth.credentials_failed")
        );
    }

    #[test]
    fn parses_all_stable_wire_values() {
        for &code in AsterErrorCode::ALL {
            assert_eq!(AsterErrorCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(AsterErrorCode::parse("AuthCredentialsFailed"), None);
    }

    #[test]
    fn display_as_ref_and_from_str_use_stable_wire_values() {
        let code = AsterErrorCode::RateLimited;

        assert_eq!(code.as_ref(), "rate_limited");
        assert_eq!(code.to_string(), "rate_limited");
        assert_eq!(
            AsterErrorCode::from_str("rate_limited").unwrap(),
            AsterErrorCode::RateLimited
        );
        assert!(AsterErrorCode::from_str("RATE_LIMITED").is_err());
    }

    #[test]
    fn deserializes_only_known_stable_wire_values() {
        assert_eq!(
            serde_json::from_str::<AsterErrorCode>(r#""database.error""#).unwrap(),
            AsterErrorCode::DatabaseError
        );
        assert!(serde_json::from_str::<AsterErrorCode>(r#""database_error""#).is_err());
    }

    #[test]
    fn parse_error_implements_display_and_error() {
        let error = ParseAsterErrorCodeError;
        assert_eq!(error.to_string(), "invalid Aster error code");
        let dyn_error: &dyn std::error::Error = &error;
        assert_eq!(dyn_error.to_string(), "invalid Aster error code");
    }
}
