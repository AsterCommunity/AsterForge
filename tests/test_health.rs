//! Integration tests for health routes.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::api::error_code::AsterErrorCode;
use aster_forge::runtime::SharedRuntimeState;
use serde_json::Value;

#[actix_web::test]
async fn health_returns_ok() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::Success.as_str());
    assert_eq!(body["data"]["status"], "ok");
    assert_eq!(body["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["data"]["build_time"].is_string());
}

#[actix_web::test]
async fn ready_checks_database() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get().uri("/health/ready").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::Success.as_str());
    assert_eq!(body["data"]["status"], "ready");
    assert_eq!(body["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["data"]["build_time"].is_string());
}

#[actix_web::test]
async fn ready_redacts_database_error() {
    let state = common::setup().await;
    let db = state.writer_db().clone();
    let app = create_test_app!(state);

    db.close_by_ref().await.unwrap();

    let req = test::TestRequest::get().uri("/health/ready").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 503);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::DatabaseError.as_str());
    assert_eq!(body["msg"], "Database unavailable");
    assert_eq!(
        body["error"]["code"],
        AsterErrorCode::DatabaseError.as_str()
    );
    assert_eq!(body["error"]["retryable"], true);
    assert!(body["internal_code"].is_null());
    assert!(body["error"]["internal_code"].is_null());
}
