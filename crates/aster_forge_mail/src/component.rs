//! Runtime component integration for mail outbox draining.
//!
//! Aster products own the concrete outbox store, template rendering, sender
//! configuration, audit hooks, and product error mapping. Forge owns the
//! repeated runtime lifecycle contract: after background workers stop, drain the
//! mail outbox before database handles close. Keeping the component names and
//! dependency edge here gives all products the same shutdown graph.

use std::future::Future;

use aster_forge_runtime::{RuntimeComponentBundleRegistration, RuntimeComponentKind};

use crate::MAIL_OUTBOX_COMPONENT;
/// Stable shutdown phase name for mail outbox draining.
pub const MAIL_OUTBOX_DRAIN_SHUTDOWN_PHASE: &str = "mail_outbox_drain";

/// Creates the mail outbox runtime component used by product entrypoints.
///
/// The `resources` value is product-defined. It usually contains a database
/// handle, runtime config snapshot, and mail sender. The `drain` callback keeps
/// product-specific rendering, repository, audit, and error mapping outside
/// Forge while still using the shared component lifecycle.
pub fn mail_outbox_component<T, F, Fut>(
    resources: T,
    drain: F,
) -> RuntimeComponentBundleRegistration<aster_forge_runtime::ShutdownResourceComponent<T>>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    aster_forge_runtime::shutdown_resource_component_after(
        MAIL_OUTBOX_COMPONENT,
        RuntimeComponentKind::Mail,
        MAIL_OUTBOX_DRAIN_SHUTDOWN_PHASE,
        &[aster_forge_tasks::BACKGROUND_TASKS_COMPONENT],
        resources,
        drain,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use aster_forge_runtime::RuntimeComponentBundle;

    use super::{MAIL_OUTBOX_COMPONENT, MAIL_OUTBOX_DRAIN_SHUTDOWN_PHASE, mail_outbox_component};

    #[test]
    fn mail_outbox_component_registers_standard_shutdown_dependency() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            mail_outbox_component((), |()| async { Ok(()) }).register(registry);
        });

        let descriptor = registry
            .descriptor(MAIL_OUTBOX_COMPONENT)
            .expect("mail outbox component should be registered");
        assert_eq!(
            descriptor.kind,
            aster_forge_runtime::RuntimeComponentKind::Mail
        );
        assert_eq!(
            descriptor.dependencies,
            vec![aster_forge_tasks::BACKGROUND_TASKS_COMPONENT]
        );
        assert_eq!(
            descriptor
                .shutdown
                .expect("mail outbox shutdown should be registered")
                .phase_name,
            MAIL_OUTBOX_DRAIN_SHUTDOWN_PHASE
        );
    }

    #[tokio::test]
    async fn mail_outbox_component_consumes_resource_once_during_shutdown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_drain = calls.clone();
        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::new();
        registry.register_bundle(mail_outbox_component(7usize, move |resource| {
            let calls = calls_for_drain.clone();
            async move {
                assert_eq!(resource, 7);
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }));
        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
