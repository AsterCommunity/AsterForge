//! Database runtime component registration.

use crate::errors::Result;

const DATABASE_SHUTDOWN_DEPENDENCIES: &[&str] = &[
    aster_forge_tasks::BACKGROUND_TASKS_COMPONENT,
    aster_forge_mail::MAIL_OUTBOX_COMPONENT,
    aster_forge_audit::AUDIT_MANAGER_COMPONENT,
];

/// Creates the database runtime component used by the product entrypoint.
pub fn database_component(
    db_handles: aster_forge_db::DbHandles,
) -> aster_forge_runtime::RuntimeComponentBundleRegistration<aster_forge_db::DatabaseRuntimeComponent>
{
    aster_forge_db::database_component_after(db_handles, DATABASE_SHUTDOWN_DEPENDENCIES)
}

/// Connects database handles.
pub async fn prepare_database_handles(
    database: &crate::config::DatabaseConfig,
    metrics: aster_forge_metrics::SharedMetricsRecorder,
) -> Result<aster_forge_db::DbHandles> {
    let config = aster_forge_db::DatabaseConfig {
        url: database.url.clone(),
        pool_size: database.pool_size,
        retry_count: database.retry_count,
    };
    let writer = aster_forge_db::connect_with_metrics(&config, metrics.clone()).await?;

    migration::Migrator::up(&writer, None).await?;

    aster_forge_db::connect_reader_for_writer_with_metrics(&config, writer, metrics)
        .await
        .map_err(Into::into)
}
