//! Product audit service boundary.
//!
//! Keep product audit action enums, detail schemas, presentation, permissions, and API filters in
//! the product repository. Forge owns only the lifecycle component and shared audit log mechanics.

pub mod runtime {
    //! Audit runtime component integration.

    /// Creates the audit runtime component used by the product entrypoint.
    pub fn audit_runtime_component() -> aster_forge_runtime::RuntimeComponentBundleRegistration<
        impl aster_forge_runtime::RuntimeComponentBundle,
    > {
        aster_forge_audit::audit_component(
            (),
            |()| async {
                tracing::info!("server start audit placeholder");
                Ok(())
            },
            |()| async {
                tracing::info!("server shutdown audit placeholder");
                Ok(())
            },
            |()| async {
                tracing::info!("audit manager flush placeholder");
                Ok(())
            },
        )
    }
}
