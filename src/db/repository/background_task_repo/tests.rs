use super::{
    AdminTaskFilters, SystemRuntimeSuccessRefresh, TaskFailureUpdate, TaskProgressUpdate,
    TaskSuccessUpdate, TerminalTaskCleanupFilters, count_active_processing_by_kinds,
    count_pending_or_retry, count_processing, create, delete_many, delete_terminal_by_filters,
    find_by_id, find_latest_by_kind_and_display_name, find_latest_system_runtime_by_payload,
    find_paginated_all, find_paginated_all_filtered, list_claimable, list_claimable_by_kinds,
    list_expired_terminal, list_recent, mark_failed, mark_progress, mark_retry, mark_succeeded,
    refresh_system_runtime_success, release_processing, reset_for_manual_retry, set_display_name,
    set_runtime_json, touch_heartbeat, try_claim,
};
use crate::api::pagination::{AdminTaskSortBy, SortOrder};
use crate::config::DatabaseConfig;
use crate::entities::background_task;
use crate::types::{
    BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload, StoredTaskResult,
    StoredTaskRuntime, StoredTaskSteps,
};
use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, Set};

async fn build_test_db() -> sea_orm::DatabaseConnection {
    let db = crate::db::connect_with_metrics(
        &DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            pool_size: 1,
            retry_count: 0,
        },
        crate::metrics_core::NoopMetrics::arc(),
    )
    .await
    .expect("background task repo test DB should connect");
    migration::Migrator::up(&db, None)
        .await
        .expect("background task repo test migrations should succeed");
    db
}

fn runtime_payload(task_name: &str) -> StoredTaskPayload {
    StoredTaskPayload(serde_json::json!({ "task_name": task_name }).to_string())
}

fn runtime_result(summary: &str) -> StoredTaskResult {
    StoredTaskResult(
        serde_json::json!({
            "duration_ms": 42,
            "summary": summary,
        })
        .to_string(),
    )
}

fn task_active_model(
    status: BackgroundTaskStatus,
    display_name: impl Into<String>,
    now: chrono::DateTime<Utc>,
) -> background_task::ActiveModel {
    let finished_at = status.is_terminal().then_some(now - Duration::seconds(10));
    let started_at = finished_at.map(|finished_at| finished_at - Duration::seconds(5));
    let progress_current = if status == BackgroundTaskStatus::Succeeded {
        1
    } else {
        0
    };

    background_task::ActiveModel {
        kind: Set(BackgroundTaskKind::SystemRuntime),
        status: Set(status),
        creator_user_id: Set(None),
        display_name: Set(display_name.into()),
        payload_json: Set(runtime_payload("task-cleanup")),
        result_json: Set(None),
        runtime_json: Set(None),
        steps_json: Set(Some(StoredTaskSteps("[]".to_string()))),
        progress_current: Set(progress_current),
        progress_total: Set(1),
        status_text: Set(None),
        attempt_count: Set(if status == BackgroundTaskStatus::Failed {
            1
        } else {
            0
        }),
        max_attempts: Set(3),
        next_run_at: Set(now),
        processing_token: Set(0),
        processing_started_at: Set(None),
        last_heartbeat_at: Set(None),
        lease_expires_at: Set(None),
        started_at: Set(started_at),
        finished_at: Set(finished_at),
        last_error: Set(if status == BackgroundTaskStatus::Failed {
            Some("previous failure".to_string())
        } else {
            None
        }),
        failure_can_retry: Set(if status == BackgroundTaskStatus::Failed {
            Some(true)
        } else {
            None
        }),
        expires_at: Set(now + Duration::hours(24)),
        created_at: Set(now - Duration::hours(1)),
        updated_at: Set(now),
        ..Default::default()
    }
}

async fn insert_task(
    db: &sea_orm::DatabaseConnection,
    status: BackgroundTaskStatus,
    display_name: impl Into<String>,
    now: chrono::DateTime<Utc>,
) -> background_task::Model {
    create(db, task_active_model(status, display_name, now))
        .await
        .expect("background task test row should insert")
}

async fn set_processing_lease(
    db: &sea_orm::DatabaseConnection,
    task: background_task::Model,
    processing_token: i64,
    now: chrono::DateTime<Utc>,
    lease_expires_at: chrono::DateTime<Utc>,
) -> background_task::Model {
    let mut active: background_task::ActiveModel = task.into();
    active.status = Set(BackgroundTaskStatus::Processing);
    active.processing_token = Set(processing_token);
    active.processing_started_at = Set(Some(now));
    active.last_heartbeat_at = Set(Some(now));
    active.lease_expires_at = Set(Some(lease_expires_at));
    active.started_at = Set(Some(now));
    active
        .update(db)
        .await
        .expect("background task test lease should update")
}

async fn reload(
    db: &sea_orm::DatabaseConnection,
    task: &background_task::Model,
) -> background_task::Model {
    find_by_id(db, task.id)
        .await
        .expect("task should reload by id")
}

#[tokio::test]
async fn create_and_find_by_id_round_trip_task_record() {
    let db = build_test_db().await;
    let now = Utc::now();
    let task = insert_task(&db, BackgroundTaskStatus::Pending, "Created task", now).await;

    let stored = find_by_id(&db, task.id).await.unwrap();
    assert_eq!(stored.id, task.id);
    assert_eq!(stored.kind, BackgroundTaskKind::SystemRuntime);
    assert_eq!(stored.status, BackgroundTaskStatus::Pending);
    assert_eq!(stored.display_name, "Created task");

    let missing = find_by_id(&db, task.id + 1000).await.unwrap_err();
    assert!(missing.message().contains("task #"));

    db.close().await.unwrap();
}

#[tokio::test]
async fn list_and_count_queries_use_task_statuses() {
    let db = build_test_db().await;
    let now = Utc::now();
    let oldest = insert_task(
        &db,
        BackgroundTaskStatus::Succeeded,
        "Oldest task",
        now - Duration::minutes(3),
    )
    .await;
    let retry = insert_task(
        &db,
        BackgroundTaskStatus::Retry,
        "Retry task",
        now - Duration::minutes(2),
    )
    .await;
    let processing = insert_task(
        &db,
        BackgroundTaskStatus::Processing,
        "Processing task",
        now - Duration::minutes(1),
    )
    .await;
    let pending = insert_task(&db, BackgroundTaskStatus::Pending, "Pending task", now).await;

    assert_eq!(count_processing(&db).await.unwrap(), 1);
    assert_eq!(count_pending_or_retry(&db).await.unwrap(), 2);

    let recent_ids = list_recent(&db, 3)
        .await
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect::<Vec<_>>();
    assert_eq!(recent_ids, vec![pending.id, processing.id, retry.id]);

    let (page, total) = find_paginated_all(&db, 2, 1).await.unwrap();
    assert_eq!(total, 4);
    assert_eq!(page.len(), 2);
    assert_eq!(
        page.into_iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![processing.id, retry.id]
    );

    assert_eq!(oldest.status, BackgroundTaskStatus::Succeeded);
    db.close().await.unwrap();
}

#[tokio::test]
async fn find_paginated_all_filtered_applies_status_and_sorting() {
    let db = build_test_db().await;
    let now = Utc::now();
    let alpha = insert_task(&db, BackgroundTaskStatus::Failed, "Alpha task", now).await;
    let beta = insert_task(&db, BackgroundTaskStatus::Failed, "Beta task", now).await;
    insert_task(&db, BackgroundTaskStatus::Pending, "Gamma task", now).await;

    let (items, total) = find_paginated_all_filtered(
        &db,
        20,
        0,
        &AdminTaskFilters {
            kind: Some(BackgroundTaskKind::SystemRuntime),
            status: Some(BackgroundTaskStatus::Failed),
        },
        AdminTaskSortBy::DisplayName,
        SortOrder::Asc,
    )
    .await
    .unwrap();

    assert_eq!(total, 2);
    assert_eq!(
        items.into_iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![alpha.id, beta.id]
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn latest_lookup_helpers_match_payload_and_display_name() {
    let db = build_test_db().await;
    let now = Utc::now();
    let first = insert_task(
        &db,
        BackgroundTaskStatus::Succeeded,
        "Repeated runtime task",
        now - Duration::minutes(3),
    )
    .await;
    let latest = insert_task(
        &db,
        BackgroundTaskStatus::Succeeded,
        "Repeated runtime task",
        now - Duration::minutes(1),
    )
    .await;

    let by_payload = find_latest_system_runtime_by_payload(&db, &runtime_payload("task-cleanup"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_payload.id, latest.id);

    let by_name = find_latest_by_kind_and_display_name(
        &db,
        BackgroundTaskKind::SystemRuntime,
        "Repeated runtime task",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(by_name.id, latest.id);
    assert_ne!(by_name.id, first.id);

    db.close().await.unwrap();
}

#[tokio::test]
async fn claimable_queries_include_due_and_stale_processing_tasks() {
    let db = build_test_db().await;
    let now = Utc::now();
    let due = insert_task(
        &db,
        BackgroundTaskStatus::Pending,
        "Due pending task",
        now - Duration::minutes(10),
    )
    .await;
    let retry = insert_task(
        &db,
        BackgroundTaskStatus::Retry,
        "Due retry task",
        now - Duration::minutes(9),
    )
    .await;
    let future = insert_task(
        &db,
        BackgroundTaskStatus::Pending,
        "Future pending task",
        now + Duration::minutes(10),
    )
    .await;
    let stale = insert_task(
        &db,
        BackgroundTaskStatus::Processing,
        "Stale processing task",
        now - Duration::minutes(8),
    )
    .await;
    let stale = set_processing_lease(
        &db,
        stale,
        4,
        now - Duration::minutes(8),
        now - Duration::seconds(1),
    )
    .await;
    let active = insert_task(
        &db,
        BackgroundTaskStatus::Processing,
        "Active processing task",
        now - Duration::minutes(7),
    )
    .await;
    set_processing_lease(
        &db,
        active,
        5,
        now - Duration::minutes(7),
        now + Duration::minutes(5),
    )
    .await;

    let claimable = list_claimable(&db, now, now - Duration::seconds(60), 10)
        .await
        .unwrap();
    let ids = claimable.iter().map(|task| task.id).collect::<Vec<_>>();
    assert!(ids.contains(&due.id));
    assert!(ids.contains(&retry.id));
    assert!(ids.contains(&stale.id));
    assert!(!ids.contains(&future.id));

    let by_kind = list_claimable_by_kinds(
        &db,
        now,
        now - Duration::seconds(60),
        &[BackgroundTaskKind::SystemRuntime],
        10,
    )
    .await
    .unwrap();
    assert_eq!(by_kind.len(), 3);
    assert!(
        list_claimable_by_kinds(&db, now, now - Duration::seconds(60), &[], 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        list_claimable_by_kinds(
            &db,
            now,
            now - Duration::seconds(60),
            &[BackgroundTaskKind::SystemRuntime],
            0,
        )
        .await
        .unwrap()
        .is_empty()
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn try_claim_uses_token_and_claimable_fences() {
    let db = build_test_db().await;
    let now = Utc::now();
    let task = insert_task(
        &db,
        BackgroundTaskStatus::Pending,
        "Claim candidate",
        now - Duration::minutes(1),
    )
    .await;

    let claimed = try_claim(
        &db,
        task.id,
        task.processing_token,
        now,
        now - Duration::seconds(60),
        task.processing_token + 1,
        now + Duration::seconds(30),
    )
    .await
    .unwrap();
    assert!(claimed);

    let stored = reload(&db, &task).await;
    assert_eq!(stored.status, BackgroundTaskStatus::Processing);
    assert_eq!(stored.processing_token, task.processing_token + 1);
    assert_eq!(stored.processing_started_at, Some(now));
    assert_eq!(stored.last_heartbeat_at, Some(now));
    assert_eq!(stored.lease_expires_at, Some(now + Duration::seconds(30)));
    assert_eq!(stored.started_at, Some(now));

    let stale_token_claim = try_claim(
        &db,
        task.id,
        task.processing_token,
        now + Duration::seconds(1),
        now - Duration::seconds(60),
        task.processing_token + 2,
        now + Duration::seconds(60),
    )
    .await
    .unwrap();
    assert!(!stale_token_claim);

    let future = insert_task(
        &db,
        BackgroundTaskStatus::Pending,
        "Future claim candidate",
        now + Duration::hours(1),
    )
    .await;
    let future_claim = try_claim(
        &db,
        future.id,
        future.processing_token,
        now,
        now - Duration::seconds(60),
        future.processing_token + 1,
        now + Duration::seconds(30),
    )
    .await
    .unwrap();
    assert!(!future_claim);

    db.close().await.unwrap();
}

#[tokio::test]
async fn touch_heartbeat_uses_processing_token_fence() {
    let db = build_test_db().await;
    let now = Utc::now();
    let task = insert_task(&db, BackgroundTaskStatus::Processing, "Heartbeat task", now).await;
    let task = set_processing_lease(&db, task, 7, now, now + Duration::seconds(30)).await;

    let touched = touch_heartbeat(
        &db,
        task.id,
        task.processing_token,
        now + Duration::seconds(5),
        now + Duration::seconds(60),
    )
    .await
    .unwrap();
    assert!(touched);

    let stored = reload(&db, &task).await;
    assert_eq!(stored.last_heartbeat_at, Some(now + Duration::seconds(5)));
    assert_eq!(stored.lease_expires_at, Some(now + Duration::seconds(60)));

    let stale_token_touch = touch_heartbeat(
        &db,
        task.id,
        task.processing_token + 1,
        now + Duration::seconds(10),
        now + Duration::seconds(90),
    )
    .await
    .unwrap();
    assert!(!stale_token_touch);

    db.close().await.unwrap();
}

#[tokio::test]
async fn count_active_processing_by_kinds_only_counts_unexpired_leases() {
    let db = build_test_db().await;
    let now = Utc::now();
    let active = insert_task(&db, BackgroundTaskStatus::Processing, "Active lease", now).await;
    set_processing_lease(&db, active, 1, now, now + Duration::seconds(30)).await;
    let stale = insert_task(&db, BackgroundTaskStatus::Processing, "Stale lease", now).await;
    set_processing_lease(&db, stale, 2, now, now - Duration::seconds(1)).await;
    insert_task(
        &db,
        BackgroundTaskStatus::Processing,
        "Processing without lease",
        now,
    )
    .await;

    assert_eq!(
        count_active_processing_by_kinds(&db, now, &[BackgroundTaskKind::SystemRuntime])
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count_active_processing_by_kinds(&db, now, &[])
            .await
            .unwrap(),
        0
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn mark_progress_runtime_display_name_and_release_are_token_fenced() {
    let db = build_test_db().await;
    let now = Utc::now();
    let task = insert_task(&db, BackgroundTaskStatus::Processing, "Mutable task", now).await;
    let task = set_processing_lease(&db, task, 7, now, now + Duration::seconds(30)).await;

    assert!(
        mark_progress(
            &db,
            TaskProgressUpdate {
                id: task.id,
                processing_token: task.processing_token,
                now: now + Duration::seconds(5),
                lease_expires_at: now + Duration::seconds(65),
                current: 3,
                total: 10,
                status_text: Some("working"),
                steps_json: Some(r#"[{"key":"step","title":"Step","status":"active","progress_current":1,"progress_total":2,"detail":null,"started_at":null,"finished_at":null}]"#),
            },
        )
        .await
        .unwrap()
    );
    assert!(
        set_runtime_json(
            &db,
            task.id,
            task.processing_token,
            Some(r#"{"worker":"test"}"#),
            now + Duration::seconds(6),
        )
        .await
        .unwrap()
    );
    assert!(
        set_display_name(
            &db,
            task.id,
            task.processing_token,
            "Renamed task",
            now + Duration::seconds(7),
        )
        .await
        .unwrap()
    );

    let stored = reload(&db, &task).await;
    assert_eq!(stored.progress_current, 3);
    assert_eq!(stored.progress_total, 10);
    assert_eq!(stored.status_text.as_deref(), Some("working"));
    assert_eq!(
        stored.runtime_json,
        Some(StoredTaskRuntime(r#"{"worker":"test"}"#.to_string()))
    );
    assert_eq!(stored.display_name, "Renamed task");

    assert!(
        !mark_progress(
            &db,
            TaskProgressUpdate {
                id: task.id,
                processing_token: task.processing_token + 1,
                now,
                lease_expires_at: now + Duration::seconds(30),
                current: 9,
                total: 10,
                status_text: None,
                steps_json: None,
            },
        )
        .await
        .unwrap()
    );
    assert!(
        !set_runtime_json(
            &db,
            task.id,
            task.processing_token + 1,
            None,
            now + Duration::seconds(8),
        )
        .await
        .unwrap()
    );
    assert!(
        !set_display_name(
            &db,
            task.id,
            task.processing_token + 1,
            "Wrong token rename",
            now + Duration::seconds(9),
        )
        .await
        .unwrap()
    );

    let released = release_processing(
        &db,
        task.id,
        task.processing_token,
        now + Duration::seconds(10),
        BackgroundTaskStatus::Retry,
    )
    .await
    .unwrap();
    assert!(released);
    let stored = reload(&db, &task).await;
    assert_eq!(stored.status, BackgroundTaskStatus::Retry);
    assert_eq!(stored.next_run_at, now + Duration::seconds(10));
    assert_eq!(stored.processing_started_at, None);
    assert_eq!(stored.last_heartbeat_at, None);
    assert_eq!(stored.lease_expires_at, None);
    assert_eq!(stored.status_text, None);

    let error = release_processing(
        &db,
        task.id,
        task.processing_token,
        now,
        BackgroundTaskStatus::Succeeded,
    )
    .await
    .unwrap_err();
    assert!(error.message().contains("pending or retry"));

    db.close().await.unwrap();
}

#[tokio::test]
async fn mark_success_retry_failed_and_manual_retry_update_expected_fields() {
    let db = build_test_db().await;
    let now = Utc::now();

    let success = insert_task(&db, BackgroundTaskStatus::Processing, "Success task", now).await;
    let success = set_processing_lease(&db, success, 11, now, now + Duration::seconds(30)).await;
    assert!(
        mark_succeeded(
            &db,
            TaskSuccessUpdate {
                id: success.id,
                processing_token: success.processing_token,
                result_json: Some(runtime_result("done").as_ref()),
                steps_json: Some("[]"),
                current: 1,
                total: 1,
                status_text: Some("done"),
                finished_at: now + Duration::seconds(5),
                expires_at: now + Duration::hours(24),
            },
        )
        .await
        .unwrap()
    );
    let stored_success = reload(&db, &success).await;
    assert_eq!(stored_success.status, BackgroundTaskStatus::Succeeded);
    assert_eq!(stored_success.result_json, Some(runtime_result("done")));
    assert_eq!(stored_success.progress_current, 1);
    assert_eq!(stored_success.processing_started_at, None);

    let retry = insert_task(&db, BackgroundTaskStatus::Processing, "Retry task", now).await;
    let retry = set_processing_lease(&db, retry, 12, now, now + Duration::seconds(30)).await;
    assert!(
        mark_retry(
            &db,
            retry.id,
            retry.processing_token,
            2,
            now + Duration::minutes(5),
            "try again later",
            Some("[]"),
        )
        .await
        .unwrap()
    );
    let stored_retry = reload(&db, &retry).await;
    assert_eq!(stored_retry.status, BackgroundTaskStatus::Retry);
    assert_eq!(stored_retry.attempt_count, 2);
    assert_eq!(stored_retry.next_run_at, now + Duration::minutes(5));
    assert_eq!(stored_retry.last_error.as_deref(), Some("try again later"));

    let failure = insert_task(&db, BackgroundTaskStatus::Processing, "Failure task", now).await;
    let failure = set_processing_lease(&db, failure, 13, now, now + Duration::seconds(30)).await;
    assert!(
        mark_failed(
            &db,
            TaskFailureUpdate {
                id: failure.id,
                processing_token: failure.processing_token,
                attempt_count: 3,
                last_error: "failed permanently",
                finished_at: now + Duration::seconds(30),
                expires_at: now + Duration::hours(48),
                steps_json: Some("[]"),
                failure_can_retry: false,
            },
        )
        .await
        .unwrap()
    );
    let stored_failure = reload(&db, &failure).await;
    assert_eq!(stored_failure.status, BackgroundTaskStatus::Failed);
    assert_eq!(stored_failure.attempt_count, 3);
    assert_eq!(
        stored_failure.last_error.as_deref(),
        Some("failed permanently")
    );
    assert_eq!(stored_failure.failure_can_retry, Some(false));

    assert!(
        reset_for_manual_retry(&db, failure.id, now + Duration::minutes(10), 5, Some("[]"),)
            .await
            .unwrap()
    );
    let stored_reset = reload(&db, &failure).await;
    assert_eq!(stored_reset.status, BackgroundTaskStatus::Pending);
    assert_eq!(stored_reset.attempt_count, 0);
    assert_eq!(stored_reset.progress_current, 0);
    assert_eq!(stored_reset.max_attempts, 5);
    assert_eq!(stored_reset.started_at, None);
    assert_eq!(stored_reset.finished_at, None);
    assert_eq!(stored_reset.last_error, None);
    assert_eq!(stored_reset.result_json, None);
    assert_eq!(stored_reset.failure_can_retry, None);

    assert!(
        !mark_succeeded(
            &db,
            TaskSuccessUpdate {
                id: success.id,
                processing_token: success.processing_token + 1,
                result_json: None,
                steps_json: None,
                current: 1,
                total: 1,
                status_text: None,
                finished_at: now,
                expires_at: now,
            },
        )
        .await
        .unwrap()
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn refresh_system_runtime_success_only_updates_matching_success_rows() {
    let db = build_test_db().await;
    let now = Utc::now();
    let task = insert_task(
        &db,
        BackgroundTaskStatus::Succeeded,
        "System health check",
        now - Duration::minutes(5),
    )
    .await;

    assert!(
        refresh_system_runtime_success(
            &db,
            SystemRuntimeSuccessRefresh {
                id: task.id,
                result_json: runtime_result("healthy").as_ref(),
                status_text: Some("healthy"),
                next_run_at: now,
                started_at: now - Duration::seconds(2),
                finished_at: now,
                expires_at: now + Duration::hours(24),
            },
        )
        .await
        .unwrap()
    );
    let stored = reload(&db, &task).await;
    assert_eq!(stored.result_json, Some(runtime_result("healthy")));
    assert_eq!(stored.status_text.as_deref(), Some("healthy"));
    assert_eq!(stored.progress_current, 1);
    assert_eq!(stored.progress_total, 1);
    assert_eq!(stored.started_at, Some(now - Duration::seconds(2)));
    assert_eq!(stored.finished_at, Some(now));

    let pending = insert_task(&db, BackgroundTaskStatus::Pending, "Pending runtime", now).await;
    assert!(
        !refresh_system_runtime_success(
            &db,
            SystemRuntimeSuccessRefresh {
                id: pending.id,
                result_json: runtime_result("ignored").as_ref(),
                status_text: None,
                next_run_at: now,
                started_at: now,
                finished_at: now,
                expires_at: now,
            },
        )
        .await
        .unwrap()
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn cleanup_queries_remove_only_matching_terminal_tasks() {
    let db = build_test_db().await;
    let now = Utc::now();
    let expired_success = insert_task(
        &db,
        BackgroundTaskStatus::Succeeded,
        "Expired success",
        now - Duration::hours(48),
    )
    .await;
    let mut active: background_task::ActiveModel = expired_success.clone().into();
    active.expires_at = Set(now - Duration::hours(1));
    let expired_success = active.update(&db).await.unwrap();

    let fresh_failed = insert_task(
        &db,
        BackgroundTaskStatus::Failed,
        "Fresh failure",
        now - Duration::minutes(10),
    )
    .await;
    let old_failed = insert_task(
        &db,
        BackgroundTaskStatus::Failed,
        "Old failure",
        now - Duration::hours(72),
    )
    .await;
    let processing = insert_task(
        &db,
        BackgroundTaskStatus::Processing,
        "Old processing",
        now - Duration::hours(72),
    )
    .await;

    let expired = list_expired_terminal(&db, now, 10).await.unwrap();
    assert_eq!(
        expired.iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![old_failed.id, expired_success.id]
    );

    assert_eq!(delete_many(&db, &[]).await.unwrap(), 0);
    assert_eq!(delete_many(&db, &[expired_success.id]).await.unwrap(), 1);
    assert!(find_by_id(&db, expired_success.id).await.is_err());

    let removed = delete_terminal_by_filters(
        &db,
        &TerminalTaskCleanupFilters {
            finished_before: now - Duration::hours(24),
            kind: Some(BackgroundTaskKind::SystemRuntime),
            status: Some(BackgroundTaskStatus::Failed),
        },
    )
    .await
    .unwrap();
    assert_eq!(removed, 1);

    assert!(find_by_id(&db, old_failed.id).await.is_err());
    assert!(find_by_id(&db, fresh_failed.id).await.is_ok());
    assert!(find_by_id(&db, processing.id).await.is_ok());

    db.close().await.unwrap();
}
