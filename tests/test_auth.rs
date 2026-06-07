//! Integration tests for local auth routes.

#[macro_use]
mod common;

use actix_web::{cookie::SameSite, test};
use aster_forge::api::error_code::AsterErrorCode;
use aster_forge::db::repository::auth_session_repo;
use aster_forge::entities::auth_session;
use aster_forge::runtime::SharedRuntimeState;
use aster_forge::utils::hash::sha256_hex;
use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::Value;

#[actix_web::test]
async fn auth_setup_login_me_and_logout_flow() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/api/v1/auth/check")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["initialized"], false);

    let access_token = setup_admin!(app);

    let req = test::TestRequest::get()
        .uri("/api/v1/auth/check")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["initialized"], true);

    let login_token = login_user!(app, "admin", "password1234");
    assert!(!login_token.is_empty());

    let req = test::TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header(common::bearer_header(&access_token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["username"], "admin");
    assert_eq!(body["data"]["role"], "admin");
}

#[actix_web::test]
async fn auth_login_sets_http_only_session_cookies_without_token_body() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let _ = setup_admin!(app);

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/login")
        .peer_addr("127.0.0.1:12345".parse().unwrap())
        .set_json(serde_json::json!({
            "identifier": "admin",
            "password": "password1234"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let access = common::extract_cookie(&resp, "aster_access").expect("access cookie missing");
    let refresh = common::extract_cookie(&resp, "aster_refresh").expect("refresh cookie missing");
    let csrf = common::extract_cookie(&resp, "aster_csrf").expect("csrf cookie missing");
    assert!(!access.is_empty());
    assert!(!refresh.is_empty());
    assert!(!csrf.is_empty());

    let access_cookie = resp
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "aster_access")
        .expect("access cookie missing");
    assert_eq!(access_cookie.path(), Some("/"));
    assert_eq!(access_cookie.same_site(), Some(SameSite::Lax));
    assert_eq!(access_cookie.http_only(), Some(true));

    let refresh_cookie = resp
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "aster_refresh")
        .expect("refresh cookie missing");
    assert_eq!(refresh_cookie.path(), Some("/api/v1/auth"));
    assert_eq!(refresh_cookie.same_site(), Some(SameSite::Lax));
    assert_eq!(refresh_cookie.http_only(), Some(true));

    let csrf_cookie = resp
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "aster_csrf")
        .expect("csrf cookie missing");
    assert_eq!(csrf_cookie.path(), Some("/"));
    assert_eq!(csrf_cookie.same_site(), Some(SameSite::Lax));
    assert_ne!(csrf_cookie.http_only(), Some(true));

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::Success.as_str());
    assert!(body["data"]["expires_in"].is_number());
    assert!(body["data"]["access_token"].is_null());
    assert!(body["data"]["refresh_token"].is_null());
    assert!(body["data"]["user"].is_null());
}

#[actix_web::test]
async fn auth_cookie_session_can_read_me_refresh_and_logout() {
    let state = common::setup().await;
    let app = create_test_app!(state.clone());
    let _ = setup_admin!(app);

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/login")
        .peer_addr("127.0.0.1:12345".parse().unwrap())
        .set_json(serde_json::json!({
            "identifier": "admin",
            "password": "password1234"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let access = common::extract_cookie(&resp, "aster_access").expect("access cookie missing");
    let refresh = common::extract_cookie(&resp, "aster_refresh").expect("refresh cookie missing");
    let csrf = common::extract_cookie(&resp, "aster_csrf").expect("csrf cookie missing");

    let req = test::TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header(("Cookie", common::access_cookie_header(&access)))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["username"], "admin");
    assert_eq!(body["data"]["role"], "admin");

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/refresh")
        .insert_header(("Origin", "http://localhost:8080"))
        .insert_header(common::csrf_header(&csrf))
        .insert_header(("Cookie", common::refresh_cookie_header(&refresh, &csrf)))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let rotated_access =
        common::extract_cookie(&resp, "aster_access").expect("rotated access cookie missing");
    let rotated_refresh =
        common::extract_cookie(&resp, "aster_refresh").expect("rotated refresh cookie missing");
    let rotated_csrf =
        common::extract_cookie(&resp, "aster_csrf").expect("rotated csrf cookie missing");
    assert!(!rotated_access.is_empty());
    assert_ne!(rotated_refresh, refresh);
    assert_ne!(rotated_csrf, csrf);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::Success.as_str());
    assert!(body["data"]["expires_in"].is_number());
    assert!(body["data"]["access_token"].is_null());
    assert!(body["data"]["refresh_token"].is_null());

    let old_refresh_session = auth_session_repo::find_active_by_refresh_hash(
        state.reader_db(),
        &sha256_hex(refresh.as_bytes()),
    )
    .await
    .expect("old refresh session lookup should succeed");
    assert!(old_refresh_session.is_none());

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/logout")
        .insert_header(("Origin", "http://localhost:8080"))
        .insert_header(common::csrf_header(&rotated_csrf))
        .insert_header((
            "Cookie",
            common::access_and_refresh_cookie_header(
                &rotated_access,
                &rotated_refresh,
                &rotated_csrf,
            ),
        ))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        common::extract_cookie(&resp, "aster_access").as_deref(),
        Some("")
    );
    assert_eq!(
        common::extract_cookie(&resp, "aster_refresh").as_deref(),
        Some("")
    );
    assert_eq!(
        common::extract_cookie(&resp, "aster_csrf").as_deref(),
        Some("")
    );
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["revoked"], true);

    let revoked_refresh_session = auth_session_repo::find_active_by_refresh_hash(
        state.reader_db(),
        &sha256_hex(rotated_refresh.as_bytes()),
    )
    .await
    .expect("revoked refresh session lookup should succeed");
    assert!(revoked_refresh_session.is_none());

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/refresh")
        .insert_header(("Origin", "http://localhost:8080"))
        .insert_header(common::csrf_header(&rotated_csrf))
        .insert_header((
            "Cookie",
            common::refresh_cookie_header(&rotated_refresh, &rotated_csrf),
        ))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}

#[actix_web::test]
async fn auth_errors_use_stable_public_codes_without_internal_code() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let _ = setup_admin!(app);

    let req = test::TestRequest::post()
        .uri("/api/v1/auth/login")
        .set_json(serde_json::json!({
            "identifier": "admin",
            "password": "wrong-password"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], AsterErrorCode::AuthCredentialsFailed.as_str());
    assert_eq!(
        body["error"]["code"],
        AsterErrorCode::AuthCredentialsFailed.as_str()
    );
    assert!(body["internal_code"].is_null());
    assert!(body["error"]["internal_code"].is_null());
}

#[actix_web::test]
async fn cleanup_expired_auth_sessions_removes_only_expired_sessions() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let _ = setup_admin!(app);
    let now = Utc::now();

    let expired = auth_session::ActiveModel {
        user_id: Set(1),
        refresh_token_hash: Set("expired-refresh-hash".to_string()),
        session_version: Set(0),
        user_agent: Set(None),
        ip_address: Set(None),
        expires_at: Set(now - Duration::minutes(1)),
        revoked_at: Set(None),
        created_at: Set(now - Duration::hours(1)),
        ..Default::default()
    }
    .insert(state_for_insert.writer_db())
    .await
    .expect("expired auth session should insert");
    let active = auth_session::ActiveModel {
        user_id: Set(1),
        refresh_token_hash: Set("active-refresh-hash".to_string()),
        session_version: Set(0),
        user_agent: Set(None),
        ip_address: Set(None),
        expires_at: Set(now + Duration::hours(1)),
        revoked_at: Set(None),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(state_for_insert.writer_db())
    .await
    .expect("active auth session should insert");

    let removed =
        aster_forge::services::auth_service::cleanup_expired_auth_sessions(&state_for_insert)
            .await
            .expect("auth session cleanup should succeed");

    assert_eq!(removed, 1);
    let expired_after = auth_session::Entity::find_by_id(expired.id)
        .one(state_for_insert.reader_db())
        .await
        .expect("expired session query should succeed");
    let active_after = auth_session::Entity::find_by_id(active.id)
        .one(state_for_insert.reader_db())
        .await
        .expect("active session query should succeed");
    assert!(expired_after.is_none());
    assert!(active_after.is_some());
}
