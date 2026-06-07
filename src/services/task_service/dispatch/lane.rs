use crate::config::operations;
use crate::runtime::SharedRuntimeState;
use crate::types::BackgroundTaskKind;

use super::super::registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::services::task_service) enum TaskLane {
    Fallback,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskLaneConfig {
    pub(super) lane: TaskLane,
    pub(super) limit: usize,
    pub(super) fast_continue: bool,
}

pub(super) const TASK_LANES: [TaskLane; 1] = [TaskLane::Fallback];

pub(super) fn task_lane_configs(state: &impl SharedRuntimeState) -> Vec<TaskLaneConfig> {
    vec![TaskLaneConfig {
        lane: TaskLane::Fallback,
        limit: operations::background_task_max_concurrency(state.runtime_config()),
        fast_continue: false,
    }]
}

impl TaskLaneConfig {
    pub(super) fn kinds(self) -> &'static [BackgroundTaskKind] {
        registry::task_lane_kinds(self.lane)
    }

    pub(super) fn lock_key(self) -> &'static str {
        match self.lane {
            TaskLane::Fallback => operations::BACKGROUND_TASK_MAX_CONCURRENCY_KEY,
        }
    }
}

pub(super) fn task_lane(kind: BackgroundTaskKind) -> TaskLane {
    registry::task_lane(kind)
}

#[cfg(test)]
mod tests {
    use super::{TASK_LANES, TaskLane, TaskLaneConfig, task_lane};
    use crate::types::BackgroundTaskKind;

    #[test]
    fn fallback_lane_contains_system_runtime_tasks() {
        assert_eq!(TASK_LANES, [TaskLane::Fallback]);
        assert_eq!(
            task_lane(BackgroundTaskKind::SystemRuntime),
            TaskLane::Fallback
        );

        let config = TaskLaneConfig {
            lane: TaskLane::Fallback,
            limit: 4,
            fast_continue: false,
        };
        assert_eq!(config.kinds(), &[BackgroundTaskKind::SystemRuntime]);
        assert_eq!(config.lock_key(), "background_task_max_concurrency");
    }
}
