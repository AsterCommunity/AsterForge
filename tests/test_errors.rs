//! Integration tests for API error responses.

use actix_web::{ResponseError, body::to_bytes, http::StatusCode};
use aster_forge::api::error_code::AsterErrorCode;
use aster_forge::errors::AsterError;
use serde_json::Value;

async fn response_body_json(resp: actix_web::HttpResponse) -> Value {
    let body = to_bytes(resp.into_body()).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[actix_web::test]
async fn internal_error_uses_stable_public_code_without_internal_code() {
    let err = AsterError::internal_error("db pool poisoned");

    let resp = err.error_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = response_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::InternalServerError.as_str());
    assert_eq!(
        body["error"]["code"],
        AsterErrorCode::InternalServerError.as_str()
    );
    assert!(body["internal_code"].is_null());
    assert!(body["error"]["internal_code"].is_null());
}

#[actix_web::test]
async fn database_error_marks_retryable() {
    let err = AsterError::database_operation("connection dropped");

    let resp = err.error_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = response_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::DatabaseError.as_str());
    assert_eq!(
        body["error"]["code"],
        AsterErrorCode::DatabaseError.as_str()
    );
    assert_eq!(body["error"]["retryable"], true);
}

#[actix_web::test]
async fn validation_error_keeps_message_and_uses_bad_request_code() {
    let err = AsterError::validation_error("value cannot be empty");

    let resp = err.error_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let body = response_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::BadRequest.as_str());
    assert_eq!(body["msg"], "value cannot be empty");
    assert_eq!(body["error"]["code"], AsterErrorCode::BadRequest.as_str());
    assert!(body["error"]["retryable"].is_null());
}

#[actix_web::test]
async fn auth_token_invalid_uses_auth_token_invalid_code() {
    let err = AsterError::auth_token_invalid("invalid token");

    let resp = err.error_response();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let body = response_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::AuthTokenInvalid.as_str());
    assert_eq!(
        body["error"]["code"],
        AsterErrorCode::AuthTokenInvalid.as_str()
    );
    assert!(body["internal_code"].is_null());
}
