//! Shared background task primitives for Aster services.
//!
//! This crate owns product-neutral task mechanics: step state transitions, typed payload/result
//! serialization, retry classification, erased task-spec adapters, and a small registration macro
//! for mapping product task kinds to specs and lanes. It deliberately does not own database
//! entities, SeaORM repositories, product task kind enums, runtime configuration, or concrete task
//! implementations. Product crates keep those boundaries and register their specs explicitly.
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
mod registry;
mod retry;
mod spec;
mod steps;

pub use dispatch::{DispatchStats, TaskDispatchOutcome};
pub use error::{Result, TaskCoreError};
pub use registry::TaskRecord;
pub use retry::TaskRetryClass;
pub use spec::{
    BackgroundTaskSpec, ErasedBackgroundTaskSpec, TaskProcessFuture, TaskSpecAdapter,
    decode_payload_as, decode_result_as, serialize_payload, serialize_result,
};
pub use steps::{
    TaskStepInfo, TaskStepSpec, TaskStepStatus, initial_task_steps_from_specs,
    mark_active_step_failed, set_task_step_active, set_task_step_skipped, set_task_step_succeeded,
};
