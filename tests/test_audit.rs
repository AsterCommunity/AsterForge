//! Integration tests for audit logging.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::config::definitions::BRANDING_TITLE_KEY;
use aster_forge::db::repository::mail_outbox_repo;
use aster_forge::entities::{audit_log, background_task, mail_outbox};
use aster_forge::errors::{AsterError, Result as AsterResult};
use aster_forge::runtime::{AppState, SharedRuntimeState};
use aster_forge::services::{
    audit_service, mail_outbox_service,
    mail_service::{MailMessage, MailSender},
};
use aster_forge::types::{
    BackgroundTaskKind, BackgroundTaskStatus, MailOutboxStatus, MailTemplateCode,
    StoredMailPayload, StoredTaskPayload,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::Set;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;

fn find_action<'a>(items: &'a [Value], action: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["action"] == action)
        .unwrap_or_else(|| {
            panic!(
                "audit log should contain {action}, got {:?}",
                items
                    .iter()
                    .map(|item| item["action"].as_str().unwrap_or("<non-string>"))
                    .collect::<Vec<_>>()
            )
        })
}

async fn insert_failed_retryable_task(
    state: &AppState,
    display_name: &str,
    attempt_count: i32,
) -> i64 {
    let now = Utc::now();
    let task = aster_forge::db::repository::background_task_repo::create(
        state.writer_db(),
        background_task::ActiveModel {
            kind: Set(BackgroundTaskKind::SystemRuntime),
            status: Set(BackgroundTaskStatus::Failed),
            creator_user_id: Set(None),
            display_name: Set(display_name.to_string()),
            payload_json: Set(StoredTaskPayload(
                serde_json::json!({ "task_name": "task-retry-audit" }).to_string(),
            )),
            result_json: Set(None),
            runtime_json: Set(None),
            steps_json: Set(None),
            progress_current: Set(0),
            progress_total: Set(1),
            status_text: Set(None),
            attempt_count: Set(attempt_count),
            max_attempts: Set(3),
            next_run_at: Set(now),
            processing_token: Set(0),
            processing_started_at: Set(None),
            last_heartbeat_at: Set(None),
            lease_expires_at: Set(None),
            started_at: Set(Some(now - Duration::minutes(5))),
            finished_at: Set(Some(now - Duration::minutes(1))),
            last_error: Set(Some("retry audit failure".to_string())),
            failure_can_retry: Set(Some(true)),
            expires_at: Set(now + Duration::hours(24)),
            created_at: Set(now - Duration::minutes(5)),
            updated_at: Set(now - Duration::minutes(1)),
            ..Default::default()
        },
    )
    .await
    .expect("test background task should insert");
    task.id
}

struct FailingMailSender;

#[async_trait]
impl MailSender for FailingMailSender {
    async fn send(&self, _message: MailMessage) -> AsterResult<()> {
        Err(AsterError::mail_delivery_failed("smtp unavailable"))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn mail_outbox_model(
    attempt_count: i32,
    payload_json: StoredMailPayload,
) -> mail_outbox::ActiveModel {
    let now = Utc::now();
    mail_outbox::ActiveModel {
        template_code: Set(MailTemplateCode::RegisterActivation),
        to_address: Set("audit-mail@example.com".to_string()),
        to_name: Set(Some("Audit Mail".to_string())),
        payload_json: Set(payload_json),
        status: Set(MailOutboxStatus::Pending),
        attempt_count: Set(attempt_count),
        next_attempt_at: Set(now),
        processing_started_at: Set(None),
        sent_at: Set(None),
        last_error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
}

async fn latest_audit_entry(
    state: &AppState,
    action: audit_service::AuditAction,
) -> audit_log::Model {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(action))
        .order_by_desc(audit_log::Column::Id)
        .one(state.writer_db())
        .await
        .expect("audit log query should succeed")
        .expect("audit log entry should exist")
}

#[actix_web::test]
async fn audit_log_persists_external_auth_provider_entry() {
    let state = common::setup().await;

    audit_service::log(
        &state,
        &audit_service::AuditContext {
            user_id: 42,
            ip_address: None,
            user_agent: None,
        },
        audit_service::AuditAction::AdminTestExternalAuthProvider,
        audit_service::AuditEntityType::ExternalAuthProvider,
        None,
        Some("draft"),
        Some(serde_json::json!({
            "provider_kind": "oidc",
            "key": "draft",
            "success": true,
        })),
    )
    .await;
    audit_service::flush_global_audit_log_manager().await;

    let entry = audit_log::Entity::find()
        .filter(
            audit_log::Column::Action.eq(audit_service::AuditAction::AdminTestExternalAuthProvider),
        )
        .one(state.writer_db())
        .await
        .expect("audit log query should succeed")
        .expect("audit log should persist");

    assert_eq!(entry.user_id, 42);
    assert_eq!(entry.entity_type, "external_auth_provider");
    assert_eq!(entry.entity_name.as_deref(), Some("draft"));
    let details: Value = serde_json::from_str(entry.details.as_deref().unwrap()).unwrap();
    assert_eq!(details["provider_kind"], "oidc");
    assert_eq!(details["key"], "draft");
    assert_eq!(details["success"], true);
}

#[actix_web::test]
async fn admin_audit_logs_are_admin_only() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let _admin_token = setup_admin!(app);
    let user_token = register_user!(app, "audit-user", "audit-user@example.com", "password1234");

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs")
        .to_request();
    assert_service_status!(app, req, 401);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs")
        .insert_header(common::bearer_header(user_token))
        .to_request();
    assert_service_status!(app, req, 403);
}

#[actix_web::test]
async fn admin_can_list_filter_and_page_audit_logs() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);

    let _login_token = login_user!(app, "admin", "password1234");

    let req = test::TestRequest::put()
        .uri(&format!("/api/v1/admin/config/{BRANDING_TITLE_KEY}"))
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "value": "Audit Test Title"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs?limit=50&sort_by=created_at&sort_order=asc")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;

    let items = body["data"]["items"]
        .as_array()
        .expect("audit log items should be an array");
    assert!(body["data"]["total"].as_u64().unwrap() >= 3);
    assert_eq!(body["data"]["limit"], 50);
    assert_eq!(body["data"]["offset"], 0);

    let setup = find_action(items, "system_setup");
    assert_eq!(setup["entity_type"], "user");
    assert_eq!(setup["entity_name"], "admin");
    assert_eq!(setup["user"]["username"], "admin");

    let login = find_action(items, "user_login");
    assert_eq!(login["entity_type"], "auth_session");
    assert_eq!(login["entity_name"], "admin");
    assert_eq!(login["presentation"]["summary"]["code"], "user_login");
    assert_eq!(login["presentation"]["target"]["code"], "auth_session");
    assert_eq!(
        login["presentation"]["detail"]["code"],
        "user_login_identifier"
    );
    assert_eq!(
        login["presentation"]["detail"]["params"]["identifier"],
        "admin"
    );
    let login_details: Value = serde_json::from_str(login["details"].as_str().unwrap()).unwrap();
    assert_eq!(login_details["identifier"], "admin");

    let config = find_action(items, "config_update");
    assert_eq!(config["entity_type"], "system_config");
    assert_eq!(config["entity_name"], BRANDING_TITLE_KEY);
    assert_eq!(config["presentation"]["summary"]["code"], "config_update");
    assert_eq!(
        config["presentation"]["summary"]["params"]["key"],
        BRANDING_TITLE_KEY
    );
    assert_eq!(
        config["presentation"]["detail"]["code"],
        "config_value_updated"
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs?action=config_update&limit=1")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["items"][0]["action"], "config_update");
    assert_eq!(body["data"]["items"][0]["entity_name"], BRANDING_TITLE_KEY);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs?limit=9999")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["limit"], 200);
}

#[actix_web::test]
async fn admin_task_cleanup_records_audit_log() {
    let state = common::setup().await;
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let finished_before = chrono::Utc::now();

    let req = test::TestRequest::post()
        .uri("/api/v1/admin/tasks/cleanup")
        .insert_header(common::bearer_header(&token))
        .set_json(serde_json::json!({
            "finished_before": finished_before,
            "kind": "system_runtime",
            "status": "failed",
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs?action=admin_cleanup_tasks&limit=1")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["total"], 1);

    let cleanup_entry = &body["data"]["items"][0];
    assert_eq!(cleanup_entry["action"], "admin_cleanup_tasks");
    assert_eq!(cleanup_entry["entity_type"], "task");
    assert_eq!(
        cleanup_entry["presentation"]["detail"]["code"],
        "tasks_cleanup_finished"
    );
    let details: Value = serde_json::from_str(cleanup_entry["details"].as_str().unwrap()).unwrap();
    assert_eq!(details["removed"], 0);
    assert!(details["finished_before"].is_string());
    assert_eq!(details["kind"], "system_runtime");
    assert_eq!(details["status"], "failed");
}

#[actix_web::test]
async fn admin_task_retry_records_audit_log() {
    let state = common::setup().await;
    let state_for_insert = state.clone();
    let app = create_test_app!(state);
    let token = setup_admin!(app);
    let task_id = insert_failed_retryable_task(&state_for_insert, "Retry audit task", 2).await;

    let req = test::TestRequest::post()
        .uri(&format!("/api/v1/admin/tasks/{task_id}/retry"))
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/audit-logs?action=task_retry&limit=1")
        .insert_header(common::bearer_header(token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["total"], 1);

    let retry_entry = &body["data"]["items"][0];
    assert_eq!(retry_entry["action"], "task_retry");
    assert_eq!(retry_entry["entity_type"], "task");
    assert_eq!(retry_entry["entity_id"], task_id);
    assert_eq!(retry_entry["entity_name"], "Retry audit task");
    assert_eq!(
        retry_entry["presentation"]["detail"]["code"],
        "task_retry_scheduled"
    );
    let details: Value = serde_json::from_str(retry_entry["details"].as_str().unwrap()).unwrap();
    assert_eq!(details["kind"], "system_runtime");
    assert_eq!(details["previous_attempt_count"], 2);
}

#[actix_web::test]
async fn mail_outbox_dispatch_records_delivery_audit_logs() {
    let state = common::setup().await;
    let payload = aster_forge::services::mail_template::MailTemplatePayload::register_activation(
        "alice",
        "token-123",
        "AsterForge",
    )
    .to_stored()
    .expect("mail payload should serialize");
    let sent_row = mail_outbox_repo::create(state.writer_db(), mail_outbox_model(0, payload))
        .await
        .expect("mail outbox row should insert");

    let stats = mail_outbox_service::dispatch_due(&state)
        .await
        .expect("mail outbox dispatch should succeed");

    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.sent, 1);
    let sent_entry = latest_audit_entry(&state, audit_service::AuditAction::MailSend).await;
    assert_eq!(sent_entry.user_id, 0);
    assert_eq!(sent_entry.entity_type, "mail");
    assert_eq!(sent_entry.entity_id, Some(sent_row.id));
    assert_eq!(sent_entry.entity_name.as_deref(), Some("mail"));
    let sent_details: Value = serde_json::from_str(sent_entry.details.as_deref().unwrap()).unwrap();
    assert_eq!(sent_details["to_address"], "audit-mail@example.com");
    assert_eq!(sent_details["template_code"], "register_activation");
    assert_eq!(sent_details["to_name"], "Audit Mail");
    assert_eq!(sent_details["outbox_id"], sent_row.id);
    assert_eq!(sent_details["attempt_count"], 1);

    let payload = aster_forge::services::mail_template::MailTemplatePayload::register_activation(
        "alice",
        "token-456",
        "AsterForge",
    )
    .to_stored()
    .expect("mail payload should serialize");
    let failed_row = mail_outbox_repo::create(state.writer_db(), mail_outbox_model(5, payload))
        .await
        .expect("mail outbox row should insert");
    let sender: Arc<dyn MailSender> = Arc::new(FailingMailSender);

    let stats =
        mail_outbox_service::dispatch_due_with(state.writer_db(), &state.runtime_config, &sender)
            .await
            .expect("mail outbox dispatch should handle final delivery failure");

    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.failed, 1);
    let failed_entry =
        latest_audit_entry(&state, audit_service::AuditAction::MailDeliveryFailed).await;
    assert_eq!(failed_entry.user_id, 0);
    assert_eq!(failed_entry.entity_type, "mail");
    assert_eq!(failed_entry.entity_id, Some(failed_row.id));
    let failed_details: Value =
        serde_json::from_str(failed_entry.details.as_deref().unwrap()).unwrap();
    assert_eq!(failed_details["to_address"], "audit-mail@example.com");
    assert_eq!(failed_details["template_code"], "register_activation");
    assert_eq!(failed_details["outbox_id"], failed_row.id);
    assert_eq!(failed_details["attempt_count"], 6);
    assert_eq!(failed_details["error"], "smtp unavailable");
}
