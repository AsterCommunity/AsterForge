//! HTTP runtime component construction.
//!
//! The HTTP server is owned by the API layer because it assembles Actix middleware, shared app
//! data, and route registration. `runtime::assembly` only registers this component into the Forge
//! runtime graph.

use std::io;

use actix_web::{App, HttpServer, middleware, web};
use aster_forge_runtime::{
    RuntimeComponentKind, RuntimeServiceComponent, TryRuntimeComponentWithShutdown,
};
use tokio_util::sync::CancellationToken;

const HTTP_SHUTDOWN_TIMEOUT_SECS: u64 = 8;

/// Configuration needed to build the HTTP service component.
pub struct HttpRuntimeConfig<'a> {
    /// Bind host.
    pub host: &'a str,
    /// Bind port.
    pub port: u16,
    /// Actix worker count. `0` keeps Actix's default worker count.
    pub workers: usize,
}

/// Builds the HTTP runtime component used by the product entrypoint.
pub fn http_component(
    config: HttpRuntimeConfig<'_>,
    state: web::Data<crate::runtime::AppState>,
    metrics_data: web::Data<dyn aster_forge_metrics::MetricsRecorder>,
) -> TryRuntimeComponentWithShutdown<
    RuntimeServiceComponent<actix_web::dev::Server>,
    impl FnOnce(CancellationToken) -> io::Result<RuntimeServiceComponent<actix_web::dev::Server>>,
    io::Error,
> {
    aster_forge_runtime::try_runtime_component_with_shutdown(move |shutdown_token| {
        build_http_service_component(config, state, shutdown_token, metrics_data)
    })
}

fn build_http_service_component(
    config: HttpRuntimeConfig<'_>,
    state: web::Data<crate::runtime::AppState>,
    shutdown_token: CancellationToken,
    metrics_data: web::Data<dyn aster_forge_metrics::MetricsRecorder>,
) -> io::Result<RuntimeServiceComponent<actix_web::dev::Server>> {
    let shutdown_data = web::Data::new(shutdown_token.clone());
    let mut server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .wrap(middleware::Logger::default())
            .wrap(aster_forge_actix_middleware::request_id::RequestIdMiddleware)
            .wrap(aster_forge_actix_middleware::security_headers::default_headers())
            .wrap(aster_forge_actix_middleware::metrics::MetricsMiddleware)
            .app_data(state.clone())
            .app_data(shutdown_data.clone())
            .app_data(metrics_data.clone())
            .configure(crate::api::configure)
    });
    if config.workers > 0 {
        server = server.workers(config.workers);
    }
    let server = server
        .bind((config.host, config.port))?
        .shutdown_timeout(HTTP_SHUTDOWN_TIMEOUT_SECS)
        .disable_signals()
        .run();
    let handle = server.handle();

    Ok(RuntimeServiceComponent::new(
        "http",
        RuntimeComponentKind::Core,
        server,
        shutdown_token,
        move || async move {
            handle.stop(true).await;
        },
    ))
}
