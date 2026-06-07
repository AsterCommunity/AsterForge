//! Integration tests for system information routes.

#[macro_use]
mod common;

use actix_web::test;
use serde_json::Value;

#[actix_web::test]
async fn system_info_uses_runtime_branding_title() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/system/info")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["name"], "AsterForge");
    assert_eq!(body["data"]["site_title"], "AsterForge");
    assert!(
        body["data"]["version"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}
