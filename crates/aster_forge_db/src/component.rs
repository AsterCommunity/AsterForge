//! Runtime component integration for database handles.
//!
//! Product crates still own database configuration, migrations, repositories,
//! and health-check semantics. Forge owns the repeated lifecycle mechanics for
//! already prepared handles: registering the `database` runtime component,
//! applying product-declared shutdown dependencies, and closing the handles
//! exactly once during dependency-aware shutdown.

use aster_forge_runtime::{
    HealthCheckOptions, HealthCheckScopes, HealthComponentReport, RuntimeComponentBundle,
    RuntimeComponentBundleRegistration, RuntimeComponentKind, RuntimeComponentRegistry,
    runtime_component,
};
use sea_orm::DatabaseConnection;
use std::time::Duration;

use crate::DbHandles;

/// Stable component name used for database handles.
pub const DATABASE_COMPONENT: &str = "database";
/// Stable shutdown phase name for database handle closing.
pub const DATABASE_CONNECTIONS_SHUTDOWN_PHASE: &str = "database_connections";
/// Stable health check name used for database ping checks.
pub const DATABASE_HEALTH_CHECK: &str = "database";
/// Default timeout for database readiness and diagnostics health checks.
pub const DATABASE_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime component that closes database handles during graceful shutdown.
pub struct DatabaseRuntimeComponent {
    db_handles: DbHandles,
    dependencies: &'static [&'static str],
}

impl DatabaseRuntimeComponent {
    /// Creates a database runtime component from prepared handles.
    pub const fn new(db_handles: DbHandles) -> Self {
        Self {
            db_handles,
            dependencies: &[],
        }
    }

    /// Declares components that must shut down before database handles close.
    pub const fn depends_on_all(mut self, dependencies: &'static [&'static str]) -> Self {
        self.dependencies = dependencies;
        self
    }
}

impl RuntimeComponentBundle for DatabaseRuntimeComponent {
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_database_health_check(registry, self.db_handles.reader().clone());
        register_database_shutdown(registry, self.db_handles, self.dependencies);
    }
}

/// Runtime component that registers the standard database health check only.
pub struct DatabaseHealthComponent {
    db: DatabaseConnection,
}

impl DatabaseHealthComponent {
    /// Creates a database health component from a prepared connection.
    pub const fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

impl RuntimeComponentBundle for DatabaseHealthComponent {
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_database_health_check(registry, self.db);
    }
}

/// Creates the database runtime component used by product entrypoints.
pub fn database_component(
    db_handles: DbHandles,
) -> RuntimeComponentBundleRegistration<DatabaseRuntimeComponent> {
    runtime_component(DatabaseRuntimeComponent::new(db_handles))
}

/// Creates the database runtime component with shutdown dependencies.
pub fn database_component_after(
    db_handles: DbHandles,
    dependencies: &'static [&'static str],
) -> RuntimeComponentBundleRegistration<DatabaseRuntimeComponent> {
    runtime_component(DatabaseRuntimeComponent::new(db_handles).depends_on_all(dependencies))
}

/// Creates the standard database health component.
pub fn database_health_component(
    db: DatabaseConnection,
) -> RuntimeComponentBundleRegistration<DatabaseHealthComponent> {
    runtime_component(DatabaseHealthComponent::new(db))
}

/// Registers database shutdown after product-declared dependency components.
fn register_database_shutdown(
    registry: &mut RuntimeComponentRegistry,
    db_handles: DbHandles,
    dependencies: &'static [&'static str],
) {
    registry
        .component(DATABASE_COMPONENT)
        .kind(RuntimeComponentKind::Database)
        .depends_on_all(dependencies)
        .shutdown_once(
            DATABASE_CONNECTIONS_SHUTDOWN_PHASE,
            None,
            db_handles,
            |db_handles| async move {
                db_handles
                    .close()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(())
            },
        );
}

/// Registers a database readiness and diagnostics health check.
fn register_database_health_check(registry: &mut RuntimeComponentRegistry, db: DatabaseConnection) {
    registry.component_health_with_options(
        DATABASE_COMPONENT,
        RuntimeComponentKind::Database,
        DATABASE_HEALTH_CHECK,
        database_health_options(),
        move || {
            let db = db.clone();
            async move { check_database_component(&db).await }
        },
    );
}

/// Returns the standard database health check options.
pub fn database_health_options() -> HealthCheckOptions {
    HealthCheckOptions::required(Some(DATABASE_HEALTH_CHECK_TIMEOUT))
        .with_scopes(HealthCheckScopes::readiness_and_diagnostics())
}

/// Runs the standard database ping health check.
pub async fn check_database_component(db: &DatabaseConnection) -> HealthComponentReport {
    match ping_database(db).await {
        Ok(()) => {
            tracing::debug!("database health check succeeded");
            HealthComponentReport::healthy(DATABASE_HEALTH_CHECK, "database ping succeeded")
        }
        Err(error) => {
            tracing::debug!(error = %error, "database health check failed");
            HealthComponentReport::unhealthy(
                DATABASE_HEALTH_CHECK,
                format!("database ping failed: {error}"),
            )
        }
    }
}

/// Pings the database connection used by the standard health check.
pub async fn ping_database(db: &DatabaseConnection) -> crate::Result<()> {
    tracing::debug!("pinging database health check");
    db.ping().await.map_err(crate::DbError::from)
}

#[cfg(test)]
mod tests {
    use aster_forge_runtime::{RuntimeComponentBundle, RuntimeComponentKind};

    use super::{
        DATABASE_COMPONENT, DATABASE_CONNECTIONS_SHUTDOWN_PHASE, DATABASE_HEALTH_CHECK,
        check_database_component, database_component_after, database_health_component,
    };
    use aster_forge_runtime::{HealthCheckScope, HealthStatus};

    #[tokio::test]
    async fn database_component_registers_dependencies_and_shutdown() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("database runtime component test database should connect");
        let db_handles = crate::DbHandles::single(db);

        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            database_component_after(db_handles, &["background_tasks", "mail_outbox"])
                .register(registry);
        });

        let descriptor = registry
            .descriptor(DATABASE_COMPONENT)
            .expect("database component should be registered");
        assert_eq!(descriptor.kind, RuntimeComponentKind::Database);
        assert_eq!(
            descriptor.dependencies,
            vec!["background_tasks", "mail_outbox"]
        );
        assert_eq!(
            descriptor
                .shutdown
                .first()
                .expect("database shutdown should be registered")
                .phase_name,
            DATABASE_CONNECTIONS_SHUTDOWN_PHASE
        );
        assert_eq!(descriptor.health_checks.len(), 1);
    }

    #[tokio::test]
    async fn database_component_reports_ping_success_and_failure() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("database health test database should connect");

        let healthy = check_database_component(&db).await;
        assert_eq!(healthy.status, HealthStatus::Healthy);
        assert_eq!(healthy.message, "database ping succeeded");

        db.close_by_ref()
            .await
            .expect("database health test database should close");
        let unhealthy = check_database_component(&db).await;
        assert_eq!(unhealthy.status, HealthStatus::Unhealthy);
        assert!(unhealthy.message.contains("database ping failed"));
    }

    #[tokio::test]
    async fn database_health_component_registers_readiness_component() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("database readiness test database should connect");
        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::new();

        database_health_component(db).register(&mut registry);

        assert_eq!(registry.len(), 1);
        let report = registry.run_health(HealthCheckScope::Readiness).await;
        let component_names = report
            .components
            .iter()
            .map(|component| component.name)
            .collect::<Vec<_>>();
        assert_eq!(component_names, vec![DATABASE_HEALTH_CHECK]);
        assert_eq!(report.status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn database_health_component_reports_healthy_status() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("database health component test database should connect");

        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            database_health_component(db).register(registry);
        });

        let descriptor = registry
            .descriptor(DATABASE_COMPONENT)
            .expect("database component should be registered");
        assert_eq!(descriptor.health_checks.len(), 1);
        let report = registry.run_health(HealthCheckScope::Readiness).await;
        assert_eq!(report.status(), HealthStatus::Healthy);
    }
}
