//! Graceful shutdown helpers.

use crate::db::DbHandles;
use crate::runtime::SharedRuntimeState;
use crate::runtime::tasks::BackgroundTasks;

pub async fn wait_for_signal() {
    wait_for_termination_signal().await;
}

#[cfg(unix)]
async fn wait_for_termination_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => tracing::info!("received SIGINT, shutting down gracefully..."),
        _ = sigterm.recv() => tracing::info!("received SIGTERM, shutting down gracefully..."),
    }
}

#[cfg(not(unix))]
async fn wait_for_termination_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    tracing::info!("received Ctrl+C, shutting down gracefully...");
}

pub async fn perform_shutdown(background_tasks: BackgroundTasks, db_handles: DbHandles) {
    tracing::info!("stopping background tasks...");
    background_tasks.shutdown().await;
    tracing::info!("background tasks stopped");

    tracing::info!("flushing audit logs...");
    crate::services::audit_service::shutdown_global_audit_log_manager().await;

    tracing::info!("closing database connections...");
    if let Err(error) = db_handles.close().await {
        tracing::error!("error closing database connections: {error}");
    } else {
        tracing::info!("database connections closed");
    }
    tracing::info!("shutdown complete");
}

pub async fn record_server_shutdown<S: SharedRuntimeState>(state: &S) {
    let backend = state.writer_db().get_database_backend();
    crate::services::audit_service::log(
        state,
        &crate::services::audit_service::AuditContext::system(),
        crate::types::AuditAction::ServerShutdown,
        crate::types::AuditEntityType::System,
        None,
        Some("server"),
        None,
    )
    .await;
    tracing::info!(?backend, "server shutdown recorded");
}
