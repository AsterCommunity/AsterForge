//! Async snapshot ordering, read isolation, restart boundaries, and cancellation.
//!
//! A store captures its authoritative rows when loading starts, then waits on a
//! oneshot gate. Tests poll futures explicitly so every interleaving is fixed;
//! an unexpectedly blocked operation fails immediately instead of timing out.

use std::future::Future;
use std::pin::pin;
use std::sync::Mutex;
use std::task::Poll;

use aster_forge_config::{
    AsyncConfigSnapshot, AsyncConfigStore, AsyncRuntimeConfig, ConfigCoreError, ConfigSource,
    ConfigValueType, ConfigVisibility, Result, RuntimeConfigChange, StoredConfig,
};
use async_trait::async_trait;
use futures::poll;
use tokio::sync::{mpsc, oneshot};

type LoadGate = oneshot::Sender<Result<()>>;

struct ControlledStore {
    rows: Mutex<Vec<StoredConfig>>,
    loads: mpsc::UnboundedSender<LoadGate>,
}

impl ControlledStore {
    fn new(rows: Vec<StoredConfig>) -> (Self, mpsc::UnboundedReceiver<LoadGate>) {
        let (loads, receiver) = mpsc::unbounded_channel();
        (
            Self {
                rows: Mutex::new(rows),
                loads,
            },
            receiver,
        )
    }

    fn set_rows(&self, rows: Vec<StoredConfig>) {
        *self.rows.lock().unwrap() = rows;
    }
}

#[async_trait]
impl AsyncConfigStore for ControlledStore {
    async fn load_all(&self) -> Result<Vec<StoredConfig>> {
        let rows = self.rows.lock().unwrap().clone();
        let (release, wait) = oneshot::channel();
        self.loads.send(release).expect("load observer stays open");
        wait.await
            .map_err(|_| ConfigCoreError::store("test gate closed"))??;
        Ok(rows)
    }
}

struct StaticStore(Vec<StoredConfig>);

#[async_trait]
impl AsyncConfigStore for StaticStore {
    async fn load_all(&self) -> Result<Vec<StoredConfig>> {
        Ok(self.0.clone())
    }
}

fn row(key: &str, value: &str, requires_restart: bool) -> StoredConfig {
    StoredConfig {
        id: 1,
        key: key.to_string(),
        value: value.to_string(),
        value_type: ConfigValueType::String,
        requires_restart,
        is_sensitive: false,
        source: ConfigSource::System,
        visibility: ConfigVisibility::Private,
        category: "test".to_string(),
        description: "test row".to_string(),
    }
}

async fn immediately<T>(future: impl Future<Output = T>) -> T {
    let future = pin!(future);
    let Poll::Ready(value) = poll!(future) else {
        panic!("operation must complete without waiting for another task or storage I/O");
    };
    value
}

fn next_load(loads: &mut mpsc::UnboundedReceiver<LoadGate>) -> LoadGate {
    loads
        .try_recv()
        .expect("the polled reload must start loading")
}

fn assert_no_load(loads: &mut mpsc::UnboundedReceiver<LoadGate>) {
    assert!(
        matches!(loads.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "a queued reload must not read the store before earlier updates finish"
    );
}

#[tokio::test]
async fn empty_and_unchanged_updates_have_empty_diffs() {
    for runtime in [AsyncRuntimeConfig::new(), AsyncRuntimeConfig::default()] {
        assert_eq!(immediately(runtime.get("missing")).await, None);
        assert_eq!(immediately(runtime.get_model("missing")).await, None);
        assert!(immediately(runtime.snapshot()).await.values().is_empty());
        assert_eq!(immediately(runtime.remove("missing")).await, None);
        assert!(
            runtime
                .reload(&StaticStore(vec![]))
                .await
                .unwrap()
                .is_empty()
        );

        let existing = row("value", "same", false);
        assert_eq!(
            runtime.apply(existing.clone()).await,
            Some(RuntimeConfigChange::Upserted(existing.clone()))
        );
        assert_eq!(runtime.apply(existing.clone()).await, None);
        let store = StaticStore(vec![existing]);
        let store: &dyn AsyncConfigStore = &store;
        assert!(runtime.reload(store).await.unwrap().is_empty());
        assert_eq!(
            runtime.remove("value").await,
            Some(RuntimeConfigChange::Removed("value".to_string()))
        );
        assert_eq!(runtime.remove("value").await, None);
    }
}

#[tokio::test]
async fn reload_reports_sorted_full_record_diffs_and_retained_snapshots_stay_unchanged() {
    let runtime = AsyncRuntimeConfig::new();
    let previous = row("b", "same", false);
    runtime
        .reload(&StaticStore(vec![
            row("a", "removed", false),
            previous.clone(),
        ]))
        .await
        .unwrap();
    let retained = runtime.snapshot().await;
    let mut metadata_update = previous.clone();
    metadata_update.id = 2;
    metadata_update.is_sensitive = true;
    metadata_update.visibility = ConfigVisibility::Public;
    metadata_update.category = "changed".to_string();
    metadata_update.description = "changed metadata".to_string();
    let added = row("c", "added", false);

    assert_eq!(
        runtime
            .reload(&StaticStore(vec![added.clone(), metadata_update.clone()]))
            .await
            .unwrap(),
        vec![
            RuntimeConfigChange::Removed("a".to_string()),
            RuntimeConfigChange::Upserted(metadata_update.clone()),
            RuntimeConfigChange::Upserted(added),
        ]
    );
    assert_eq!(retained.get("a"), Some("removed"));
    assert_eq!(retained.get_model("b"), Some(&previous));
    assert_eq!(retained.get("c"), None);
    assert_eq!(runtime.get_model("b").await, Some(metadata_update));
    assert_eq!(
        runtime.reload(&StaticStore(vec![])).await.unwrap(),
        vec![
            RuntimeConfigChange::Removed("b".to_string()),
            RuntimeConfigChange::Removed("c".to_string()),
        ]
    );
}

#[tokio::test]
async fn restart_decisions_use_the_incoming_flag_and_preserve_the_entire_existing_record() {
    for previous_restart in [false, true] {
        for incoming_restart in [false, true] {
            for use_reload in [false, true] {
                let runtime = AsyncRuntimeConfig::new();
                let previous = row("value", "old", previous_restart);
                runtime.apply(previous.clone()).await;
                let mut incoming = row("value", "new", incoming_restart);
                incoming.id = 2;
                incoming.is_sensitive = true;
                incoming.visibility = ConfigVisibility::Public;
                incoming.description = "new metadata".to_string();
                let changes = if use_reload {
                    runtime
                        .reload(&StaticStore(vec![incoming.clone()]))
                        .await
                        .unwrap()
                } else {
                    runtime.apply(incoming.clone()).await.into_iter().collect()
                };
                let (expected_record, expected_diff) = if incoming_restart {
                    (previous, vec![])
                } else {
                    (
                        incoming.clone(),
                        vec![RuntimeConfigChange::Upserted(incoming)],
                    )
                };
                assert_eq!(changes, expected_diff);
                assert_eq!(runtime.get_model("value").await, Some(expected_record));
            }
        }
    }
}

#[tokio::test]
async fn readers_return_the_old_complete_snapshot_while_reload_and_updates_are_pending() {
    let runtime = AsyncRuntimeConfig::new();
    let existing = row("value", "old", false);
    let removed = row("removed", "present", false);
    runtime.apply(existing.clone()).await;
    runtime.apply(removed.clone()).await;
    let before = runtime.snapshot().await;
    let new = row("value", "new", false);
    let added = row("added", "new", false);
    let (store, mut loads) = ControlledStore::new(vec![new.clone(), added.clone()]);
    let mut reload = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut reload).is_pending());
    let release = next_load(&mut loads);
    let queued_row = row("queued", "later", false);
    let mut apply = Box::pin(runtime.apply(queued_row.clone()));
    let mut remove = Box::pin(runtime.remove("value"));
    assert!(poll!(&mut apply).is_pending());
    assert!(poll!(&mut remove).is_pending());

    // Even with waiting writers, storage I/O does not enqueue a snapshot writer.
    assert_eq!(
        immediately(runtime.get("value")).await.as_deref(),
        Some("old")
    );
    assert_eq!(
        immediately(runtime.get_model("value")).await,
        Some(existing)
    );
    assert_eq!(immediately(runtime.get("missing")).await, None);
    assert_eq!(immediately(runtime.get_model("missing")).await, None);
    assert_eq!(immediately(runtime.snapshot()).await, before);
    assert_eq!(
        immediately(runtime.get("removed")).await.as_deref(),
        Some("present")
    );
    assert_eq!(immediately(runtime.get("added")).await, None);
    assert_eq!(immediately(runtime.get("queued")).await, None);

    release.send(Ok(())).unwrap();
    assert_eq!(
        immediately(reload).await.unwrap(),
        vec![
            RuntimeConfigChange::Upserted(added.clone()),
            RuntimeConfigChange::Removed("removed".to_string()),
            RuntimeConfigChange::Upserted(new.clone()),
        ]
    );
    assert_eq!(
        immediately(runtime.snapshot()).await,
        AsyncConfigSnapshot::from_configs(vec![new, added])
    );
    assert_eq!(
        immediately(apply).await,
        Some(RuntimeConfigChange::Upserted(queued_row))
    );
    assert_eq!(
        immediately(remove).await,
        Some(RuntimeConfigChange::Removed("value".to_string()))
    );
    assert_eq!(before.get("value"), Some("old"));
    assert_eq!(before.get_model("removed"), Some(&removed));
}

#[tokio::test]
async fn concurrent_reloads_capture_and_publish_authoritative_rows_in_order() {
    let runtime = AsyncRuntimeConfig::new();
    let old = row("value", "old", false);
    let new = row("value", "new", false);
    let (store, mut loads) = ControlledStore::new(vec![old.clone()]);
    let mut first = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut first).is_pending());
    let first_release = next_load(&mut loads);
    store.set_rows(vec![new.clone()]);
    let mut second = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut second).is_pending());
    assert_no_load(&mut loads);
    assert_eq!(immediately(runtime.get("value")).await, None);

    first_release.send(Ok(())).unwrap();
    assert_eq!(
        immediately(first).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(old)]
    );
    assert!(poll!(&mut second).is_pending());
    next_load(&mut loads).send(Ok(())).unwrap();
    assert_eq!(
        immediately(second).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(new.clone())]
    );
    assert_eq!(runtime.get_model("value").await, Some(new));
}

#[tokio::test]
async fn queued_apply_publishes_after_reload_and_reports_its_own_diff() {
    let runtime = AsyncRuntimeConfig::new();
    let loaded = row("value", "loaded", false);
    let applied = row("value", "applied", false);
    let (store, mut loads) = ControlledStore::new(vec![loaded.clone()]);
    let mut reload = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut reload).is_pending());
    let release = next_load(&mut loads);
    let mut apply = Box::pin(runtime.apply(applied.clone()));
    assert!(poll!(&mut apply).is_pending());

    release.send(Ok(())).unwrap();
    assert_eq!(
        immediately(reload).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(loaded)]
    );
    assert_eq!(
        immediately(apply).await,
        Some(RuntimeConfigChange::Upserted(applied.clone()))
    );
    assert_eq!(runtime.get_model("value").await, Some(applied));
}

#[tokio::test]
async fn queued_apply_checks_noop_and_restart_against_the_published_reload() {
    for requires_restart in [false, true] {
        let runtime = AsyncRuntimeConfig::new();
        let loaded = row("value", "loaded", false);
        let incoming = if requires_restart {
            row("value", "restart-only", true)
        } else {
            loaded.clone()
        };
        let (store, mut loads) = ControlledStore::new(vec![loaded.clone()]);
        let mut reload = Box::pin(runtime.reload(&store));
        assert!(poll!(&mut reload).is_pending());
        let release = next_load(&mut loads);
        let mut apply = Box::pin(runtime.apply(incoming));
        assert!(poll!(&mut apply).is_pending());

        release.send(Ok(())).unwrap();
        immediately(reload).await.unwrap();
        assert_eq!(immediately(apply).await, None);
        assert_eq!(runtime.get_model("value").await, Some(loaded));
    }
}

#[tokio::test]
async fn queued_remove_uses_reload_presence_even_when_the_key_was_initially_missing() {
    for initially_present in [false, true] {
        for loaded_present in [false, true] {
            let runtime = AsyncRuntimeConfig::new();
            if initially_present {
                runtime.apply(row("value", "existing", false)).await;
            }
            let loaded = if loaded_present {
                vec![row("value", "loaded", false)]
            } else {
                vec![]
            };
            let (store, mut loads) = ControlledStore::new(loaded);
            let mut reload = Box::pin(runtime.reload(&store));
            assert!(poll!(&mut reload).is_pending());
            let release = next_load(&mut loads);
            let mut remove = Box::pin(runtime.remove("value"));
            assert!(poll!(&mut remove).is_pending());

            release.send(Ok(())).unwrap();
            immediately(reload).await.unwrap();
            assert_eq!(
                immediately(remove).await,
                loaded_present.then(|| RuntimeConfigChange::Removed("value".to_string()))
            );
            assert_eq!(runtime.get_model("value").await, None);
        }
    }
}

#[tokio::test]
async fn mixed_queued_updates_cannot_be_overtaken_by_a_later_reload() {
    let runtime = AsyncRuntimeConfig::new();
    let (store, mut loads) = ControlledStore::new(vec![row("value", "first", false)]);
    let mut first = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut first).is_pending());
    let release = next_load(&mut loads);
    let mut apply = Box::pin(runtime.apply(row("value", "applied", false)));
    let mut remove = Box::pin(runtime.remove("value"));
    let mut second = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut apply).is_pending());
    assert!(poll!(&mut remove).is_pending());
    assert!(poll!(&mut second).is_pending());
    store.set_rows(vec![row("value", "latest", false)]);

    release.send(Ok(())).unwrap();
    immediately(first).await.unwrap();
    // Poll the last waiter first: it must not overtake apply or remove.
    assert!(poll!(&mut second).is_pending());
    assert_no_load(&mut loads);
    assert!(immediately(apply).await.is_some());
    assert!(poll!(&mut second).is_pending());
    assert_no_load(&mut loads);
    assert_eq!(
        immediately(remove).await,
        Some(RuntimeConfigChange::Removed("value".to_string()))
    );
    assert!(poll!(&mut second).is_pending());
    next_load(&mut loads).send(Ok(())).unwrap();
    assert_eq!(
        immediately(second).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(row("value", "latest", false))]
    );
}

#[tokio::test]
async fn restart_required_keys_can_be_removed_and_reinserted_in_queue_order() {
    let runtime = AsyncRuntimeConfig::new();
    let previous = row("value", "old", true);
    let incoming = row("value", "new", true);
    runtime.apply(previous.clone()).await;
    let (store, mut loads) = ControlledStore::new(vec![incoming.clone()]);
    let mut first = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut first).is_pending());
    let release = next_load(&mut loads);
    let mut remove = Box::pin(runtime.remove("value"));
    let mut second = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut remove).is_pending());
    assert!(poll!(&mut second).is_pending());

    release.send(Ok(())).unwrap();
    assert!(immediately(first).await.unwrap().is_empty());
    assert_eq!(runtime.get_model("value").await, Some(previous));
    assert_eq!(
        immediately(remove).await,
        Some(RuntimeConfigChange::Removed("value".to_string()))
    );
    assert!(poll!(&mut second).is_pending());
    next_load(&mut loads).send(Ok(())).unwrap();
    assert_eq!(
        immediately(second).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(incoming.clone())]
    );
    assert_eq!(runtime.get_model("value").await, Some(incoming));
    assert_eq!(
        runtime.reload(&StaticStore(vec![])).await.unwrap(),
        vec![RuntimeConfigChange::Removed("value".to_string())]
    );
    assert_eq!(
        runtime.apply(row("value", "fresh", true)).await,
        Some(RuntimeConfigChange::Upserted(row("value", "fresh", true)))
    );
}

#[tokio::test]
async fn failed_reload_keeps_the_full_snapshot_and_unblocks_queued_reload() {
    for populated in [false, true] {
        let runtime = AsyncRuntimeConfig::new();
        if populated {
            runtime.apply(row("value", "existing", false)).await;
            runtime.apply(row("restart", "existing", true)).await;
        }
        let before = runtime.snapshot().await;
        let (store, mut loads) = ControlledStore::new(vec![row("value", "failed", false)]);
        let mut failed = Box::pin(runtime.reload(&store));
        assert!(poll!(&mut failed).is_pending());
        let release = next_load(&mut loads);
        let mut recovery = Box::pin(runtime.reload(&store));
        assert!(poll!(&mut recovery).is_pending());
        assert_no_load(&mut loads);
        release
            .send(Err(ConfigCoreError::store("database unavailable")))
            .unwrap();
        assert!(matches!(
            immediately(failed).await,
            Err(ConfigCoreError::Store(message)) if message == "database unavailable"
        ));
        assert_eq!(immediately(runtime.snapshot()).await, before);

        let recovered = row("value", "recovered", false);
        store.set_rows(vec![recovered.clone()]);
        assert!(poll!(&mut recovery).is_pending());
        next_load(&mut loads).send(Ok(())).unwrap();
        let mut expected = vec![];
        if populated {
            expected.push(RuntimeConfigChange::Removed("restart".to_string()));
        }
        expected.push(RuntimeConfigChange::Upserted(recovered.clone()));
        assert_eq!(immediately(recovery).await.unwrap(), expected);
        assert_eq!(runtime.get_model("value").await, Some(recovered));
        assert!(
            immediately(runtime.apply(row("after", "failure", false)))
                .await
                .is_some()
        );
        assert!(immediately(runtime.remove("after")).await.is_some());
    }
}

#[tokio::test]
async fn failed_reload_unblocks_already_queued_apply_and_remove() {
    let runtime = AsyncRuntimeConfig::new();
    runtime.apply(row("removed", "existing", false)).await;
    let (store, mut loads) = ControlledStore::new(vec![row("failed", "never-published", false)]);
    let mut reload = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut reload).is_pending());
    let release = next_load(&mut loads);
    let applied = row("applied", "new", false);
    let mut apply = Box::pin(runtime.apply(applied.clone()));
    let mut remove = Box::pin(runtime.remove("removed"));
    assert!(poll!(&mut apply).is_pending());
    assert!(poll!(&mut remove).is_pending());

    release
        .send(Err(ConfigCoreError::store("database unavailable")))
        .unwrap();
    assert!(immediately(reload).await.is_err());
    assert_eq!(
        immediately(apply).await,
        Some(RuntimeConfigChange::Upserted(applied.clone()))
    );
    assert_eq!(
        immediately(remove).await,
        Some(RuntimeConfigChange::Removed("removed".to_string()))
    );
    assert_eq!(
        runtime.snapshot().await,
        AsyncConfigSnapshot::from_configs(vec![applied])
    );
}

#[tokio::test]
async fn cancelling_active_reload_discards_the_load_and_unblocks_every_update_kind() {
    let runtime = AsyncRuntimeConfig::new();
    let existing = row("removed", "existing", false);
    runtime.apply(existing.clone()).await;
    let before = runtime.snapshot().await;
    let (store, mut loads) = ControlledStore::new(vec![row("cancelled", "never-published", false)]);
    let mut cancelled = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut cancelled).is_pending());
    let release = next_load(&mut loads);
    let applied = row("applied", "new", false);
    let mut apply = Box::pin(runtime.apply(applied.clone()));
    let mut remove = Box::pin(runtime.remove("removed"));
    let mut recovery = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut apply).is_pending());
    assert!(poll!(&mut remove).is_pending());
    assert!(poll!(&mut recovery).is_pending());

    drop(cancelled);
    assert!(
        release.is_closed(),
        "cancellation must drop the store future"
    );
    assert_eq!(immediately(runtime.snapshot()).await, before);
    assert!(poll!(&mut recovery).is_pending());
    assert_no_load(&mut loads);
    assert_eq!(
        immediately(apply).await,
        Some(RuntimeConfigChange::Upserted(applied))
    );
    assert_eq!(
        immediately(remove).await,
        Some(RuntimeConfigChange::Removed("removed".to_string()))
    );

    let recovered = row("recovered", "latest", false);
    store.set_rows(vec![recovered.clone()]);
    assert!(poll!(&mut recovery).is_pending());
    next_load(&mut loads).send(Ok(())).unwrap();
    assert_eq!(
        immediately(recovery).await.unwrap(),
        vec![
            RuntimeConfigChange::Removed("applied".to_string()),
            RuntimeConfigChange::Upserted(recovered.clone()),
        ]
    );
    assert_eq!(
        runtime.snapshot().await,
        AsyncConfigSnapshot::from_configs(vec![recovered])
    );
}

#[tokio::test]
async fn cancelled_waiters_neither_load_nor_mutate_and_do_not_block_the_survivor() {
    let runtime = AsyncRuntimeConfig::new();
    let loaded = row("value", "loaded", false);
    let (store, mut loads) = ControlledStore::new(vec![loaded.clone()]);
    let mut active = Box::pin(runtime.reload(&store));
    assert!(poll!(&mut active).is_pending());
    let release = next_load(&mut loads);
    let mut cancelled_reload = Box::pin(runtime.reload(&store));
    let mut cancelled_apply = Box::pin(runtime.apply(row("cancelled", "never-published", false)));
    let mut cancelled_remove = Box::pin(runtime.remove("value"));
    let survived = row("survived", "new", false);
    let mut survivor = Box::pin(runtime.apply(survived.clone()));
    assert!(poll!(&mut cancelled_reload).is_pending());
    assert!(poll!(&mut cancelled_apply).is_pending());
    assert!(poll!(&mut cancelled_remove).is_pending());
    assert!(poll!(&mut survivor).is_pending());
    assert_no_load(&mut loads);
    // Remove a middle waiter first, then the head and the next waiter.
    drop(cancelled_apply);
    drop(cancelled_reload);
    drop(cancelled_remove);

    release.send(Ok(())).unwrap();
    assert_eq!(
        immediately(active).await.unwrap(),
        vec![RuntimeConfigChange::Upserted(loaded.clone())]
    );
    assert_eq!(
        immediately(survivor).await,
        Some(RuntimeConfigChange::Upserted(survived.clone()))
    );
    assert_no_load(&mut loads);
    assert_eq!(
        runtime.snapshot().await,
        AsyncConfigSnapshot::from_configs(vec![loaded, survived])
    );
}
