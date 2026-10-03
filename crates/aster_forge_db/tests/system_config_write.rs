//! Validation, persistence, and product transaction boundaries for system config writes.
#![cfg(feature = "system-config")]

use aster_forge_config::{
    ConfigChangeEvent, ConfigChangeNotifier, ConfigCoreError, ConfigDefinition, ConfigNotification,
    ConfigNotificationSource, ConfigRegistry, ConfigReloadMessage, ConfigSource, ConfigValue,
    ConfigValueLookup, ConfigValueType, ConfigVisibility, InMemoryConfigNotifier,
    SyncRuntimeConfig, normalize_bounded_u64_config_value,
};
use aster_forge_db::system_config::{
    self, Model, SystemConfigDbBinding, SystemConfigDbStore, SystemConfigUpsert,
};
use aster_forge_db::{DbError, transaction};
use futures::FutureExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

fn normalize_ttl(
    _: &dyn ConfigValueLookup,
    key: &str,
    value: &str,
) -> aster_forge_config::Result<String> {
    normalize_bounded_u64_config_value(key, value, 1, 60)
}

fn require_enabled(
    lookup: &dyn ConfigValueLookup,
    _: &str,
    _: &str,
) -> aster_forge_config::Result<()> {
    if lookup.get_config_value("enabled").as_deref() != Some("true") {
        return Err(ConfigCoreError::invalid_value("requires enabled=true"));
    }
    Ok(())
}

const TTL: ConfigDefinition = ConfigDefinition {
    key: "ttl",
    value_type: ConfigValueType::Number,
    is_sensitive: true,
    normalize_fn: Some(normalize_ttl),
    dependency_validator_fn: Some(require_enabled),
    ..ConfigDefinition::private_system()
};
static REGISTRY: ConfigRegistry = ConfigRegistry::new(&[
    TTL,
    ConfigDefinition {
        key: "restart_ttl",
        requires_restart: true,
        ..TTL
    },
]);
static BINDING: SystemConfigDbBinding = SystemConfigDbBinding::new(&REGISTRY, &[]);

async fn database() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute(&system_config::create_system_config_table(
        db.get_database_backend(),
    ))
    .await
    .unwrap();
    db.execute(&system_config::create_system_config_key_unique_index())
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("CREATE TABLE audit_actors (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    db.execute_unprepared(
        "CREATE TABLE product_audit (
            config_key TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action = 'config.write'),
            value TEXT NOT NULL,
            actor INTEGER REFERENCES audit_actors(id) DEFERRABLE INITIALLY DEFERRED
        )",
    )
    .await
    .unwrap();
    db
}

#[derive(Clone, Copy, Debug)]
enum Writer {
    Binding,
    Store,
    FreeFunction,
}

async fn write(writer: Writer, db: &DatabaseConnection, request: SystemConfigUpsert<'_>) -> Model {
    match writer {
        Writer::Binding => BINDING.upsert_prevalidated(db, request).await.unwrap(),
        Writer::Store => SystemConfigDbStore::new(db.clone(), &REGISTRY, &[])
            .upsert_prevalidated(request)
            .await
            .unwrap(),
        Writer::FreeFunction => system_config::upsert_prevalidated(db, &REGISTRY, request)
            .await
            .unwrap(),
    }
}

#[tokio::test]
async fn all_persistence_entries_write_verbatim_without_running_validation() {
    for writer in [Writer::Binding, Writer::Store, Writer::FreeFunction] {
        let db = database().await;
        let mut row_id = None;
        for value in ["not-a-number", "61", "secret:v1:opaque-envelope", ""] {
            assert!(
                REGISTRY
                    .normalize_value(&|_: &str| None, "ttl", value)
                    .is_err()
            );
            let row = write(
                writer,
                &db,
                SystemConfigUpsert {
                    key: "ttl",
                    value,
                    visibility: Some(ConfigVisibility::Public),
                    updated_by: Some(42),
                },
            )
            .await;
            assert_eq!(row.value, value, "{writer:?}");
            assert_eq!(row.value_type, ConfigValueType::Number);
            assert!(row.is_sensitive);
            assert_eq!(row.source, ConfigSource::System);
            assert_eq!(row.visibility, ConfigVisibility::Private);
            assert_eq!(row.updated_by, Some(42));
            if let Some(id) = row_id {
                assert_eq!(row.id, id);
            }
            row_id = Some(row.id);
            assert_eq!(BINDING.find_by_key(&db, "ttl").await.unwrap().unwrap(), row);
        }
    }
}

#[tokio::test]
async fn prevalidated_boundary_values_and_custom_updates_work_through_every_entry() {
    for writer in [Writer::Binding, Writer::Store, Writer::FreeFunction] {
        let db = database().await;
        for input in ["1", " 060 "] {
            let value = REGISTRY
                .value_to_storage_for_key(
                    &|key: &str| (key == "enabled").then(|| "true".to_string()),
                    "ttl",
                    &ConfigValue::from(input),
                )
                .unwrap();
            let row = write(
                writer,
                &db,
                SystemConfigUpsert {
                    key: "ttl",
                    value: &value,
                    visibility: None,
                    updated_by: None,
                },
            )
            .await;
            assert_eq!(row.value, input.trim().parse::<u64>().unwrap().to_string());
        }
        let custom = write(
            writer,
            &db,
            SystemConfigUpsert {
                key: "custom",
                value: "  unmodified  ",
                visibility: Some(ConfigVisibility::Public),
                updated_by: Some(7),
            },
        )
        .await;
        let updated = write(
            writer,
            &db,
            SystemConfigUpsert {
                key: "custom",
                value: "",
                visibility: Some(ConfigVisibility::Authenticated),
                updated_by: None,
            },
        )
        .await;
        assert_eq!(custom.value, "  unmodified  ");
        assert_eq!(custom.source, ConfigSource::Custom);
        assert_eq!(updated.id, custom.id);
        assert_eq!(updated.value, "");
        assert_eq!(updated.visibility, ConfigVisibility::Authenticated);
        assert_eq!(updated.updated_by, None);
    }
}

#[derive(Debug, thiserror::Error)]
enum WriteError {
    #[error(transparent)]
    Core(#[from] ConfigCoreError),
    #[error(transparent)]
    Database(#[from] DbError),
    #[error("encoding failed")]
    Encoding,
    #[error("audit failed")]
    Audit,
}

#[derive(Clone, Copy, PartialEq)]
enum Failure {
    None,
    Encoding,
    Audit,
    Commit,
}

// A test-only product service: Forge never owns encoding or audit policy.
async fn save(
    db: &DatabaseConnection,
    runtime: &SyncRuntimeConfig<Model>,
    notifier: &dyn ConfigChangeNotifier,
    key: &str,
    value: &ConfigValue,
    failure: Failure,
) -> Result<Model, WriteError> {
    let before = runtime.snapshot();
    let normalized = REGISTRY.value_to_storage_for_key(&before, key, value)?;
    if failure == Failure::Encoding {
        return Err(WriteError::Encoding);
    }
    // Opaque fixture, not an encryption implementation.
    let encoded = format!("secret:v1:fixture:{normalized}");
    let saved = transaction::with_transaction(db, async |txn| -> Result<Model, WriteError> {
        let saved = BINDING
            .upsert_prevalidated(
                txn,
                SystemConfigUpsert {
                    key,
                    value: &encoded,
                    visibility: None,
                    updated_by: Some(42),
                },
            )
            .await?;
        assert_eq!(
            runtime.snapshot(),
            before,
            "uncommitted writes must not reach runtime"
        );
        txn.execute_raw(Statement::from_sql_and_values(
            txn.get_database_backend(),
            "INSERT INTO product_audit (config_key, action, value, actor) VALUES (?, ?, ?, ?)",
            [
                key.into(),
                (if failure == Failure::Audit {
                    "invalid"
                } else {
                    "config.write"
                })
                .into(),
                ConfigValue::REDACTED.into(),
                (if failure == Failure::Commit {
                    Some(999_i64)
                } else {
                    None
                })
                .into(),
            ],
        ))
        .await
        .map_err(|_| WriteError::Audit)?;
        Ok(saved)
    })
    .await?;
    runtime.apply(saved.clone());
    notifier
        .publish_reload(ConfigReloadMessage::new(
            "test-product",
            "writer",
            [key],
            ConfigNotificationSource::Api,
        ))
        .await?;
    Ok(saved)
}

async fn seeded_runtime(db: &DatabaseConnection) -> SyncRuntimeConfig<Model> {
    for (key, value) in [("ttl", "5"), ("restart_ttl", "5"), ("enabled", "true")] {
        BINDING
            .upsert_prevalidated(
                db,
                SystemConfigUpsert {
                    key,
                    value,
                    visibility: None,
                    updated_by: None,
                },
            )
            .await
            .unwrap();
    }
    let runtime = SyncRuntimeConfig::new();
    runtime.replace(BINDING.find_all(db).await.unwrap());
    runtime
}

async fn audit_values(db: &DatabaseConnection) -> Vec<String> {
    db.query_all_raw(Statement::from_string(
        db.get_database_backend(),
        "SELECT value FROM product_audit",
    ))
    .await
    .unwrap()
    .iter()
    .map(|row| row.try_get("", "value").unwrap())
    .collect()
}

#[tokio::test]
async fn validation_failures_leave_database_snapshot_audit_and_notifications_unchanged() {
    let db = database().await;
    let runtime = seeded_runtime(&db).await;
    let notifier = InMemoryConfigNotifier::default();
    let mut subscription = notifier.subscribe().await.unwrap();
    let rows_before = BINDING.find_all(&db).await.unwrap();
    let snapshot_before = runtime.snapshot();
    for value in [
        ConfigValue::from("invalid"),
        ConfigValue::from("NaN"),
        ConfigValue::from("0"),
        ConfigValue::from("61"),
        ConfigValue::StringArray(vec![]),
    ] {
        assert!(matches!(
            save(&db, &runtime, &notifier, "ttl", &value, Failure::None).await,
            Err(WriteError::Core(_))
        ));
        assert_eq!(BINDING.find_all(&db).await.unwrap(), rows_before);
        assert_eq!(runtime.snapshot(), snapshot_before);
        assert!(audit_values(&db).await.is_empty());
        assert!(subscription.recv().now_or_never().is_none());
    }
    let lookup = |_: &str| Some("false".to_string());
    assert!(
        REGISTRY
            .value_to_storage_for_key(&lookup, "ttl", &ConfigValue::from("5"))
            .is_err()
    );
    let mut disabled = runtime.get_model("enabled").unwrap();
    disabled.value = "false".to_string();
    runtime.apply(disabled);
    let disabled_before = runtime.snapshot();
    assert!(matches!(
        save(
            &db,
            &runtime,
            &notifier,
            "ttl",
            &ConfigValue::from("5"),
            Failure::None
        )
        .await,
        Err(WriteError::Core(ConfigCoreError::InvalidValue(_)))
    ));
    assert_eq!(BINDING.find_all(&db).await.unwrap(), rows_before);
    assert_eq!(runtime.snapshot(), disabled_before);
    assert!(audit_values(&db).await.is_empty());
    assert!(subscription.recv().now_or_never().is_none());
}

#[tokio::test]
async fn encoding_audit_and_commit_failures_do_not_escape_the_transaction_boundary() {
    for failure in [Failure::Encoding, Failure::Audit, Failure::Commit] {
        let db = database().await;
        let runtime = seeded_runtime(&db).await;
        let notifier = InMemoryConfigNotifier::default();
        let mut subscription = notifier.subscribe().await.unwrap();
        let before = runtime.snapshot();
        let rows_before = BINDING.find_all(&db).await.unwrap();
        let error = save(
            &db,
            &runtime,
            &notifier,
            "ttl",
            &ConfigValue::from("60"),
            failure,
        )
        .await
        .unwrap_err();
        match failure {
            Failure::Encoding => assert!(matches!(error, WriteError::Encoding)),
            Failure::Audit => assert!(matches!(error, WriteError::Audit)),
            Failure::Commit => assert!(matches!(error, WriteError::Database(_))),
            Failure::None => unreachable!(),
        }
        assert_eq!(BINDING.find_all(&db).await.unwrap(), rows_before);
        assert_eq!(runtime.snapshot(), before);
        assert!(audit_values(&db).await.is_empty());
        assert!(subscription.recv().now_or_never().is_none());
    }
}

#[tokio::test]
async fn write_failure_preserves_existing_row_and_runtime() {
    let db = database().await;
    let runtime = seeded_runtime(&db).await;
    db.execute_unprepared("CREATE TRIGGER reject_write BEFORE UPDATE ON system_config BEGIN SELECT RAISE(ABORT, 'write rejected'); END").await.unwrap();
    let notifier = InMemoryConfigNotifier::default();
    let mut subscription = notifier.subscribe().await.unwrap();
    let before = runtime.snapshot();
    let rows_before = BINDING.find_all(&db).await.unwrap();
    assert!(matches!(
        save(
            &db,
            &runtime,
            &notifier,
            "ttl",
            &ConfigValue::from("60"),
            Failure::None
        )
        .await,
        Err(WriteError::Database(_))
    ));
    assert_eq!(BINDING.find_all(&db).await.unwrap(), rows_before);
    assert_eq!(runtime.snapshot(), before);
    assert!(audit_values(&db).await.is_empty());
    assert!(subscription.recv().now_or_never().is_none());
}

#[tokio::test]
async fn successful_commit_updates_snapshot_then_publishes_keys_only_and_redacts_audit() {
    let db = database().await;
    let runtime = seeded_runtime(&db).await;
    let notifier = InMemoryConfigNotifier::default();
    let mut subscription = notifier.subscribe().await.unwrap();
    let observing_notifier = ObservingNotifier {
        db: &db,
        runtime: &runtime,
        inner: &notifier,
    };
    let saved = save(
        &db,
        &runtime,
        &observing_notifier,
        "ttl",
        &ConfigValue::from(" 060 "),
        Failure::None,
    )
    .await
    .unwrap();
    assert_eq!(saved.value, "secret:v1:fixture:60");
    assert_eq!(
        BINDING.find_by_key(&db, "ttl").await.unwrap().unwrap(),
        saved
    );
    assert_eq!(runtime.get_model("ttl").unwrap(), saved);
    assert_eq!(audit_values(&db).await, [ConfigValue::REDACTED]);
    let event = subscription.recv().now_or_never().unwrap().unwrap();
    assert_eq!(
        event,
        ConfigChangeEvent::Reload(ConfigReloadMessage::new(
            "test-product",
            "writer",
            ["ttl"],
            ConfigNotificationSource::Api
        ))
    );
    assert!(subscription.recv().now_or_never().is_none());
}

struct ObservingNotifier<'a> {
    db: &'a DatabaseConnection,
    runtime: &'a SyncRuntimeConfig<Model>,
    inner: &'a InMemoryConfigNotifier,
}

#[async_trait::async_trait]
impl ConfigChangeNotifier for ObservingNotifier<'_> {
    async fn publish_reload(&self, message: ConfigReloadMessage) -> aster_forge_config::Result<()> {
        for key in &message.keys {
            let committed = BINDING.find_by_key(self.db, key).await.unwrap().unwrap();
            assert_eq!(
                self.runtime.get_model(key).unwrap(),
                committed,
                "publish must follow commit and snapshot update"
            );
        }
        assert_eq!(audit_values(self.db).await, [ConfigValue::REDACTED]);
        self.inner.publish_reload(message).await
    }

    async fn subscribe(&self) -> aster_forge_config::Result<ConfigNotification> {
        self.inner.subscribe().await
    }
}

#[tokio::test]
async fn restart_only_write_commits_and_notifies_without_hot_applying() {
    let db = database().await;
    let runtime = seeded_runtime(&db).await;
    let notifier = InMemoryConfigNotifier::default();
    let mut subscription = notifier.subscribe().await.unwrap();
    let saved = save(
        &db,
        &runtime,
        &notifier,
        "restart_ttl",
        &ConfigValue::from("60"),
        Failure::None,
    )
    .await
    .unwrap();
    assert_eq!(
        BINDING
            .find_by_key(&db, "restart_ttl")
            .await
            .unwrap()
            .unwrap(),
        saved
    );
    assert_eq!(runtime.get("restart_ttl").as_deref(), Some("5"));
    assert!(subscription.recv().now_or_never().unwrap().is_ok());
}

struct FailingNotifier;

#[async_trait::async_trait]
impl ConfigChangeNotifier for FailingNotifier {
    async fn publish_reload(&self, _: ConfigReloadMessage) -> aster_forge_config::Result<()> {
        Err(ConfigCoreError::notification("test transport failure"))
    }
    async fn subscribe(&self) -> aster_forge_config::Result<ConfigNotification> {
        Err(ConfigCoreError::notification("unused subscription"))
    }
}

#[tokio::test]
async fn notification_failure_cannot_roll_back_an_already_committed_write() {
    let db = database().await;
    let runtime = seeded_runtime(&db).await;
    assert!(matches!(
        save(
            &db,
            &runtime,
            &FailingNotifier,
            "ttl",
            &ConfigValue::from("60"),
            Failure::None
        )
        .await,
        Err(WriteError::Core(ConfigCoreError::Notification(_)))
    ));
    assert_eq!(
        BINDING
            .find_by_key(&db, "ttl")
            .await
            .unwrap()
            .unwrap()
            .value,
        "secret:v1:fixture:60"
    );
    assert_eq!(runtime.get("ttl").as_deref(), Some("secret:v1:fixture:60"));
    assert_eq!(audit_values(&db).await, [ConfigValue::REDACTED]);
}

#[tokio::test]
async fn binding_and_free_function_support_rollback_for_inserts_and_updates() {
    for binding in [true, false] {
        for existing in [true, false] {
            let db = database().await;
            if existing {
                seeded_runtime(&db).await;
            }
            let before = BINDING.find_all(&db).await.unwrap();
            let txn = transaction::begin(&db).await.unwrap();
            let request = SystemConfigUpsert {
                key: "ttl",
                value: "secret:v1:opaque",
                visibility: None,
                updated_by: None,
            };
            let row = if binding {
                BINDING.upsert_prevalidated(&txn, request).await.unwrap()
            } else {
                system_config::upsert_prevalidated(&txn, &REGISTRY, request)
                    .await
                    .unwrap()
            };
            assert_eq!(
                BINDING.find_by_key(&txn, "ttl").await.unwrap().unwrap(),
                row
            );
            transaction::rollback(txn).await.unwrap();
            assert_eq!(BINDING.find_all(&db).await.unwrap(), before);
        }
    }
}

#[cfg(feature = "database-container-tests")]
async fn real_backend_write_contract(db: &DatabaseConnection) {
    db.execute(&system_config::create_system_config_table(
        db.get_database_backend(),
    ))
    .await
    .unwrap();
    db.execute(&system_config::create_system_config_key_unique_index())
        .await
        .unwrap();
    let lookup = |key: &str| (key == "enabled").then(|| "true".to_string());
    for writer in [Writer::Binding, Writer::Store, Writer::FreeFunction] {
        for input in ["1", "060"] {
            let normalized = REGISTRY.normalize_value(&lookup, "ttl", input).unwrap();
            let envelope = format!("secret:v1:fixture:{normalized}");
            let row = write(
                writer,
                db,
                SystemConfigUpsert {
                    key: "ttl",
                    value: &envelope,
                    visibility: None,
                    updated_by: Some(42),
                },
            )
            .await;
            assert_eq!(row.value, envelope);
            assert!(row.is_sensitive);
        }
    }
    let before = BINDING.find_all(db).await.unwrap();
    for bad in ["not-a-number", "0", "61", "NaN"] {
        assert!(REGISTRY.normalize_value(&lookup, "ttl", bad).is_err());
        assert_eq!(BINDING.find_all(db).await.unwrap(), before);
    }
    for binding in [true, false] {
        let txn = transaction::begin(db).await.unwrap();
        for key in ["ttl", "restart_ttl"] {
            let request = SystemConfigUpsert {
                key,
                value: "secret:v1:rollback",
                visibility: None,
                updated_by: None,
            };
            if binding {
                BINDING.upsert_prevalidated(&txn, request).await.unwrap();
            } else {
                system_config::upsert_prevalidated(&txn, &REGISTRY, request)
                    .await
                    .unwrap();
            }
        }
        transaction::rollback(txn).await.unwrap();
        assert_eq!(BINDING.find_all(db).await.unwrap(), before);
    }
}

#[cfg(feature = "database-container-tests")]
#[tokio::test]
async fn postgres_preserves_encoded_writes_and_rolls_back_insert_and_update() {
    use aster_forge_test::{postgres::PostgresTestContainer, suite::TestContainerSuite};
    let suite = TestContainerSuite::new("asterforge-db-credentials");
    let container = PostgresTestContainer::start(&suite).await;
    let fixture = container
        .create_database(&format!("config_{}", uuid::Uuid::new_v4().simple()))
        .await;
    let db = fixture.connect().await;
    real_backend_write_contract(&db).await;
    db.close().await.unwrap();
    fixture.cleanup().await;
}

#[cfg(feature = "database-container-tests")]
#[tokio::test]
async fn mysql_preserves_encoded_writes_and_rolls_back_insert_and_update() {
    use aster_forge_test::{mysql::MysqlTestContainer, suite::TestContainerSuite};
    let suite = TestContainerSuite::new("asterforge-db-credentials");
    let container = MysqlTestContainer::start(&suite).await;
    let name = format!("config_{}", uuid::Uuid::new_v4().simple());
    container.create_shared_database(&name).await;
    let db = Database::connect(container.database_url(&name))
        .await
        .unwrap();
    real_backend_write_contract(&db).await;
    db.close().await.unwrap();
    container.drop_shared_database(&name).await;
}
