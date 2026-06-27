//! Shared runtime primitives for Aster services.
//!
//! This crate contains small process/runtime building blocks that are not tied
//! to a concrete product domain: health report aggregation, startup phase
//! coordination, shutdown phase coordination, buffered side-effect writing, and
//! termination signal handling. Product crates still own runtime state, audit
//! events, background task shutdown order, database handles, and
//! service-specific readiness checks.
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

pub mod buffered;
pub mod component;
pub mod health;
pub mod lease;
pub mod lifecycle;
pub mod shutdown;
pub mod startup;

pub use buffered::{BufferedBatchConfig, BufferedBatchWriter};
pub use component::{
    RuntimeComponentBuilder, RuntimeComponentBundle, RuntimeComponentDescriptor,
    RuntimeComponentGraphError, RuntimeComponentKind, RuntimeComponentRegistry,
    RuntimeShutdownDescriptor, RuntimeStartupDescriptor, RuntimeTaskDescriptor,
};
pub use health::{
    HealthCheckDescriptor, HealthCheckOptions, HealthCheckRegistry, HealthCheckRegistryBuilder,
    HealthCheckRequirement, HealthCheckScope, HealthCheckScopes, HealthComponentDetail,
    HealthComponentDetailValue, HealthComponentReport, HealthMetricsRecorder, HealthStatus,
    SystemHealthReport,
};
pub use lease::{
    DEFAULT_RUNTIME_LEASE_RETRY_INTERVAL, DEFAULT_RUNTIME_LEASE_TTL, RuntimeLeaseAcquire,
    RuntimeLeaseClaim, RuntimeLeaseConfig, RuntimeLeaseOwner, RuntimeLeaseStore,
    new_runtime_lease_owner_id, run_runtime_lease_supervisor,
};
pub use lifecycle::{
    AsterRuntime, AsterRuntimeBuilder, AsterRuntimeComponent, AsterRuntimeError,
    RuntimeComponentBundleRegistration, RuntimeComponentWithShutdown, RuntimeServiceComponent,
    ServiceLifecycle, ShutdownResourceComponent, TryRuntimeComponentWithShutdown,
    runtime_component, runtime_component_with_shutdown, shutdown_resource_component,
    shutdown_resource_component_after, try_runtime_component_with_shutdown,
};
pub use shutdown::{
    RuntimeSignalError, ShutdownCoordinator, ShutdownPhaseReport, ShutdownPhaseStatus,
    ShutdownReport, TerminationSignal, log_shutdown_report, spawn_termination_signal_handler,
    wait_for_termination_signal,
};
pub use startup::{
    RuntimeTempDirError, StartupCoordinator, StartupPhaseFailurePolicy, StartupPhaseOutcome,
    StartupPhaseReport, StartupPhaseStatus, StartupReport, create_runtime_temp_dir_guard,
    ensure_runtime_temp_dir, run_optional_startup_phase, run_required_startup_phase,
};
