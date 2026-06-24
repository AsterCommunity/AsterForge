//! Shared dispatcher result counters.

/// Aggregate counters returned by a background task dispatch pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DispatchStats {
    /// Number of tasks claimed for execution.
    pub claimed: usize,
    /// Number of tasks completed successfully.
    pub succeeded: usize,
    /// Number of tasks scheduled for retry.
    pub retried: usize,
    /// Number of tasks permanently failed.
    pub failed: usize,
}

impl DispatchStats {
    /// Adds another dispatch counter set into this one.
    pub fn add(&mut self, other: Self) {
        self.claimed += other.claimed;
        self.succeeded += other.succeeded;
        self.retried += other.retried;
        self.failed += other.failed;
    }

    /// Returns whether any dispatch activity happened.
    pub const fn has_activity(&self) -> bool {
        self.claimed > 0 || self.succeeded > 0 || self.retried > 0 || self.failed > 0
    }

    /// Adds a task execution outcome to the aggregate counters.
    pub fn add_outcome(&mut self, outcome: TaskDispatchOutcome) {
        self.succeeded += outcome.succeeded;
        self.retried += outcome.retried;
        self.failed += outcome.failed;
    }
}

/// Counters returned by one claimed task execution.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TaskDispatchOutcome {
    /// Number of tasks completed successfully.
    pub succeeded: usize,
    /// Number of tasks scheduled for retry.
    pub retried: usize,
    /// Number of tasks permanently failed.
    pub failed: usize,
}

impl TaskDispatchOutcome {
    /// Creates a successful task outcome.
    pub const fn succeeded() -> Self {
        Self {
            succeeded: 1,
            retried: 0,
            failed: 0,
        }
    }

    /// Creates a retried task outcome.
    pub const fn retried() -> Self {
        Self {
            succeeded: 0,
            retried: 1,
            failed: 0,
        }
    }

    /// Creates a permanently failed task outcome.
    pub const fn failed() -> Self {
        Self {
            succeeded: 0,
            retried: 0,
            failed: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DispatchStats, TaskDispatchOutcome};

    #[test]
    fn dispatch_stats_tracks_activity_and_adds_outcomes() {
        let mut stats = DispatchStats::default();
        assert!(!stats.has_activity());

        stats.claimed = 2;
        assert!(stats.has_activity());

        stats.add_outcome(TaskDispatchOutcome {
            succeeded: 1,
            retried: 2,
            failed: 3,
        });
        assert_eq!(stats.succeeded, 1);
        assert_eq!(stats.retried, 2);
        assert_eq!(stats.failed, 3);

        stats.add(DispatchStats {
            claimed: 4,
            succeeded: 5,
            retried: 6,
            failed: 7,
        });
        assert_eq!(stats.claimed, 6);
        assert_eq!(stats.succeeded, 6);
        assert_eq!(stats.retried, 8);
        assert_eq!(stats.failed, 10);
    }
}
