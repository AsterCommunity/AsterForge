//! Shared background task primitives for Aster services.
//!
//! This crate owns product-neutral task mechanics: step state transitions, typed payload/result
//! serialization, retry classification, erased task-spec adapters, registry generation, runtime
//! worker loops, lease guards, heartbeat loops, lane claiming, dispatch aggregation, drain loops,
//! and task artifact temporary-directory helpers. It deliberately does not own database entities,
//! SeaORM repositories, product task kind enums, runtime configuration, metrics labels, or concrete
//! task implementations. Product crates keep those boundaries and register their specs and storage
//! adapters explicitly.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

mod dispatch;
mod error;
mod heartbeat;
mod lease;
mod registry;
mod retry;
mod runtime;
mod spec;
mod steps;
mod temp;

pub use dispatch::{
    ClaimableTaskRecord, ClaimedTask, DispatchStats, TaskClaimCandidate, TaskClaimStore,
    TaskDispatchOutcome, TaskLaneConfig, available_lane_capacity, claim_due_for_lane,
    claim_limit_to_u64, dispatch_lanes, drain_dispatcher, run_claimed_task_batch,
    run_with_concurrency_limit,
};
pub use error::{Result, TaskCoreError};
pub use heartbeat::{
    TaskHeartbeatStore, evaluate_heartbeat_result, run_task_heartbeat_loop,
    spawn_task_heartbeat_with_interval, stop_task_heartbeat,
};
pub use lease::{
    TaskExecutionContext, TaskLease, TaskLeaseGuard, task_lease_expires_at,
    task_lease_renewal_timeout,
};
pub use registry::TaskRecord;
pub use retry::TaskRetryClass;
pub use runtime::{
    BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP, BACKGROUND_TASK_SHUTDOWN_GRACE,
    BackgroundTaskDispatchBackoff, BackgroundTaskDispatchIteration, BackgroundTaskDispatchTrigger,
    BackgroundTasks, PeriodicTask, RecordedTaskHooks, effective_dispatch_base_interval,
    effective_dispatch_max_interval, effective_jitter_cap, periodic_sleep_duration,
    run_dispatch_worker, run_periodic_task, run_recorded_task_iteration,
};
pub use spec::{
    BackgroundTaskSpec, ErasedBackgroundTaskSpec, TaskProcessFuture, TaskSpecAdapter,
    decode_payload_as, decode_result_as, serialize_payload, serialize_result,
};
pub use steps::{
    TaskStepInfo, TaskStepSpec, TaskStepStatus, initial_task_steps_from_specs,
    mark_active_step_failed, set_task_step_active, set_task_step_skipped, set_task_step_succeeded,
};
pub use temp::{
    cleanup_runtime_temp_root, cleanup_task_temp_dir_for_lease_in_root,
    cleanup_task_temp_dir_for_task_in_root, cleanup_temp_dir, prepare_task_temp_dir_in_root,
};
