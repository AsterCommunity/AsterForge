//! API response envelope.

use crate::api::error_code::AsterErrorCode;
use actix_web::HttpResponse;
use serde::Serialize;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ApiErrorInfo {
    pub code: AsterErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub build_time: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ApiResponse<T>
where
    T: Serialize,
{
    pub code: AsterErrorCode,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiErrorInfo>,
}

impl<T> ApiResponse<T>
where
    T: Serialize,
{
    pub fn ok(data: T) -> Self {
        Self {
            code: AsterErrorCode::Success,
            msg: String::new(),
            data: Some(data),
            error: None,
        }
    }

    pub fn ok_empty() -> ApiResponse<()> {
        ApiResponse {
            code: AsterErrorCode::Success,
            msg: String::new(),
            data: None,
            error: None,
        }
    }

    pub fn error(code: AsterErrorCode, message: impl Into<String>) -> ApiResponse<()> {
        Self::error_with_details(code, message, None)
    }

    pub fn error_with_details(
        code: AsterErrorCode,
        message: impl Into<String>,
        error: Option<ApiErrorInfo>,
    ) -> ApiResponse<()> {
        ApiResponse {
            code,
            msg: message.into(),
            data: None,
            error,
        }
    }

    pub fn into_response(self) -> HttpResponse {
        HttpResponse::Ok().json(self)
    }
}

impl ApiResponse<()> {
    pub fn error_body(
        code: AsterErrorCode,
        message: impl Into<String>,
        retryable: Option<bool>,
    ) -> Self {
        Self {
            code,
            msg: message.into(),
            data: None,
            error: Some(ApiErrorInfo { code, retryable }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiErrorInfo, ApiResponse};
    use crate::api::error_code::AsterErrorCode;
    use actix_web::body::to_bytes;

    #[test]
    fn ok_response_serializes_data_without_error() {
        let value = serde_json::to_value(ApiResponse::ok(serde_json::json!({
            "ready": true
        })))
        .unwrap();

        assert_eq!(value["code"], AsterErrorCode::Success.as_str());
        assert_eq!(value["msg"], "");
        assert_eq!(value["data"]["ready"], true);
        assert!(value.get("error").is_none() || value["error"].is_null());
    }

    #[test]
    fn ok_empty_omits_data_and_error_fields() {
        let value = serde_json::to_value(ApiResponse::<()>::ok_empty()).unwrap();

        assert_eq!(value["code"], AsterErrorCode::Success.as_str());
        assert!(value.get("data").is_none());
        assert!(value.get("error").is_none());
    }

    #[test]
    fn error_helpers_keep_stable_public_code_and_retryability() {
        let plain = serde_json::to_value(ApiResponse::<()>::error(
            AsterErrorCode::BadRequest,
            "bad input",
        ))
        .unwrap();
        assert_eq!(plain["code"], AsterErrorCode::BadRequest.as_str());
        assert_eq!(plain["msg"], "bad input");
        assert!(plain.get("data").is_none());
        assert!(plain.get("error").is_none());

        let detailed = serde_json::to_value(ApiResponse::<()>::error_with_details(
            AsterErrorCode::DatabaseError,
            "database unavailable",
            Some(ApiErrorInfo {
                code: AsterErrorCode::DatabaseError,
                retryable: Some(true),
            }),
        ))
        .unwrap();
        assert_eq!(detailed["code"], AsterErrorCode::DatabaseError.as_str());
        assert_eq!(
            detailed["error"]["code"],
            AsterErrorCode::DatabaseError.as_str()
        );
        assert_eq!(detailed["error"]["retryable"], true);

        let body = serde_json::to_value(ApiResponse::<()>::error_body(
            AsterErrorCode::RateLimited,
            "retry later",
            Some(true),
        ))
        .unwrap();
        assert_eq!(body["code"], AsterErrorCode::RateLimited.as_str());
        assert_eq!(body["error"]["code"], AsterErrorCode::RateLimited.as_str());
        assert_eq!(body["error"]["retryable"], true);
    }

    #[actix_web::test]
    async fn into_response_uses_http_ok_with_json_envelope() {
        let response = ApiResponse::ok(serde_json::json!({ "value": 42 })).into_response();

        assert_eq!(response.status(), 200);
        let body = to_bytes(response.into_body()).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], AsterErrorCode::Success.as_str());
        assert_eq!(value["data"]["value"], 42);
    }
}
