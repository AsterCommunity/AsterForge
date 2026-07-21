#[cfg(not(feature = "redis-pubsub"))]
use super::CONFIG_SYNC_BACKEND_REDIS;
use super::{
    CONFIG_SYNC_BACKEND_DISABLED, ConfigChangeEvent, ConfigChangeNotifier,
    ConfigNotificationSource, ConfigReloadDecision, ConfigReloadMessage,
    ConfigReloadReconnectPolicy, ConfigReloadWorkerConfig, ConfigSyncConfig,
    ConfigSyncConnectionObservation, ConfigSyncConnectionState, ConfigSyncRuntime,
    InMemoryConfigNotifier, SharedConfigChangeNotifier, build_config_sync_runtime,
    build_config_sync_runtime_with_runtime_id, config_reload_reconnect_delay,
    decode_config_reload_transport_payload, default_config_sync_topic,
    handle_config_reload_notification, redis_channel_from_topic,
    run_config_reload_supervisor_inner, run_config_reload_worker,
    run_config_reload_worker_with_observer,
};
use crate::ConfigCoreError;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

async fn wait_for_subscriber(notifier: &InMemoryConfigNotifier) {
    timeout(Duration::from_secs(1), async {
        while notifier.sender.receiver_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

enum SubscribeStep {
    Fail(&'static str),
    Channel(broadcast::Sender<ConfigChangeEvent>),
    Pending,
}

struct ScriptedConfigNotifier {
    steps: Mutex<VecDeque<SubscribeStep>>,
    subscribe_attempts: AtomicUsize,
}

impl ScriptedConfigNotifier {
    fn new(steps: impl IntoIterator<Item = SubscribeStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
            subscribe_attempts: AtomicUsize::new(0),
        }
    }

    fn subscribe_attempts(&self) -> usize {
        self.subscribe_attempts.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ConfigChangeNotifier for ScriptedConfigNotifier {
    async fn publish_reload(&self, _message: ConfigReloadMessage) -> super::Result<()> {
        Ok(())
    }

    async fn subscribe(&self) -> super::Result<super::ConfigNotification> {
        self.subscribe_attempts.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SubscribeStep::Pending);
        match step {
            SubscribeStep::Fail(message) => Err(ConfigCoreError::notification(message)),
            SubscribeStep::Channel(sender) => {
                Ok(super::ConfigNotification::new(sender.subscribe()))
            }
            SubscribeStep::Pending => std::future::pending().await,
        }
    }
}

#[derive(Default)]
struct TestConnectionObserver {
    observations: Mutex<Vec<ConfigSyncConnectionObservation>>,
}

impl super::ConfigSyncConnectionObserver for TestConnectionObserver {
    fn observe_config_sync_connection(&self, observation: ConfigSyncConnectionObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

impl TestConnectionObserver {
    fn snapshot(&self) -> Vec<ConfigSyncConnectionObservation> {
        self.observations.lock().unwrap().clone()
    }
}

fn zero_reconnect_policy() -> ConfigReloadReconnectPolicy {
    ConfigReloadReconnectPolicy {
        initial_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        stable_reset_after: Duration::from_secs(30),
        jitter_min_percent: 50,
        jitter_max_percent: 100,
    }
}

#[tokio::test]
async fn in_memory_notifier_broadcasts_reload_messages() {
    let notifier = InMemoryConfigNotifier::default();
    let mut subscription = notifier.subscribe().await.unwrap();

    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "runtime-a",
            ["b", "a", "a"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();

    let event = subscription.recv().await.unwrap();
    let ConfigChangeEvent::Reload(message) = event;
    assert_eq!(message.namespace, "aster_test");
    assert_eq!(message.origin_runtime_id, "runtime-a");
    assert_eq!(message.keys, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn reload_message_round_trips_json() {
    let message = ConfigReloadMessage::new(
        "aster_test",
        "runtime-a",
        ["feature_enabled"],
        ConfigNotificationSource::Cli,
    );

    let encoded = message.encode().unwrap();
    let decoded = ConfigReloadMessage::decode(&encoded).unwrap();

    assert_eq!(decoded, message);
}

#[test]
fn transport_payload_decode_surfaces_malformed_messages() {
    let message = ConfigReloadMessage::new(
        "aster_test",
        "runtime-a",
        ["feature_enabled"],
        ConfigNotificationSource::Cli,
    );
    let encoded = message.encode().unwrap();
    let event = decode_config_reload_transport_payload(&encoded).unwrap();
    assert_eq!(event.reload_message(), &message);

    assert!(decode_config_reload_transport_payload("{not-json").is_err());
}

#[test]
fn config_sync_config_defaults_to_disabled_generic_topic() {
    let config = ConfigSyncConfig::default();

    assert!(!config.enabled());
    assert_eq!(config.backend, CONFIG_SYNC_BACKEND_DISABLED);
    assert_eq!(config.endpoint, "");
    assert_eq!(config.topic, "aster.config_reload");
}

#[test]
fn config_sync_runtime_holds_namespace_runtime_id_and_notifier() {
    let disabled = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");
    assert_eq!(disabled.namespace(), "aster_test");
    assert_eq!(disabled.runtime_id(), "runtime-a");
    assert!(!disabled.enabled());
    assert!(disabled.notifier().is_none());

    let notifier: SharedConfigChangeNotifier = Arc::new(InMemoryConfigNotifier::default());
    let enabled = ConfigSyncRuntime::new("aster_test", "runtime-b", notifier);
    assert_eq!(enabled.namespace(), "aster_test");
    assert_eq!(enabled.runtime_id(), "runtime-b");
    assert!(enabled.enabled());
    assert!(enabled.notifier().is_some());
    assert_eq!(
        enabled.worker_config(),
        ConfigReloadWorkerConfig::new("aster_test", "runtime-b")
    );
}

#[test]
fn config_sync_runtime_builds_disabled_defaults() {
    let runtime = build_config_sync_runtime(&ConfigSyncConfig::default(), "aster_test").unwrap();

    assert!(!runtime.enabled());
    assert_eq!(runtime.namespace(), "aster_test");
    assert!(runtime.runtime_id().starts_with("runtime-"));
    assert_eq!(
        default_config_sync_topic("aster_test"),
        "aster_test.config_reload"
    );
}

#[test]
fn config_sync_runtime_can_use_explicit_runtime_id() {
    let runtime = build_config_sync_runtime_with_runtime_id(
        &ConfigSyncConfig::default(),
        "aster_test",
        "runtime-explicit",
    )
    .unwrap();

    assert!(!runtime.enabled());
    assert_eq!(runtime.namespace(), "aster_test");
    assert_eq!(runtime.runtime_id(), "runtime-explicit");
}

#[tokio::test]
async fn disabled_config_sync_publish_is_noop() {
    let runtime = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");

    runtime
        .publish_reload(["feature"], ConfigNotificationSource::Api)
        .await
        .unwrap();
}

#[tokio::test]
async fn disabled_config_sync_waits_for_shutdown_without_invoking_callbacks() {
    let runtime = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let reconciles = Arc::new(AtomicUsize::new(0));
    let reloads = Arc::new(AtomicUsize::new(0));
    let worker_reconciles = reconciles.clone();
    let worker_reloads = reloads.clone();

    let worker = tokio::spawn(async move {
        runtime
            .run_reload_subscription_with_reconcile(
                worker_shutdown,
                move || {
                    let worker_reconciles = worker_reconciles.clone();
                    async move {
                        worker_reconciles.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
                move |_| {
                    let worker_reloads = worker_reloads.clone();
                    async move {
                        worker_reloads.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            )
            .await
    });

    tokio::task::yield_now().await;
    assert!(!worker.is_finished());
    assert_eq!(reconciles.load(Ordering::SeqCst), 0);
    assert_eq!(reloads.load(Ordering::SeqCst), 0);

    shutdown.cancel();
    timeout(Duration::from_millis(100), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
fn config_sync_topic_maps_to_redis_channel_shape() {
    assert_eq!(
        redis_channel_from_topic(&default_config_sync_topic("aster_test")),
        "aster_test:config_reload"
    );
    assert_eq!(
        redis_channel_from_topic("custom.config.reload"),
        "custom:config:reload"
    );
}

#[cfg(not(feature = "redis-pubsub"))]
#[test]
fn redis_config_sync_backend_requires_feature() {
    let result = build_config_sync_runtime(
        &ConfigSyncConfig {
            backend: CONFIG_SYNC_BACKEND_REDIS.to_string(),
            endpoint: "redis://127.0.0.1:6379/0".to_string(),
            ..ConfigSyncConfig::default()
        },
        "aster_test",
    );
    let Err(error) = result else {
        panic!("redis config sync without redis-pubsub feature should fail");
    };

    assert!(
        error
            .to_string()
            .contains("requires the redis-pubsub feature")
    );
}

#[tokio::test]
async fn config_sync_runtime_publishes_namespaced_reload_messages() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let mut subscription = notifier.subscribe().await.unwrap();
    let runtime = ConfigSyncRuntime::with_notifier_for_test(
        "aster_test",
        "runtime-a",
        notifier as SharedConfigChangeNotifier,
    );

    runtime
        .publish_reload(["b", "a", "a"], ConfigNotificationSource::Api)
        .await
        .unwrap();

    let message = subscription.recv().await.unwrap().reload_message().clone();
    assert_eq!(message.namespace, "aster_test");
    assert_eq!(message.origin_runtime_id, "runtime-a");
    assert_eq!(message.keys, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(message.source, ConfigNotificationSource::Api);
}

#[tokio::test]
async fn config_sync_runtime_runs_reload_subscription_with_runtime_filter() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let runtime = ConfigSyncRuntime::with_notifier_for_test(
        "aster_test",
        "runtime-a",
        notifier.clone() as SharedConfigChangeNotifier,
    );
    let shutdown = CancellationToken::new();
    let observed = Arc::new(AtomicUsize::new(0));
    let observed_reload = observed.clone();
    let worker_shutdown = shutdown.clone();

    let worker = tokio::spawn(async move {
        runtime
            .run_reload_subscription(worker_shutdown, move |_| {
                let observed_reload = observed_reload.clone();
                async move {
                    observed_reload.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
            .unwrap();
    });
    wait_for_subscriber(&notifier).await;

    notifier
        .publish_reload(ConfigReloadMessage::new(
            "other",
            "runtime-b",
            ["ignored"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "runtime-a",
            ["ignored"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "runtime-b",
            ["accepted"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();

    timeout(Duration::from_secs(1), async {
        while observed.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn reload_handler_filters_namespace_and_origin() {
    let config = ConfigReloadWorkerConfig::new("aster_test", "runtime-a");
    let reloads = Arc::new(AtomicUsize::new(0));

    let decision = handle_config_reload_notification(
        &config,
        ConfigReloadMessage::new(
            "other",
            "runtime-b",
            ["feature"],
            ConfigNotificationSource::Api,
        ),
        |_| async {
            reloads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(decision, ConfigReloadDecision::IgnoredNamespace);

    let decision = handle_config_reload_notification(
        &config,
        ConfigReloadMessage::new(
            "aster_test",
            "runtime-a",
            ["feature"],
            ConfigNotificationSource::Api,
        ),
        |_| async {
            reloads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(decision, ConfigReloadDecision::IgnoredOrigin);

    let decision = handle_config_reload_notification(
        &config,
        ConfigReloadMessage::new(
            "aster_test",
            "runtime-b",
            ["feature"],
            ConfigNotificationSource::Api,
        ),
        |_| async {
            reloads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(decision, ConfigReloadDecision::Reloaded);
    assert_eq!(reloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reload_supervisor_recovers_from_initial_subscribe_failures_and_reconciles() {
    let (sender, _) = broadcast::channel(8);
    let notifier = Arc::new(ScriptedConfigNotifier::new([
        SubscribeStep::Fail("redis unavailable"),
        SubscribeStep::Channel(sender.clone()),
    ]));
    let shutdown = CancellationToken::new();
    let reconciles = Arc::new(AtomicUsize::new(0));
    let observer = Arc::new(TestConnectionObserver::default());
    let worker_notifier = notifier.clone();
    let worker_shutdown = shutdown.clone();
    let worker_reconciles = reconciles.clone();
    let worker_observer = observer.clone();

    let worker = tokio::spawn(async move {
        let mut reconcile = move || {
            let worker_reconciles = worker_reconciles.clone();
            async move {
                worker_reconciles.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            Some(worker_observer.as_ref()),
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 1 || notifier.subscribe_attempts() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let observations = observer.snapshot();
    let states = observations
        .iter()
        .map(|observation| observation.state)
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            ConfigSyncConnectionState::Disconnected,
            ConfigSyncConnectionState::Reconnecting,
            ConfigSyncConnectionState::Recovered,
        ]
    );
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.reconnect_attempt)
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    assert!(
        observations
            .iter()
            .all(|observation| observation.backoff_seconds == 0.0)
    );

    shutdown.cancel();
    worker.await.unwrap().unwrap();
    drop(sender);
}

#[tokio::test]
async fn reload_supervisor_subscribes_before_reconcile_to_close_startup_race() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();
    let reloads = Arc::new(AtomicUsize::new(0));
    let worker_reloads = reloads.clone();

    let worker = tokio::spawn(async move {
        let reconcile_notifier = worker_notifier.clone();
        let mut reconcile = move || {
            let reconcile_notifier = reconcile_notifier.clone();
            async move {
                reconcile_notifier
                    .publish_reload(ConfigReloadMessage::new(
                        "aster_test",
                        "node-b",
                        ["during_reconcile"],
                        ConfigNotificationSource::Api,
                    ))
                    .await
            }
        };
        let mut reload = move |_| {
            let worker_reloads = worker_reloads.clone();
            async move {
                worker_reloads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            None,
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reloads.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    shutdown.cancel();
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn lagged_subscription_reconnects_and_reconciles_without_reload_observation() {
    let (first_sender, _) = broadcast::channel(1);
    let (second_sender, _) = broadcast::channel(1);
    let notifier = Arc::new(ScriptedConfigNotifier::new([
        SubscribeStep::Channel(first_sender.clone()),
        SubscribeStep::Channel(second_sender.clone()),
    ]));
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();
    let reconciles = Arc::new(AtomicUsize::new(0));
    let worker_reconciles = reconciles.clone();
    let reload_observer = Arc::new(TestReloadObserver::default());
    let worker_reload_observer = reload_observer.clone();

    let worker = tokio::spawn(async move {
        let lag_sender = first_sender.clone();
        let mut reconcile = move || {
            let lag_sender = lag_sender.clone();
            let worker_reconciles = worker_reconciles.clone();
            async move {
                let attempt = worker_reconciles.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    for key in ["first", "second"] {
                        lag_sender
                            .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
                                "aster_test",
                                "node-b",
                                [key],
                                ConfigNotificationSource::Api,
                            )))
                            .expect("lag test subscription should still exist");
                    }
                }
                Ok(())
            }
        };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            Some(worker_reload_observer.as_ref()),
            None,
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    assert!(reload_observer.snapshot().is_empty());
    shutdown.cancel();
    worker.await.unwrap().unwrap();
    drop(second_sender);
}

#[tokio::test]
async fn stable_subscription_resets_reconnect_attempt_sequence() {
    let (first_sender, _) = broadcast::channel(8);
    let (second_sender, _) = broadcast::channel(8);
    let notifier = Arc::new(ScriptedConfigNotifier::new([
        SubscribeStep::Fail("initial outage"),
        SubscribeStep::Channel(first_sender.clone()),
        SubscribeStep::Channel(second_sender.clone()),
    ]));
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();
    let reconciles = Arc::new(AtomicUsize::new(0));
    let worker_reconciles = reconciles.clone();
    let observer = Arc::new(TestConnectionObserver::default());
    let worker_observer = observer.clone();
    let policy = ConfigReloadReconnectPolicy {
        initial_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        stable_reset_after: Duration::ZERO,
        jitter_min_percent: 50,
        jitter_max_percent: 100,
    };

    let worker = tokio::spawn(async move {
        let mut reconcile = move || {
            let worker_reconciles = worker_reconciles.clone();
            async move {
                worker_reconciles.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            policy,
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            Some(worker_observer.as_ref()),
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(first_sender);
    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let recovered_attempts = observer
        .snapshot()
        .into_iter()
        .filter(|observation| observation.state == ConfigSyncConnectionState::Recovered)
        .map(|observation| observation.reconnect_attempt)
        .collect::<Vec<_>>();
    assert_eq!(recovered_attempts, vec![1, 1]);

    shutdown.cancel();
    worker.await.unwrap().unwrap();
    drop(second_sender);
}

#[tokio::test]
async fn reload_supervisor_reconnects_after_subscription_closes_and_reconciles_again() {
    let (first_sender, _) = broadcast::channel(8);
    let (second_sender, _) = broadcast::channel(8);
    let notifier = Arc::new(ScriptedConfigNotifier::new([
        SubscribeStep::Channel(first_sender.clone()),
        SubscribeStep::Channel(second_sender.clone()),
    ]));
    let shutdown = CancellationToken::new();
    let reconciles = Arc::new(AtomicUsize::new(0));
    let observer = Arc::new(TestConnectionObserver::default());
    let worker_notifier = notifier.clone();
    let worker_shutdown = shutdown.clone();
    let worker_reconciles = reconciles.clone();
    let worker_observer = observer.clone();

    let worker = tokio::spawn(async move {
        let mut reconcile = move || {
            let worker_reconciles = worker_reconciles.clone();
            async move {
                worker_reconciles.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            Some(worker_observer.as_ref()),
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(first_sender);

    timeout(Duration::from_secs(1), async {
        while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let states = observer
        .snapshot()
        .into_iter()
        .map(|observation| observation.state)
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            ConfigSyncConnectionState::Connected,
            ConfigSyncConnectionState::Disconnected,
            ConfigSyncConnectionState::Reconnecting,
            ConfigSyncConnectionState::Recovered,
        ]
    );

    shutdown.cancel();
    worker.await.unwrap().unwrap();
    drop(second_sender);
}

#[tokio::test]
async fn reload_supervisor_keeps_subscription_after_reconcile_error() {
    let (sender, _) = broadcast::channel(8);
    let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Channel(
        sender.clone(),
    )]));
    let shutdown = CancellationToken::new();
    let reconcile_attempts = Arc::new(AtomicUsize::new(0));
    let reloads = Arc::new(AtomicUsize::new(0));
    let worker_notifier = notifier.clone();
    let worker_shutdown = shutdown.clone();
    let worker_reconcile_attempts = reconcile_attempts.clone();
    let worker_reloads = reloads.clone();

    let worker = tokio::spawn(async move {
        let mut reconcile = move || {
            let worker_reconcile_attempts = worker_reconcile_attempts.clone();
            async move {
                worker_reconcile_attempts.fetch_add(1, Ordering::SeqCst);
                Err(ConfigCoreError::store("temporary reconcile failure"))
            }
        };
        let mut reload = move |_| {
            let worker_reloads = worker_reloads.clone();
            async move {
                worker_reloads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            None,
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while reconcile_attempts.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    sender
        .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
            "aster_test",
            "node-b",
            ["feature"],
            ConfigNotificationSource::Api,
        )))
        .unwrap();

    timeout(Duration::from_secs(1), async {
        while reloads.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(notifier.subscribe_attempts(), 1);

    shutdown.cancel();
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn reload_supervisor_shutdown_interrupts_reconnect_backoff() {
    let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Fail(
        "redis unavailable",
    )]));
    let shutdown = CancellationToken::new();
    let observer = Arc::new(TestConnectionObserver::default());
    let worker_shutdown = shutdown.clone();
    let worker_observer = observer.clone();
    let policy = ConfigReloadReconnectPolicy {
        initial_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(60),
        stable_reset_after: Duration::from_secs(30),
        jitter_min_percent: 50,
        jitter_max_percent: 100,
    };

    let worker = tokio::spawn(async move {
        let mut reconcile = || async { Ok(()) };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            policy,
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            Some(worker_observer.as_ref()),
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while !observer
            .snapshot()
            .iter()
            .any(|observation| observation.state == ConfigSyncConnectionState::Reconnecting)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    timeout(Duration::from_millis(100), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn reload_supervisor_shutdown_interrupts_pending_subscribe() {
    let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Pending]));
    let shutdown = CancellationToken::new();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();

    let worker = tokio::spawn(async move {
        let mut reconcile = || async { Ok(()) };
        let mut reload = |_| async { Ok(()) };
        run_config_reload_supervisor_inner(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            zero_reconnect_policy(),
            worker_shutdown,
            &mut reconcile,
            &mut reload,
            None,
            None,
        )
        .await
    });

    timeout(Duration::from_secs(1), async {
        while notifier.subscribe_attempts() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    timeout(Duration::from_millis(100), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
fn reconnect_delay_grows_with_equal_jitter_and_caps() {
    let policy = ConfigReloadReconnectPolicy {
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(250),
        stable_reset_after: Duration::from_secs(30),
        jitter_min_percent: 50,
        jitter_max_percent: 100,
    };
    let expected_bounds = [(1, 50, 100), (2, 100, 200), (3, 125, 250), (64, 125, 250)];

    for (attempt, min_ms, max_ms) in expected_bounds {
        for _ in 0..64 {
            let delay_ms =
                super::duration_millis_u64(config_reload_reconnect_delay(policy, attempt));
            assert!(
                (min_ms..=max_ms).contains(&delay_ms),
                "attempt {attempt} produced {delay_ms}ms outside [{min_ms}, {max_ms}]"
            );
        }
    }
}

#[tokio::test]
async fn reload_worker_reloads_matching_remote_messages_until_cancelled() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let shutdown = CancellationToken::new();
    let reloads = Arc::new(AtomicUsize::new(0));
    let observed = reloads.clone();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();

    let worker = tokio::spawn(async move {
        run_config_reload_worker(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            worker_shutdown,
            move |_| {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await
    });

    wait_for_subscriber(&notifier).await;

    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-a",
            ["local"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "other",
            "node-b",
            ["foreign"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-b",
            ["remote"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();

    timeout(Duration::from_secs(1), async {
        while reloads.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    shutdown.cancel();
    worker.await.unwrap().unwrap();
    assert_eq!(reloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reload_worker_keeps_listening_after_reload_error() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let shutdown = CancellationToken::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();

    let worker = tokio::spawn(async move {
        run_config_reload_worker(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            worker_shutdown,
            move |_| {
                let observed = observed.clone();
                async move {
                    let attempt = observed.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err(ConfigCoreError::store("temporary reload failure"))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await
    });

    wait_for_subscriber(&notifier).await;

    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-b",
            ["first"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-c",
            ["second"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();

    timeout(Duration::from_secs(1), async {
        while attempts.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    shutdown.cancel();
    worker.await.unwrap().unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[derive(Default)]
struct TestReloadObserver {
    observations: Mutex<Vec<super::ConfigReloadObservation>>,
}

impl super::ConfigReloadObserver for TestReloadObserver {
    fn observe_config_reload(&self, observation: super::ConfigReloadObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

impl TestReloadObserver {
    fn snapshot(&self) -> Vec<super::ConfigReloadObservation> {
        self.observations.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn reload_worker_observes_decisions_and_reload_errors() {
    let notifier = Arc::new(InMemoryConfigNotifier::default());
    let shutdown = CancellationToken::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed_attempts = attempts.clone();
    let observer = Arc::new(TestReloadObserver::default());
    let worker_observer = observer.clone();
    let worker_shutdown = shutdown.clone();
    let worker_notifier = notifier.clone();

    let worker = tokio::spawn(async move {
        run_config_reload_worker_with_observer(
            worker_notifier,
            ConfigReloadWorkerConfig::new("aster_test", "node-a"),
            worker_shutdown,
            move |_| {
                let observed_attempts = observed_attempts.clone();
                async move {
                    let attempt = observed_attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err(ConfigCoreError::store("temporary reload failure"))
                    } else {
                        Ok(())
                    }
                }
            },
            Some(worker_observer.as_ref()),
        )
        .await
    });

    wait_for_subscriber(&notifier).await;

    notifier
        .publish_reload(ConfigReloadMessage::new(
            "other",
            "node-b",
            ["foreign"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-a",
            ["local"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-b",
            ["first"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "aster_test",
            "node-c",
            ["second", "third"],
            ConfigNotificationSource::Api,
        ))
        .await
        .unwrap();

    timeout(Duration::from_secs(1), async {
        while observer.snapshot().len() != 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    shutdown.cancel();
    worker.await.unwrap().unwrap();

    let observations = observer.snapshot();
    assert_eq!(observations.len(), 4);
    assert_eq!(
        observations[0].decision,
        ConfigReloadDecision::IgnoredNamespace
    );
    assert_eq!(observations[0].status, "ok");
    assert_eq!(observations[0].changed_keys, 1);
    assert_eq!(
        observations[1].decision,
        ConfigReloadDecision::IgnoredOrigin
    );
    assert_eq!(observations[1].status, "ok");
    assert_eq!(observations[2].decision, ConfigReloadDecision::Reloaded);
    assert_eq!(observations[2].status, "error");
    assert_eq!(observations[3].decision, ConfigReloadDecision::Reloaded);
    assert_eq!(observations[3].status, "ok");
    assert_eq!(observations[3].changed_keys, 2);
    assert!(observations.iter().all(|observation| {
        observation.source == "pubsub" && observation.duration_seconds >= 0.0
    }));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
