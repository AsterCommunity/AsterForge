//! Runtime component assembly.
//!
//! This module mirrors the shape used by Aster products: prepare product state, then register the
//! HTTP service and domain components into `AsterRuntime`.

use actix_web::web;

use crate::errors::Result;

/// Prepares all resources needed by the runtime component graph.
pub async fn prepare_state(config: crate::config::AppConfig) -> Result<crate::runtime::AppState> {
    let metrics = crate::runtime::metrics::create_metrics_recorder();
    let db_handles =
        crate::db::runtime::prepare_database_handles(&config.database, metrics.clone()).await?;
    let cache = aster_forge_cache::create_cache(&config.cache).await;
    let config_sync =
        aster_forge_config::build_config_sync_runtime(&config.config_sync, env!("CARGO_PKG_NAME"))?;
    let mail_sender = aster_forge_mail::memory_sender();

    Ok(crate::runtime::AppState::new(
        config,
        db_handles,
        cache,
        config_sync,
        metrics,
        mail_sender,
    ))
}

/// Assembles and runs the Forge runtime.
pub async fn run(state: crate::runtime::AppState) -> std::io::Result<()> {
    let host = state.config.server.host.clone();
    let port = state.config.server.port;
    let workers = state.config.server.workers;
    let state = web::Data::new(state);
    let metrics_data: web::Data<dyn aster_forge_metrics::MetricsRecorder> =
        web::Data::from(state.get_ref().metrics.clone());
    let app_state = state.get_ref();

    aster_forge_runtime::AsterRuntime::builder()
        .component(crate::api::http::http_component(
            crate::api::http::HttpRuntimeConfig {
                host: host.as_str(),
                port,
                workers,
            },
            state.clone(),
            metrics_data,
        ))?
        .component(crate::tasks::runtime::background_tasks_component(
            app_state.metrics.clone(),
        ))
        .component(crate::services::mail_outbox_service::runtime::mail_runtime_component(app_state))
        .component(crate::services::audit_service::runtime::audit_runtime_component())
        .component(crate::db::runtime::database_component(
            app_state.db_handles.clone(),
        ))
        .run()
        .await
        .map_err(to_io_error)?
}

fn to_io_error(error: impl ToString) -> std::io::Error {
    std::io::Error::other(error.to_string())
}
