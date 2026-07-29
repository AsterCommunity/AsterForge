//! Windows Cloud Files bindings for the product-neutral Forge cloud-files core.
//!
//! This crate owns CFAPI-specific item/sync-root identity encoding, placeholder metadata, filename
//! validation, persistent registration policies, active connection/session fencing, owned
//! `FETCH_DATA`/`CANCEL_FETCH_DATA` snapshots, bounded callback ingress, active hydration waiter
//! cancellation, and native calls. Product authentication, remote DTOs, account policy,
//! persistence adapters, and user-visible error mapping remain in product crates.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![deny(clippy::undocumented_unsafe_blocks, unsafe_op_in_unsafe_fn)]
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

mod connection;
mod error;
mod identity;
mod placeholder;
mod progress;
mod registration;
mod waiter_registry;
mod watchdog;

#[cfg(windows)]
mod native;
#[cfg(windows)]
mod native_connection;

pub use connection::{
    CFAPI_MAX_PRIORITY_HINT, CFAPI_TRANSFER_ALIGNMENT_BYTES, WindowsCallbackInfoSnapshot,
    WindowsCallbackLease, WindowsCallbackQueueMetrics, WindowsCallbackRange,
    WindowsCallbackRangeLength, WindowsCallbackRequest, WindowsCancelFetchDataFlags,
    WindowsCancelFetchDataRequest, WindowsCancelFetchDataSnapshot, WindowsConnectionKey,
    WindowsConnectionSession, WindowsFetchDataCorrelation, WindowsFetchDataFailure,
    WindowsFetchDataFlags, WindowsFetchDataPreparation, WindowsFetchDataProgressReporter,
    WindowsFetchDataRequest, WindowsFetchDataSnapshot, WindowsFetchDataTransfer,
    WindowsObservedNotification, WindowsPreflightRequest, WindowsPreflightSnapshot,
    WindowsProcessInfoSnapshot, WindowsRequestKey, WindowsRestartHydration,
    WindowsSyncRootConnectOptions, WindowsTransferKey,
};
pub use error::{Result, WindowsCloudFilesError};
pub use identity::{CFAPI_FILE_IDENTITY_MAX_BYTES, WindowsFileIdentity};
#[cfg(windows)]
pub use native::{
    WindowsPinRecursion, WindowsPinState, WindowsPlaceholderBatchOutcome,
    WindowsPlaceholderCreateOutcome, create_placeholders, dehydrate_placeholder,
    register_sync_root, set_placeholder_in_sync, set_placeholder_pin_state, unregister_sync_root,
};
#[cfg(windows)]
pub use native_connection::{
    WindowsDisconnectOutcome, WindowsSyncRootConnection, connect_sync_root,
};
pub use placeholder::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, WindowsFileTimes, WindowsPlaceholder,
    WindowsPlaceholderMetadata, WindowsPlaceholderOptions,
};
pub use progress::WindowsFetchDataProgress;
pub use registration::{
    CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES, WindowsHardLinkPolicy, WindowsHydrationPolicy,
    WindowsHydrationPolicyPrimary, WindowsInSyncPolicy, WindowsPlaceholderManagementPolicy,
    WindowsPlatformVersion, WindowsPopulationPolicy, WindowsProviderId, WindowsSyncRootIdentity,
    WindowsSyncRootPolicies, WindowsSyncRootRegistration, WindowsSyncRootRegistrationOptions,
};
pub use waiter_registry::{
    WindowsFetchDataCancellationOutcome, WindowsFetchDataWaiterMetrics,
    WindowsFetchDataWaiterRegistry, WindowsFetchDataWatchdogOutcome,
};
pub use watchdog::{
    WINDOWS_CFAPI_CALLBACK_TIMEOUT, WindowsFetchDataWatchdog, WindowsFetchDataWatchdogConfig,
};
