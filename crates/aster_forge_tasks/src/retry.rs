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

#[cfg(test)]
mod tests {
    use super::TaskRetryClass;

    #[test]
    fn retry_class_helpers_match_retry_capabilities() {
        assert!(TaskRetryClass::Auto.should_auto_retry());
        assert!(TaskRetryClass::Auto.can_manual_retry());

        assert!(!TaskRetryClass::Manual.should_auto_retry());
        assert!(TaskRetryClass::Manual.can_manual_retry());

        assert!(!TaskRetryClass::Never.should_auto_retry());
        assert!(!TaskRetryClass::Never.can_manual_retry());
    }
}
