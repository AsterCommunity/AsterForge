//! Integration tests for generic external auth placeholder routes.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::entities::{external_auth_login_flow, external_auth_provider};
use aster_forge::runtime::SharedRuntimeState;
use aster_forge::types::ExternalAuthKind;
use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::Value;

#[actix_web::test]
async fn external_auth_lists_enabled_providers() {
    let state = common::setup().await;
    external_auth_provider::ActiveModel {
        slug: Set("example".to_string()),
        display_name: Set("Example".to_string()),
        kind: Set(ExternalAuthKind::Oidc),
        enabled: Set(true),
        issuer_url: Set(Some("https://id.example.test".to_string())),
        authorize_url: Set(Some("https://id.example.test/authorize".to_string())),
        token_url: Set(Some("https://id.example.test/token".to_string())),
        userinfo_url: Set(Some("https://id.example.test/userinfo".to_string())),
        client_id: Set("client-id".to_string()),
        client_secret: Set("client-secret".to_string()),
        scopes: Set("openid email profile".to_string()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(state.db_handles.writer())
    .await
    .expect("external auth provider should insert");

    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/external-auth/providers")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"][0]["slug"], "example");
    assert_eq!(body["data"][0]["display_name"], "Example");
    assert_eq!(body["data"][0]["kind"], "oidc");
}

#[actix_web::test]
async fn external_auth_start_returns_authorize_url() {
    let state = common::setup().await;
    external_auth_provider::ActiveModel {
        slug: Set("example".to_string()),
        display_name: Set("Example".to_string()),
        kind: Set(ExternalAuthKind::Oauth2),
        enabled: Set(true),
        issuer_url: Set(None),
        authorize_url: Set(Some("https://id.example.test/authorize".to_string())),
        token_url: Set(Some("https://id.example.test/token".to_string())),
        userinfo_url: Set(Some("https://id.example.test/userinfo".to_string())),
        client_id: Set("client-id".to_string()),
        client_secret: Set("client-secret".to_string()),
        scopes: Set("email profile".to_string()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(state.db_handles.writer())
    .await
    .expect("external auth provider should insert");

    let app = create_test_app!(state);

    let req = test::TestRequest::post()
        .uri("/api/v1/external-auth/example/start")
        .set_json(serde_json::json!({
            "redirect_uri": "https://app.example.test/callback"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Value = test::read_body_json(resp).await;
    let authorize_url = body["data"]["authorize_url"]
        .as_str()
        .expect("authorize url should exist");
    assert!(authorize_url.starts_with("https://id.example.test/authorize?"));
    assert!(authorize_url.contains("client_id=client-id"));
    assert!(authorize_url.contains("state="));
}

#[actix_web::test]
async fn cleanup_expired_external_auth_flows_removes_only_expired_flows() {
    let state = common::setup().await;
    let now = Utc::now();
    let provider = external_auth_provider::ActiveModel {
        slug: Set("cleanup-provider".to_string()),
        display_name: Set("Cleanup Provider".to_string()),
        kind: Set(ExternalAuthKind::Oidc),
        enabled: Set(true),
        issuer_url: Set(Some("https://id.example.test".to_string())),
        authorize_url: Set(Some("https://id.example.test/authorize".to_string())),
        token_url: Set(Some("https://id.example.test/token".to_string())),
        userinfo_url: Set(Some("https://id.example.test/userinfo".to_string())),
        client_id: Set("client-id".to_string()),
        client_secret: Set("client-secret".to_string()),
        scopes: Set("openid email profile".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(state.writer_db())
    .await
    .expect("external auth provider should insert");

    let expired = external_auth_login_flow::ActiveModel {
        provider_id: Set(provider.id),
        state: Set("expired-flow-state".to_string()),
        redirect_uri: Set("https://app.example.test/callback".to_string()),
        expires_at: Set(now - Duration::minutes(1)),
        consumed_at: Set(None),
        created_at: Set(now - Duration::hours(1)),
        ..Default::default()
    }
    .insert(state.writer_db())
    .await
    .expect("expired external auth flow should insert");
    let active = external_auth_login_flow::ActiveModel {
        provider_id: Set(provider.id),
        state: Set("active-flow-state".to_string()),
        redirect_uri: Set("https://app.example.test/callback".to_string()),
        expires_at: Set(now + Duration::minutes(10)),
        consumed_at: Set(None),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(state.writer_db())
    .await
    .expect("active external auth flow should insert");

    let removed = aster_forge::services::external_auth_service::cleanup_expired_flows(&state)
        .await
        .expect("external auth flow cleanup should succeed");

    assert_eq!(removed, 1);
    let expired_after = external_auth_login_flow::Entity::find_by_id(expired.id)
        .one(state.reader_db())
        .await
        .expect("expired flow query should succeed");
    let active_after = external_auth_login_flow::Entity::find_by_id(active.id)
        .one(state.reader_db())
        .await
        .expect("active flow query should succeed");
    assert!(expired_after.is_none());
    assert!(active_after.is_some());
}
