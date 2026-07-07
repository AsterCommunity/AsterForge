//! Shared audit runtime integration for Aster services.
//!
//! Product crates own audit actions, detail schemas, authorization rules,
//! operator-facing presentation, and the concrete manager implementation.
//! Forge owns the runtime lifecycle contract shared by Aster products: record a
//! best-effort server shutdown audit event, then flush the product audit manager
//! before database handles close. Products that also use a mail outbox can enable
//! the `mail-outbox-dependency` feature to make the standard audit constructors
//! depend on the mail outbox drain component.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use std::future::Future;

use aster_forge_runtime::{
    RuntimeComponentBundleRegistration, RuntimeComponentKind, RuntimeComponentRegistry,
    StartupPhaseFailurePolicy, runtime_component,
};

/// Stable component name used for process lifecycle audit records.
pub const AUDIT_LOGS_COMPONENT: &str = "audit_logs";
/// Stable component name used for the buffered audit manager.
pub const AUDIT_MANAGER_COMPONENT: &str = "audit_manager";
/// Stable startup phase name for recording the process start event.
pub const SERVER_START_AUDIT_PHASE: &str = "server_start_audit";
/// Stable shutdown phase name for recording the process shutdown event.
pub const SERVER_SHUTDOWN_AUDIT_PHASE: &str = "server_shutdown_audit";
/// Stable shutdown phase name for flushing the buffered audit manager.
pub const AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE: &str = "audit_manager_flush";

#[cfg(feature = "mail-outbox-dependency")]
const DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES: &[&str] =
    &[aster_forge_mail::MAIL_OUTBOX_COMPONENT];
#[cfg(not(feature = "mail-outbox-dependency"))]
const DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES: &[&str] = &[];

/// Creates the full audit lifecycle component bundle used by product entrypoints.
///
/// `resources` is product-defined. It commonly contains a database connection
/// and runtime config snapshot. `record_server_start` and `record_server_shutdown`
/// record the product's own lifecycle audit entries. `flush_audit_manager` drains
/// the product's buffered audit writer. With the `mail-outbox-dependency`
/// feature enabled, the shutdown audit phase automatically runs after the mail
/// outbox drain component.
pub fn audit_component<T, StartFn, StartFut, ShutdownFn, ShutdownFut, FlushFn, FlushFut>(
    resources: T,
    record_server_start: StartFn,
    record_server_shutdown: ShutdownFn,
    flush_audit_manager: FlushFn,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle>
where
    T: Clone + Send + 'static,
    StartFn: FnOnce(T) -> StartFut + Send + Sync + 'static,
    StartFut: Future<Output = Result<(), String>> + Send + 'static,
    ShutdownFn: FnOnce(T) -> ShutdownFut + Send + Sync + 'static,
    ShutdownFut: Future<Output = Result<(), String>> + Send + 'static,
    FlushFn: FnOnce(()) -> FlushFut + Send + Sync + 'static,
    FlushFut: Future<Output = Result<(), String>> + Send + 'static,
{
    audit_component_after(
        resources,
        DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES,
        record_server_start,
        record_server_shutdown,
        flush_audit_manager,
    )
}

/// Creates the full audit lifecycle component bundle with caller-provided
/// shutdown dependencies for the server-shutdown audit phase.
///
/// Use this when a product has another product-neutral component that must
/// finish before recording `server_shutdown`. The audit-manager flush still runs
/// after `AUDIT_LOGS_COMPONENT`.
pub fn audit_component_after<T, StartFn, StartFut, ShutdownFn, ShutdownFut, FlushFn, FlushFut>(
    resources: T,
    shutdown_dependencies: &'static [&'static str],
    record_server_start: StartFn,
    record_server_shutdown: ShutdownFn,
    flush_audit_manager: FlushFn,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle>
where
    T: Clone + Send + 'static,
    StartFn: FnOnce(T) -> StartFut + Send + Sync + 'static,
    StartFut: Future<Output = Result<(), String>> + Send + 'static,
    ShutdownFn: FnOnce(T) -> ShutdownFut + Send + Sync + 'static,
    ShutdownFut: Future<Output = Result<(), String>> + Send + 'static,
    FlushFn: FnOnce(()) -> FlushFut + Send + Sync + 'static,
    FlushFut: Future<Output = Result<(), String>> + Send + 'static,
{
    let startup_resources = resources.clone();
    runtime_component((
        server_start_audit_component(startup_resources, record_server_start),
        server_shutdown_audit_component_after(
            resources,
            shutdown_dependencies,
            record_server_shutdown,
        ),
        audit_manager_component(flush_audit_manager),
    ))
}

/// Creates the audit shutdown component bundle without the server-start phase.
///
/// Use this only when a product intentionally records server startup elsewhere.
/// Normal Aster services should use [`audit_component`] so startup, shutdown,
/// and manager flush share one lifecycle component. With the
/// `mail-outbox-dependency` feature enabled, the shutdown audit phase
/// automatically runs after the mail outbox drain component.
pub fn shutdown_audit_component<T, RecordFn, RecordFut, FlushFn, FlushFut>(
    resources: T,
    record_server_shutdown: RecordFn,
    flush_audit_manager: FlushFn,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle>
where
    T: Send + 'static,
    RecordFn: FnOnce(T) -> RecordFut + Send + Sync + 'static,
    RecordFut: Future<Output = Result<(), String>> + Send + 'static,
    FlushFn: FnOnce(()) -> FlushFut + Send + Sync + 'static,
    FlushFut: Future<Output = Result<(), String>> + Send + 'static,
{
    shutdown_audit_component_after(
        resources,
        DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES,
        record_server_shutdown,
        flush_audit_manager,
    )
}

/// Creates the audit shutdown component bundle with caller-provided dependencies
/// for the server-shutdown audit phase.
pub fn shutdown_audit_component_after<T, RecordFn, RecordFut, FlushFn, FlushFut>(
    resources: T,
    shutdown_dependencies: &'static [&'static str],
    record_server_shutdown: RecordFn,
    flush_audit_manager: FlushFn,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle>
where
    T: Send + 'static,
    RecordFn: FnOnce(T) -> RecordFut + Send + Sync + 'static,
    RecordFut: Future<Output = Result<(), String>> + Send + 'static,
    FlushFn: FnOnce(()) -> FlushFut + Send + Sync + 'static,
    FlushFut: Future<Output = Result<(), String>> + Send + 'static,
{
    runtime_component((
        server_shutdown_audit_component_after(
            resources,
            shutdown_dependencies,
            record_server_shutdown,
        ),
        audit_manager_component(flush_audit_manager),
    ))
}

/// Creates the server-start audit startup component.
pub fn server_start_audit_component<T, F, Fut>(
    resources: T,
    record_server_start: F,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    runtime_component(move |registry: &mut RuntimeComponentRegistry| {
        let mut resources = Some(resources);
        let mut record_server_start = Some(record_server_start);
        registry.component_startup(
            AUDIT_LOGS_COMPONENT,
            RuntimeComponentKind::Product,
            SERVER_START_AUDIT_PHASE,
            StartupPhaseFailurePolicy::Required,
            move || {
                let resources = resources.take();
                let record_server_start = record_server_start.take();
                async move {
                    let Some(resources) = resources else {
                        return Err(
                            "server start audit startup phase resources already consumed"
                                .to_string(),
                        );
                    };
                    let Some(record_server_start) = record_server_start else {
                        return Err("server start audit startup phase callback already consumed"
                            .to_string());
                    };
                    record_server_start(resources).await
                }
            },
        );
    })
}

/// Creates the server-shutdown audit component with the crate's default
/// dependencies.
///
/// The default dependency list is empty unless the `mail-outbox-dependency`
/// feature is enabled.
pub fn server_shutdown_audit_component<T, F, Fut>(
    resources: T,
    record_server_shutdown: F,
) -> RuntimeComponentBundleRegistration<aster_forge_runtime::ShutdownResourceComponent<T>>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    server_shutdown_audit_component_after(
        resources,
        DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES,
        record_server_shutdown,
    )
}

/// Creates the server-shutdown audit component with caller-provided dependencies.
pub fn server_shutdown_audit_component_after<T, F, Fut>(
    resources: T,
    shutdown_dependencies: &'static [&'static str],
    record_server_shutdown: F,
) -> RuntimeComponentBundleRegistration<aster_forge_runtime::ShutdownResourceComponent<T>>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    aster_forge_runtime::shutdown_resource_component_after(
        AUDIT_LOGS_COMPONENT,
        RuntimeComponentKind::Product,
        SERVER_SHUTDOWN_AUDIT_PHASE,
        shutdown_dependencies,
        resources,
        record_server_shutdown,
    )
}

/// Creates the audit-manager flush component.
pub fn audit_manager_component<F, Fut>(
    flush_audit_manager: F,
) -> RuntimeComponentBundleRegistration<aster_forge_runtime::ShutdownResourceComponent<()>>
where
    F: FnOnce(()) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    aster_forge_runtime::shutdown_resource_component_after(
        AUDIT_MANAGER_COMPONENT,
        RuntimeComponentKind::Product,
        AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE,
        &[AUDIT_LOGS_COMPONENT],
        (),
        flush_audit_manager,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use aster_forge_runtime::RuntimeComponentBundle;

    use super::{
        AUDIT_LOGS_COMPONENT, AUDIT_MANAGER_COMPONENT, AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE,
        SERVER_SHUTDOWN_AUDIT_PHASE, SERVER_START_AUDIT_PHASE, audit_component,
        audit_component_after, audit_manager_component, server_shutdown_audit_component,
        server_shutdown_audit_component_after, server_start_audit_component,
        shutdown_audit_component,
    };

    #[cfg(not(feature = "mail-outbox-dependency"))]
    #[test]
    fn server_shutdown_audit_component_registers_without_dependencies() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            server_shutdown_audit_component((), |()| async { Ok(()) }).register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(
            descriptor.kind,
            aster_forge_runtime::RuntimeComponentKind::Product
        );
        assert!(descriptor.dependencies.is_empty());
        assert_eq!(
            descriptor
                .shutdown
                .expect("audit logs shutdown should be registered")
                .phase_name,
            SERVER_SHUTDOWN_AUDIT_PHASE
        );
    }

    #[cfg(feature = "mail-outbox-dependency")]
    #[test]
    fn server_shutdown_audit_component_registers_mail_outbox_dependency() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            server_shutdown_audit_component((), |()| async { Ok(()) }).register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(
            descriptor.kind,
            aster_forge_runtime::RuntimeComponentKind::Product
        );
        assert_eq!(
            descriptor.dependencies,
            vec![aster_forge_mail::MAIL_OUTBOX_COMPONENT]
        );
        assert_eq!(
            descriptor
                .shutdown
                .expect("audit logs shutdown should be registered")
                .phase_name,
            SERVER_SHUTDOWN_AUDIT_PHASE
        );
    }

    #[test]
    fn server_shutdown_audit_component_after_registers_caller_dependencies() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            server_shutdown_audit_component_after(
                (),
                &["background_tasks", "mail_outbox"],
                |()| async { Ok(()) },
            )
            .register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(
            descriptor.dependencies,
            vec!["background_tasks", "mail_outbox"]
        );
    }

    #[tokio::test]
    async fn server_start_audit_component_registers_and_runs_startup_phase() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_start = calls.clone();
        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            server_start_audit_component((), move |()| {
                let calls = calls_for_start.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(descriptor.startup.len(), 1);
        assert_eq!(descriptor.startup[0].phase_name, SERVER_START_AUDIT_PHASE);

        let report = registry.startup().await;

        assert!(!report.aborted());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn audit_manager_component_registers_after_audit_logs() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            audit_manager_component(|()| async { Ok(()) }).register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_MANAGER_COMPONENT)
            .expect("audit manager component should be registered");
        assert_eq!(descriptor.dependencies, vec![AUDIT_LOGS_COMPONENT]);
        assert_eq!(
            descriptor
                .shutdown
                .expect("audit manager shutdown should be registered")
                .phase_name,
            AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE
        );
    }

    #[tokio::test]
    async fn audit_component_registers_startup_shutdown_and_manager_flush() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let order_for_start = order.clone();
        let order_for_record = order.clone();
        let order_for_flush = order.clone();

        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            audit_component(
                (),
                move |()| {
                    let order = order_for_start.clone();
                    async move {
                        order
                            .lock()
                            .expect("audit component test order should lock")
                            .push("start");
                        Ok(())
                    }
                },
                move |()| {
                    let order = order_for_record.clone();
                    async move {
                        order
                            .lock()
                            .expect("audit component test order should lock")
                            .push("record");
                        Ok(())
                    }
                },
                move |()| {
                    let order = order_for_flush.clone();
                    async move {
                        order
                            .lock()
                            .expect("audit component test order should lock")
                            .push("flush");
                        Ok(())
                    }
                },
            )
            .register(registry);
        });

        let startup_report = registry.startup().await;
        assert!(!startup_report.aborted());
        let shutdown_report = registry.shutdown().await;

        assert!(!shutdown_report.has_failures());
        assert_eq!(
            order
                .lock()
                .expect("audit component test order should lock")
                .as_slice(),
            ["start", "record", "flush"]
        );
    }

    #[test]
    fn audit_component_after_registers_shutdown_dependencies() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            audit_component_after(
                (),
                &["mail_outbox"],
                |()| async { Ok(()) },
                |()| async { Ok(()) },
                |()| async { Ok(()) },
            )
            .register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(descriptor.dependencies, vec!["mail_outbox"]);
    }

    #[cfg(feature = "mail-outbox-dependency")]
    #[test]
    fn audit_component_registers_mail_outbox_dependency_when_feature_enabled() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            audit_component(
                (),
                |()| async { Ok(()) },
                |()| async { Ok(()) },
                |()| async { Ok(()) },
            )
            .register(registry);
        });

        let descriptor = registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .expect("audit logs component should be registered");
        assert_eq!(
            descriptor.dependencies,
            vec![aster_forge_mail::MAIL_OUTBOX_COMPONENT]
        );
    }

    #[tokio::test]
    async fn shutdown_audit_component_runs_shutdown_record_before_manager_flush() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let order_for_record = order.clone();
        let order_for_flush = order.clone();

        let report = aster_forge_runtime::RuntimeComponentRegistry::shutdown_bundle(
            shutdown_audit_component(
                (),
                move |()| {
                    let order = order_for_record.clone();
                    async move {
                        order
                            .lock()
                            .expect("audit component test order should lock")
                            .push("record");
                        Ok(())
                    }
                },
                move |()| {
                    let order = order_for_flush.clone();
                    async move {
                        order
                            .lock()
                            .expect("audit component test order should lock")
                            .push("flush");
                        Ok(())
                    }
                },
            ),
        )
        .await;

        assert!(!report.has_failures());
        assert_eq!(
            order
                .lock()
                .expect("audit component test order should lock")
                .as_slice(),
            ["record", "flush"]
        );
    }
}
