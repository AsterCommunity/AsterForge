//! Run with --no-default-features --features axum (optionally add metrics).
use aster_forge_middleware::axum::{
    cors::{
        CorsAllowedOrigins, CorsMiddlewareErrorKind, RuntimeCorsConfig, RuntimeCorsPolicy,
        runtime_cors,
    },
    csrf::{CsrfConfig, CsrfTokenNames, RequestSourceMode, RequestSourcePolicy, csrf},
    rate_limit::{IpRateLimitConfig, ip_rate_limit},
    request_id, security_headers,
};
use axum::{Router, middleware, response::IntoResponse, routing::get};
use http::{HeaderValue, StatusCode};
use std::{
    net::SocketAddr,
    num::{NonZeroU32, NonZeroU64},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = RequestSourcePolicy {
        scheme: "http".into(),
        host: "127.0.0.1:3000".into(),
        public_site_origins: vec!["http://localhost:5173".into()],
        mode: RequestSourceMode::Required,
    };
    let csrf_config = CsrfConfig::new(
        CsrfTokenNames::new("example_csrf", "X-Example-CSRF")?,
        |_| true,
        move |_| Ok(source.clone()),
        |error| (StatusCode::FORBIDDEN, format!("CSRF: {:?}", error.kind())).into_response(),
    );
    let cors_config = RuntimeCorsConfig::new(
        |_| {
            Ok(RuntimeCorsPolicy {
                enabled: true,
                allowed_origins: CorsAllowedOrigins::List(vec!["http://localhost:5173".into()]),
                allow_credentials: true,
                max_age_secs: 600,
            })
        },
        |_| false,
        |error| {
            let status = match error.kind() {
                CorsMiddlewareErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
                CorsMiddlewareErrorKind::InvalidResponse => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, format!("CORS: {:?}", error.kind())).into_response()
        },
    )
    .allowed_methods(["GET", "POST", "OPTIONS"])
    .allowed_headers(["content-type", "x-example-csrf"])
    .exposed_headers(["x-request-id"])
    .request_origin(|_| Ok(Some("http://127.0.0.1:3000".into())));
    let ip_config = IpRateLimitConfig::new(
        true,
        NonZeroU64::new(1).ok_or("invalid quota period")?,
        NonZeroU32::new(20).ok_or("invalid burst")?,
        &[],
        |rejection| {
            let wait = rejection.retry_after_seconds();
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
            response
        },
    );
    let router = Router::new()
        .route(
            "/api/demo",
            get(|| async { "ok" }).post(|| async { "updated" }),
        )
        .route_layer(middleware::from_fn_with_state(csrf_config, csrf))
        .route("/health", get(|| async { "healthy" }))
        .layer(middleware::from_fn_with_state(ip_config, ip_rate_limit))
        .layer(middleware::from_fn_with_state(cors_config, runtime_cors))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(request_id));
    #[cfg(feature = "metrics")]
    let router = router
        .layer(middleware::from_fn(aster_forge_middleware::axum::metrics))
        .layer(axum::Extension(aster_forge_metrics::NoopMetrics::arc()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
