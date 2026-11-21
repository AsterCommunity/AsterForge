//! Background task step state helpers.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

use crate::{Result, TaskCoreError};

/// Runtime status for a task step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TaskStepStatus {
    /// The step has not started.
    Pending,
    /// The step is currently running.
    Active,
    /// The step completed successfully.
    Succeeded,
    /// The step failed.
    Failed,
    /// The step was intentionally skipped.
    Skipped,
    /// The step was canceled.
    Canceled,
}

/// Serialized task step shown in task APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskStepInfo {
    /// Stable step key.
    pub key: String,
    /// Human-readable step title.
    pub title: String,
    /// Current step status.
    pub status: TaskStepStatus,
    /// Current progress amount.
    pub progress_current: i64,
    /// Total progress amount.
    pub progress_total: i64,
    /// Optional detail text.
    pub detail: Option<String>,
    /// Step start time.
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub started_at: Option<OffsetDateTime>,
    /// Step finish time.
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub finished_at: Option<OffsetDateTime>,
}

/// Static step definition used to create initial task steps.
#[derive(Debug, Clone, Copy)]
pub struct TaskStepSpec {
    /// Stable step key.
    pub key: &'static str,
    /// Human-readable step title.
    pub title: &'static str,
}

fn new_task_step(spec: TaskStepSpec, status: TaskStepStatus, detail: Option<&str>) -> TaskStepInfo {
    let now = (status == TaskStepStatus::Active).then(OffsetDateTime::now_utc);
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

/// Creates initial task step state from static specs.
#[must_use]
pub fn initial_task_steps_from_specs(specs: &[TaskStepSpec]) -> Vec<TaskStepInfo> {
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

fn find_task_step_mut<'a>(
    steps: &'a mut [TaskStepInfo],
    key: &str,
) -> Result<&'a mut TaskStepInfo> {
    steps
        .iter_mut()
        .find(|step| step.key == key)
        .ok_or_else(|| TaskCoreError::invalid_value(format!("task step '{key}' not found")))
}

/// Marks a task step active.
///
/// # Errors
///
/// Returns [`TaskCoreError`] when the step key or progress transition is invalid.
pub fn set_task_step_active(
    steps: &mut [TaskStepInfo],
    key: &str,
    detail: Option<&str>,
    progress: Option<(i64, i64)>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let step = find_task_step_mut(steps, key)?;
    step.status = TaskStepStatus::Active;
    if step.started_at.is_none() {
        step.started_at = Some(now);
    }
    step.finished_at = None;
    step.detail = detail.map(str::to_string);
    if let Some((current, total)) = progress {
        step.progress_current = current;
        step.progress_total = total;
    }
    Ok(())
}

/// Marks a task step succeeded.
///
/// # Errors
///
/// Returns [`TaskCoreError`] when the step key or progress transition is invalid.
pub fn set_task_step_succeeded(
    steps: &mut [TaskStepInfo],
    key: &str,
    detail: Option<&str>,
    progress: Option<(i64, i64)>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let step = find_task_step_mut(steps, key)?;
    step.status = TaskStepStatus::Succeeded;
    if step.started_at.is_none() {
        step.started_at = Some(now);
    }
    step.finished_at = Some(now);
    step.detail = detail.map(str::to_string);
    if let Some((current, total)) = progress {
        step.progress_current = current;
        step.progress_total = total;
    } else if step.progress_total > 0 {
        step.progress_current = step.progress_total;
    }
    Ok(())
}

/// Marks a task step skipped.
///
/// # Errors
///
/// Returns [`TaskCoreError`] when the step key is unknown or the transition is invalid.
pub fn set_task_step_skipped(
    steps: &mut [TaskStepInfo],
    key: &str,
    detail: Option<&str>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let step = find_task_step_mut(steps, key)?;
    step.status = TaskStepStatus::Skipped;
    if step.started_at.is_none() {
        step.started_at = Some(now);
    }
    step.finished_at = Some(now);
    step.detail = detail.map(str::to_string);
    Ok(())
}

/// Marks the active step failed, or the last pending step when no step is active.
pub fn mark_active_step_failed(steps: &mut [TaskStepInfo], detail: Option<&str>) {
    let now = OffsetDateTime::now_utc();
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
        TaskStepInfo, TaskStepSpec, TaskStepStatus, initial_task_steps_from_specs,
        mark_active_step_failed, set_task_step_active, set_task_step_skipped,
        set_task_step_succeeded,
    };

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
    fn step_state_helpers_update_timestamps_progress_and_detail() {
        let mut steps = vec![step("prepare", TaskStepStatus::Pending)];

        set_task_step_active(&mut steps, "prepare", Some("running"), Some((2, 5))).unwrap();
        assert_eq!(steps[0].status, TaskStepStatus::Active);
        assert_eq!(steps[0].detail.as_deref(), Some("running"));
        assert_eq!(steps[0].progress_current, 2);
        assert_eq!(steps[0].progress_total, 5);
        assert!(steps[0].started_at.is_some());
        assert!(steps[0].finished_at.is_none());

        set_task_step_succeeded(&mut steps, "prepare", Some("done"), None).unwrap();
        assert_eq!(steps[0].status, TaskStepStatus::Succeeded);
        assert_eq!(steps[0].detail.as_deref(), Some("done"));
        assert_eq!(steps[0].progress_current, 5);
        assert!(steps[0].finished_at.is_some());

        set_task_step_skipped(&mut steps, "prepare", Some("skip")).unwrap();
        assert_eq!(steps[0].status, TaskStepStatus::Skipped);
        assert_eq!(steps[0].detail.as_deref(), Some("skip"));
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

    #[test]
    fn step_time_round_trip_keeps_rfc3339_nanoseconds_and_null() {
        let mut value = step("prepare", TaskStepStatus::Pending);
        let instant = time::macros::datetime!(2000-02-29 12:34:56.123456789 UTC);
        value.started_at = Some(instant);
        value.finished_at = None;
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["started_at"], "2000-02-29T12:34:56.123456789Z");
        assert!(json["finished_at"].is_null());
        let decoded: TaskStepInfo = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(decoded.started_at, Some(instant));
        assert_eq!(decoded.finished_at, None);
        let mut missing = json.clone();
        missing.as_object_mut().unwrap().remove("finished_at");
        assert_eq!(
            serde_json::from_value::<TaskStepInfo>(missing)
                .unwrap()
                .finished_at,
            None
        );
        let mut invalid = json;
        invalid["started_at"] = serde_json::json!("invalid");
        assert!(serde_json::from_value::<TaskStepInfo>(invalid).is_err());
    }
}
