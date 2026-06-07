//! Background task step helpers.

use chrono::Utc;

use crate::errors::{AsterError, MapAsterErr, Result};
use crate::types::StoredTaskSteps;

use super::types::{TaskStepInfo, TaskStepStatus};

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskStepSpec {
    pub(super) key: &'static str,
    pub(super) title: &'static str,
}

fn new_task_step(spec: TaskStepSpec, status: TaskStepStatus, detail: Option<&str>) -> TaskStepInfo {
    let now = (status == TaskStepStatus::Active).then(Utc::now);
    TaskStepInfo {
        key: spec.key.to_string(),
        title: spec.title.to_string(),
        status,
        progress_current: 0,
        progress_total: 0,
        detail: detail.map(str::to_string),
        started_at: now,
        finished_at: None,
    }
}

pub(super) fn initial_task_steps_from_specs(specs: &[TaskStepSpec]) -> Vec<TaskStepInfo> {
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            new_task_step(
                *spec,
                if index == 0 {
                    TaskStepStatus::Active
                } else {
                    TaskStepStatus::Pending
                },
                if index == 0 {
                    Some("Waiting for worker")
                } else {
                    None
                },
            )
        })
        .collect()
}

pub(super) fn parse_task_steps_json(steps_json: Option<&str>) -> Result<Vec<TaskStepInfo>> {
    match steps_json {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw)
            .map_aster_err_ctx("parse task steps json", AsterError::internal_error),
        _ => Ok(Vec::new()),
    }
}

pub(super) fn serialize_task_steps(steps: &[TaskStepInfo]) -> Result<StoredTaskSteps> {
    serde_json::to_string(steps)
        .map(StoredTaskSteps)
        .map_aster_err_ctx("serialize task steps", AsterError::internal_error)
}

pub(super) fn mark_active_step_failed(steps: &mut [TaskStepInfo], detail: Option<&str>) {
    let now = Utc::now();
    if let Some(step) = steps
        .iter_mut()
        .find(|step| step.status == TaskStepStatus::Active)
    {
        step.status = TaskStepStatus::Failed;
        if step.started_at.is_none() {
            step.started_at = Some(now);
        }
        step.finished_at = Some(now);
        step.detail = detail.map(str::to_string);
        return;
    }
    if let Some(step) = steps
        .iter_mut()
        .rev()
        .find(|step| step.status == TaskStepStatus::Pending)
    {
        step.status = TaskStepStatus::Failed;
        step.started_at = Some(now);
        step.finished_at = Some(now);
        step.detail = detail.map(str::to_string);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TaskStepSpec, initial_task_steps_from_specs, mark_active_step_failed,
        parse_task_steps_json, serialize_task_steps,
    };
    use crate::services::task_service::types::{TaskStepInfo, TaskStepStatus};

    fn step(key: &str, status: TaskStepStatus) -> TaskStepInfo {
        TaskStepInfo {
            key: key.to_string(),
            title: key.to_string(),
            status,
            progress_current: 0,
            progress_total: 1,
            detail: None,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn initial_steps_activate_first_spec_and_leave_rest_pending() {
        let steps = initial_task_steps_from_specs(&[
            TaskStepSpec {
                key: "prepare",
                title: "Prepare",
            },
            TaskStepSpec {
                key: "finish",
                title: "Finish",
            },
        ]);

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].key, "prepare");
        assert_eq!(steps[0].title, "Prepare");
        assert_eq!(steps[0].status, TaskStepStatus::Active);
        assert_eq!(steps[0].detail.as_deref(), Some("Waiting for worker"));
        assert!(steps[0].started_at.is_some());
        assert_eq!(steps[1].status, TaskStepStatus::Pending);
        assert_eq!(steps[1].detail, None);
        assert!(steps[1].started_at.is_none());
    }

    #[test]
    fn parse_steps_json_accepts_missing_blank_and_valid_json() {
        assert!(parse_task_steps_json(None).unwrap().is_empty());
        assert!(parse_task_steps_json(Some("  ")).unwrap().is_empty());

        let stored = serialize_task_steps(&[step("prepare", TaskStepStatus::Succeeded)]).unwrap();
        let parsed = parse_task_steps_json(Some(stored.as_ref())).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].key, "prepare");
        assert_eq!(parsed[0].status, TaskStepStatus::Succeeded);

        let error = parse_task_steps_json(Some("not json")).unwrap_err();
        assert!(error.message().contains("parse task steps json"));
    }

    #[test]
    fn mark_active_step_failed_updates_active_step_first() {
        let mut steps = vec![
            step("prepare", TaskStepStatus::Succeeded),
            step("process", TaskStepStatus::Active),
            step("finish", TaskStepStatus::Pending),
        ];

        mark_active_step_failed(&mut steps, Some("failed"));

        assert_eq!(steps[1].status, TaskStepStatus::Failed);
        assert_eq!(steps[1].detail.as_deref(), Some("failed"));
        assert!(steps[1].started_at.is_some());
        assert!(steps[1].finished_at.is_some());
        assert_eq!(steps[2].status, TaskStepStatus::Pending);
    }

    #[test]
    fn mark_active_step_failed_falls_back_to_last_pending_step() {
        let mut steps = vec![
            step("prepare", TaskStepStatus::Succeeded),
            step("process", TaskStepStatus::Pending),
            step("finish", TaskStepStatus::Pending),
        ];

        mark_active_step_failed(&mut steps, Some("pending failed"));

        assert_eq!(steps[1].status, TaskStepStatus::Pending);
        assert_eq!(steps[2].status, TaskStepStatus::Failed);
        assert_eq!(steps[2].detail.as_deref(), Some("pending failed"));
    }
}
