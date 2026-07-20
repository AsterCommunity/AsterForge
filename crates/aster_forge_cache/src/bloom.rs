//! Concurrent Bloom-filter primitives for cache-aside existence checks.

use std::sync::Arc;

use bloomfilter::Bloom;
use parking_lot::{Mutex, RwLock};

/// Configuration used to size a [`BloomFilter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomConfig {
    /// Expected number of stored keys before reserve capacity is applied.
    pub expected_items: usize,
    /// Desired false-positive probability in the range `(0, 1)`.
    pub false_positive_rate: f64,
}

impl BloomConfig {
    /// Creates a Bloom-filter configuration.
    pub const fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        Self {
            expected_items,
            false_positive_rate,
        }
    }
}

/// Bloom-filter construction or rebuild failure.
#[derive(Debug, thiserror::Error)]
pub enum BloomError {
    /// The underlying Bloom filter rejected its capacity or probability.
    #[error("invalid Bloom filter configuration: {0}")]
    InvalidConfiguration(String),
    /// A second rebuild was started before the active rebuild completed.
    #[error("Bloom filter rebuild already in progress")]
    RebuildInProgress,
}

/// Concurrent Bloom filter with atomic rebuild support.
///
/// Inserts made while a rebuild is active are recorded and applied to the new
/// filter immediately before it replaces the old filter.
pub struct BloomFilter {
    inner: RwLock<Bloom<str>>,
    rebuild_buffer: Mutex<Option<Vec<String>>>,
}

impl BloomFilter {
    /// Creates a Bloom filter from the expected item count and false-positive rate.
    pub fn new(config: BloomConfig) -> Result<Self, BloomError> {
        Ok(Self {
            inner: RwLock::new(build_filter(config)?),
            rebuild_buffer: Mutex::new(None),
        })
    }

    /// Returns whether the key may be present.
    pub fn contains(&self, key: &str) -> bool {
        self.inner.read().check(key)
    }

    /// Inserts a key, including it in any active rebuild.
    pub fn insert(&self, key: &str) {
        let mut rebuild_buffer = self.rebuild_buffer.lock();
        self.inner.write().set(key);
        if let Some(pending) = rebuild_buffer.as_mut() {
            pending.push(key.to_string());
        }
    }

    /// Inserts several keys into the current filter.
    pub fn insert_many<'a>(&self, keys: impl IntoIterator<Item = &'a str>) {
        let mut rebuild_buffer = self.rebuild_buffer.lock();
        let mut filter = self.inner.write();
        for key in keys {
            filter.set(key);
            if let Some(pending) = rebuild_buffer.as_mut() {
                pending.push(key.to_string());
            }
        }
    }

    /// Replaces the current filter with an empty filter using new sizing.
    pub fn clear(&self, config: BloomConfig) -> Result<(), BloomError> {
        let replacement = build_filter(config)?;
        let rebuild_buffer = self.rebuild_buffer.lock();
        if rebuild_buffer.is_some() {
            return Err(BloomError::RebuildInProgress);
        }
        *self.inner.write() = replacement;
        Ok(())
    }

    /// Starts an atomic rebuild session.
    ///
    /// Feed streamed batches into the returned session and call
    /// [`BloomRebuild::commit`] after the source has completed. Dropping the
    /// session leaves the previous filter active and stops buffering inserts.
    pub fn start_rebuild(
        self: &Arc<Self>,
        config: BloomConfig,
    ) -> Result<BloomRebuild, BloomError> {
        let replacement = build_filter(config)?;
        let mut rebuild_buffer = self.rebuild_buffer.lock();
        if rebuild_buffer.is_some() {
            return Err(BloomError::RebuildInProgress);
        }
        *rebuild_buffer = Some(Vec::new());
        Ok(BloomRebuild {
            owner: Arc::clone(self),
            replacement: Some(replacement),
            loaded: 0,
        })
    }
}

/// In-progress atomic rebuild created by [`BloomFilter::start_rebuild`].
pub struct BloomRebuild {
    owner: Arc<BloomFilter>,
    replacement: Option<Bloom<str>>,
    loaded: usize,
}

impl BloomRebuild {
    /// Adds a streamed batch to the replacement filter.
    pub fn insert_many<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) {
        let Some(replacement) = self.replacement.as_mut() else {
            return;
        };
        for key in keys {
            replacement.set(key);
            self.loaded = self.loaded.saturating_add(1);
        }
    }

    /// Atomically installs the rebuilt filter and returns the number of keys
    /// loaded from the source plus concurrent inserts captured during rebuild.
    pub fn commit(mut self) -> usize {
        let Some(mut replacement) = self.replacement.take() else {
            return self.loaded;
        };
        let mut rebuild_buffer = self.owner.rebuild_buffer.lock();
        let buffered = rebuild_buffer.take().unwrap_or_default();
        for key in &buffered {
            replacement.set(key);
        }
        *self.owner.inner.write() = replacement;
        self.loaded.saturating_add(buffered.len())
    }
}

impl Drop for BloomRebuild {
    fn drop(&mut self) {
        if self.replacement.is_some() {
            self.owner.rebuild_buffer.lock().take();
        }
    }
}

fn build_filter(config: BloomConfig) -> Result<Bloom<str>, BloomError> {
    Bloom::new_for_fp_rate(
        reserved_capacity(config.expected_items),
        config.false_positive_rate,
    )
    .map_err(|error| BloomError::InvalidConfiguration(error.to_string()))
}

fn reserved_capacity(count: usize) -> usize {
    let reserve = if count < 5_000 {
        count / 2
    } else if count < 100_000 {
        count / 5
    } else {
        (count / 10).min(1_000_000)
    };
    count.saturating_add(reserve.max(1_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(expected_items: usize) -> BloomConfig {
        BloomConfig::new(expected_items, 0.001)
    }

    #[test]
    fn insert_and_clear_update_membership() {
        let filter = BloomFilter::new(config(100)).expect("valid Bloom config");
        assert!(!filter.contains("missing"));

        filter.insert("present");
        assert!(filter.contains("present"));

        filter.clear(config(1_000)).expect("valid Bloom config");
        assert!(!filter.contains("present"));
    }

    #[test]
    fn rebuild_replaces_old_keys_and_keeps_concurrent_inserts() {
        let filter = Arc::new(BloomFilter::new(config(100)).expect("valid Bloom config"));
        filter.insert("old");

        let mut rebuild = filter
            .start_rebuild(config(2))
            .expect("first rebuild should start");
        rebuild.insert_many(["new-a", "new-b"]);
        filter.insert("concurrent");

        assert_eq!(rebuild.commit(), 3);
        assert!(!filter.contains("old"));
        assert!(filter.contains("new-a"));
        assert!(filter.contains("new-b"));
        assert!(filter.contains("concurrent"));
    }

    #[test]
    fn dropped_rebuild_keeps_old_filter_and_releases_session() {
        let filter = Arc::new(BloomFilter::new(config(100)).expect("valid Bloom config"));
        filter.insert("old");

        let rebuild = filter
            .start_rebuild(config(1))
            .expect("first rebuild should start");
        filter.insert("concurrent");
        drop(rebuild);

        assert!(filter.contains("old"));
        assert!(filter.contains("concurrent"));
        assert!(filter.start_rebuild(config(1)).is_ok());
    }

    #[test]
    fn overlapping_rebuilds_are_rejected() {
        let filter = Arc::new(BloomFilter::new(config(100)).expect("valid Bloom config"));
        let _active = filter
            .start_rebuild(config(1))
            .expect("first rebuild should start");

        assert!(matches!(
            filter.start_rebuild(config(1)),
            Err(BloomError::RebuildInProgress)
        ));
    }

    #[test]
    fn clear_during_rebuild_is_rejected_without_discarding_the_session() {
        let filter = Arc::new(BloomFilter::new(config(100)).expect("valid Bloom config"));
        let mut rebuild = filter
            .start_rebuild(config(1))
            .expect("first rebuild should start");
        rebuild.insert_many(["rebuilt"]);

        assert!(matches!(
            filter.clear(config(1_000)),
            Err(BloomError::RebuildInProgress)
        ));
        assert_eq!(rebuild.commit(), 1);
        assert!(filter.contains("rebuilt"));
    }

    #[test]
    fn bulk_insert_preserves_all_keys() {
        let filter = BloomFilter::new(config(100)).expect("valid Bloom config");
        let keys: Vec<String> = (0..100).map(|index| format!("key-{index}")).collect();
        filter.insert_many(keys.iter().map(String::as_str));

        assert!(keys.iter().all(|key| filter.contains(key)));
    }
}
