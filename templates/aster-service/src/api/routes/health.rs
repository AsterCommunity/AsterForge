//! Health API routes.

use actix_web::{HttpResponse, Scope, web};
use aster_forge_runtime::{HealthComponentReport, SystemHealthReport};

use crate::api::response::{HealthResponse, ReadinessComponent, ReadinessResponse};

/// Returns the health route scope.
pub fn routes() -> Scope {
    let scope = web::scope("")
        .route("/healthz", web::get().to(healthz))
        .route("/readyz", web::get().to(readyz));

    crate::metrics::configure_route(scope)
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
    let started = std::time::Instant::now();
    let component_reports = vec![
        check_database(state.get_ref()).await,
        check_cache(state.get_ref()).await,
    ];
    let report = SystemHealthReport::with_duration(component_reports, started.elapsed());
    crate::metrics::record_health_report(aster_forge_runtime::HealthCheckScope::Readiness, &report);

    let ready = report
        .components
        .iter()
        .all(|component| !component.status.is_issue());
    let response = ReadinessResponse {
        components: report
            .components
            .iter()
            .map(readiness_component_response)
            .collect(),
        service: env!("CARGO_PKG_NAME"),
        status: if ready { "ready" } else { "not_ready" },
    };

    if ready {
        HttpResponse::Ok().json(response)
    } else {
        HttpResponse::ServiceUnavailable().json(response)
    }
}

async fn check_database(state: &crate::runtime::AppState) -> HealthComponentReport {
    match aster_forge_db::ping_database(state.db_handles.reader()).await {
        Ok(()) => HealthComponentReport::healthy("database", "database ping succeeded"),
        Err(error) => {
            HealthComponentReport::unhealthy("database", format!("database ping failed: {error}"))
        }
    }
}

async fn check_cache(state: &crate::runtime::AppState) -> HealthComponentReport {
    match state.cache.health_check().await {
        Ok(()) => HealthComponentReport::healthy("cache", "cache health check succeeded"),
        Err(error) => {
            HealthComponentReport::unhealthy("cache", format!("cache health check failed: {error}"))
        }
    }
}

fn readiness_component_response(component: &HealthComponentReport) -> ReadinessComponent {
    ReadinessComponent {
        name: component.name,
        status: component.status.as_str(),
        message: component.message.clone(),
    }
}
