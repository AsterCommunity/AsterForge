//! Shared task retry classification.

/// Retry behavior selected after a task failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRetryClass {
    /// The dispatcher may automatically retry while the retry budget remains.
    Auto,
    /// The task may be retried manually, but not automatically.
    Manual,
    /// The failure is permanent and must not be retried.
    Never,
}

impl TaskRetryClass {
    /// Returns whether the class permits automatic retry.
    pub const fn should_auto_retry(self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Returns whether the class permits manual retry.
    pub const fn can_manual_retry(self) -> bool {
        matches!(self, Self::Auto | Self::Manual)
    }
}

/// Default retry delay used by Aster task dispatchers.
///
/// The delay is intentionally short for the first two attempts and then backs off to a stable
/// five-minute retry interval. Product crates can pass a custom delay function into the execution
/// runner when a task subsystem needs a different retry cadence.
pub const fn default_task_retry_delay_secs(attempt_count: i32) -> i64 {
    match attempt_count {
        1 => 5,
        2 => 15,
        3 => 60,
        _ => 300,
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskRetryClass, default_task_retry_delay_secs};

    #[test]
    fn retry_class_helpers_match_retry_capabilities() {
        assert!(TaskRetryClass::Auto.should_auto_retry());
        assert!(TaskRetryClass::Auto.can_manual_retry());

        assert!(!TaskRetryClass::Manual.should_auto_retry());
        assert!(TaskRetryClass::Manual.can_manual_retry());

        assert!(!TaskRetryClass::Never.should_auto_retry());
        assert!(!TaskRetryClass::Never.can_manual_retry());
    }

    #[test]
    fn default_task_retry_delay_matches_existing_dispatcher_cadence() {
        assert_eq!(default_task_retry_delay_secs(1), 5);
        assert_eq!(default_task_retry_delay_secs(2), 15);
        assert_eq!(default_task_retry_delay_secs(3), 60);
        assert_eq!(default_task_retry_delay_secs(4), 300);
        assert_eq!(default_task_retry_delay_secs(99), 300);
    }
}
