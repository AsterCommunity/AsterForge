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
fn assert_sync<T: Sync>() {}
fn assert_api<T>(_: T) {}

#[test]
fn windows_connection_api_keeps_owned_sender_generation_and_raii_handle() {
    let connect: fn(
        &Path,
        SessionGeneration,
        SyncSender<WindowsCallbackRequest>,
        WindowsSyncRootConnectOptions,
    ) -> Result<WindowsSyncRootConnection> = connect_sync_root;
    let disconnect: fn(&mut WindowsSyncRootConnection) -> Result<WindowsDisconnectOutcome> =
        WindowsSyncRootConnection::disconnect;
    let fail: fn(WindowsFetchDataRequest, WindowsFetchDataFailure) -> Result<()> =
        WindowsFetchDataRequest::fail;
    let complete: fn(WindowsFetchDataRequest, &ContentRevision, ContentReadResponse) -> Result<()> =
        WindowsFetchDataRequest::complete;
    let hydrate = WindowsFetchDataRequest::hydrate;
    let restart: fn(WindowsFetchDataRequest, &WindowsRestartHydration) -> Result<()> =
        WindowsFetchDataRequest::restart_hydration;
    let reporter: fn(&WindowsFetchDataRequest) -> WindowsFetchDataProgressReporter =
        WindowsFetchDataRequest::progress_reporter;
    let progress: fn(
        &WindowsFetchDataProgressReporter,
        &WindowsFetchDataWaiterRegistry,
        WindowsFetchDataProgress,
        std::time::Instant,
    ) -> Result<usize> = WindowsFetchDataProgressReporter::report;
    let waiters: fn(&WindowsSyncRootConnection) -> WindowsFetchDataWaiterRegistry =
        WindowsSyncRootConnection::fetch_data_waiters;
    let queue_metrics: fn(&WindowsSyncRootConnection) -> WindowsCallbackQueueMetrics =
        WindowsSyncRootConnection::queue_metrics;
    let unregister: fn(&Path) -> Result<()> = unregister_sync_root;
    let in_sync: fn(&Path, bool) -> Result<()> = set_placeholder_in_sync;
    let pin: fn(&Path, WindowsPinState, WindowsPinRecursion) -> Result<()> =
        set_placeholder_pin_state;
    let dehydrate: fn(&Path, Option<ByteRange>, bool) -> Result<()> = dehydrate_placeholder;

    assert_api(connect);
    assert_api(disconnect);
    assert_api(fail);
    assert_api(complete);
    assert_api(hydrate);
    assert_api(restart);
    assert_api(reporter);
    assert_api(progress);
    assert_api(waiters);
    assert_api(queue_metrics);
    assert_api(unregister);
    assert_api(in_sync);
    assert_api(pin);
    assert_api(dehydrate);

    assert_send::<WindowsSyncRootConnection>();
    assert_send::<WindowsCallbackRequest>();
    assert_send::<WindowsFetchDataWaiterRegistry>();
    assert_sync::<WindowsFetchDataWaiterRegistry>();
}
