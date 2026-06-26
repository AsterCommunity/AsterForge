//! Shared audit runtime integration for Aster services.
//!
//! Product crates own audit actions, detail schemas, authorization rules,
//! operator-facing presentation, and the concrete manager implementation.
//! Forge owns the runtime lifecycle contract shared by Aster products: record a
//! best-effort server shutdown audit event after outbound mail drains, then
//! flush the product audit manager before database handles close.
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
    runtime_component,
};

/// Stable component name used for process lifecycle audit records.
pub const AUDIT_LOGS_COMPONENT: &str = "audit_logs";
/// Stable component name used for the buffered audit manager.
pub const AUDIT_MANAGER_COMPONENT: &str = "audit_manager";
/// Stable shutdown phase name for recording the process shutdown event.
pub const SERVER_SHUTDOWN_AUDIT_PHASE: &str = "server_shutdown_audit";
/// Stable shutdown phase name for flushing the buffered audit manager.
pub const AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE: &str = "audit_manager_flush";

/// Creates the full audit runtime component bundle used by product entrypoints.
///
/// `resources` is product-defined. It commonly contains a database connection
/// and runtime config snapshot. `record_server_shutdown` records the product's
/// own server-shutdown audit entry. `flush_audit_manager` drains the product's
/// buffered audit writer.
pub fn audit_component<T, RecordFn, RecordFut, FlushFn, FlushFut>(
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
    runtime_component(move |registry: &mut RuntimeComponentRegistry| {
        register_server_shutdown_audit(registry, resources, record_server_shutdown);
        register_audit_manager_shutdown(registry, flush_audit_manager);
    })
}

/// Creates the server-shutdown audit component.
pub fn server_shutdown_audit_component<T, F, Fut>(
    resources: T,
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
        &[aster_forge_mail::MAIL_OUTBOX_COMPONENT],
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

/// Registers the process shutdown audit event before the audit manager flushes.
pub fn register_server_shutdown_audit<T, F, Fut>(
    registry: &mut RuntimeComponentRegistry,
    resources: T,
    record_server_shutdown: F,
) where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    registry.register_bundle(server_shutdown_audit_component(
        resources,
        record_server_shutdown,
    ));
}

/// Registers graceful shutdown for the product audit manager.
pub fn register_audit_manager_shutdown<F, Fut>(
    registry: &mut RuntimeComponentRegistry,
    flush_audit_manager: F,
) where
    F: FnOnce(()) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    registry.register_bundle(audit_manager_component(flush_audit_manager));
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
        SERVER_SHUTDOWN_AUDIT_PHASE, audit_component, audit_manager_component,
        register_audit_manager_shutdown, server_shutdown_audit_component,
    };

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
    async fn audit_component_runs_shutdown_record_before_manager_flush() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let order_for_record = order.clone();
        let order_for_flush = order.clone();

        let report =
            aster_forge_runtime::RuntimeComponentRegistry::shutdown_bundle(audit_component(
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
            ))
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

    #[test]
    fn audit_manager_shutdown_registrar_can_be_used_directly() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_flush = calls.clone();
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            register_audit_manager_shutdown(registry, move |()| {
                let calls = calls_for_flush.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            });
        });

        assert!(registry.descriptor(AUDIT_MANAGER_COMPONENT).is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
