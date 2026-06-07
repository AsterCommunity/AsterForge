//! Health and readiness routes.

use crate::api::error_code::AsterErrorCode;
use crate::api::response::{ApiResponse, HealthResponse};
use crate::runtime::AppState;
use crate::services::health_service;
use actix_web::{HttpResponse, web};

const READY_DB_UNAVAILABLE_MESSAGE: &str = "Database unavailable";

pub fn routes() -> actix_web::Scope {
    let scope = web::scope("/health")
        .route("", web::get().to(health))
        .route("", web::head().to(health))
        .route("/ready", web::get().to(ready))
        .route("/ready", web::head().to(ready));

    #[cfg(feature = "metrics")]
    let scope = scope.route("/metrics", web::get().to(metrics));

    scope
}

#[api_docs_macros::path(
    get,
    path = "/health",
    tag = "health",
    operation_id = "health",
    responses(
        (status = 200, description = "Service is healthy", body = inline(ApiResponse<HealthResponse>)),
    ),
)]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(ApiResponse::ok(status_response("ok")))
}

#[api_docs_macros::path(
    get,
    path = "/health/ready",
    tag = "health",
    operation_id = "ready",
    responses(
        (status = 200, description = "Service is ready", body = inline(ApiResponse<HealthResponse>)),
        (status = 503, description = "Database is unavailable"),
    ),
)]
pub async fn ready(state: web::Data<AppState>) -> HttpResponse {
    match health_service::check_ready(state.get_ref()).await {
        Ok(_) => HttpResponse::Ok().json(ApiResponse::ok(status_response("ready"))),
        Err(error) => ready_database_error(error),
    }
}

fn ready_database_error(error: crate::errors::AsterError) -> HttpResponse {
    tracing::error!(error = %error, "health readiness database ping failed");
    HttpResponse::ServiceUnavailable().json(ApiResponse::<()>::error_body(
        AsterErrorCode::DatabaseError,
        READY_DB_UNAVAILABLE_MESSAGE,
        Some(true),
    ))
}

#[inline]
fn compile_time() -> &'static str {
    option_env!("ASTER_BUILD_TIME").unwrap_or("unknown")
}

fn status_response(status: &str) -> HealthResponse {
    HealthResponse {
        status: status.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_time: compile_time().to_string(),
    }
}

#[cfg(feature = "metrics")]
async fn metrics() -> HttpResponse {
    let Some(metrics) = crate::metrics::get_metrics() else {
        return HttpResponse::ServiceUnavailable().body("metrics not initialized");
    };

    match metrics.export() {
        Ok(body) => HttpResponse::Ok()
            .content_type("text/plain; version=0.0.4; charset=utf-8")
            .body(body),
        Err(error) => HttpResponse::InternalServerError().body(error),
    }
}
