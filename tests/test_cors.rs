//! Integration tests for runtime CORS middleware.

#[macro_use]
mod common;

use actix_web::{http::header, test};
use serde_json::Value;

const EXPECTED_ALLOW_HEADERS: &str =
    "authorization, accept, content-type, range, timeout, x-csrf-token, x-request-id";
const EXPECTED_EXPOSE_HEADERS: &str = "content-length, etag, x-request-id";

macro_rules! set_config {
    ($app:expr, $token:expr, $key:expr, $value:expr $(,)?) => {{
        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/admin/config/{}", $key))
            .insert_header(common::bearer_header(&$token))
            .set_json(serde_json::json!({ "value": $value }))
            .to_request();
        test::call_service(&$app, req).await
    }};
}

macro_rules! enable_cors {
    ($app:expr, $token:expr $(,)?) => {{
        let resp = set_config!($app, $token, "cors_enabled", "true");
        assert_eq!(resp.status(), 200);
    }};
}

fn header_contains<B>(
    resp: &actix_web::dev::ServiceResponse<B>,
    name: header::HeaderName,
    value: &str,
) {
    let actual = resp
        .headers()
        .get(name)
        .unwrap()
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        actual.contains(&value.to_ascii_lowercase()),
        "expected header to contain '{value}', got '{actual}'"
    );
}

#[actix_web::test]
async fn runtime_cors_defaults_passthrough_cross_origin_actual_request() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/health")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[actix_web::test]
async fn runtime_cors_same_origin_origin_header_is_not_blocked() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let req = test::TestRequest::get()
        .uri("/health")
        .insert_header((header::HOST, "localhost:8080"))
        .insert_header((header::ORIGIN, "http://localhost:8080"))
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[actix_web::test]
async fn runtime_cors_hot_reload_updates_whitelist_and_max_age() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    enable_cors!(app, token);

    let resp = set_config!(
        app,
        token,
        "cors_allowed_origins",
        "https://app.example.com/",
    );
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["value"], "https://app.example.com");

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        "https://app.example.com"
    );
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_MAX_AGE)
            .unwrap()
            .to_str()
            .unwrap(),
        "3600"
    );

    let resp = set_config!(
        app,
        token,
        "cors_allowed_origins",
        "https://dashboard.example.com",
    );
    assert_eq!(resp.status(), 200);

    let resp = set_config!(app, token, "cors_max_age_secs", "600");
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 403);

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://dashboard.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        "https://dashboard.example.com"
    );
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_MAX_AGE)
            .unwrap()
            .to_str()
            .unwrap(),
        "600"
    );
}

#[actix_web::test]
async fn runtime_cors_credentials_require_explicit_origin_list() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);

    let resp = set_config!(app, token, "cors_allowed_origins", "*");
    assert_eq!(resp.status(), 200);

    let resp = set_config!(app, token, "cors_allow_credentials", "true");
    assert_eq!(resp.status(), 400);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["msg"]
            .as_str()
            .unwrap()
            .contains("cors_allow_credentials cannot be true")
    );
}

#[actix_web::test]
async fn runtime_cors_adds_credentials_header_for_allowed_origin() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    enable_cors!(app, token);

    let resp = set_config!(
        app,
        token,
        "cors_allowed_origins",
        "https://panel.example.com",
    );
    assert_eq!(resp.status(), 200);

    let resp = set_config!(app, token, "cors_allow_credentials", "true");
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/api/v1/examples/public")
        .insert_header((header::ORIGIN, "https://panel.example.com"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        "https://panel.example.com"
    );
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .unwrap()
            .to_str()
            .unwrap(),
        "true"
    );
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .unwrap()
            .to_str()
            .unwrap(),
        EXPECTED_EXPOSE_HEADERS
    );
}

#[actix_web::test]
async fn runtime_cors_wildcard_allows_any_origin_without_credentials() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    enable_cors!(app, token);

    let resp = set_config!(app, token, "cors_allowed_origins", "*");
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://random.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
    assert!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none()
    );
}

#[actix_web::test]
async fn runtime_cors_rejects_unknown_requested_header() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    enable_cors!(app, token);

    let resp = set_config!(
        app,
        token,
        "cors_allowed_origins",
        "https://panel.example.com",
    );
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://panel.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_HEADERS, "x-not-allowed"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 403);
}

#[actix_web::test]
async fn runtime_cors_preflight_lists_expected_methods_and_headers() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    enable_cors!(app, token);

    let resp = set_config!(
        app,
        token,
        "cors_allowed_origins",
        "https://panel.example.com",
    );
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri("/health")
        .insert_header((header::ORIGIN, "https://panel.example.com"))
        .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH"))
        .insert_header((
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization, x-request-id",
        ))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
    header_contains(&resp, header::ACCESS_CONTROL_ALLOW_METHODS, "PATCH");
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .unwrap()
            .to_str()
            .unwrap(),
        EXPECTED_ALLOW_HEADERS
    );
}
