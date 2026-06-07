//! Integration tests for administrator external auth routes.

#[macro_use]
mod common;

use actix_web::test;
use serde_json::Value;

#[actix_web::test]
async fn admin_external_auth_requires_authentication() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/external-auth/providers")
        .to_request();
    assert_service_status!(app, req, 401);
}

#[actix_web::test]
async fn admin_external_auth_crud_redacts_secret_and_exposes_public_aliases() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);

    let req = test::TestRequest::post()
        .uri("/api/v1/admin/external-auth/providers")
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "key": "example",
            "kind": "oidc",
            "display_name": "Example IdP",
            "issuer_url": "https://id.example.test",
            "authorization_url": "https://id.example.test/authorize",
            "token_url": "https://id.example.test/token",
            "userinfo_url": "https://id.example.test/userinfo",
            "client_id": "client-id",
            "client_secret": "client-secret",
            "scopes": "openid email profile",
            "enabled": true
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 201);
    let body: Value = test::read_body_json(resp).await;
    let provider_id = body["data"]["id"]
        .as_i64()
        .expect("provider id should be returned");
    assert_eq!(body["data"]["key"], "example");
    assert_eq!(body["data"]["slug"], "example");
    assert_eq!(body["data"]["kind"], "oidc");
    assert_eq!(body["data"]["provider_kind"], "oidc");
    assert_eq!(body["data"]["client_secret"], "***REDACTED***");
    assert_eq!(body["data"]["client_secret_configured"], true);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/external-auth/provider-kinds")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["data"]
            .as_array()
            .expect("provider kind list should be an array")
            .iter()
            .any(|kind| kind["kind"] == "oidc")
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/external-auth/providers")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["items"][0]["key"], "example");

    let req = test::TestRequest::patch()
        .uri(&format!(
            "/api/v1/admin/external-auth/providers/{provider_id}"
        ))
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "display_name": "Example Login"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["display_name"], "Example Login");

    let req = test::TestRequest::post()
        .uri(&format!(
            "/api/v1/admin/external-auth/providers/{provider_id}/test"
        ))
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["provider"], "oidc");
    assert!(
        body["data"]["checks"]
            .as_array()
            .expect("checks should be an array")
            .iter()
            .all(|check| check["success"].as_bool().unwrap_or(false))
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/external-auth/providers")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"][0]["slug"], "example");

    let req = test::TestRequest::get()
        .uri("/api/v1/auth/external-auth/oidc/providers")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"][0]["key"], "example");

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/external-auth/oidc/example/start")
        .set_json(serde_json::json!({
            "redirect_uri": "https://app.example.test/callback"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    let authorize_url = body["data"]["authorization_url"]
        .as_str()
        .expect("authorization URL should be returned");
    assert!(authorize_url.starts_with("https://id.example.test/authorize?"));
    assert!(authorize_url.contains("client_id=client-id"));
    assert!(authorize_url.contains("state="));
}
