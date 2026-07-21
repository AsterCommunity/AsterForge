use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use aster_forge_config::{
    CONFIG_SYNC_BACKEND_REDIS, ConfigNotificationSource, ConfigSyncConfig,
    ConfigSyncConnectionObservation, ConfigSyncConnectionState,
    build_config_sync_runtime_with_runtime_id,
};
use aster_forge_test::redis::RedisTestContainer;
use aster_forge_test::suite::TestContainerSuite;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("asterforge-config"))
}

async fn wait_for_connection_state(
    observations: &Arc<Mutex<Vec<ConfigSyncConnectionObservation>>>,
    expected: ConfigSyncConnectionState,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|observation| observation.state == expected)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("config subscriber did not reach state {expected:?}"));
}

async fn wait_for_connection_ready(
    observations: &Arc<Mutex<Vec<ConfigSyncConnectionObservation>>>,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ready = observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|observation| {
                    matches!(
                        observation.state,
                        ConfigSyncConnectionState::Connected | ConfigSyncConnectionState::Recovered
                    )
                });
            if ready {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("config subscriber did not become ready");
}

fn clear_observations(observations: &Arc<Mutex<Vec<ConfigSyncConnectionObservation>>>) {
    observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_runtime_reconciles_and_delivers_after_redis_recovery() {
    let redis = RedisTestContainer::start(test_suite()).await;
    let topic = format!("asterforge.config.{}", uuid::Uuid::new_v4().simple());
    let config = ConfigSyncConfig {
        backend: CONFIG_SYNC_BACKEND_REDIS.to_string(),
        endpoint: redis.url().to_string(),
        topic,
    };
    let publisher =
        build_config_sync_runtime_with_runtime_id(&config, "aster_test", "publisher-runtime")
            .expect("publisher runtime should build");
    let subscriber =
        build_config_sync_runtime_with_runtime_id(&config, "aster_test", "subscriber-runtime")
            .expect("subscriber runtime should build");

    let reconciles = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_reconciles = reconciles.clone();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let worker_observations = observations.clone();
    let connection_observer = move |observation| {
        worker_observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(observation);
    };
    let (reload_tx, mut reload_rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let worker = tokio::spawn(async move {
        subscriber
            .run_reload_subscription_with_reconcile_and_observers(
                worker_shutdown,
                move || {
                    let worker_reconciles = worker_reconciles.clone();
                    async move {
                        worker_reconciles.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }
                },
                move |message| {
                    let reload_tx = reload_tx.clone();
                    async move {
                        reload_tx.send(message).expect("reload receiver stays open");
                        Ok(())
                    }
                },
                None,
                Some(&connection_observer),
            )
            .await
    });

    wait_for_connection_ready(&observations).await;
    clear_observations(&observations);
    publisher
        .publish_reload(["before_outage"], ConfigNotificationSource::Api)
        .await
        .expect("publish before outage");
    let before = tokio::time::timeout(Duration::from_secs(5), reload_rx.recv())
        .await
        .expect("receive before outage")
        .expect("reload channel should remain open");
    assert_eq!(before.keys, vec!["before_outage"]);

    redis.stop().await;
    wait_for_connection_state(&observations, ConfigSyncConnectionState::Disconnected).await;
    wait_for_connection_state(&observations, ConfigSyncConnectionState::Reconnecting).await;
    redis.restart().await;
    wait_for_connection_state(&observations, ConfigSyncConnectionState::Recovered).await;
    assert!(
        reconciles.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "initial connection and recovery should both reconcile"
    );

    publisher
        .publish_reload(["after_outage"], ConfigNotificationSource::Cli)
        .await
        .expect("publish after outage");
    let after = tokio::time::timeout(Duration::from_secs(5), reload_rx.recv())
        .await
        .expect("receive after outage")
        .expect("reload channel should remain open");
    assert_eq!(after.keys, vec!["after_outage"]);

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .expect("config worker should stop promptly")
        .expect("config worker should not panic")
        .expect("config worker should finish successfully");
}
