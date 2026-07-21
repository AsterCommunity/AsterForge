use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use aster_forge_events::{
    EventConnectionObservation, EventConnectionState, RedisEventBus, RedisEventReconnectPolicy,
};
use aster_forge_test::redis::RedisTestContainer;
use aster_forge_test::suite::TestContainerSuite;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("asterforge-events"))
}

fn redis_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn test_policy() -> RedisEventReconnectPolicy {
    RedisEventReconnectPolicy {
        initial_delay: Duration::from_millis(25),
        max_delay: Duration::from_millis(100),
        stable_reset_after: Duration::from_millis(250),
        jitter_min_percent: 100,
        jitter_max_percent: 100,
    }
}

async fn wait_for_state(
    observations: &Arc<Mutex<Vec<EventConnectionObservation>>>,
    expected: EventConnectionState,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let found = observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|observation| observation.state == expected);
            if found {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("event subscriber did not reach state {expected:?}"));
}

async fn wait_for_ready(observations: &Arc<Mutex<Vec<EventConnectionObservation>>>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ready = observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|observation| {
                    matches!(
                        observation.state,
                        EventConnectionState::Connected | EventConnectionState::Recovered
                    )
                });
            if ready {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("event subscriber did not become ready");
}

fn clear_observations(observations: &Arc<Mutex<Vec<EventConnectionObservation>>>) {
    observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redis_event_bus_recovers_and_delivers_after_outage() {
    let _guard = redis_lock().lock().await;
    let redis = RedisTestContainer::start(test_suite()).await;
    let topic = format!("asterforge.events.{}", uuid::Uuid::new_v4().simple());
    let bus = RedisEventBus::from_url(redis.url(), topic)
        .expect("create Redis event bus")
        .with_reconnect_policy(test_policy());
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observer_values = observations.clone();
    let observer = move |observation| {
        observer_values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(observation);
    };
    let (payload_tx, mut payload_rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    let subscriber_bus = bus.clone();
    let subscriber_shutdown = shutdown.clone();
    let subscriber = tokio::spawn(async move {
        subscriber_bus
            .run_subscription(subscriber_shutdown, Some(&observer), move |payload| {
                let payload_tx = payload_tx.clone();
                async move {
                    payload_tx
                        .send(payload)
                        .expect("payload receiver stays open");
                }
            })
            .await;
    });

    wait_for_ready(&observations).await;
    clear_observations(&observations);
    bus.publish("before-outage")
        .await
        .expect("publish before outage");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), payload_rx.recv())
            .await
            .expect("receive before outage"),
        Some("before-outage".to_string())
    );

    redis.stop().await;
    wait_for_state(&observations, EventConnectionState::Disconnected).await;
    wait_for_state(&observations, EventConnectionState::Reconnecting).await;
    redis.restart().await;
    wait_for_state(&observations, EventConnectionState::Recovered).await;

    bus.publish("after-outage")
        .await
        .expect("publish after recovery");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), payload_rx.recv())
            .await
            .expect("receive after recovery"),
        Some("after-outage".to_string())
    );

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), subscriber)
        .await
        .expect("subscriber shutdown should interrupt promptly")
        .expect("subscriber task should not panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_interrupts_reconnect_backoff() {
    let _guard = redis_lock().lock().await;
    let redis = RedisTestContainer::start(test_suite()).await;
    let topic = format!("asterforge.events.{}", uuid::Uuid::new_v4().simple());
    let bus = RedisEventBus::from_url(redis.url(), topic)
        .expect("create Redis event bus")
        .with_reconnect_policy(RedisEventReconnectPolicy {
            initial_delay: Duration::from_secs(30),
            max_delay: Duration::from_secs(30),
            stable_reset_after: Duration::from_secs(30),
            jitter_min_percent: 100,
            jitter_max_percent: 100,
        });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observer_values = observations.clone();
    let observer = move |observation| {
        observer_values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(observation);
    };
    let shutdown = CancellationToken::new();
    let subscriber_shutdown = shutdown.clone();
    let subscriber = tokio::spawn(async move {
        bus.run_subscription(subscriber_shutdown, Some(&observer), |_| async {})
            .await;
    });

    wait_for_ready(&observations).await;
    clear_observations(&observations);
    redis.stop().await;
    wait_for_state(&observations, EventConnectionState::Reconnecting).await;

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), subscriber)
        .await
        .expect("shutdown should interrupt a long reconnect delay")
        .expect("subscriber task should not panic");
    redis.restart().await;
}
