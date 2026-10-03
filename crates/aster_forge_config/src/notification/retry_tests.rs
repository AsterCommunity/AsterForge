use super::supervisor::{ConfigReloadReconnectPolicy, run_config_reload_supervisor_inner};
use super::tests::{
    ScriptedConfigNotifier, SubscribeStep, TestConnectionObserver, TestReloadObserver,
};
use super::{
    ConfigChangeEvent, ConfigNotificationSource, ConfigReloadMessage, ConfigReloadWorkerConfig,
};
use crate::{ConfigCoreError, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::broadcast;
use tokio::time::{Duration, advance};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
enum Call {
    Reconcile,
    Reload(ConfigReloadMessage),
}

#[derive(Default)]
struct Store {
    authoritative: AtomicUsize,
    snapshot: AtomicUsize,
    fail_reconcile: AtomicBool,
    fail_reload: AtomicBool,
    block: AtomicBool,
    active: AtomicBool,
    calls: Mutex<Vec<Call>>,
}

impl Store {
    async fn load(&self, call: Call) -> Result<()> {
        let failed = match &call {
            Call::Reconcile => self.fail_reconcile.load(Ordering::SeqCst),
            Call::Reload(_) => self.fail_reload.load(Ordering::SeqCst),
        };
        self.calls.lock().unwrap().push(call);
        self.active.store(true, Ordering::SeqCst);
        let _active = ActiveCall(&self.active);
        if self.block.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if failed {
            return Err(ConfigCoreError::store("temporary read failure"));
        }
        self.snapshot
            .store(self.authoritative.load(Ordering::SeqCst), Ordering::SeqCst);
        Ok(())
    }

    fn reconciles(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call, Call::Reconcile))
            .count()
    }

    fn reloads(&self) -> Vec<ConfigReloadMessage> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|call| match call {
                Call::Reload(message) => Some(message.clone()),
                Call::Reconcile => None,
            })
            .collect()
    }
}

struct ActiveCall<'a>(&'a AtomicBool);

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct Worker {
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
    notifier: Arc<ScriptedConfigNotifier>,
    reload_observer: Arc<TestReloadObserver>,
    connection_observer: Arc<TestConnectionObserver>,
}

impl Worker {
    fn start(
        store: Arc<Store>,
        senders: &[broadcast::Sender<ConfigChangeEvent>],
        reconcile: bool,
    ) -> Self {
        let notifier = Arc::new(ScriptedConfigNotifier::new(
            senders.iter().cloned().map(SubscribeStep::Channel),
        ));
        let shutdown = CancellationToken::new();
        let reload_observer = Arc::new(TestReloadObserver::default());
        let connection_observer = Arc::new(TestConnectionObserver::default());
        let worker_notifier = notifier.clone();
        let worker_shutdown = shutdown.clone();
        let worker_reload_observer = reload_observer.clone();
        let worker_connection_observer = connection_observer.clone();
        let task = tokio::spawn(async move {
            let reconcile_store = store.clone();
            let mut reconcile_callback = move || {
                let store = reconcile_store.clone();
                async move { store.load(Call::Reconcile).await }
            };
            let mut reload = move |message| {
                let store = store.clone();
                async move { store.load(Call::Reload(message)).await }
            };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "local"),
                ConfigReloadReconnectPolicy {
                    initial_delay: Duration::from_millis(100),
                    max_delay: Duration::from_millis(400),
                    stable_reset_after: Duration::from_secs(30),
                    jitter_min_percent: 100,
                    jitter_max_percent: 100,
                },
                worker_shutdown,
                reconcile.then_some(&mut reconcile_callback),
                &mut reload,
                Some(worker_reload_observer.as_ref()),
                Some(worker_connection_observer.as_ref()),
            )
            .await
        });
        Self {
            shutdown,
            task,
            notifier,
            reload_observer,
            connection_observer,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        pump().await;
        assert!(
            self.task.is_finished(),
            "shutdown must cancel active callbacks and waits"
        );
        self.task.await.unwrap().unwrap();
    }
}

// Keep runnable work on the paused runtime so idle time never advances implicitly.
async fn pump() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn tick(milliseconds: u64) {
    advance(Duration::from_millis(milliseconds)).await;
    pump().await;
}

fn send(
    sender: &broadcast::Sender<ConfigChangeEvent>,
    namespace: &str,
    origin: &str,
    keys: &[&str],
) {
    sender
        .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
            namespace,
            origin,
            keys.iter().copied(),
            ConfigNotificationSource::Api,
        )))
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn failed_notification_reconciles_without_new_messages_or_reconnects() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.authoritative.store(1, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), true);
    pump().await;
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 1);

    store.authoritative.store(2, Ordering::SeqCst);
    store.fail_reload.store(true, Ordering::SeqCst);
    send(&sender, "aster_test", "remote", &["changed"]);
    send(&sender, "foreign", "remote", &["ignored"]);
    send(&sender, "aster_test", "local", &["ignored"]);
    pump().await;
    tick(99).await;
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 1);
    assert_eq!(store.reconciles(), 1);
    tick(1).await;
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 2);
    assert_eq!(store.reconciles(), 2);
    assert_eq!(store.reloads().len(), 1);
    assert_eq!(worker.reload_observer.snapshot().len(), 3);
    assert_eq!(worker.connection_observer.snapshot().len(), 1);
    assert_eq!(worker.notifier.subscribe_attempts(), 1);
    tick(5000).await;
    assert_eq!(store.reconciles(), 2);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn initial_and_recovered_reconcile_failures_retry_without_notifications() {
    let (first, _) = broadcast::channel(16);
    let (second, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.authoritative.store(1, Ordering::SeqCst);
    store.fail_reconcile.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), &[first.clone(), second], true);
    pump().await;
    assert_eq!(store.reconciles(), 1);
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(100).await;
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 1);

    store.authoritative.store(2, Ordering::SeqCst);
    store.fail_reconcile.store(true, Ordering::SeqCst);
    drop(first);
    pump().await;
    tick(100).await;
    assert_eq!(worker.notifier.subscribe_attempts(), 2);
    assert_eq!(store.reconciles(), 3);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 1);
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(100).await;
    assert_eq!(store.reconciles(), 4);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 2);
    tick(5000).await;
    assert_eq!(store.reconciles(), 4);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn retries_back_off_cap_coalesce_and_reset_only_after_reconcile_success() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.fail_reconcile.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), true);
    pump().await;
    tick(50).await;
    // Even successful per-key reloads cannot clear a failed full reconciliation.
    for key in ["a", "b", "c"] {
        send(&sender, "aster_test", "remote", &[key]);
    }
    pump().await;
    tick(49).await;
    assert_eq!(store.reconciles(), 1);
    tick(1).await;
    assert_eq!(store.reconciles(), 2);
    tick(199).await;
    assert_eq!(store.reconciles(), 2);
    tick(1).await;
    assert_eq!(store.reconciles(), 3);
    tick(399).await;
    assert_eq!(store.reconciles(), 3);
    tick(1).await;
    assert_eq!(store.reconciles(), 4);
    tick(400).await;
    assert_eq!(store.reconciles(), 5);
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(400).await;
    assert_eq!(store.reconciles(), 6);
    tick(5000).await;
    assert_eq!(store.reconciles(), 6);

    store.fail_reload.store(true, Ordering::SeqCst);
    send(&sender, "aster_test", "remote", &["d"]);
    pump().await;
    tick(99).await;
    assert_eq!(store.reconciles(), 6);
    tick(1).await;
    assert_eq!(store.reconciles(), 7, "success resets retry delay to 100ms");
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn successful_reconnect_reconcile_clears_an_existing_retry() {
    let (first, _) = broadcast::channel(16);
    let (second, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.fail_reconcile.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), &[first.clone(), second], true);
    pump().await;
    tick(100).await;
    assert_eq!(store.reconciles(), 2);
    drop(first);
    pump().await;
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(100).await;
    assert_eq!(store.reconciles(), 3);
    assert_eq!(worker.notifier.subscribe_attempts(), 2);
    tick(100).await;
    tick(5000).await;
    assert_eq!(store.reconciles(), 3);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn worker_retries_coalesced_failed_hints_and_preserves_reload_all() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.authoritative.store(7, Ordering::SeqCst);
    store.fail_reload.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), false);
    pump().await;
    send(&sender, "aster_test", "remote", &["a"]);
    pump().await;
    tick(50).await;
    sender
        .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
            "aster_test",
            "remote-b",
            ["a", "b"],
            ConfigNotificationSource::Cli,
        )))
        .unwrap();
    send(&sender, "foreign", "remote", &["ignored"]);
    send(&sender, "aster_test", "local", &["ignored"]);
    pump().await;
    store.fail_reload.store(false, Ordering::SeqCst);
    send(&sender, "aster_test", "remote", &["c"]);
    pump().await;
    tick(50).await;
    assert_eq!(store.reloads().len(), 4);
    assert_eq!(store.reloads()[3].keys, vec!["a", "b"]);
    assert_eq!(store.reloads()[3].origin_runtime_id, "remote-b");
    assert_eq!(store.reloads()[3].source, ConfigNotificationSource::Cli);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 7);
    tick(5000).await;
    assert_eq!(store.reloads().len(), 4);

    store.fail_reload.store(true, Ordering::SeqCst);
    send(&sender, "aster_test", "remote", &[]);
    send(&sender, "aster_test", "remote", &["d"]);
    pump().await;
    store.fail_reload.store(false, Ordering::SeqCst);
    tick(100).await;
    assert_eq!(store.reloads().len(), 7);
    assert!(store.reloads()[6].keys.is_empty());
    assert_eq!(worker.notifier.subscribe_attempts(), 1);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_stops_retry_wait_and_drops_in_flight_retry() {
    for (reconcile, block_retry) in [(true, false), (true, true), (false, false), (false, true)] {
        let (sender, _) = broadcast::channel(16);
        let store = Arc::new(Store::default());
        store.fail_reconcile.store(true, Ordering::SeqCst);
        store.fail_reload.store(true, Ordering::SeqCst);
        let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), reconcile);
        pump().await;
        if !reconcile {
            send(&sender, "aster_test", "remote", &["a"]);
            pump().await;
        }
        if block_retry {
            store.block.store(true, Ordering::SeqCst);
            tick(100).await;
            assert!(store.active.load(Ordering::SeqCst));
        }
        let attempts = store.calls.lock().unwrap().len();
        worker.stop().await;
        assert!(!store.active.load(Ordering::SeqCst));
        tick(5000).await;
        assert_eq!(store.calls.lock().unwrap().len(), attempts);
    }
}

#[tokio::test(start_paused = true)]
async fn worker_recovers_single_failed_notification_and_reads_latest_store_value() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.authoritative.store(1, Ordering::SeqCst);
    store.fail_reload.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), false);
    pump().await;
    send(&sender, "aster_test", "remote", &["a"]);
    pump().await;
    tick(100).await;
    assert_eq!(store.reloads().len(), 2);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 0);
    store.authoritative.store(2, Ordering::SeqCst);
    store.fail_reload.store(false, Ordering::SeqCst);
    tick(199).await;
    assert_eq!(store.reloads().len(), 2);
    tick(1).await;
    assert_eq!(store.reloads().len(), 3);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 2);
    let observations = worker.reload_observer.snapshot();
    assert_eq!(
        observations.iter().map(|o| o.status).collect::<Vec<_>>(),
        vec!["error", "error", "ok"]
    );
    assert!(observations.iter().all(|o| o.changed_keys == 1));
    assert_eq!(worker.notifier.subscribe_attempts(), 1);
    tick(5000).await;
    assert_eq!(store.reloads().len(), 3);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn filtered_notifications_do_not_schedule_retries() {
    for reconcile in [false, true] {
        let (sender, _) = broadcast::channel(16);
        let store = Arc::new(Store::default());
        let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), reconcile);
        pump().await;
        store.fail_reload.store(true, Ordering::SeqCst);
        send(&sender, "foreign", "remote", &[]);
        send(&sender, "aster_test", "local", &[]);
        pump().await;
        tick(5000).await;
        assert!(store.reloads().is_empty());
        assert_eq!(store.reconciles(), usize::from(reconcile));
        assert_eq!(worker.reload_observer.snapshot().len(), 2);
        assert_eq!(worker.notifier.subscribe_attempts(), 1);
        worker.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn failed_notification_bursts_do_not_postpone_or_multiply_retry_timers() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), true);
    pump().await;
    store.fail_reload.store(true, Ordering::SeqCst);
    store.fail_reconcile.store(true, Ordering::SeqCst);
    for _ in 0..10 {
        send(&sender, "aster_test", "remote", &["a"]);
        pump().await;
        tick(10).await;
    }
    assert_eq!(store.reconciles(), 2);
    for _ in 0..20 {
        send(&sender, "aster_test", "remote", &["b"]);
        pump().await;
        tick(10).await;
    }
    assert_eq!(store.reconciles(), 3);
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(400).await;
    assert_eq!(store.reconciles(), 4);
    tick(5000).await;
    assert_eq!(store.reconciles(), 4);
    assert_eq!(store.reloads().len(), 30);
    assert_eq!(worker.notifier.subscribe_attempts(), 1);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pending_reconcile_recovers_while_transport_subscribe_is_stuck() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), true);
    pump().await;
    store.fail_reload.store(true, Ordering::SeqCst);
    store.fail_reconcile.store(true, Ordering::SeqCst);
    send(&sender, "aster_test", "remote", &["a"]);
    pump().await;
    drop(sender);
    pump().await;
    tick(100).await;
    assert_eq!(worker.notifier.subscribe_attempts(), 2);
    assert_eq!(store.reconciles(), 2);
    store.authoritative.store(9, Ordering::SeqCst);
    store.fail_reconcile.store(false, Ordering::SeqCst);
    tick(200).await;
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 9);
    assert_eq!(store.reconciles(), 3);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn too_many_failed_keys_collapse_to_a_full_reload_hint() {
    for count in [1024, 1025] {
        let (sender, _) = broadcast::channel(16);
        let store = Arc::new(Store::default());
        store.fail_reload.store(true, Ordering::SeqCst);
        let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), false);
        pump().await;
        for offset in [0, 512] {
            let keys = (offset..offset + count / 2).map(|i| format!("key-{i}"));
            sender
                .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
                    "aster_test",
                    "remote",
                    keys,
                    ConfigNotificationSource::Api,
                )))
                .unwrap();
            pump().await;
        }
        if count == 1025 {
            send(&sender, "aster_test", "remote", &["extra"]);
            pump().await;
        }
        store.fail_reload.store(false, Ordering::SeqCst);
        tick(100).await;
        let reloads = store.reloads();
        let last = reloads.last().unwrap();
        assert_eq!(last.keys.len(), if count == 1024 { 1024 } else { 0 });
        worker.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn already_cancelled_supervisor_never_subscribes_or_calls_products() {
    let notifier = Arc::new(ScriptedConfigNotifier::new([]));
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    super::run_config_reload_supervisor(
        notifier.clone(),
        ConfigReloadWorkerConfig::new("aster_test", "local"),
        shutdown,
        || async { panic!("cancelled reconcile must not run") },
        |_| async { panic!("cancelled reload must not run") },
    )
    .await
    .unwrap();
    assert_eq!(notifier.subscribe_attempts(), 0);
}

#[tokio::test(start_paused = true)]
async fn worker_reconnect_keeps_failed_hints_until_reload_succeeds() {
    let (first, _) = broadcast::channel(16);
    let (second, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.fail_reload.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), &[first.clone(), second], false);
    pump().await;
    send(&first, "aster_test", "remote", &["a"]);
    pump().await;
    tick(100).await;
    assert_eq!(store.reloads().len(), 2);
    drop(first);
    pump().await;
    tick(100).await;
    assert_eq!(worker.notifier.subscribe_attempts(), 2);
    store.fail_reload.store(false, Ordering::SeqCst);
    store.authoritative.store(9, Ordering::SeqCst);
    tick(100).await;
    assert_eq!(store.reloads().len(), 3);
    assert_eq!(store.snapshot.load(Ordering::SeqCst), 9);
    tick(5000).await;
    assert_eq!(store.reloads().len(), 3);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn reload_all_dominates_key_hints_in_either_arrival_order() {
    for key_lists in [[&["a"][..], &[][..]], [&[][..], &["a"][..]]] {
        let (sender, _) = broadcast::channel(16);
        let store = Arc::new(Store::default());
        store.fail_reload.store(true, Ordering::SeqCst);
        let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), false);
        pump().await;
        for keys in key_lists {
            send(&sender, "aster_test", "remote", keys);
            pump().await;
        }
        store.fail_reload.store(false, Ordering::SeqCst);
        tick(100).await;
        assert_eq!(store.reloads().len(), 3);
        assert!(store.reloads()[2].keys.is_empty());
        worker.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_takes_priority_over_a_due_retry() {
    let (sender, _) = broadcast::channel(16);
    let store = Arc::new(Store::default());
    store.fail_reconcile.store(true, Ordering::SeqCst);
    let worker = Worker::start(store.clone(), &[sender], true);
    pump().await;
    worker.shutdown.cancel();
    tick(100).await;
    assert_eq!(store.reconciles(), 1);
    worker.stop().await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_drops_initial_reconcile_and_notification_callbacks() {
    for reconcile in [true, false] {
        let (sender, _) = broadcast::channel(16);
        let store = Arc::new(Store::default());
        store.block.store(true, Ordering::SeqCst);
        let worker = Worker::start(store.clone(), std::slice::from_ref(&sender), reconcile);
        pump().await;
        if !reconcile {
            send(&sender, "aster_test", "remote", &["a"]);
            pump().await;
        }
        assert!(store.active.load(Ordering::SeqCst));
        worker.stop().await;
        assert!(!store.active.load(Ordering::SeqCst));
    }
}
