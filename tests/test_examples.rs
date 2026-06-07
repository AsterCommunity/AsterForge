//! Integration tests for template example routes.

#[macro_use]
mod common;

use actix_web::test;
use serde_json::Value;

#[actix_web::test]
async fn public_example_route_works() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/examples/public")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(
        body["data"]["message"],
        "AsterForge public example API is working"
    );
}

#[actix_web::test]
async fn protected_example_route_uses_bearer_auth() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);

    let req = test::TestRequest::get()
        .uri("/api/v1/examples/protected")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(
        body["data"]["message"],
        "AsterForge protected example API is working"
    );
    assert_eq!(body["data"]["user"]["username"], "admin");
}
