//! Integration tests for administrator background task routes.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::entities::background_task;
use aster_forge::runtime::{AppState, SharedRuntimeState};
use aster_forge::types::{
    BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload, StoredTaskResult,
};
use chrono::{DateTime, Duration, Utc};
use sea_orm::Set;
use serde_json::Value;

struct TestTaskInsert {
    status: BackgroundTaskStatus,
    display_name: String,
    payload: Value,
    result: Option<Value>,
    finished_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    progress_current: i64,
    progress_total: i64,
    failure_can_retry: Option<bool>,
    creator_user_id: Option<i64>,
}

impl TestTaskInsert {
    fn new(status: BackgroundTaskStatus, display_name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            status,
            display_name: display_name.into(),
            payload: serde_json::json!({ "task_name": "task-cleanup" }),
            result: None,
            finished_at: None,
            updated_at: now,
            progress_current: if status == BackgroundTaskStatus::Succeeded {
                1
            } else {
                0
            },
            progress_total: 1,
            failure_can_retry: None,
            creator_user_id: None,
        }
    }

    fn finished_at(mut self, finished_at: DateTime<Utc>) -> Self {
        self.finished_at = Some(finished_at);
        self.updated_at = finished_at;
        self
    }

    fn updated_at(mut self, updated_at: DateTime<Utc>) -> Self {
        self.updated_at = updated_at;
        self
    }

    fn progress(mut self, current: i64, total: i64) -> Self {
        self.progress_current = current;
        self.progress_total = total;
        self
    }

    fn failure_can_retry(mut self, failure_can_retry: Option<bool>) -> Self {
        self.failure_can_retry = failure_can_retry;
        self
    }

    fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    fn result(mut self, result: Value) -> Self {
        self.result = Some(result);
        self
    }

    fn creator_user_id(mut self, creator_user_id: i64) -> Self {
        self.creator_user_id = Some(creator_user_id);
        self
    }
}

async fn insert_task(state: &AppState, task_insert: TestTaskInsert) -> i64 {
    let now = Utc::now();
    let task = aster_forge::db::repository::background_task_repo::create(
        state.writer_db(),
        background_task::ActiveModel {
            kind: Set(BackgroundTaskKind::SystemRuntime),
            status: Set(task_insert.status),
            creator_user_id: Set(task_insert.creator_user_id),
            display_name: Set(task_insert.display_name),
            payload_json: Set(StoredTaskPayload(task_insert.payload.to_string())),
            result_json: Set(task_insert
                .result
                .map(|value| StoredTaskResult(value.to_string()))),
            runtime_json: Set(None),
            steps_json: Set(None),
            progress_current: Set(task_insert.progress_current),
            progress_total: Set(task_insert.progress_total),
            status_text: Set(None),
            attempt_count: Set(if task_insert.status == BackgroundTaskStatus::Failed {
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
            started_at: Set(task_insert.finished_at.map(|at| at - Duration::minutes(1))),
            finished_at: Set(task_insert.finished_at),
            last_error: Set(if task_insert.status == BackgroundTaskStatus::Failed {
                Some("test failure".to_string())
            } else {
                None
            }),
            failure_can_retry: Set(task_insert.failure_can_retry),
            expires_at: Set(now + Duration::hours(24)),
            created_at: Set(task_insert.updated_at),
            updated_at: Set(task_insert.updated_at),
            ..Default::default()
        },
    )
    .await
    .expect("test background task should insert");
    task.id
}

fn json_string_values(items: &[Value], key: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item[key]
                .as_str()
                .unwrap_or_else(|| panic!("{key} should be a string in {item}"))
                .to_string()
        })
        .collect()
}

fn json_i64_values(items: &[Value], key: &str) -> Vec<i64> {
    items
        .iter()
        .map(|item| {
            item[key]
                .as_i64()
                .unwrap_or_else(|| panic!("{key} should be an integer in {item}"))
        })
        .collect()
}

#[actix_web::test]
async fn admin_task_routes_are_admin_only() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let admin_token = setup_admin!(app);
    let user_token = register_user!(
        app,
        "regular-user",
        "regular-user@example.com",
        "password1234"
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks")
        .to_request();
    assert_service_status!(app, req, 401);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks")
        .insert_header(common::bearer_header(user_token))
        .to_request();
    assert_service_status!(app, req, 403);

    let req = test::TestRequest::get()
        .uri("/api/v1/tasks")
        .insert_header(common::bearer_header(admin_token))
        .to_request();
    assert_service_status!(app, req, 404);
}

#[actix_web::test]
async fn admin_can_list_retry_and_cleanup_tasks() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let creator_token = register_user!(
        app,
        "task-creator",
        "task-creator@example.com",
        "password1234"
    );
    let creator = aster_forge::services::auth_service::current_user_from_token(
        &state_for_insert,
        &creator_token,
    )
    .await
    .expect("creator user should resolve");

    let retry_task_id = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Failed, "Retryable task")
            .finished_at(Utc::now() - Duration::minutes(10))
            .failure_can_retry(Some(true))
            .creator_user_id(creator.id),
    )
    .await;
    let cleanup_task_id = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Succeeded, "Old completed task")
            .finished_at(Utc::now() - Duration::hours(2)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=10&status=failed")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["items"][0]["id"], retry_task_id);
    assert_eq!(body["data"]["items"][0]["status"], "failed");
    assert_eq!(body["data"]["items"][0]["can_retry"], true);
    assert_eq!(body["data"]["items"][0]["creator_user_id"], creator.id);
    assert_eq!(
        body["data"]["items"][0]["creator"]["username"],
        "task-creator"
    );
    assert_eq!(
        body["data"]["items"][0]["presentation"]["title"]["code"],
        "runtime_task_task_cleanup"
    );

    let req = test::TestRequest::post()
        .uri(&format!("/api/v1/admin/tasks/{retry_task_id}/retry"))
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["id"], retry_task_id);
    assert_eq!(body["data"]["status"], "pending");
    assert_eq!(body["data"]["creator"]["email"], "task-creator@example.com");
    assert_eq!(body["data"]["attempt_count"], 0);
    assert!(body["data"]["last_error"].is_null());

    let req = test::TestRequest::post()
        .uri("/api/v1/admin/tasks/cleanup")
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "finished_before": Utc::now() - Duration::hours(1),
            "status": "succeeded"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["removed"], 1);

    let req = test::TestRequest::post()
        .uri(&format!("/api/v1/admin/tasks/{cleanup_task_id}/retry"))
        .insert_header(common::bearer_header(token))
        .to_request();
    assert_service_status!(app, req, 404);
}

#[actix_web::test]
async fn admin_tasks_support_explicit_sorting_and_id_tiebreaker() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let now = Utc::now();
    let mut inserted_ids = Vec::new();

    for display_name in ["Task Sort Zeta", "Task Sort Alpha", "Task Sort Beta"] {
        inserted_ids.push(
            insert_task(
                &state_for_insert,
                TestTaskInsert::new(BackgroundTaskStatus::Pending, display_name)
                    .updated_at(now)
                    .progress(5, 10),
            )
            .await,
        );
    }

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?sort_by=display_name&sort_order=asc&limit=10")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    let tasks = body["data"]["items"].as_array().unwrap();
    assert_eq!(
        json_string_values(tasks, "display_name"),
        vec!["Task Sort Alpha", "Task Sort Beta", "Task Sort Zeta"]
    );

    inserted_ids.sort_unstable();
    inserted_ids.reverse();
    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?sort_by=progress&sort_order=desc&limit=3")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    let tasks = body["data"]["items"].as_array().unwrap();
    assert_eq!(json_i64_values(tasks, "id"), inserted_ids);
}

#[actix_web::test]
async fn admin_tasks_default_list_supports_limit_offset() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let now = Utc::now();

    let old_task = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Succeeded, "Old runtime task")
            .finished_at(now - Duration::hours(3)),
    )
    .await;
    let middle_task = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Failed, "Middle runtime task")
            .finished_at(now - Duration::hours(2))
            .failure_can_retry(Some(true)),
    )
    .await;
    let newest_task = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Processing, "Newest runtime task")
            .updated_at(now - Duration::hours(1))
            .progress(3, 5),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=2")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["limit"], 2);
    assert_eq!(body["data"]["offset"], 0);
    assert_eq!(body["data"]["total"], 3);

    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], newest_task);
    assert_eq!(items[0]["status"], "processing");
    assert_eq!(items[0]["progress_percent"], 60);
    assert_eq!(items[1]["id"], middle_task);
    assert_eq!(items[1]["status"], "failed");
    assert_eq!(items[1]["can_retry"], true);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=2&offset=2")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], old_task);
    assert_eq!(items[0]["status"], "succeeded");
}

#[actix_web::test]
async fn admin_tasks_cleanup_uses_explicit_filters() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let now = Utc::now();

    let old_failed = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Failed, "Old failed runtime task")
            .finished_at(now - Duration::hours(72))
            .failure_can_retry(Some(true)),
    )
    .await;
    let recent_failed = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Failed, "Recent failed runtime task")
            .finished_at(now - Duration::hours(2))
            .failure_can_retry(Some(true)),
    )
    .await;
    let old_succeeded = insert_task(
        &state_for_insert,
        TestTaskInsert::new(
            BackgroundTaskStatus::Succeeded,
            "Old succeeded runtime task",
        )
        .finished_at(now - Duration::hours(96)),
    )
    .await;
    let active_task = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Processing, "Active runtime task")
            .updated_at(now - Duration::hours(96)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/api/v1/admin/tasks/cleanup")
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "finished_before": now - Duration::hours(24),
            "kind": "system_runtime",
            "status": "failed"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["removed"], 1);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=10")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    let ids = body["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_i64())
        .collect::<Vec<_>>();

    assert!(!ids.contains(&old_failed));
    assert!(ids.contains(&recent_failed));
    assert!(ids.contains(&old_succeeded));
    assert!(ids.contains(&active_task));
}

#[actix_web::test]
async fn admin_tasks_present_system_runtime_health_result() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let now = Utc::now();
    let task_id = insert_task(
        &state_for_insert,
        TestTaskInsert::new(BackgroundTaskStatus::Succeeded, "System health check")
            .payload(serde_json::json!({ "task_name": "system-health-check" }))
            .result(serde_json::json!({
                "duration_ms": 12,
                "summary": "System health check completed",
                "system_health": {
                    "status": "healthy",
                    "components": []
                }
            }))
            .finished_at(now),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=1")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;

    assert_eq!(body["data"]["items"][0]["id"], task_id);
    assert_eq!(
        body["data"]["items"][0]["presentation"]["title"]["code"],
        "runtime_task_system_health_check"
    );
    assert_eq!(
        body["data"]["items"][0]["presentation"]["status"]["code"],
        "status_text_system_healthy"
    );
}
