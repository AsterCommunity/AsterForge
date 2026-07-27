//! Windows-only CFAPI callback connection and trampoline ownership.

use std::{
    ffi::{OsStr, OsString, c_void},
    mem::{offset_of, size_of},
    os::windows::ffi::{OsStrExt, OsStringExt},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr,
    sync::mpsc::{SyncSender, TrySendError},
};

use aster_forge_cloud_files_core::{SessionGeneration, SessionState};
use windows::{
    Win32::{
        Foundation::{
            NTSTATUS, STATUS_CLOUD_FILE_ACCESS_DENIED, STATUS_CLOUD_FILE_AUTHENTICATION_FAILED,
            STATUS_CLOUD_FILE_INSUFFICIENT_RESOURCES, STATUS_CLOUD_FILE_INVALID_REQUEST,
            STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE, STATUS_CLOUD_FILE_NOT_IN_SYNC,
            STATUS_CLOUD_FILE_NOT_SUPPORTED, STATUS_CLOUD_FILE_PROVIDER_TERMINATED,
            STATUS_CLOUD_FILE_UNSUCCESSFUL, STATUS_SUCCESS,
        },
        Storage::{
            CloudFilters::{
                CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_PARAMETERS_0_0,
                CF_CALLBACK_PARAMETERS_0_0_0_0, CF_CALLBACK_PARAMETERS_0_1,
                CF_CALLBACK_PARAMETERS_0_2, CF_CALLBACK_PARAMETERS_0_6, CF_CALLBACK_PARAMETERS_0_7,
                CF_CALLBACK_PARAMETERS_0_8, CF_CALLBACK_PARAMETERS_0_9,
                CF_CALLBACK_PARAMETERS_0_10, CF_CALLBACK_PARAMETERS_0_11, CF_CALLBACK_REGISTRATION,
                CF_CALLBACK_TYPE_CANCEL_FETCH_DATA, CF_CALLBACK_TYPE_FETCH_DATA,
                CF_CALLBACK_TYPE_NONE, CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE,
                CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE_COMPLETION, CF_CALLBACK_TYPE_NOTIFY_DELETE,
                CF_CALLBACK_TYPE_NOTIFY_DELETE_COMPLETION, CF_CALLBACK_TYPE_NOTIFY_RENAME,
                CF_CALLBACK_TYPE_NOTIFY_RENAME_COMPLETION, CF_CALLBACK_TYPE_VALIDATE_DATA,
                CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION, CF_CONNECT_FLAG_NONE,
                CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH, CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO,
                CF_CONNECT_FLAGS, CF_CONNECTION_KEY, CF_FS_METADATA,
                CF_OPERATION_ACK_DATA_FLAG_NONE, CF_OPERATION_ACK_DEHYDRATE_FLAG_NONE,
                CF_OPERATION_ACK_DELETE_FLAG_NONE, CF_OPERATION_ACK_RENAME_FLAG_NONE,
                CF_OPERATION_INFO, CF_OPERATION_PARAMETERS, CF_OPERATION_PARAMETERS_0,
                CF_OPERATION_PARAMETERS_0_0, CF_OPERATION_PARAMETERS_0_2,
                CF_OPERATION_PARAMETERS_0_3, CF_OPERATION_PARAMETERS_0_5,
                CF_OPERATION_PARAMETERS_0_6, CF_OPERATION_PARAMETERS_0_7,
                CF_OPERATION_RESTART_HYDRATION_FLAG_MARK_IN_SYNC,
                CF_OPERATION_RESTART_HYDRATION_FLAG_NONE, CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CF_OPERATION_TYPE_ACK_DATA, CF_OPERATION_TYPE_ACK_DEHYDRATE,
                CF_OPERATION_TYPE_ACK_DELETE, CF_OPERATION_TYPE_ACK_RENAME,
                CF_OPERATION_TYPE_RESTART_HYDRATION, CF_OPERATION_TYPE_TRANSFER_DATA,
                CF_PROCESS_INFO, CfConnectSyncRoot, CfDisconnectSyncRoot, CfExecute,
                CfReportProviderProgress,
            },
            FileSystem::FILE_BASIC_INFO,
        },
    },
    core::PCWSTR,
};

use crate::{
    CFAPI_FILE_IDENTITY_MAX_BYTES, CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES, Result,
    WindowsCallbackInfoSnapshot, WindowsCallbackQueueMetrics, WindowsCallbackRange,
    WindowsCallbackRequest, WindowsCancelFetchDataFlags, WindowsCancelFetchDataSnapshot,
    WindowsCloudFilesError, WindowsConnectionKey, WindowsConnectionSession,
    WindowsFetchDataFailure, WindowsFetchDataFlags, WindowsFetchDataProgress,
    WindowsFetchDataSnapshot, WindowsFetchDataTransfer, WindowsFetchDataWaiterRegistry,
    WindowsFileIdentity, WindowsObservedNotification, WindowsPreflightSnapshot,
    WindowsProcessInfoSnapshot, WindowsRequestKey, WindowsRestartHydration,
    WindowsSyncRootConnectOptions, WindowsSyncRootIdentity, WindowsTransferKey,
};

const CALLBACK_PARAMETERS_UNION_OFFSET: usize = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous);
const FETCH_PARAMETERS_SIZE: usize =
    CALLBACK_PARAMETERS_UNION_OFFSET + size_of::<CF_CALLBACK_PARAMETERS_0_1>();
const CANCEL_FETCH_PARAMETERS_SIZE: usize = CALLBACK_PARAMETERS_UNION_OFFSET
    + offset_of!(CF_CALLBACK_PARAMETERS_0_0, Anonymous)
    + size_of::<CF_CALLBACK_PARAMETERS_0_0_0_0>();
const OPERATION_PARAMETERS_UNION_OFFSET: usize = offset_of!(CF_OPERATION_PARAMETERS, Anonymous);
const TRANSFER_DATA_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_0>();
const RESTART_HYDRATION_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_3>();
const ACK_DEHYDRATE_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_5>();
const ACK_RENAME_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_6>();
const ACK_DELETE_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_7>();
const ACK_DATA_PARAMETERS_SIZE: usize =
    OPERATION_PARAMETERS_UNION_OFFSET + size_of::<CF_OPERATION_PARAMETERS_0_2>();
const MAX_CALLBACK_WIDE_UNITS: usize = 32_767;

static CALLBACK_TABLE: [CF_CALLBACK_REGISTRATION; 10] = [
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_FETCH_DATA,
        Callback: Some(fetch_data_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_CANCEL_FETCH_DATA,
        Callback: Some(cancel_fetch_data_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_VALIDATE_DATA,
        Callback: Some(validate_data_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE,
        Callback: Some(dehydrate_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE_COMPLETION,
        Callback: Some(dehydrate_completion_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_DELETE,
        Callback: Some(delete_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_DELETE_COMPLETION,
        Callback: Some(delete_completion_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
        Callback: Some(rename_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NOTIFY_RENAME_COMPLETION,
        Callback: Some(rename_completion_callback),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NONE,
        Callback: None,
    },
];

#[derive(Debug, Clone)]
struct CallbackContext {
    session: WindowsConnectionSession,
    sender: SyncSender<WindowsCallbackRequest>,
    waiters: WindowsFetchDataWaiterRegistry,
}

/// Outcome of an explicit connection disconnect request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsDisconnectOutcome {
    /// This call performed the one native disconnect attempt.
    Disconnected,
    /// This connection had already attempted native disconnect.
    AlreadyDisconnected,
}

/// Owned active CFAPI connection with exactly-once RAII disconnect.
///
/// Dropping the value starts the closing fence before calling `CfDisconnectSyncRoot`. Accepted
/// callback snapshots remain alive through their independent session leases and never borrow this
/// native context.
pub struct WindowsSyncRootConnection {
    connection_key: WindowsConnectionKey,
    session: WindowsConnectionSession,
    waiters: WindowsFetchDataWaiterRegistry,
    context: Option<Box<CallbackContext>>,
    disconnect_attempted: bool,
}

impl WindowsSyncRootConnection {
    /// Returns the opaque native connection key.
    pub const fn connection_key(&self) -> WindowsConnectionKey {
        self.connection_key
    }

    /// Returns the durable generation fence owned by this connection.
    pub fn generation(&self) -> SessionGeneration {
        self.session.generation()
    }

    /// Returns the current lifecycle state without exposing transition authority.
    pub fn state(&self) -> SessionState {
        self.session.state()
    }

    /// Returns the number of accepted callbacks still draining.
    pub fn active_callbacks(&self) -> usize {
        self.session.active_callbacks()
    }

    /// Returns callback ingress counters for this native connection.
    pub fn queue_metrics(&self) -> WindowsCallbackQueueMetrics {
        self.session.queue_metrics()
    }

    /// Returns the registry workers use to make pending hydration cancellable by CFAPI callbacks.
    pub fn fetch_data_waiters(&self) -> WindowsFetchDataWaiterRegistry {
        self.waiters.clone()
    }

    /// Explicitly disconnects this connection at most once.
    pub fn disconnect(&mut self) -> Result<WindowsDisconnectOutcome> {
        self.disconnect_inner()
    }

    fn disconnect_inner(&mut self) -> Result<WindowsDisconnectOutcome> {
        if self.disconnect_attempted {
            return Ok(WindowsDisconnectOutcome::AlreadyDisconnected);
        }
        self.disconnect_attempted = true;
        self.session.begin_closing();

        let result = unsafe {
            // SAFETY: `connection_key` came directly from a successful `CfConnectSyncRoot` call
            // owned by this value, and `disconnect_attempted` prevents duplicate native calls.
            CfDisconnectSyncRoot(CF_CONNECTION_KEY(self.connection_key.get()))
        };
        match result {
            Ok(()) => {
                self.session.mark_disconnected()?;
                self.context.take();
                Ok(WindowsDisconnectOutcome::Disconnected)
            }
            Err(error) => {
                if let Some(context) = self.context.take() {
                    // A failed disconnect may leave the platform callback channel active. Keep the
                    // stable callback context alive until process exit rather than freeing memory
                    // that CFAPI could still reference.
                    let _ = Box::leak(context);
                }
                Err(error.into())
            }
        }
    }
}

impl std::fmt::Debug for WindowsSyncRootConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsSyncRootConnection")
            .field("connection_key", &self.connection_key)
            .field("generation", &self.session.generation())
            .field("state", &self.session.state())
            .field("disconnect_attempted", &self.disconnect_attempted)
            .finish()
    }
}

impl Drop for WindowsSyncRootConnection {
    fn drop(&mut self) {
        let _ = self.disconnect_inner();
    }
}

/// Connects a registered sync root and registers bounded `FETCH_DATA`/`CANCEL_FETCH_DATA` ingress.
///
/// The supplied synchronous channel should be bounded. Callback threads only copy native values,
/// acquire a generation lease, and call `try_send`; they never wait for remote I/O or a database.
pub fn connect_sync_root(
    sync_root_path: &Path,
    generation: SessionGeneration,
    sender: SyncSender<WindowsCallbackRequest>,
    options: WindowsSyncRootConnectOptions,
) -> Result<WindowsSyncRootConnection> {
    validate_sync_root_path(sync_root_path)?;
    let path = wide_null(sync_root_path.as_os_str())?;
    let session = WindowsConnectionSession::new(generation);
    let waiters = WindowsFetchDataWaiterRegistry::new();
    let context = Box::new(CallbackContext {
        session: session.clone(),
        sender,
        waiters: waiters.clone(),
    });
    let context_pointer = (&raw const *context).cast::<c_void>();
    let connection_key = unsafe {
        // SAFETY: the callback table is static and sentinel-terminated; the context has a stable
        // Box allocation that remains alive until `CfDisconnectSyncRoot` returns successfully;
        // the UTF-16 path remains alive for the complete synchronous connect call.
        CfConnectSyncRoot(
            PCWSTR(path.as_ptr()),
            CALLBACK_TABLE.as_ptr(),
            Some(context_pointer),
            native_connect_flags(options),
        )
    }?;
    Ok(WindowsSyncRootConnection {
        connection_key: WindowsConnectionKey::new(connection_key.0),
        session,
        waiters,
        context: Some(context),
        disconnect_attempted: false,
    })
}

unsafe extern "system" fn fetch_data_callback(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        handle_fetch_data(callback_info, callback_parameters);
    }));
    if outcome.is_err() {
        // The context may be unavailable for a malformed native callback, so the raw fallback is
        // still the only terminal operation available here. A valid callback context still gets a
        // process-local panic counter for diagnostics.
        // SAFETY: CFAPI owns the callback structures until this trampoline returns; the helper
        // validates the leading structure size before reading the context pointer.
        unsafe { record_panic_metric(callback_info) };
        // SAFETY: CFAPI owns both callback pointers for the duration of this callback. The helper
        // copies only size-validated scalar data and stores no native borrow.
        unsafe {
            fail_fetch_from_raw(
                callback_info,
                callback_parameters,
                STATUS_CLOUD_FILE_UNSUCCESSFUL,
            );
        }
    }
}

unsafe fn record_panic_metric(callback_info: *const CF_CALLBACK_INFO) {
    // SAFETY: the trampoline guarantees callback-scoped readability, and the reader validates
    // `StructSize` before copying the structure.
    let Ok(info) = (unsafe { read_callback_info(callback_info) }) else {
        return;
    };
    // SAFETY: the callback info came from CFAPI and contains the exact context pointer supplied by
    // `CfConnectSyncRoot`; the connection keeps that Box allocation alive through the callback.
    let Ok(context) = (unsafe { clone_callback_context(info.CallbackContext) }) else {
        return;
    };
    context.session.record_panic_failure();
}

unsafe extern "system" fn cancel_fetch_data_callback(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        handle_cancel_fetch_data(callback_info, callback_parameters);
    }));
    if outcome.is_err() {
        // SAFETY: identical callback-lifetime and size-validation contract to the fetch
        // trampoline's panic metric path.
        unsafe { record_panic_metric(callback_info) };
    }
}

#[derive(Debug, Clone, Copy)]
enum PreflightKind {
    ValidateData,
    Dehydrate,
    Delete,
    Rename,
}

#[derive(Debug, Clone, Copy)]
enum ObservationKind {
    Dehydrate,
    Delete,
    Rename,
}

macro_rules! preflight_callback {
    ($name:ident, $kind:expr) => {
        unsafe extern "system" fn $name(
            callback_info: *const CF_CALLBACK_INFO,
            callback_parameters: *const CF_CALLBACK_PARAMETERS,
        ) {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                handle_preflight(callback_info, callback_parameters, $kind);
            }));
            if outcome.is_err() {
                // SAFETY: CFAPI keeps callback info readable until this trampoline returns; the
                // helper validates `StructSize` before copying the context pointer.
                unsafe { record_panic_metric(callback_info) };
                // SAFETY: the same callback lifetime remains active; the fallback independently
                // validates `StructSize` before copying native correlation keys.
                unsafe { fail_preflight_from_raw(callback_info, $kind) };
            }
        }
    };
}

macro_rules! observation_callback {
    ($name:ident, $kind:expr) => {
        unsafe extern "system" fn $name(
            callback_info: *const CF_CALLBACK_INFO,
            callback_parameters: *const CF_CALLBACK_PARAMETERS,
        ) {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                handle_observation(callback_info, callback_parameters, $kind);
            }));
            if outcome.is_err() {
                // SAFETY: CFAPI keeps callback info readable until the trampoline returns and the
                // helper validates `StructSize` before accessing the owned context handle.
                unsafe { record_panic_metric(callback_info) };
            }
        }
    };
}

preflight_callback!(dehydrate_callback, PreflightKind::Dehydrate);
preflight_callback!(validate_data_callback, PreflightKind::ValidateData);
preflight_callback!(delete_callback, PreflightKind::Delete);
preflight_callback!(rename_callback, PreflightKind::Rename);
observation_callback!(dehydrate_completion_callback, ObservationKind::Dehydrate);
observation_callback!(delete_completion_callback, ObservationKind::Delete);
observation_callback!(rename_completion_callback, ObservationKind::Rename);

fn handle_preflight(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
    kind: PreflightKind,
) {
    // SAFETY: this handler runs only from a registered CFAPI trampoline; callback allocations and
    // every pointer-backed member remain readable until that trampoline returns.
    let result = unsafe { snapshot_preflight(callback_info, callback_parameters, kind) };
    let (context, snapshot) = match result {
        Ok(value) => value,
        Err(_) => {
            // SAFETY: the callback lifetime still holds and the raw fallback validates
            // `StructSize` before copying native correlation keys.
            unsafe { fail_preflight_from_raw(callback_info, kind) };
            return;
        }
    };
    let lease = match context.session.begin_callback(snapshot.info().generation()) {
        Ok(lease) => lease,
        Err(_) => {
            let _ = complete_preflight(
                &snapshot,
                WindowsFetchDataFailure::ProviderTerminated,
                false,
            );
            return;
        }
    };
    context.session.record_accepted_callback();
    let request = match WindowsCallbackRequest::preflight(snapshot, lease) {
        Ok(request) => request,
        Err(_) => return,
    };
    match context.sender.try_send(request) {
        Ok(()) => context.session.record_queued_preflight(),
        Err(TrySendError::Full(mut request)) => {
            context.session.record_preflight_queue_full();
            fail_preflight_request(&mut request, WindowsFetchDataFailure::InsufficientResources);
        }
        Err(TrySendError::Disconnected(mut request)) => {
            context.session.record_receiver_disconnected();
            fail_preflight_request(&mut request, WindowsFetchDataFailure::ProviderTerminated);
        }
    }
}

fn handle_observation(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
    kind: ObservationKind,
) {
    // SAFETY: identical callback-scoped ownership contract to `handle_preflight`.
    let Ok((context, notification)) =
        (unsafe { snapshot_observation(callback_info, callback_parameters, kind) })
    else {
        return;
    };
    let Ok(lease) = context
        .session
        .begin_callback(notification.info().generation())
    else {
        return;
    };
    context.session.record_accepted_callback();
    let Ok(request) = WindowsCallbackRequest::observation(notification, lease) else {
        return;
    };
    match context.sender.try_send(request) {
        Ok(()) => context.session.record_queued_observation(),
        Err(TrySendError::Full(_)) => context.session.record_observation_queue_full(),
        Err(TrySendError::Disconnected(_)) => context.session.record_receiver_disconnected(),
    }
}

fn handle_fetch_data(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: this handler is reachable only from the registered CFAPI callback. CFAPI keeps both
    // callback structures and all pointer-backed fields readable until the trampoline returns.
    let result = unsafe { snapshot_fetch_data(callback_info, callback_parameters) };
    let (context, snapshot) = match result {
        Ok(value) => value,
        Err(_) => {
            // SAFETY: the pointers retain the callback-scoped validity described above. The
            // fallback copies only size-validated scalar keys and range data.
            unsafe {
                fail_fetch_from_raw(
                    callback_info,
                    callback_parameters,
                    STATUS_CLOUD_FILE_INVALID_REQUEST,
                )
            };
            return;
        }
    };
    let lease = match context.session.begin_callback(snapshot.info().generation()) {
        Ok(lease) => lease,
        Err(WindowsCloudFilesError::ConnectionNotAccepting { state }) => {
            context.session.record_closing_rejection();
            let status = if state == SessionState::Closing
                || state == SessionState::Draining
                || state == SessionState::Closed
            {
                STATUS_CLOUD_FILE_PROVIDER_TERMINATED
            } else {
                STATUS_CLOUD_FILE_UNSUCCESSFUL
            };
            let _ = fail_fetch_snapshot(&snapshot, status);
            return;
        }
        Err(_) => {
            context.session.record_invalid_snapshot();
            let _ = fail_fetch_snapshot(&snapshot, STATUS_CLOUD_FILE_INVALID_REQUEST);
            return;
        }
    };
    context.session.record_accepted_callback();
    let request = match WindowsCallbackRequest::fetch_data(snapshot, lease) {
        Ok(request) => request,
        Err(_) => return,
    };
    match context.sender.try_send(request) {
        Ok(()) => context.session.record_queued_fetch(),
        Err(TrySendError::Full(mut request)) => {
            context.session.record_fetch_queue_full();
            fail_fetch_request(&mut request, WindowsFetchDataFailure::InsufficientResources);
        }
        Err(TrySendError::Disconnected(mut request)) => {
            context.session.record_receiver_disconnected();
            fail_fetch_request(&mut request, WindowsFetchDataFailure::ProviderTerminated);
        }
    }
}

fn handle_cancel_fetch_data(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: this handler is reachable only from the registered CFAPI callback. CFAPI keeps both
    // callback structures and all pointer-backed fields readable until the trampoline returns.
    let Ok((context, snapshot)) =
        (unsafe { snapshot_cancel_fetch_data(callback_info, callback_parameters) })
    else {
        return;
    };
    // Cancellation matching is bounded, lock-local work and must not depend on the observation
    // channel having capacity. The worker owns all remote/backend cancellation effects through the
    // core cancellation handle registered for its waiter.
    let _ = context.waiters.cancel(&snapshot);
    let Ok(lease) = context.session.begin_callback(snapshot.info().generation()) else {
        context.session.record_closing_rejection();
        return;
    };
    context.session.record_accepted_callback();
    let Ok(request) = WindowsCallbackRequest::cancel_fetch_data(snapshot, lease) else {
        return;
    };
    match context.sender.try_send(request) {
        Ok(()) => context.session.record_queued_cancel(),
        Err(TrySendError::Full(_)) => context.session.record_cancel_queue_full(),
        Err(TrySendError::Disconnected(_)) => context.session.record_receiver_disconnected(),
    }
}

unsafe fn snapshot_fetch_data(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) -> Result<(CallbackContext, WindowsFetchDataSnapshot)> {
    // SAFETY: the caller is the registered FETCH_DATA trampoline, and CFAPI keeps callback info
    // plus every pointer-backed field readable until that trampoline returns.
    let (context, info) = unsafe { snapshot_info(callback_info)? };
    // SAFETY: the caller is the registered FETCH_DATA trampoline, and CFAPI keeps the parameter
    // buffer readable for its reported `ParamSize` until that trampoline returns.
    let fetch = unsafe { read_fetch_parameters(callback_parameters)? };
    let required_range =
        WindowsCallbackRange::from_cfapi(fetch.RequiredFileOffset, fetch.RequiredLength)?;
    let optional_range = match fetch.OptionalLength {
        0 => None,
        length => Some(WindowsCallbackRange::from_cfapi(
            fetch.OptionalFileOffset,
            length,
        )?),
    };
    Ok((
        context,
        WindowsFetchDataSnapshot::new(
            info,
            WindowsFetchDataFlags::from_bits_retain(signed_bits(fetch.Flags.0)),
            required_range,
            optional_range,
            fetch.LastDehydrationTime,
            signed_bits(fetch.LastDehydrationReason.0),
        ),
    ))
}

unsafe fn snapshot_cancel_fetch_data(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) -> Result<(CallbackContext, WindowsCancelFetchDataSnapshot)> {
    // SAFETY: the caller is the registered CANCEL_FETCH_DATA trampoline, and CFAPI keeps callback
    // info plus every pointer-backed field readable until that trampoline returns.
    let (context, info) = unsafe { snapshot_info(callback_info)? };
    // SAFETY: the caller is the registered CANCEL_FETCH_DATA trampoline, and CFAPI keeps the
    // parameter buffer readable for its reported `ParamSize` until that trampoline returns.
    let (cancel_flags, fetch) = unsafe { read_cancel_fetch_parameters(callback_parameters)? };
    Ok((
        context,
        WindowsCancelFetchDataSnapshot::new(
            info,
            WindowsCancelFetchDataFlags::from_bits_retain(signed_bits(cancel_flags)),
            WindowsCallbackRange::from_cfapi(fetch.FileOffset, fetch.Length)?,
        ),
    ))
}

unsafe fn snapshot_preflight(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
    kind: PreflightKind,
) -> Result<(CallbackContext, WindowsPreflightSnapshot)> {
    // SAFETY: the registered trampoline guarantees callback-scoped ownership of callback info.
    let (context, info) = unsafe { snapshot_info(callback_info)? };
    let snapshot = match kind {
        PreflightKind::ValidateData => {
            // SAFETY: the trampoline's active member is ValidateData and `ParamSize` is validated
            // for the exact generated structure extent.
            let value =
                unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_2>(callback_parameters)? };
            WindowsPreflightSnapshot::ValidateData {
                info,
                flags: signed_bits(value.Flags.0),
                range: WindowsCallbackRange::from_cfapi(
                    value.RequiredFileOffset,
                    value.RequiredLength,
                )?,
            }
        }
        PreflightKind::Dehydrate => {
            // SAFETY: the trampoline's active union member is Dehydrate and the generic reader
            // validates `ParamSize` for its exact ABI extent.
            let value =
                unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_6>(callback_parameters)? };
            WindowsPreflightSnapshot::Dehydrate {
                info,
                flags: signed_bits(value.Flags.0),
                reason: signed_bits(value.Reason.0),
            }
        }
        PreflightKind::Delete => {
            // SAFETY: identical contract for the active Delete union member.
            let value =
                unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_8>(callback_parameters)? };
            WindowsPreflightSnapshot::Delete {
                info,
                flags: signed_bits(value.Flags.0),
            }
        }
        PreflightKind::Rename => {
            // SAFETY: identical contract for the active Rename union member.
            let value = unsafe {
                read_callback_member::<CF_CALLBACK_PARAMETERS_0_10>(callback_parameters)?
            };
            // SAFETY: CFAPI keeps the optional target path readable through its NUL terminator
            // until the callback returns; the helper applies a bounded scan.
            let target_path = unsafe { copy_optional_wide(value.TargetPath)? }.map(PathBuf::from);
            WindowsPreflightSnapshot::Rename {
                info,
                flags: signed_bits(value.Flags.0),
                target_path,
            }
        }
    };
    Ok((context, snapshot))
}

unsafe fn snapshot_observation(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
    kind: ObservationKind,
) -> Result<(CallbackContext, WindowsObservedNotification)> {
    // SAFETY: the registered trampoline guarantees callback-scoped ownership of callback info.
    let (context, info) = unsafe { snapshot_info(callback_info)? };
    let notification = match kind {
        ObservationKind::Dehydrate => {
            // SAFETY: the active member is DehydrateCompletion and `ParamSize` is validated.
            let value =
                unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_7>(callback_parameters)? };
            WindowsObservedNotification::DehydrateCompleted {
                info,
                flags: signed_bits(value.Flags.0),
                reason: signed_bits(value.Reason.0),
            }
        }
        ObservationKind::Delete => {
            // SAFETY: the active member is DeleteCompletion and `ParamSize` is validated.
            let value =
                unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_9>(callback_parameters)? };
            WindowsObservedNotification::DeleteCompleted {
                info,
                flags: signed_bits(value.Flags.0),
            }
        }
        ObservationKind::Rename => {
            // SAFETY: the active member is RenameCompletion and `ParamSize` is validated.
            let value = unsafe {
                read_callback_member::<CF_CALLBACK_PARAMETERS_0_11>(callback_parameters)?
            };
            // SAFETY: CFAPI keeps the optional source path readable through its NUL terminator
            // until the callback returns; the helper applies a bounded scan.
            let source_path = unsafe { copy_optional_wide(value.SourcePath)? }.map(PathBuf::from);
            WindowsObservedNotification::RenameCompleted {
                info,
                flags: signed_bits(value.Flags.0),
                source_path,
            }
        }
    };
    Ok((context, notification))
}

unsafe fn read_callback_member<T: Copy>(
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) -> Result<T> {
    if callback_parameters.is_null() {
        return Err(invalid_callback("callback parameters pointer is null"));
    }
    // SAFETY: CFAPI guarantees the callback parameter pointer addresses at least its leading
    // `ParamSize` field for the callback duration.
    let raw_parameter_size = unsafe { callback_parameters.cast::<u32>().read_unaligned() };
    let parameter_size = usize::try_from(raw_parameter_size)
        .map_err(|_| invalid_callback("callback parameter size exceeds usize"))?;
    let required_size = CALLBACK_PARAMETERS_UNION_OFFSET
        .checked_add(size_of::<T>())
        .ok_or_else(|| invalid_callback("callback parameter member size exceeds usize"))?;
    if parameter_size < required_size {
        return Err(invalid_callback("callback parameter member is truncated"));
    }
    let member = callback_parameters
        .cast::<u8>()
        .wrapping_add(CALLBACK_PARAMETERS_UNION_OFFSET)
        .cast::<T>();
    // SAFETY: the validated `ParamSize` covers the complete active member at the union ABI offset,
    // and CFAPI keeps the allocation readable for this callback.
    Ok(unsafe { member.read_unaligned() })
}

unsafe fn read_fetch_parameters(
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) -> Result<CF_CALLBACK_PARAMETERS_0_1> {
    if callback_parameters.is_null() {
        return Err(invalid_callback("callback parameters pointer is null"));
    }
    // SAFETY: CFAPI guarantees the callback parameter pointer addresses at least its leading
    // `ParamSize` field for the callback duration.
    let raw_parameter_size = unsafe { callback_parameters.cast::<u32>().read_unaligned() };
    let parameter_size = usize::try_from(raw_parameter_size)
        .map_err(|_| invalid_callback("FETCH_DATA parameter size exceeds usize"))?;
    if parameter_size < FETCH_PARAMETERS_SIZE {
        return Err(invalid_callback("FETCH_DATA parameter size is truncated"));
    }
    let fetch_pointer = callback_parameters
        .cast::<u8>()
        .wrapping_add(CALLBACK_PARAMETERS_UNION_OFFSET)
        .cast::<CF_CALLBACK_PARAMETERS_0_1>();
    // SAFETY: the validated `ParamSize` covers the complete `Anonymous.FetchData` member at its
    // exact ABI offset, and CFAPI keeps that parameter buffer readable for this callback.
    Ok(unsafe { fetch_pointer.read_unaligned() })
}

unsafe fn read_cancel_fetch_parameters(
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
) -> Result<(i32, CF_CALLBACK_PARAMETERS_0_0_0_0)> {
    if callback_parameters.is_null() {
        return Err(invalid_callback("callback parameters pointer is null"));
    }
    // SAFETY: CFAPI guarantees the callback parameter pointer addresses at least its leading
    // `ParamSize` field for the callback duration.
    let raw_parameter_size = unsafe { callback_parameters.cast::<u32>().read_unaligned() };
    let parameter_size = usize::try_from(raw_parameter_size)
        .map_err(|_| invalid_callback("CANCEL_FETCH_DATA parameter size exceeds usize"))?;
    if parameter_size < CANCEL_FETCH_PARAMETERS_SIZE {
        return Err(invalid_callback(
            "CANCEL_FETCH_DATA parameter size is truncated",
        ));
    }
    let cancel_base = callback_parameters
        .cast::<u8>()
        .wrapping_add(CALLBACK_PARAMETERS_UNION_OFFSET);
    // SAFETY: the validated `ParamSize` covers the leading cancel flags at the union's exact ABI
    // offset, and CFAPI keeps the parameter buffer readable for this callback.
    let flags = unsafe { cancel_base.cast::<i32>().read_unaligned() };
    let fetch_pointer = cancel_base
        .wrapping_add(offset_of!(CF_CALLBACK_PARAMETERS_0_0, Anonymous))
        .cast::<CF_CALLBACK_PARAMETERS_0_0_0_0>();
    // SAFETY: the validated `ParamSize` covers the complete nested cancel FetchData member at its
    // exact ABI offset, and CFAPI keeps the parameter buffer readable for this callback.
    let fetch = unsafe { fetch_pointer.read_unaligned() };
    Ok((flags, fetch))
}

unsafe fn snapshot_info(
    callback_info: *const CF_CALLBACK_INFO,
) -> Result<(CallbackContext, WindowsCallbackInfoSnapshot)> {
    // SAFETY: the caller is a registered CFAPI trampoline, and CFAPI keeps the info allocation
    // readable for its reported `StructSize` until that trampoline returns.
    let info = unsafe { read_callback_info(callback_info)? };
    // SAFETY: CFAPI returns the exact Box-backed context pointer supplied to `CfConnectSyncRoot`.
    // The allocation remains alive through every callback and until a successful disconnect
    // returns; a failed disconnect intentionally retains it.
    let context = unsafe { clone_callback_context(info.CallbackContext)? };

    let sync_root_identity_length = usize::try_from(info.SyncRootIdentityLength)
        .map_err(|_| invalid_callback("sync-root identity length exceeds usize"))?;
    if sync_root_identity_length > CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES {
        return Err(WindowsCloudFilesError::SyncRootIdentityTooLarge {
            actual: sync_root_identity_length,
            maximum: CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
        });
    }
    let file_identity_length = usize::try_from(info.FileIdentityLength)
        .map_err(|_| invalid_callback("file identity length exceeds usize"))?;
    if file_identity_length > CFAPI_FILE_IDENTITY_MAX_BYTES {
        return Err(WindowsCloudFilesError::FileIdentityTooLarge {
            actual: file_identity_length,
            maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
        });
    }
    // SAFETY: the length was bounded by the documented CFAPI sync-root identity limit, and CFAPI
    // keeps the reported identity buffer readable for the callback duration.
    let sync_root_identity_bytes = unsafe {
        copy_bytes(
            info.SyncRootIdentity,
            sync_root_identity_length,
            "sync-root identity pointer is null",
        )?
    };
    let sync_root_identity = WindowsSyncRootIdentity::from_bytes(sync_root_identity_bytes)?;
    // SAFETY: the length was bounded by the documented CFAPI file identity limit, and CFAPI keeps
    // the reported identity buffer readable for the callback duration.
    let file_identity_bytes = unsafe {
        copy_bytes(
            info.FileIdentity,
            file_identity_length,
            "file identity pointer is null",
        )?
    };
    let file_identity = WindowsFileIdentity::from_bytes(file_identity_bytes)?;
    let file_size = u64::try_from(info.FileSize)
        .map_err(|_| invalid_callback("callback file size must be non-negative"))?;
    // SAFETY: CFAPI keeps each optional callback PCWSTR readable through its NUL terminator until
    // the callback returns. The helper also applies an explicit maximum scan length.
    let volume_guid_name = unsafe { copy_optional_wide(info.VolumeGuidName)? };
    // SAFETY: identical callback-lifetime and bounded-scan contract to `VolumeGuidName`.
    let volume_dos_name = unsafe { copy_optional_wide(info.VolumeDosName)? };
    // SAFETY: identical callback-lifetime and bounded-scan contract to the other callback strings.
    let normalized_path = unsafe { copy_optional_wide(info.NormalizedPath)? }.map(PathBuf::from);
    // SAFETY: CFAPI keeps the optional process structure and its nested strings readable for the
    // callback duration; the helper validates `StructSize` before copying the full structure.
    let process = unsafe { copy_process_info(info.ProcessInfo)? };

    let generation = context.session.generation();
    Ok((
        context,
        WindowsCallbackInfoSnapshot::new(
            generation,
            WindowsConnectionKey::new(info.ConnectionKey.0),
            WindowsTransferKey::new(info.TransferKey),
            WindowsRequestKey::new(info.RequestKey),
            volume_guid_name,
            volume_dos_name,
            info.VolumeSerialNumber,
            info.SyncRootFileId,
            sync_root_identity,
            info.FileId,
            file_size,
            file_identity,
            normalized_path,
            info.PriorityHint,
            process,
        )?,
    ))
}

unsafe fn clone_callback_context(pointer: *mut c_void) -> Result<CallbackContext> {
    if pointer.is_null() {
        return Err(invalid_callback("callback context pointer is null"));
    }
    // SAFETY: the caller guarantees this is the exact Box-backed `CallbackContext` pointer passed
    // to CFAPI and that the allocation remains alive for the current callback. The reference is
    // used only to clone owned handles and does not escape.
    let context = unsafe { &*pointer.cast::<CallbackContext>() };
    Ok(context.clone())
}

unsafe fn read_callback_info(callback_info: *const CF_CALLBACK_INFO) -> Result<CF_CALLBACK_INFO> {
    if callback_info.is_null() {
        return Err(invalid_callback("callback info pointer is null"));
    }
    // SAFETY: CFAPI guarantees a callback-info pointer addresses at least its leading `StructSize`
    // field for the callback duration.
    let raw_structure_size = unsafe { callback_info.cast::<u32>().read_unaligned() };
    let structure_size = usize::try_from(raw_structure_size)
        .map_err(|_| invalid_callback("callback info structure size exceeds usize"))?;
    if structure_size < size_of::<CF_CALLBACK_INFO>() {
        return Err(invalid_callback("callback info structure is truncated"));
    }
    // SAFETY: the validated `StructSize` covers the complete `CF_CALLBACK_INFO`, and CFAPI keeps
    // the allocation readable for the callback duration.
    Ok(unsafe { callback_info.read_unaligned() })
}

unsafe fn copy_bytes(
    pointer: *const c_void,
    length: usize,
    null_reason: &'static str,
) -> Result<Vec<u8>> {
    if length == 0 {
        return Err(invalid_callback("callback identity must be non-empty"));
    }
    if pointer.is_null() {
        return Err(invalid_callback(null_reason));
    }
    // SAFETY: `snapshot_info` has already bounded `length` by the documented CFAPI identity limit,
    // and its callback ABI precondition guarantees the non-null identity buffer is readable for
    // exactly the reported length. `to_vec` immediately detaches the owned bytes.
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
    Ok(bytes.to_vec())
}

unsafe fn copy_optional_wide(value: PCWSTR) -> Result<Option<OsString>> {
    if value.is_null() {
        return Ok(None);
    }
    let pointer = value.as_ptr();
    for length in 0..=MAX_CALLBACK_WIDE_UNITS {
        // SAFETY: the caller's CFAPI ABI precondition guarantees the string remains readable
        // through its terminating NUL. This block reads exactly one UTF-16 unit within the cap.
        let unit = unsafe { pointer.add(length).read() };
        if unit == 0 {
            // SAFETY: every unit in this range was read successfully while searching for the NUL,
            // and the callback still owns the unchanged string allocation.
            let units = unsafe { std::slice::from_raw_parts(pointer, length) };
            return Ok(Some(OsString::from_wide(units)));
        }
    }
    Err(invalid_callback(
        "callback wide string exceeds 32767 UTF-16 units or lacks a terminator",
    ))
}

unsafe fn copy_process_info(
    pointer: *mut CF_PROCESS_INFO,
) -> Result<Option<WindowsProcessInfoSnapshot>> {
    if pointer.is_null() {
        return Ok(None);
    }
    // SAFETY: CFAPI guarantees a non-null process-info pointer addresses at least its leading
    // `StructSize` field for the callback duration.
    let raw_structure_size = unsafe { pointer.cast::<u32>().read_unaligned() };
    let structure_size = usize::try_from(raw_structure_size)
        .map_err(|_| invalid_callback("process info structure size exceeds usize"))?;
    if structure_size < size_of::<CF_PROCESS_INFO>() {
        return Err(invalid_callback("process info structure is truncated"));
    }
    // SAFETY: the validated `StructSize` covers the complete `CF_PROCESS_INFO`, and CFAPI keeps
    // the allocation readable for the callback duration.
    let process = unsafe { pointer.read_unaligned() };
    // SAFETY: CFAPI keeps the optional nested process-info PCWSTR readable through its terminating
    // NUL until the callback returns; the helper applies a bounded scan.
    let image_path = unsafe { copy_optional_wide(process.ImagePath)? }.map(PathBuf::from);
    // SAFETY: identical nested-string contract to `ImagePath`.
    let package_name = unsafe { copy_optional_wide(process.PackageName)? };
    // SAFETY: identical nested-string contract to `ImagePath`.
    let application_id = unsafe { copy_optional_wide(process.ApplicationId)? };
    // SAFETY: identical nested-string contract to `ImagePath`.
    let command_line = unsafe { copy_optional_wide(process.CommandLine)? };
    Ok(Some(WindowsProcessInfoSnapshot {
        process_id: process.ProcessId,
        image_path,
        package_name,
        application_id,
        command_line,
        session_id: process.SessionId,
    }))
}

fn fail_fetch_request(request: &mut WindowsCallbackRequest, failure: WindowsFetchDataFailure) {
    if let WindowsCallbackRequest::FetchData(fetch) = request {
        let _ = fetch.fail_inner(failure);
    }
}

fn fail_preflight_request(request: &mut WindowsCallbackRequest, failure: WindowsFetchDataFailure) {
    if let WindowsCallbackRequest::Preflight(preflight) = request {
        let _ = preflight.acknowledge_inner(failure, false);
    }
}

pub(crate) fn complete_preflight(
    snapshot: &WindowsPreflightSnapshot,
    failure: WindowsFetchDataFailure,
    approved: bool,
) -> Result<()> {
    let status = if approved {
        STATUS_SUCCESS
    } else {
        failure_status(failure)
    };
    let kind = match snapshot {
        WindowsPreflightSnapshot::ValidateData { .. } => PreflightKind::ValidateData,
        WindowsPreflightSnapshot::Dehydrate { .. } => PreflightKind::Dehydrate,
        WindowsPreflightSnapshot::Delete { .. } => PreflightKind::Delete,
        WindowsPreflightSnapshot::Rename { .. } => PreflightKind::Rename,
    };
    let info = snapshot.info();
    let validation_range = match snapshot {
        WindowsPreflightSnapshot::ValidateData { range, .. } => Some((
            i64::try_from(range.offset())
                .map_err(|_| invalid_callback("validation offset exceeds i64"))?,
            match range.length() {
                crate::WindowsCallbackRangeLength::Exact(length) => i64::try_from(length)
                    .map_err(|_| invalid_callback("validation length exceeds i64"))?,
                crate::WindowsCallbackRangeLength::ToEnd => -1,
            },
        )),
        _ => None,
    };
    execute_preflight_ack(
        info.connection_key(),
        info.transfer_key(),
        info.request_key(),
        kind,
        Some(info.file_identity()),
        validation_range,
        status,
    )
    .map_err(Into::into)
}

unsafe fn fail_preflight_from_raw(callback_info: *const CF_CALLBACK_INFO, kind: PreflightKind) {
    // SAFETY: the registered trampoline retains callback info through this call and the reader
    // validates `StructSize` before copying the complete structure.
    let Ok(info) = (unsafe { read_callback_info(callback_info) }) else {
        return;
    };
    let _ = execute_preflight_ack(
        WindowsConnectionKey::new(info.ConnectionKey.0),
        WindowsTransferKey::new(info.TransferKey),
        WindowsRequestKey::new(info.RequestKey),
        kind,
        None,
        None,
        STATUS_CLOUD_FILE_INVALID_REQUEST,
    );
}

pub(crate) fn complete_fetch_failure(
    snapshot: &WindowsFetchDataSnapshot,
    failure: WindowsFetchDataFailure,
) -> Result<()> {
    fail_fetch_snapshot(snapshot, failure_status(failure)).map_err(Into::into)
}

pub(crate) fn complete_fetch_success(
    snapshot: &WindowsFetchDataSnapshot,
    transfer: &WindowsFetchDataTransfer,
) -> Result<()> {
    let info = snapshot.info();
    execute_fetch_transfer(
        info.connection_key(),
        info.transfer_key(),
        info.request_key(),
        transfer.bytes().as_ptr().cast(),
        transfer.offset(),
        transfer.length(),
        STATUS_SUCCESS,
    )
    .map_err(Into::into)
}

pub(crate) fn report_fetch_progress(
    info: &WindowsCallbackInfoSnapshot,
    progress: WindowsFetchDataProgress,
) -> Result<()> {
    let (total, completed) = progress.as_cfapi();
    // SAFETY: keys and signed progress values were copied and validated from the active callback
    // snapshot; this call retains no Rust pointer or reference.
    unsafe {
        CfReportProviderProgress(
            CF_CONNECTION_KEY(info.connection_key().get()),
            info.transfer_key().get(),
            total,
            completed,
        )
    }
    .map_err(Into::into)
}

pub(crate) fn restart_fetch_hydration(
    snapshot: &WindowsFetchDataSnapshot,
    replacement: &WindowsRestartHydration,
) -> Result<()> {
    let info = snapshot.info();
    let metadata = replacement.metadata().map(native_fs_metadata);
    let identity = replacement.identity();
    let identity_length = match identity {
        Some(identity) => u32::try_from(identity.len()).map_err(|_| {
            WindowsCloudFilesError::FileIdentityTooLarge {
                actual: identity.len(),
                maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
            }
        })?,
        None => 0,
    };
    let operation_info = CF_OPERATION_INFO {
        StructSize: u32::try_from(size_of::<CF_OPERATION_INFO>()).unwrap_or(u32::MAX),
        Type: CF_OPERATION_TYPE_RESTART_HYDRATION,
        ConnectionKey: CF_CONNECTION_KEY(info.connection_key().get()),
        TransferKey: info.transfer_key().get(),
        CorrelationVector: ptr::null(),
        SyncStatus: ptr::null(),
        RequestKey: info.request_key().get(),
    };
    let restart = CF_OPERATION_PARAMETERS_0_3 {
        Flags: if replacement.should_mark_in_sync() {
            CF_OPERATION_RESTART_HYDRATION_FLAG_MARK_IN_SYNC
        } else {
            CF_OPERATION_RESTART_HYDRATION_FLAG_NONE
        },
        FsMetadata: metadata
            .as_ref()
            .map_or(ptr::null(), |metadata| metadata as *const CF_FS_METADATA),
        FileIdentity: identity.map_or(ptr::null(), |identity| identity.as_bytes().as_ptr().cast()),
        FileIdentityLength: identity_length,
    };
    let mut operation_parameters = CF_OPERATION_PARAMETERS {
        ParamSize: u32::try_from(RESTART_HYDRATION_PARAMETERS_SIZE).unwrap_or(u32::MAX),
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            RestartHydration: restart,
        },
    };
    // SAFETY: all operation fields are initialized for `RESTART_HYDRATION`; optional metadata and
    // identity are owned by this stack frame and remain alive through the synchronous call. CFAPI
    // retains no pointer passed by this operation.
    unsafe { CfExecute(&raw const operation_info, &raw mut operation_parameters) }
        .map_err(Into::into)
}

fn native_fs_metadata(metadata: crate::WindowsPlaceholderMetadata) -> CF_FS_METADATA {
    let times = metadata.times();
    CF_FS_METADATA {
        BasicInfo: FILE_BASIC_INFO {
            CreationTime: times.creation,
            LastAccessTime: times.last_access,
            LastWriteTime: times.last_write,
            ChangeTime: times.change,
            FileAttributes: metadata.file_attributes(),
        },
        FileSize: metadata.file_size(),
    }
}

fn execute_preflight_ack(
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    kind: PreflightKind,
    identity: Option<&WindowsFileIdentity>,
    validation_range: Option<(i64, i64)>,
    status: NTSTATUS,
) -> windows::core::Result<()> {
    let operation_info = CF_OPERATION_INFO {
        StructSize: u32::try_from(size_of::<CF_OPERATION_INFO>()).unwrap_or(u32::MAX),
        Type: match kind {
            PreflightKind::ValidateData => CF_OPERATION_TYPE_ACK_DATA,
            PreflightKind::Dehydrate => CF_OPERATION_TYPE_ACK_DEHYDRATE,
            PreflightKind::Delete => CF_OPERATION_TYPE_ACK_DELETE,
            PreflightKind::Rename => CF_OPERATION_TYPE_ACK_RENAME,
        },
        ConnectionKey: CF_CONNECTION_KEY(connection_key.get()),
        TransferKey: transfer_key.get(),
        CorrelationVector: ptr::null(),
        SyncStatus: ptr::null(),
        RequestKey: request_key.get(),
    };
    let mut operation_parameters = match kind {
        PreflightKind::ValidateData => {
            let (offset, length) = validation_range.unwrap_or((0, -1));
            CF_OPERATION_PARAMETERS {
                ParamSize: u32::try_from(ACK_DATA_PARAMETERS_SIZE).unwrap_or(u32::MAX),
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    AckData: CF_OPERATION_PARAMETERS_0_2 {
                        Flags: CF_OPERATION_ACK_DATA_FLAG_NONE,
                        CompletionStatus: status,
                        Offset: offset,
                        Length: length,
                    },
                },
            }
        }
        PreflightKind::Dehydrate => {
            let identity_length = identity
                .and_then(|identity| u32::try_from(identity.len()).ok())
                .unwrap_or(0);
            CF_OPERATION_PARAMETERS {
                ParamSize: u32::try_from(ACK_DEHYDRATE_PARAMETERS_SIZE).unwrap_or(u32::MAX),
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    AckDehydrate: CF_OPERATION_PARAMETERS_0_5 {
                        Flags: CF_OPERATION_ACK_DEHYDRATE_FLAG_NONE,
                        CompletionStatus: status,
                        FileIdentity: identity
                            .map_or(ptr::null(), |identity| identity.as_bytes().as_ptr().cast()),
                        FileIdentityLength: identity_length,
                    },
                },
            }
        }
        PreflightKind::Delete => CF_OPERATION_PARAMETERS {
            ParamSize: u32::try_from(ACK_DELETE_PARAMETERS_SIZE).unwrap_or(u32::MAX),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckDelete: CF_OPERATION_PARAMETERS_0_7 {
                    Flags: CF_OPERATION_ACK_DELETE_FLAG_NONE,
                    CompletionStatus: status,
                },
            },
        },
        PreflightKind::Rename => CF_OPERATION_PARAMETERS {
            ParamSize: u32::try_from(ACK_RENAME_PARAMETERS_SIZE).unwrap_or(u32::MAX),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckRename: CF_OPERATION_PARAMETERS_0_6 {
                    Flags: CF_OPERATION_ACK_RENAME_FLAG_NONE,
                    CompletionStatus: status,
                },
            },
        },
    };
    // SAFETY: the operation type and active parameter union member match `kind`; optional identity
    // bytes are owned by the callback snapshot and remain alive for the synchronous native call.
    unsafe { CfExecute(&raw const operation_info, &raw mut operation_parameters) }
}

fn fail_fetch_snapshot(
    snapshot: &WindowsFetchDataSnapshot,
    status: NTSTATUS,
) -> windows::core::Result<()> {
    let info = snapshot.info();
    let Some((offset, length)) = failure_range(snapshot.required_range(), info.file_size()) else {
        return Err(windows::core::Error::from_hresult(
            windows::core::HRESULT::from_win32(87),
        ));
    };
    execute_fetch_failure(
        info.connection_key(),
        info.transfer_key(),
        info.request_key(),
        offset,
        length,
        status,
    )
}

unsafe fn fail_fetch_from_raw(
    callback_info: *const CF_CALLBACK_INFO,
    callback_parameters: *const CF_CALLBACK_PARAMETERS,
    status: NTSTATUS,
) {
    // SAFETY: this helper runs before the registered FETCH_DATA trampoline returns, so CFAPI keeps
    // the callback-info allocation readable for its reported `StructSize`.
    let Ok(info) = (unsafe { read_callback_info(callback_info) }) else {
        return;
    };
    // SAFETY: identical callback lifetime; the reader validates `ParamSize` before copying the
    // active FetchData member. A malformed parameter falls back to a conservative failure range.
    let fetch = unsafe { read_fetch_parameters(callback_parameters) }.ok();
    let (offset, length) = raw_failure_range(&info, fetch.as_ref());
    let _ = execute_fetch_failure(
        WindowsConnectionKey::new(info.ConnectionKey.0),
        WindowsTransferKey::new(info.TransferKey),
        WindowsRequestKey::new(info.RequestKey),
        offset,
        length,
        status,
    );
}

fn execute_fetch_failure(
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    offset: i64,
    length: i64,
    status: NTSTATUS,
) -> windows::core::Result<()> {
    execute_fetch_transfer(
        connection_key,
        transfer_key,
        request_key,
        ptr::null(),
        offset,
        length,
        status,
    )
}

fn execute_fetch_transfer(
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    buffer: *const c_void,
    offset: i64,
    length: i64,
    status: NTSTATUS,
) -> windows::core::Result<()> {
    let (operation_info, mut operation_parameters) = fetch_transfer_operation(
        connection_key,
        transfer_key,
        request_key,
        buffer,
        offset,
        length,
        status,
    );
    unsafe {
        // SAFETY: both operation structures are fully initialized for one synchronous
        // `TRANSFER_DATA`. A non-null buffer belongs to the caller-owned transfer and remains alive
        // until `CfExecute` returns; failure paths pass null. No native pointer escapes the call.
        CfExecute(&raw const operation_info, &raw mut operation_parameters)
    }
}

fn fetch_transfer_operation(
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    buffer: *const c_void,
    offset: i64,
    length: i64,
    status: NTSTATUS,
) -> (CF_OPERATION_INFO, CF_OPERATION_PARAMETERS) {
    let operation_info = CF_OPERATION_INFO {
        StructSize: u32::try_from(size_of::<CF_OPERATION_INFO>()).unwrap_or(u32::MAX),
        Type: CF_OPERATION_TYPE_TRANSFER_DATA,
        ConnectionKey: CF_CONNECTION_KEY(connection_key.get()),
        TransferKey: transfer_key.get(),
        CorrelationVector: ptr::null(),
        SyncStatus: ptr::null(),
        RequestKey: request_key.get(),
    };
    let transfer_data = CF_OPERATION_PARAMETERS_0_0 {
        Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
        CompletionStatus: status,
        Buffer: buffer,
        Offset: offset,
        Length: length,
    };
    let operation_parameters = CF_OPERATION_PARAMETERS {
        ParamSize: u32::try_from(TRANSFER_DATA_PARAMETERS_SIZE).unwrap_or(u32::MAX),
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            TransferData: transfer_data,
        },
    };
    (operation_info, operation_parameters)
}

const fn failure_status(failure: WindowsFetchDataFailure) -> NTSTATUS {
    match failure {
        WindowsFetchDataFailure::InvalidRequest => STATUS_CLOUD_FILE_INVALID_REQUEST,
        WindowsFetchDataFailure::InsufficientResources => STATUS_CLOUD_FILE_INSUFFICIENT_RESOURCES,
        WindowsFetchDataFailure::ProviderTerminated => STATUS_CLOUD_FILE_PROVIDER_TERMINATED,
        WindowsFetchDataFailure::AuthenticationFailed => STATUS_CLOUD_FILE_AUTHENTICATION_FAILED,
        WindowsFetchDataFailure::AccessDenied => STATUS_CLOUD_FILE_ACCESS_DENIED,
        WindowsFetchDataFailure::NotInSync => STATUS_CLOUD_FILE_NOT_IN_SYNC,
        WindowsFetchDataFailure::NetworkUnavailable => STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE,
        WindowsFetchDataFailure::NotSupported => STATUS_CLOUD_FILE_NOT_SUPPORTED,
        WindowsFetchDataFailure::Cancelled => {
            windows::Win32::Foundation::STATUS_CLOUD_FILE_REQUEST_CANCELED
        }
        WindowsFetchDataFailure::Unsuccessful => STATUS_CLOUD_FILE_UNSUCCESSFUL,
    }
}

fn failure_range(range: WindowsCallbackRange, file_size: u64) -> Option<(i64, i64)> {
    let offset = i64::try_from(range.offset()).ok()?;
    let length = match range.length() {
        crate::WindowsCallbackRangeLength::Exact(length) => i64::try_from(length).ok()?,
        crate::WindowsCallbackRangeLength::ToEnd => {
            i64::try_from(file_size.checked_sub(range.offset())?).ok()?
        }
    };
    (length > 0).then_some((offset, length))
}

fn raw_failure_range(
    info: &CF_CALLBACK_INFO,
    fetch: Option<&CF_CALLBACK_PARAMETERS_0_1>,
) -> (i64, i64) {
    if let Some(fetch) = fetch
        && let Ok(range) =
            WindowsCallbackRange::from_cfapi(fetch.RequiredFileOffset, fetch.RequiredLength)
        && let Ok(file_size) = u64::try_from(info.FileSize)
        && let Some(range) = failure_range(range, file_size)
    {
        return range;
    }
    (0, info.FileSize.max(4096))
}

fn native_connect_flags(options: WindowsSyncRootConnectOptions) -> CF_CONNECT_FLAGS {
    let mut flags = CF_CONNECT_FLAG_NONE;
    if options.require_process_info {
        flags |= CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO;
    }
    if options.require_full_file_path {
        flags |= CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH;
    }
    if options.block_self_implicit_hydration {
        flags |= CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION;
    }
    flags
}

fn validate_sync_root_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(WindowsCloudFilesError::InvalidSyncRootPath {
            reason: "path must be non-empty",
        });
    }
    Ok(())
}

fn wide_null(value: &OsStr) -> Result<Vec<u16>> {
    let mut encoded = value.encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(WindowsCloudFilesError::EmbeddedNul);
    }
    encoded.push(0);
    Ok(encoded)
}

const fn signed_bits(value: i32) -> u32 {
    u32::from_ne_bytes(value.to_ne_bytes())
}

fn invalid_callback(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidCallbackSnapshot { reason }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, mem::size_of, path::Path, sync::mpsc::sync_channel};

    use aster_forge_cloud_files_core::{
        CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
    };
    use windows::Win32::Storage::CloudFilters::{
        CF_CALLBACK_CANCEL_FLAGS, CF_CALLBACK_DEHYDRATION_REASON, CF_CALLBACK_FETCH_DATA_FLAGS,
        CF_CALLBACK_PARAMETERS_0, CF_CALLBACK_PARAMETERS_0_0_0, CF_PROCESS_INFO,
    };

    use super::*;

    fn generation(value: u64) -> SessionGeneration {
        SessionGeneration::new(value).expect("test generation must be non-zero")
    }

    fn scope() -> CloudScope {
        CloudScope::new(
            CloudNamespaceId::new("namespace").expect("namespace fixture must be valid"),
            CloudRootId::new("root").expect("root fixture must be valid"),
        )
    }

    fn item_key() -> CloudItemKey {
        CloudItemKey::new(
            scope(),
            CloudItemId::new("item").expect("item fixture must be valid"),
        )
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    #[test]
    fn callback_parameter_sizes_include_real_union_padding() {
        assert_eq!(
            FETCH_PARAMETERS_SIZE,
            offset_of!(CF_CALLBACK_PARAMETERS, Anonymous) + size_of::<CF_CALLBACK_PARAMETERS_0_1>()
        );
        assert_eq!(
            CANCEL_FETCH_PARAMETERS_SIZE,
            offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
                + offset_of!(CF_CALLBACK_PARAMETERS_0_0, Anonymous)
                + size_of::<CF_CALLBACK_PARAMETERS_0_0_0_0>()
        );
        assert_eq!(
            TRANSFER_DATA_PARAMETERS_SIZE,
            offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_0>()
        );
        assert!(CALLBACK_PARAMETERS_UNION_OFFSET >= size_of::<u32>());
        assert!(OPERATION_PARAMETERS_UNION_OFFSET >= size_of::<u32>());
    }

    #[test]
    fn callback_table_registers_hydration_validation_and_mutation_notifications() {
        let actual = CALLBACK_TABLE
            .iter()
            .map(|registration| registration.Type)
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            vec![
                CF_CALLBACK_TYPE_FETCH_DATA,
                CF_CALLBACK_TYPE_CANCEL_FETCH_DATA,
                CF_CALLBACK_TYPE_VALIDATE_DATA,
                CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE,
                CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE_COMPLETION,
                CF_CALLBACK_TYPE_NOTIFY_DELETE,
                CF_CALLBACK_TYPE_NOTIFY_DELETE_COMPLETION,
                CF_CALLBACK_TYPE_NOTIFY_RENAME,
                CF_CALLBACK_TYPE_NOTIFY_RENAME_COMPLETION,
                CF_CALLBACK_TYPE_NONE,
            ]
        );
        assert!(
            CALLBACK_TABLE[..CALLBACK_TABLE.len() - 1]
                .iter()
                .all(|registration| registration.Callback.is_some())
        );
        assert!(CALLBACK_TABLE.last().unwrap().Callback.is_none());
    }

    #[test]
    fn generic_callback_member_reader_validates_exact_union_extent() {
        let truncated = u32::try_from(CALLBACK_PARAMETERS_UNION_OFFSET)
            .expect("callback union offset should fit u32");
        // SAFETY: the allocation contains only `ParamSize`; the reader must reject it before
        // copying the requested union member.
        let result = unsafe {
            read_callback_member::<CF_CALLBACK_PARAMETERS_0_10>(
                (&raw const truncated).cast::<CF_CALLBACK_PARAMETERS>(),
            )
        };
        assert!(matches!(
            result,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "callback parameter member is truncated"
            })
        ));

        let expected = CF_CALLBACK_PARAMETERS_0_8 {
            Flags: windows::Win32::Storage::CloudFilters::CF_CALLBACK_DELETE_FLAGS(i32::MIN),
        };
        let parameters = CF_CALLBACK_PARAMETERS {
            ParamSize: u32::try_from(
                CALLBACK_PARAMETERS_UNION_OFFSET + size_of::<CF_CALLBACK_PARAMETERS_0_8>(),
            )
            .expect("delete member extent should fit u32"),
            Anonymous: CF_CALLBACK_PARAMETERS_0 { Delete: expected },
        };
        // SAFETY: this test provides a complete initialized Delete member with exact `ParamSize`.
        let actual =
            unsafe { read_callback_member::<CF_CALLBACK_PARAMETERS_0_8>(&raw const parameters) }
                .expect("complete member should decode");
        assert_eq!(actual, expected);
    }

    #[test]
    fn transfer_operation_preserves_success_buffer_keys_range_and_exact_param_size() {
        let bytes = [0x5a_u8; 4096];
        let (info, parameters) = fetch_transfer_operation(
            WindowsConnectionKey::new(i64::MIN),
            WindowsTransferKey::new(-42),
            WindowsRequestKey::new(i64::MAX),
            bytes.as_ptr().cast(),
            4096,
            4096,
            STATUS_SUCCESS,
        );

        assert_eq!(
            info.StructSize,
            u32::try_from(size_of::<CF_OPERATION_INFO>())
                .expect("operation structure size must fit u32")
        );
        assert_eq!(info.ConnectionKey.0, i64::MIN);
        assert_eq!(info.TransferKey, -42);
        assert_eq!(info.RequestKey, i64::MAX);
        assert_eq!(
            parameters.ParamSize,
            u32::try_from(TRANSFER_DATA_PARAMETERS_SIZE)
                .expect("transfer parameter size must fit u32")
        );
        // SAFETY: `fetch_transfer_operation` initialized the active union member as TransferData.
        let transfer = unsafe { parameters.Anonymous.TransferData };
        assert_eq!(transfer.Buffer, bytes.as_ptr().cast());
        assert_eq!(transfer.Offset, 4096);
        assert_eq!(transfer.Length, 4096);
        assert_eq!(transfer.CompletionStatus, STATUS_SUCCESS);
        assert_eq!(transfer.Flags, CF_OPERATION_TRANSFER_DATA_FLAG_NONE);
    }

    #[test]
    fn transfer_operation_uses_null_buffer_for_terminal_failure() {
        let (_, parameters) = fetch_transfer_operation(
            WindowsConnectionKey::new(1),
            WindowsTransferKey::new(2),
            WindowsRequestKey::new(3),
            ptr::null(),
            0,
            4096,
            STATUS_CLOUD_FILE_INVALID_REQUEST,
        );
        // SAFETY: `fetch_transfer_operation` initialized the active union member as TransferData.
        let transfer = unsafe { parameters.Anonymous.TransferData };
        assert!(transfer.Buffer.is_null());
        assert_eq!(transfer.CompletionStatus, STATUS_CLOUD_FILE_INVALID_REQUEST);
    }

    #[test]
    fn fetch_failure_classifications_map_to_exact_native_statuses() {
        let cases = [
            (
                WindowsFetchDataFailure::InvalidRequest,
                STATUS_CLOUD_FILE_INVALID_REQUEST,
            ),
            (
                WindowsFetchDataFailure::InsufficientResources,
                STATUS_CLOUD_FILE_INSUFFICIENT_RESOURCES,
            ),
            (
                WindowsFetchDataFailure::ProviderTerminated,
                STATUS_CLOUD_FILE_PROVIDER_TERMINATED,
            ),
            (
                WindowsFetchDataFailure::AuthenticationFailed,
                STATUS_CLOUD_FILE_AUTHENTICATION_FAILED,
            ),
            (
                WindowsFetchDataFailure::AccessDenied,
                STATUS_CLOUD_FILE_ACCESS_DENIED,
            ),
            (
                WindowsFetchDataFailure::NotInSync,
                STATUS_CLOUD_FILE_NOT_IN_SYNC,
            ),
            (
                WindowsFetchDataFailure::NetworkUnavailable,
                STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE,
            ),
            (
                WindowsFetchDataFailure::NotSupported,
                STATUS_CLOUD_FILE_NOT_SUPPORTED,
            ),
            (
                WindowsFetchDataFailure::Cancelled,
                windows::Win32::Foundation::STATUS_CLOUD_FILE_REQUEST_CANCELED,
            ),
            (
                WindowsFetchDataFailure::Unsuccessful,
                STATUS_CLOUD_FILE_UNSUCCESSFUL,
            ),
        ];
        for (failure, status) in cases {
            assert_eq!(failure_status(failure), status);
        }
    }

    #[test]
    fn fetch_parameter_reader_rejects_header_only_and_copies_exact_member() {
        let header_only = u32::try_from(CALLBACK_PARAMETERS_UNION_OFFSET)
            .expect("callback parameter offset must fit u32");
        // SAFETY: the test provides exactly the leading `ParamSize` field. The reader must reject
        // it before attempting any full-structure or union-member read.
        let truncated = unsafe {
            read_fetch_parameters((&raw const header_only).cast::<CF_CALLBACK_PARAMETERS>())
        };
        assert!(matches!(
            truncated,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "FETCH_DATA parameter size is truncated"
            })
        ));

        let expected = CF_CALLBACK_PARAMETERS_0_1 {
            Flags: CF_CALLBACK_FETCH_DATA_FLAGS(-1),
            RequiredFileOffset: 4,
            RequiredLength: 8,
            OptionalFileOffset: 0,
            OptionalLength: -1,
            LastDehydrationTime: i64::MIN,
            LastDehydrationReason: CF_CALLBACK_DEHYDRATION_REASON(i32::MIN),
        };
        let parameters = CF_CALLBACK_PARAMETERS {
            ParamSize: u32::try_from(FETCH_PARAMETERS_SIZE)
                .expect("fetch parameter size must fit u32"),
            Anonymous: CF_CALLBACK_PARAMETERS_0 {
                FetchData: expected,
            },
        };
        // SAFETY: this test supplies a complete initialized FetchData parameter with an exact
        // `ParamSize`, and the allocation remains alive for the call.
        let actual = unsafe { read_fetch_parameters(&raw const parameters) }
            .expect("complete fetch parameters must decode");
        assert_eq!(actual, expected);
    }

    #[test]
    fn cancel_parameter_reader_rejects_header_only_and_copies_nested_member() {
        let header_only = u32::try_from(CALLBACK_PARAMETERS_UNION_OFFSET)
            .expect("callback parameter offset must fit u32");
        // SAFETY: the test provides exactly the leading `ParamSize` field. The reader must reject
        // it before attempting either the cancel flags or nested FetchData read.
        let truncated = unsafe {
            read_cancel_fetch_parameters((&raw const header_only).cast::<CF_CALLBACK_PARAMETERS>())
        };
        assert!(matches!(
            truncated,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "CANCEL_FETCH_DATA parameter size is truncated"
            })
        ));

        let expected = CF_CALLBACK_PARAMETERS_0_0_0_0 {
            FileOffset: 16,
            Length: 32,
        };
        let parameters = CF_CALLBACK_PARAMETERS {
            ParamSize: u32::try_from(CANCEL_FETCH_PARAMETERS_SIZE)
                .expect("cancel parameter size must fit u32"),
            Anonymous: CF_CALLBACK_PARAMETERS_0 {
                Cancel: CF_CALLBACK_PARAMETERS_0_0 {
                    Flags: CF_CALLBACK_CANCEL_FLAGS(-1),
                    Anonymous: CF_CALLBACK_PARAMETERS_0_0_0 {
                        FetchData: expected,
                    },
                },
            },
        };
        // SAFETY: this test supplies a complete initialized Cancel.FetchData parameter with an
        // exact `ParamSize`, and the allocation remains alive for the call.
        let (flags, actual) = unsafe { read_cancel_fetch_parameters(&raw const parameters) }
            .expect("complete cancel parameters must decode");
        assert_eq!(flags, -1);
        assert_eq!(actual, expected);
    }

    #[test]
    fn structure_readers_reject_header_only_allocations() {
        let callback_header = u32::try_from(size_of::<u32>()).expect("u32 size must fit u32");
        // SAFETY: the test provides exactly `StructSize`. The reader must reject it before a
        // full-width `CF_CALLBACK_INFO` copy.
        let callback_result =
            unsafe { read_callback_info((&raw const callback_header).cast::<CF_CALLBACK_INFO>()) };
        assert!(matches!(
            callback_result,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "callback info structure is truncated"
            })
        ));

        let mut process_header = callback_header;
        // SAFETY: the test provides exactly `StructSize`. The reader must reject it before a
        // full-width `CF_PROCESS_INFO` copy.
        let process_result =
            unsafe { copy_process_info((&raw mut process_header).cast::<CF_PROCESS_INFO>()) };
        assert!(matches!(
            process_result,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "process info structure is truncated"
            })
        ));
    }

    #[test]
    fn wide_string_copy_is_bounded_and_owned() {
        let mut value = wide("callback-value");
        // SAFETY: `value` is a live NUL-terminated UTF-16 allocation for the call.
        let copied = unsafe { copy_optional_wide(PCWSTR(value.as_ptr())) }
            .expect("valid callback string must copy")
            .expect("non-null callback string must be present");
        value.fill(u16::from(b'x'));
        assert_eq!(copied, OsString::from("callback-value"));

        let unterminated = vec![u16::from(b'x'); MAX_CALLBACK_WIDE_UNITS + 1];
        // SAFETY: the allocation contains exactly every code unit the bounded reader may inspect.
        let result = unsafe { copy_optional_wide(PCWSTR(unterminated.as_ptr())) };
        assert!(matches!(
            result,
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "callback wide string exceeds 32767 UTF-16 units or lacks a terminator"
            })
        ));
    }

    #[test]
    fn snapshot_info_detaches_context_identities_strings_and_process_data() {
        let (sender, _receiver) = sync_channel(1);
        let mut context = Box::new(CallbackContext {
            session: WindowsConnectionSession::new(generation(7)),
            sender,
            waiters: WindowsFetchDataWaiterRegistry::new(),
        });
        let mut sync_root_identity = WindowsSyncRootIdentity::encode(&scope())
            .expect("sync-root identity fixture must encode")
            .as_bytes()
            .to_vec();
        let mut file_identity = WindowsFileIdentity::encode(&item_key())
            .expect("file identity fixture must encode")
            .as_bytes()
            .to_vec();
        let mut volume_guid = wide(r"\\?\Volume{fixture}");
        let mut volume_dos = wide("X:");
        let mut normalized_path = wide(r"\folder\file.txt");
        let mut image_path = wide(r"C:\fixture.exe");
        let mut package_name = wide("package");
        let mut application_id = wide("application");
        let mut command_line = wide("fixture.exe --test");
        let mut process = CF_PROCESS_INFO {
            StructSize: u32::try_from(size_of::<CF_PROCESS_INFO>())
                .expect("process structure size must fit u32"),
            ProcessId: 41,
            ImagePath: PCWSTR(image_path.as_ptr()),
            PackageName: PCWSTR(package_name.as_ptr()),
            ApplicationId: PCWSTR(application_id.as_ptr()),
            CommandLine: PCWSTR(command_line.as_ptr()),
            SessionId: 42,
        };
        let info = CF_CALLBACK_INFO {
            StructSize: u32::try_from(size_of::<CF_CALLBACK_INFO>())
                .expect("callback structure size must fit u32"),
            ConnectionKey: CF_CONNECTION_KEY(-1),
            CallbackContext: (&raw mut *context).cast::<c_void>(),
            VolumeGuidName: PCWSTR(volume_guid.as_ptr()),
            VolumeDosName: PCWSTR(volume_dos.as_ptr()),
            VolumeSerialNumber: 43,
            SyncRootFileId: 44,
            SyncRootIdentity: sync_root_identity.as_ptr().cast(),
            SyncRootIdentityLength: u32::try_from(sync_root_identity.len())
                .expect("sync-root identity length must fit u32"),
            FileId: 45,
            FileSize: 46,
            FileIdentity: file_identity.as_ptr().cast(),
            FileIdentityLength: u32::try_from(file_identity.len())
                .expect("file identity length must fit u32"),
            NormalizedPath: PCWSTR(normalized_path.as_ptr()),
            TransferKey: 47,
            PriorityHint: 15,
            CorrelationVector: ptr::null_mut(),
            ProcessInfo: &raw mut process,
            RequestKey: 48,
        };

        // SAFETY: every pointer in `info` targets a live allocation with the exact reported size,
        // and the Box-backed callback context remains alive for the call.
        let (owned_context, snapshot) = unsafe { snapshot_info(&raw const info) }
            .expect("complete callback info must snapshot");
        sync_root_identity.fill(0);
        file_identity.fill(0);
        volume_guid.fill(0);
        volume_dos.fill(0);
        normalized_path.fill(0);
        image_path.fill(0);
        package_name.fill(0);
        application_id.fill(0);
        command_line.fill(0);
        drop(context);

        assert_eq!(owned_context.session.generation(), generation(7));
        assert_eq!(snapshot.generation(), generation(7));
        assert_eq!(snapshot.connection_key().get(), -1);
        assert_eq!(snapshot.transfer_key().get(), 47);
        assert_eq!(snapshot.request_key().get(), 48);
        assert_eq!(
            snapshot.volume_guid_name(),
            Some(OsStr::new(r"\\?\Volume{fixture}"))
        );
        assert_eq!(snapshot.volume_dos_name(), Some(OsStr::new("X:")));
        assert_eq!(
            snapshot.normalized_path(),
            Some(Path::new(r"\folder\file.txt"))
        );
        assert_eq!(
            snapshot
                .sync_root_identity()
                .decode()
                .expect("scope must decode"),
            scope()
        );
        assert_eq!(
            snapshot.file_identity().decode().expect("item must decode"),
            item_key()
        );
        let process = snapshot
            .process()
            .expect("process snapshot must be present");
        assert_eq!(
            process.image_path.as_deref(),
            Some(Path::new(r"C:\fixture.exe"))
        );
        assert_eq!(process.package_name.as_deref(), Some(OsStr::new("package")));
        assert_eq!(
            process.application_id.as_deref(),
            Some(OsStr::new("application"))
        );
        assert_eq!(
            process.command_line.as_deref(),
            Some(OsStr::new("fixture.exe --test"))
        );
    }
}
