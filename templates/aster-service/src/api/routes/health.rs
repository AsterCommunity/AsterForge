//! Health API routes.

use actix_web::{HttpResponse, Scope, web};

use crate::api::response::{HealthResponse, ReadinessComponent, ReadinessResponse};

/// Returns the health route scope.
pub fn routes() -> Scope {
    let scope = web::scope("")
        .route("/healthz", web::get().to(healthz))
        .route("/readyz", web::get().to(readyz));

    #[cfg(feature = "metrics")]
    let scope = scope.route("/metrics", web::get().to(metrics));

    scope
}

#[aster_forge_api_docs_macros::path(
    get,
    path = "/healthz",
    tag = "health",
    responses(
        (status = 200, description = "Service health status", body = HealthResponse)
    )
)]
pub async fn healthz(state: web::Data<crate::runtime::AppState>) -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse {
        cache_backend: state.cache_backend_name(),
        config_sync_enabled: state.config_sync_enabled(),
        runtime_id: state.runtime_id().to_string(),
        service: env!("CARGO_PKG_NAME"),
        status: "ok",
    })
}

#[aster_forge_api_docs_macros::path(
    get,
    path = "/readyz",
    tag = "health",
    responses(
        (status = 200, description = "Service is ready", body = ReadinessResponse),
        (status = 503, description = "Service dependency is not ready", body = ReadinessResponse)
    )
)]
pub async fn readyz(state: web::Data<crate::runtime::AppState>) -> HttpResponse {
    let components = vec![
        check_database(state.get_ref()).await,
        check_cache(state.get_ref()).await,
    ];
    let ready = components
        .iter()
        .all(|component| component.status == "healthy");
    let response = ReadinessResponse {
        components,
        service: env!("CARGO_PKG_NAME"),
        status: if ready { "ready" } else { "not_ready" },
    };

    if ready {
        HttpResponse::Ok().json(response)
    } else {
        HttpResponse::ServiceUnavailable().json(response)
    }
}

async fn check_database(state: &crate::runtime::AppState) -> ReadinessComponent {
    match aster_forge_db::ping_database(state.db_handles.reader()).await {
        Ok(()) => ReadinessComponent {
            name: "database",
            status: "healthy",
            message: "database ping succeeded".to_string(),
        },
        Err(error) => ReadinessComponent {
            name: "database",
            status: "unhealthy",
            message: format!("database ping failed: {error}"),
        },
    }
}

async fn check_cache(state: &crate::runtime::AppState) -> ReadinessComponent {
    match state.cache.health_check().await {
        Ok(()) => ReadinessComponent {
            name: "cache",
            status: "healthy",
            message: "cache health check succeeded".to_string(),
        },
        Err(error) => ReadinessComponent {
            name: "cache",
            status: "unhealthy",
            message: format!("cache health check failed: {error}"),
        },
    }
}

#[cfg(feature = "metrics")]
async fn metrics() -> HttpResponse {
    match crate::metrics::export_metrics() {
        Ok(body) => HttpResponse::Ok()
            .content_type("text/plain; version=0.0.4; charset=utf-8")
            .body(body),
        Err(error) => {
            tracing::debug!(error = %error, "metrics export failed");
            HttpResponse::ServiceUnavailable().body(error)
        }
    }
}
