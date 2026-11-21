//! Dedupe keys for idempotent task enqueueing.
//!
//! Runtime leases prevent multiple instances from normally running the same
//! scheduler, and task processing leases prevent duplicate execution of one
//! persisted row. Dedupe keys cover the remaining boundary: enqueueing the same
//! logical task more than once during leader handoff, retries, or split-brain
//! windows. Product repositories should persist this key in a nullable unique
//! column and return the existing row when a duplicate insert races.

use time::OffsetDateTime;

use crate::{Result, TaskCoreError};

/// Maximum length for persisted task dedupe keys.
pub const TASK_DEDUPE_KEY_MAX_LEN: usize = 191;

/// Validated task dedupe key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskDedupeKey(String);

impl TaskDedupeKey {
    /// Validates a product-provided dedupe key.
    ///
    /// # Errors
    ///
    /// Returns [`TaskCoreError`] when the key is empty or exceeds the persisted length limit.
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
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the key and returns the owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Builds a stable dedupe key for one scheduled task firing.
///
/// # Errors
///
/// Returns [`TaskCoreError`] when the namespace is empty or the resulting key is too long.
pub fn scheduled_task_dedupe_key(
    namespace: &str,
    task_name: &str,
    scheduled_at: OffsetDateTime,
) -> Result<TaskDedupeKey> {
    TaskDedupeKey::new(format!(
        "schedule:{namespace}:{task_name}:{}",
        scheduled_at
            .to_offset(time::UtcOffset::UTC)
            .format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .map_err(|_| TaskCoreError::invalid_value(
                "scheduled task timestamp is not representable"
            ))?
    ))
}

#[cfg(test)]
mod tests {

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
            time::macros::datetime!(2026-06-26 01:02:03 UTC),
        )
        .unwrap();

        assert_eq!(
            key.as_str(),
            "schedule:aster_yggdrasil:task-cleanup:2026-06-26T01:02:03Z"
        );
    }

    #[test]
    fn schedule_dedupe_normalizes_offsets_and_discards_subseconds() {
        let utc = time::macros::datetime!(2026-06-26 01:02:03.999999999 UTC);
        let offset = utc.to_offset(time::macros::offset!(+08:00));
        assert_eq!(
            scheduled_task_dedupe_key("app", "job", utc).unwrap(),
            scheduled_task_dedupe_key("app", "job", offset).unwrap()
        );
        assert_eq!(
            scheduled_task_dedupe_key("app", "job", utc)
                .unwrap()
                .as_str(),
            "schedule:app:job:2026-06-26T01:02:03Z"
        );
    }
}
