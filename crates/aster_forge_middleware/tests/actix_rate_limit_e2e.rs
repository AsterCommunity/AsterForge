//! Real HTTP tests for Actix middleware composition around rate-limit responses.
#![cfg(feature = "actix")]

use actix_web::{App, HttpResponse, HttpServer, web};
use aster_forge_middleware::actix::{
    cors::{CorsAllowedOrigins, RuntimeCors, RuntimeCorsConfig, RuntimeCorsPolicy},
    rate_limit::{RateLimit, RateLimiterConfig, TrustedProxyIpKeyExtractor},
    request_id::RequestIdMiddleware,
    security_headers::default_headers,
};
use governor::clock::FakeRelativeClock;
use std::num::{NonZeroU32, NonZeroU64};

#[actix_web::test]
async fn rate_limit_rejection_keeps_outer_http_middleware_headers() {
    initialize_tls();

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();

    let clock = FakeRelativeClock::default();
    let limit = RateLimiterConfig::with_clock(
        aster_forge_middleware::shared::rate_limit::rate_limit_quota(
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        ),
        TrustedProxyIpKeyExtractor::new(&[]).with_rejection_response(
            move |retry_after, mut response| {
                response
                    .insert_header(("Retry-After", retry_after.to_string()))
                    .body(format!("retry in {retry_after}s"))
            },
        ),
        clock,
    );
    let server = HttpServer::new(move || {
        let cors = RuntimeCorsConfig::new(
            |_| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::List(vec![
                        "https://client.example".to_owned(),
                    ]),
                    allow_credentials: false,
                    max_age_secs: 60,
                })
            },
            |_| false,
            |error| actix_web::error::ErrorInternalServerError(error.to_string()),
        )
        .exposed_headers(["Retry-After", "X-Request-ID"]);
        App::new()
            .wrap(RateLimit::new(&limit))
            .wrap(RuntimeCors::new(cors))
            .wrap(default_headers())
            .wrap(RequestIdMiddleware)
            .route(
                "/limited",
                web::get().to(|| async { HttpResponse::Ok().body("ok") }),
            )
    })
    .workers(1)
    .listen(listener)
    .unwrap()
    .run();
    let server_handle = server.handle();
    let server_task = actix_web::rt::spawn(server);

    let client = aster_forge_http::HttpClient::builder()
        .no_proxy()
        .build()
        .unwrap();
    let url = format!("http://{address}/limited");
    let first = client
        .get(&url)
        .header("Origin", "https://client.example")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), http::StatusCode::OK);
    assert!(first.headers().contains_key("x-request-id"));
    assert!(first.headers().contains_key("access-control-allow-origin"));

    let rejected = client
        .get(&url)
        .header("Origin", "https://client.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        rejected.headers()["access-control-allow-origin"],
        "https://client.example"
    );
    assert!(rejected.headers().contains_key("x-request-id"));
    assert_eq!(
        rejected.headers()["access-control-expose-headers"],
        "Retry-After, X-Request-ID"
    );
    assert_eq!(rejected.headers()["x-content-type-options"], "nosniff");
    let retry_after = rejected.headers()["retry-after"].to_str().unwrap();
    let retry_after_seconds = retry_after.parse::<u64>().unwrap();
    assert!(retry_after_seconds > 0);
    assert!(
        rejected
            .text()
            .await
            .unwrap()
            .contains(&format!("retry in {retry_after_seconds}s"))
    );

    server_handle.stop(true).await;
    server_task.await.unwrap().unwrap();
}

#[actix_web::test]
async fn rate_limit_wraps_body_transforming_cors_middleware() {
    let limit = RateLimiterConfig::new(
        aster_forge_middleware::shared::rate_limit::rate_limit_quota(
            NonZeroU64::new(60).unwrap(),
            NonZeroU32::new(1).unwrap(),
        ),
        TrustedProxyIpKeyExtractor::new(&[]),
    );
    let cors = RuntimeCorsConfig::new(
        |_| {
            Ok(RuntimeCorsPolicy {
                enabled: true,
                allowed_origins: CorsAllowedOrigins::List(vec![
                    "https://client.example".to_owned(),
                ]),
                allow_credentials: false,
                max_age_secs: 60,
            })
        },
        |_| false,
        |error| actix_web::error::ErrorInternalServerError(error.to_string()),
    );
    let app = actix_web::test::init_service(
        App::new()
            .wrap(RuntimeCors::new(cors))
            .wrap(aster_forge_middleware::actix::rate_limit::RateLimit::new(
                &limit,
            ))
            .route("/", web::get().to(|| async { HttpResponse::Ok().finish() })),
    )
    .await;

    let response = actix_web::test::call_service(
        &app,
        actix_web::test::TestRequest::get().uri("/").to_request(),
    )
    .await;
    assert_eq!(response.status(), actix_web::http::StatusCode::OK);
}

fn initialize_tls() {
    let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
    let _ = provider.install_default();
}
