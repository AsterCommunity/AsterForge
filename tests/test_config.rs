//! Integration tests for administrator config routes.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::config::definitions::BRANDING_TITLE_KEY;
use serde_json::Value;

#[actix_web::test]
async fn admin_config_requires_authentication() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/config")
        .to_request();
    assert_service_status!(app, req, 401);
}

#[actix_web::test]
async fn admin_config_lists_schema_and_updates_runtime_value() {
    let state = common::setup().await;
    let state_for_assert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/config")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["data"]["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/config/schema")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["data"]
            .as_array()
            .expect("schema should be an array")
            .iter()
            .any(|item| item["key"] == BRANDING_TITLE_KEY)
    );

    let req = test::TestRequest::put()
        .uri(&format!("/api/v1/admin/config/{BRANDING_TITLE_KEY}"))
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "value": "Template Title"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["key"], BRANDING_TITLE_KEY);
    assert_eq!(body["data"]["value"], "Template Title");

    assert_eq!(
        state_for_assert
            .runtime_config
            .get(BRANDING_TITLE_KEY)
            .as_deref(),
        Some("Template Title")
    );
}
