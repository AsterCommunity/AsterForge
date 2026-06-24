//! In-process runtime configuration snapshots.
//!
//! Runtime configuration is read often and updated rarely. This module keeps a
//! cloneable snapshot behind a lock, applies single-key changes, computes reload
//! diffs, and delegates persistence loading to a store trait implemented by
//! product crates.

use std::collections::{BTreeSet, HashMap};
use std::sync::{
    RwLock as StdRwLock, RwLockReadGuard as StdRwLockReadGuard,
    RwLockWriteGuard as StdRwLockWriteGuard,
};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{ConfigSource, ConfigValueLookup, ConfigValueType, ConfigVisibility, Result};

/// Stored representation of a configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConfig {
    /// Database identifier owned by the product storage layer.
    pub id: i64,
    /// Stable storage key.
    pub key: String,
    /// Storage string.
    pub value: String,
    /// Storage and API value kind.
    pub value_type: ConfigValueType,
    /// Whether a running process should ignore hot updates after first load.
    pub requires_restart: bool,
    /// Whether the value must be redacted in API and audit output.
    pub is_sensitive: bool,
    /// Source of this value.
    pub source: ConfigSource,
    /// Consumer visibility.
    pub visibility: ConfigVisibility,
    /// Product-defined category.
    pub category: String,
    /// Backend-facing description.
    pub description: String,
}

/// Record type that can be stored in a runtime configuration snapshot.
///
/// Product crates can implement this trait for their database entity model when
/// they need the runtime cache to preserve product-only columns such as audit
/// metadata, timestamps, namespaces, or SeaORM enum wrappers. Forge only needs
/// a stable key, a storage string, and the restart boundary to provide common
/// snapshot behavior.
pub trait RuntimeConfigRecord: Clone + PartialEq {
    /// Returns the stable configuration key.
    fn config_key(&self) -> &str;

    /// Returns the storage string for this configuration row.
    fn config_value(&self) -> &str;

    /// Returns whether hot updates should be ignored after first load.
    fn config_requires_restart(&self) -> bool;
}

impl RuntimeConfigRecord for StoredConfig {
    fn config_key(&self) -> &str {
        &self.key
    }

    fn config_value(&self) -> &str {
        &self.value
    }

    fn config_requires_restart(&self) -> bool {
        self.requires_restart
    }
}

/// Trait implemented by product storage adapters that can load config rows for
/// [`AsyncRuntimeConfig`].
#[async_trait]
pub trait AsyncConfigStore: Send + Sync {
    /// Loads every configuration row visible to this process.
    async fn load_all(&self) -> Result<Vec<StoredConfig>>;
}

/// Immutable generic snapshot used by synchronous runtime caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfigSnapshot<T = StoredConfig> {
    values: HashMap<String, T>,
}

impl<T> Default for SyncConfigSnapshot<T> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
        }
    }
}

impl<T> SyncConfigSnapshot<T>
where
    T: RuntimeConfigRecord,
{
    /// Creates a snapshot from stored rows, keyed by config key.
    pub fn from_configs(configs: Vec<T>) -> Self {
        Self {
            values: configs
                .into_iter()
                .map(|config| (config.config_key().to_string(), config))
                .collect(),
        }
    }

    /// Returns the stored model for `key`.
    pub fn get_model(&self, key: &str) -> Option<&T> {
        self.values.get(key)
    }

    /// Returns the storage string for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.get_model(key).map(RuntimeConfigRecord::config_value)
    }

    /// Parses a bool-like storage string for `key`.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        let value = self.get(key)?;
        parse_bool(value)
    }

    /// Parses an i64 storage string for `key`.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Parses a u64 storage string for `key`.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Returns a string value or `default`.
    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.get(key)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default.to_string())
    }

    /// Returns a bool value or `default`.
    pub fn get_bool_or(&self, key: &str, default: bool) -> bool {
        self.get_bool(key).unwrap_or(default)
    }

    /// Returns an i64 value or `default`.
    pub fn get_i64_or(&self, key: &str, default: i64) -> i64 {
        self.get_i64(key).unwrap_or(default)
    }

    /// Returns a u64 value or `default`.
    pub fn get_u64_or(&self, key: &str, default: u64) -> u64 {
        self.get_u64(key).unwrap_or(default)
    }

    /// Returns all values.
    pub fn values(&self) -> &HashMap<String, T> {
        &self.values
    }
}

impl<T> ConfigValueLookup for SyncConfigSnapshot<T>
where
    T: RuntimeConfigRecord,
{
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).map(ToOwned::to_owned)
    }
}

/// Immutable snapshot exposed by [`AsyncRuntimeConfig`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsyncConfigSnapshot {
    values: HashMap<String, StoredConfig>,
}

impl AsyncConfigSnapshot {
    /// Creates a snapshot from stored rows, keyed by config key.
    pub fn from_configs(configs: Vec<StoredConfig>) -> Self {
        Self {
            values: configs
                .into_iter()
                .map(|config| (config.key.clone(), config))
                .collect(),
        }
    }

    /// Returns the stored model for `key`.
    pub fn get_model(&self, key: &str) -> Option<&StoredConfig> {
        self.values.get(key)
    }

    /// Returns the storage string for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.get_model(key).map(|config| config.value.as_str())
    }

    /// Parses a bool-like storage string for `key`.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        let value = self.get(key)?;
        parse_bool(value)
    }

    /// Parses an i64 storage string for `key`.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Parses a u64 storage string for `key`.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Returns a string value or `default`.
    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.get(key)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default.to_string())
    }

    /// Returns a bool value or `default`.
    pub fn get_bool_or(&self, key: &str, default: bool) -> bool {
        self.get_bool(key).unwrap_or(default)
    }

    /// Returns an i64 value or `default`.
    pub fn get_i64_or(&self, key: &str, default: i64) -> i64 {
        self.get_i64(key).unwrap_or(default)
    }

    /// Returns a u64 value or `default`.
    pub fn get_u64_or(&self, key: &str, default: u64) -> u64 {
        self.get_u64(key).unwrap_or(default)
    }

    /// Returns all values.
    pub fn values(&self) -> &HashMap<String, StoredConfig> {
        &self.values
    }
}

impl ConfigValueLookup for AsyncConfigSnapshot {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).map(ToOwned::to_owned)
    }
}

/// Description of one change applied to a runtime snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeConfigChange<T = StoredConfig> {
    /// Key was inserted or changed.
    Upserted(T),
    /// Key was removed.
    Removed(String),
}

/// Synchronous runtime configuration cache.
///
/// This type is intended for hot read paths where configuration is loaded from
/// storage asynchronously at the boundary, but request handlers, middleware,
/// policy builders, and task registries need cheap synchronous reads from an
/// in-memory snapshot.
#[derive(Debug, Default)]
pub struct SyncRuntimeConfig<T = StoredConfig> {
    snapshot: StdRwLock<SyncConfigSnapshot<T>>,
}

impl<T> SyncRuntimeConfig<T>
where
    T: RuntimeConfigRecord,
{
    /// Creates an empty synchronous runtime cache.
    pub fn new() -> Self {
        Self {
            snapshot: StdRwLock::new(SyncConfigSnapshot::default()),
        }
    }

    /// Replaces the snapshot from a full record list and returns the diff.
    pub fn replace(&self, configs: Vec<T>) -> Vec<RuntimeConfigChange<T>> {
        let next = SyncConfigSnapshot::from_configs(configs);
        let mut guard = self.write_snapshot();
        let changes = diff_sync_snapshots(&guard, &next);
        *guard = next;
        changes
    }

    /// Returns a cloned snapshot for lock-free derived-state processing.
    pub fn snapshot(&self) -> SyncConfigSnapshot<T> {
        self.read_snapshot().clone()
    }

    /// Returns the stored model for `key`.
    pub fn get_model(&self, key: &str) -> Option<T> {
        self.read_snapshot().get_model(key).cloned()
    }

    /// Returns the storage string for `key`.
    pub fn get(&self, key: &str) -> Option<String> {
        self.read_snapshot().get(key).map(ToOwned::to_owned)
    }

    /// Parses a bool-like storage string for `key`.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.read_snapshot().get_bool(key)
    }

    /// Parses an i64 storage string for `key`.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.read_snapshot().get_i64(key)
    }

    /// Parses a u64 storage string for `key`.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.read_snapshot().get_u64(key)
    }

    /// Returns a string value or `default`.
    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.read_snapshot().get_string_or(key, default)
    }

    /// Returns a bool value or `default`.
    pub fn get_bool_or(&self, key: &str, default: bool) -> bool {
        self.read_snapshot().get_bool_or(key, default)
    }

    /// Returns an i64 value or `default`.
    pub fn get_i64_or(&self, key: &str, default: i64) -> i64 {
        self.read_snapshot().get_i64_or(key, default)
    }

    /// Returns a u64 value or `default`.
    pub fn get_u64_or(&self, key: &str, default: u64) -> u64 {
        self.read_snapshot().get_u64_or(key, default)
    }

    /// Applies one row to the snapshot.
    ///
    /// If the incoming row requires restart and the key already exists, the
    /// update is ignored to preserve the in-process value until restart.
    pub fn apply(&self, config: T) -> Option<RuntimeConfigChange<T>> {
        let mut guard = self.write_snapshot();
        let key = config.config_key().to_string();
        if config.config_requires_restart() && guard.values.contains_key(&key) {
            return None;
        }

        let changed = guard.values.get(&key) != Some(&config);
        guard.values.insert(key, config.clone());
        changed.then_some(RuntimeConfigChange::Upserted(config))
    }

    /// Removes one key from the snapshot.
    pub fn remove(&self, key: &str) -> Option<RuntimeConfigChange<T>> {
        let mut guard = self.write_snapshot();
        guard
            .values
            .remove(key)
            .map(|_| RuntimeConfigChange::Removed(key.to_string()))
    }

    fn read_snapshot(&self) -> StdRwLockReadGuard<'_, SyncConfigSnapshot<T>> {
        match self.snapshot.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_snapshot(&self) -> StdRwLockWriteGuard<'_, SyncConfigSnapshot<T>> {
        match self.snapshot.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Async runtime configuration cache.
///
/// This type uses `tokio::sync::RwLock` and is intended for async-first
/// services that want to load and query runtime configuration through async
/// boundaries. Services with synchronous hot read paths should use
/// [`SyncRuntimeConfig`] instead.
#[derive(Debug, Default)]
pub struct AsyncRuntimeConfig {
    snapshot: RwLock<AsyncConfigSnapshot>,
}

impl AsyncRuntimeConfig {
    /// Creates an empty async runtime cache.
    pub fn new() -> Self {
        Self {
            snapshot: RwLock::new(AsyncConfigSnapshot::default()),
        }
    }

    /// Reloads all values from `store` and returns the diff.
    pub async fn reload<S>(&self, store: &S) -> Result<Vec<RuntimeConfigChange>>
    where
        S: AsyncConfigStore + ?Sized,
    {
        let next = AsyncConfigSnapshot::from_configs(store.load_all().await?);
        let mut guard = self.snapshot.write().await;
        let changes = diff_snapshots(&guard, &next);
        *guard = next;
        Ok(changes)
    }

    /// Returns a cloned snapshot for lock-free derived-state processing.
    pub async fn snapshot(&self) -> AsyncConfigSnapshot {
        self.snapshot.read().await.clone()
    }

    /// Returns the stored model for `key`.
    pub async fn get_model(&self, key: &str) -> Option<StoredConfig> {
        self.snapshot.read().await.get_model(key).cloned()
    }

    /// Returns the storage string for `key`.
    pub async fn get(&self, key: &str) -> Option<String> {
        self.snapshot.read().await.get(key).map(ToOwned::to_owned)
    }

    /// Applies one row to the snapshot.
    ///
    /// If the incoming row requires restart and the key already exists, the
    /// update is ignored to preserve the in-process value until restart.
    pub async fn apply(&self, config: StoredConfig) -> Option<RuntimeConfigChange> {
        let mut guard = self.snapshot.write().await;
        if config.requires_restart && guard.values.contains_key(&config.key) {
            return None;
        }

        let changed = guard.values.get(&config.key) != Some(&config);
        guard.values.insert(config.key.clone(), config.clone());
        changed.then_some(RuntimeConfigChange::Upserted(config))
    }

    /// Removes one key from the snapshot.
    pub async fn remove(&self, key: &str) -> Option<RuntimeConfigChange> {
        let mut guard = self.snapshot.write().await;
        guard
            .values
            .remove(key)
            .map(|_| RuntimeConfigChange::Removed(key.to_string()))
    }
}

fn diff_snapshots(
    previous: &AsyncConfigSnapshot,
    next: &AsyncConfigSnapshot,
) -> Vec<RuntimeConfigChange> {
    let mut keys = BTreeSet::new();
    keys.extend(previous.values.keys().map(String::as_str));
    keys.extend(next.values.keys().map(String::as_str));

    let mut changes = Vec::new();
    for key in keys {
        match (previous.values.get(key), next.values.get(key)) {
            (Some(old), Some(new)) if old == new => {}
            (_, Some(new)) => changes.push(RuntimeConfigChange::Upserted(new.clone())),
            (Some(_), None) => changes.push(RuntimeConfigChange::Removed(key.to_string())),
            (None, None) => {}
        }
    }
    changes
}

fn diff_sync_snapshots<T>(
    previous: &SyncConfigSnapshot<T>,
    next: &SyncConfigSnapshot<T>,
) -> Vec<RuntimeConfigChange<T>>
where
    T: RuntimeConfigRecord,
{
    let mut keys = BTreeSet::new();
    keys.extend(previous.values.keys().map(String::as_str));
    keys.extend(next.values.keys().map(String::as_str));

    let mut changes = Vec::new();
    for key in keys {
        match (previous.values.get(key), next.values.get(key)) {
            (Some(old), Some(new)) if old == new => {}
            (_, Some(new)) => changes.push(RuntimeConfigChange::Upserted(new.clone())),
            (Some(_), None) => changes.push(RuntimeConfigChange::Removed(key.to_string())),
            (None, None) => {}
        }
    }
    changes
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::{
        AsyncConfigStore, AsyncRuntimeConfig, RuntimeConfigChange, StoredConfig, SyncRuntimeConfig,
    };
    use crate::{ConfigSource, ConfigValueType, ConfigVisibility, Result};

    fn config(key: &str, value: &str, requires_restart: bool) -> StoredConfig {
        StoredConfig {
            id: 1,
            key: key.to_string(),
            value: value.to_string(),
            value_type: ConfigValueType::String,
            requires_restart,
            is_sensitive: false,
            source: ConfigSource::System,
            visibility: ConfigVisibility::Private,
            category: "general".to_string(),
            description: "test config".to_string(),
        }
    }

    struct StaticStore(Vec<StoredConfig>);

    #[async_trait]
    impl AsyncConfigStore for StaticStore {
        async fn load_all(&self) -> Result<Vec<StoredConfig>> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn reload_replaces_snapshot_and_reports_changes() {
        let runtime_config = AsyncRuntimeConfig::new();

        let changes = runtime_config
            .reload(&StaticStore(vec![config("enabled", "yes", false)]))
            .await
            .unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(
            runtime_config.snapshot().await.get_bool("enabled"),
            Some(true)
        );

        let changes = runtime_config
            .reload(&StaticStore(vec![config("limit", "10", false)]))
            .await
            .unwrap();

        assert_eq!(
            changes,
            vec![
                RuntimeConfigChange::Removed("enabled".to_string()),
                RuntimeConfigChange::Upserted(config("limit", "10", false)),
            ]
        );
        assert_eq!(runtime_config.snapshot().await.get_u64("limit"), Some(10));
    }

    #[tokio::test]
    async fn apply_ignores_hot_update_for_restart_required_existing_value() {
        let runtime_config = AsyncRuntimeConfig::new();
        runtime_config
            .apply(config("static_key", "old", false))
            .await;

        let change = runtime_config
            .apply(config("static_key", "new", true))
            .await;

        assert_eq!(change, None);
        assert_eq!(
            runtime_config.get("static_key").await.as_deref(),
            Some("old")
        );
    }

    #[test]
    fn sync_runtime_config_supports_hot_reads_and_diffs() {
        let runtime_config = SyncRuntimeConfig::new();

        let changes = runtime_config.replace(vec![config("enabled", "yes", false)]);

        assert_eq!(changes.len(), 1);
        assert_eq!(runtime_config.get_bool("enabled"), Some(true));

        let changes = runtime_config.replace(vec![config("limit", "10", false)]);

        assert_eq!(
            changes,
            vec![
                RuntimeConfigChange::Removed("enabled".to_string()),
                RuntimeConfigChange::Upserted(config("limit", "10", false)),
            ]
        );
        assert_eq!(runtime_config.get_u64("limit"), Some(10));
        assert_eq!(runtime_config.snapshot().get("limit"), Some("10"));
    }

    #[test]
    fn sync_runtime_config_ignores_restart_required_hot_update() {
        let runtime_config = SyncRuntimeConfig::new();
        runtime_config.apply(config("static_key", "old", false));

        let change = runtime_config.apply(config("static_key", "new", true));

        assert_eq!(change, None);
        assert_eq!(runtime_config.get("static_key").as_deref(), Some("old"));
    }
}
