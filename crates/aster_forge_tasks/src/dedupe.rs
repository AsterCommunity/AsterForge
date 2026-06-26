//! Dedupe keys for idempotent task enqueueing.
//!
//! Runtime leases prevent multiple instances from normally running the same
//! scheduler, and task processing leases prevent duplicate execution of one
//! persisted row. Dedupe keys cover the remaining boundary: enqueueing the same
//! logical task more than once during leader handoff, retries, or split-brain
//! windows. Product repositories should persist this key in a nullable unique
//! column and return the existing row when a duplicate insert races.

use chrono::{DateTime, SecondsFormat, Utc};

use crate::{Result, TaskCoreError};

/// Maximum length for persisted task dedupe keys.
pub const TASK_DEDUPE_KEY_MAX_LEN: usize = 191;

/// Validated task dedupe key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskDedupeKey(String);

impl TaskDedupeKey {
    /// Validates a product-provided dedupe key.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TaskCoreError::invalid_value(
                "task dedupe key must not be empty",
            ));
        }
        if value.len() > TASK_DEDUPE_KEY_MAX_LEN {
            return Err(TaskCoreError::invalid_value(format!(
                "task dedupe key must be at most {TASK_DEDUPE_KEY_MAX_LEN} bytes"
            )));
        }
        Ok(Self(value))
    }

    /// Returns the validated key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the key and returns the owned string.
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Builds a stable dedupe key for one scheduled task firing.
pub fn scheduled_task_dedupe_key(
    namespace: &str,
    task_name: &str,
    scheduled_at: DateTime<Utc>,
) -> Result<TaskDedupeKey> {
    TaskDedupeKey::new(format!(
        "schedule:{namespace}:{task_name}:{}",
        scheduled_at.to_rfc3339_opts(SecondsFormat::Secs, true)
    ))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{TASK_DEDUPE_KEY_MAX_LEN, TaskDedupeKey, scheduled_task_dedupe_key};

    #[test]
    fn task_dedupe_key_rejects_empty_values() {
        assert!(TaskDedupeKey::new("   ").is_err());
    }

    #[test]
    fn task_dedupe_key_rejects_values_over_storage_limit() {
        assert!(TaskDedupeKey::new("x".repeat(TASK_DEDUPE_KEY_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn scheduled_task_dedupe_key_is_stable_and_compact() {
        let key = scheduled_task_dedupe_key(
            "aster_yggdrasil",
            "task-cleanup",
            Utc.with_ymd_and_hms(2026, 6, 26, 1, 2, 3).unwrap(),
        )
        .unwrap();

        assert_eq!(
            key.as_str(),
            "schedule:aster_yggdrasil:task-cleanup:2026-06-26T01:02:03Z"
        );
    }
}
