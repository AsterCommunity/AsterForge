#![cfg(windows)]

use std::{path::Path, sync::mpsc::SyncSender};

use aster_forge_cloud_files_core::{
    ByteRange, ContentReadResponse, ContentRevision, SessionGeneration,
};
use aster_forge_cloud_files_windows::{
    Result, WindowsCallbackQueueMetrics, WindowsCallbackRequest, WindowsDisconnectOutcome,
    WindowsFetchDataFailure, WindowsFetchDataProgress, WindowsFetchDataProgressReporter,
    WindowsFetchDataRequest, WindowsFetchDataWaiterRegistry, WindowsPinRecursion, WindowsPinState,
    WindowsRestartHydration, WindowsSyncRootConnectOptions, WindowsSyncRootConnection,
    connect_sync_root, dehydrate_placeholder, set_placeholder_in_sync, set_placeholder_pin_state,
    unregister_sync_root,
};

fn assert_send<T: Send>() {}

#[test]
fn windows_connection_api_keeps_owned_sender_generation_and_raii_handle() {
    let _connect: fn(
        &Path,
        SessionGeneration,
        SyncSender<WindowsCallbackRequest>,
        WindowsSyncRootConnectOptions,
    ) -> Result<WindowsSyncRootConnection> = connect_sync_root;
    let _disconnect: fn(&mut WindowsSyncRootConnection) -> Result<WindowsDisconnectOutcome> =
        WindowsSyncRootConnection::disconnect;
    let _fail: fn(WindowsFetchDataRequest, WindowsFetchDataFailure) -> Result<()> =
        WindowsFetchDataRequest::fail;
    let _complete: fn(
        WindowsFetchDataRequest,
        &ContentRevision,
        ContentReadResponse,
    ) -> Result<()> = WindowsFetchDataRequest::complete;
    let _hydrate = WindowsFetchDataRequest::hydrate;
    let _restart: fn(WindowsFetchDataRequest, WindowsRestartHydration) -> Result<()> =
        WindowsFetchDataRequest::restart_hydration;
    let _reporter: fn(&WindowsFetchDataRequest) -> WindowsFetchDataProgressReporter =
        WindowsFetchDataRequest::progress_reporter;
    let _progress: fn(
        &WindowsFetchDataProgressReporter,
        &WindowsFetchDataWaiterRegistry,
        WindowsFetchDataProgress,
        std::time::Instant,
    ) -> Result<usize> = WindowsFetchDataProgressReporter::report;
    let _waiters: fn(&WindowsSyncRootConnection) -> WindowsFetchDataWaiterRegistry =
        WindowsSyncRootConnection::fetch_data_waiters;
    let _queue_metrics: fn(&WindowsSyncRootConnection) -> WindowsCallbackQueueMetrics =
        WindowsSyncRootConnection::queue_metrics;
    let _unregister: fn(&Path) -> Result<()> = unregister_sync_root;
    let _in_sync: fn(&Path, bool) -> Result<()> = set_placeholder_in_sync;
    let _pin: fn(&Path, WindowsPinState, WindowsPinRecursion) -> Result<()> =
        set_placeholder_pin_state;
    let _dehydrate: fn(&Path, Option<ByteRange>, bool) -> Result<()> = dehydrate_placeholder;

    assert_send::<WindowsSyncRootConnection>();
    assert_send::<WindowsCallbackRequest>();
    assert_send::<WindowsFetchDataWaiterRegistry>();
    fn assert_sync<T: Sync>() {}
    assert_sync::<WindowsFetchDataWaiterRegistry>();
}
