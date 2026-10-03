//! Wire contracts exercised through both transport stacks.
use actix_web::{App, HttpMessage, HttpRequest, HttpResponse, test, web};
use axum::response::IntoResponse;
use axum::{
    Extension, Router,
    body::{Body, to_bytes},
    middleware,
    routing::get,
};
use http::Request;
use tower::ServiceExt;

#[actix_web::test]
async fn both_transports_replace_inbound_request_ids_with_server_generated_uuid_v4() {
    let actix = test::init_service(
        App::new()
            .wrap(crate::actix::request_id::RequestIdMiddleware)
            .route(
                "/",
                web::get().to(|req: HttpRequest| async move {
                    let id = req
                        .extensions()
                        .get::<crate::actix::request_id::RequestId>()
                        .unwrap()
                        .0
                        .clone();
                    HttpResponse::BadRequest().body(id)
                }),
            ),
    )
    .await;
    let axum = Router::new()
        .route(
            "/",
            get(
                |Extension(id): Extension<crate::axum::request_id::RequestId>| async move {
                    (http::StatusCode::BAD_REQUEST, id.0)
                },
            ),
        )
        .layer(middleware::from_fn(crate::axum::request_id::request_id));
    for inbound in ["untrusted", "11111111-1111-4111-8111-111111111111"] {
        let response = test::call_service(
            &actix,
            test::TestRequest::get()
                .insert_header(("x-request-id", inbound))
                .to_request(),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400);
        let id = response
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
        assert_ne!(id, inbound);
        assert_eq!(test::read_body(response).await.as_ref(), id.as_bytes());
        let response = axum
            .clone()
            .oneshot(
                Request::builder()
                    .header("x-request-id", inbound)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
        assert_ne!(id, inbound);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            id.as_bytes()
        );
    }
}

#[actix_web::test]
async fn both_transports_preserve_product_security_policy_and_share_defaults() {
    let actix = test::init_service(
        App::new()
            .wrap(crate::actix::security_headers::default_headers())
            .route("/", web::get().to(HttpResponse::Ok))
            .route(
                "/strict",
                web::get().to(|| async {
                    HttpResponse::BadRequest()
                        .insert_header(("referrer-policy", "no-referrer"))
                        .finish()
                }),
            ),
    )
    .await;
    let axum = Router::new()
        .route("/", get(|| async { "called" }))
        .route(
            "/strict",
            get(|| async {
                let mut response = http::StatusCode::BAD_REQUEST.into_response();
                response.headers_mut().insert(
                    "referrer-policy",
                    http::HeaderValue::from_static("no-referrer"),
                );
                response
            }),
        )
        .layer(middleware::from_fn(
            crate::axum::security_headers::security_headers,
        ));
    for path in ["/", "/strict"] {
        let left =
            test::call_service(&actix, test::TestRequest::get().uri(path).to_request()).await;
        let right = axum
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(left.status().as_u16(), right.status().as_u16());
        for name in [
            "referrer-policy",
            "x-frame-options",
            "x-content-type-options",
        ] {
            assert_eq!(
                left.headers().get(name).unwrap().as_bytes(),
                right.headers()[name].as_bytes()
            );
        }
    }
}

#[actix_web::test]
async fn cors_wire_status_and_headers_match_on_actual_preflight_and_denial() {
    use crate::shared::cors::{CorsAllowedOrigins, RuntimeCorsPolicy};
    let policy = || RuntimeCorsPolicy {
        enabled: true,
        allowed_origins: CorsAllowedOrigins::List(vec!["https://panel.example.com".into()]),
        allow_credentials: true,
        max_age_secs: 600,
    };
    let left_config = crate::actix::cors::RuntimeCorsConfig::new(
        move |_| Ok(policy()),
        |_| false,
        |error| actix_web::error::ErrorBadRequest(error.to_string()),
    )
    .allowed_methods(["GET", "POST"])
    .allowed_headers(["X-CSRF-Token"])
    .exposed_headers(["x-request-id"]);
    let right_config = crate::axum::cors::RuntimeCorsConfig::new(
        move |_| Ok(policy()),
        |_| false,
        |_| http::StatusCode::BAD_REQUEST.into_response(),
    )
    .allowed_methods(["GET", "POST"])
    .allowed_headers(["X-CSRF-Token"])
    .exposed_headers(["x-request-id"]);
    let actix = test::init_service(
        App::new()
            .wrap(crate::actix::cors::RuntimeCors::new(left_config))
            .route("/", web::get().to(HttpResponse::Ok)),
    )
    .await;
    let axum =
        Router::new()
            .route("/", get(|| async { "called" }))
            .layer(middleware::from_fn_with_state(
                right_config,
                crate::axum::cors::runtime_cors,
            ));
    for (method, origin) in [
        ("GET", "https://panel.example.com"),
        ("OPTIONS", "https://panel.example.com"),
        ("GET", "https://evil.example.com"),
    ] {
        let mut left = test::TestRequest::default()
            .method(method.parse().unwrap())
            .insert_header(("origin", origin));
        let mut right = Request::builder().method(method).header("origin", origin);
        if method == "OPTIONS" {
            left = left
                .insert_header(("access-control-request-method", "POST"))
                .insert_header(("access-control-request-headers", "x-csrf-token"));
            right = right
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "x-csrf-token");
        }
        let left = test::call_service(&actix, left.to_request()).await;
        let right = axum
            .clone()
            .oneshot(right.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(left.status().as_u16(), right.status().as_u16());
        for name in [
            "access-control-allow-origin",
            "access-control-allow-credentials",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "access-control-expose-headers",
            "access-control-max-age",
            "vary",
        ] {
            assert_eq!(
                left.headers()
                    .get(name)
                    .map(actix_web::http::header::HeaderValue::as_bytes),
                right.headers().get(name).map(http::HeaderValue::as_bytes)
            );
        }
    }
}
