//! PostgreSQL / MySQL smoke tests backed by testcontainers.

#[macro_use]
mod common;

use actix_web::test;
use aster_forge::config::definitions::BRANDING_TITLE_KEY;
use aster_forge::db::repository::{background_task_repo, system_config_repo, user_repo};
use aster_forge::entities::background_task;
use aster_forge::runtime::SharedRuntimeState;
use aster_forge::types::{
    BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload, StoredTaskResult,
};
use chrono::{Duration, Utc};
use sea_orm::{ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use tokio::time::timeout;

const OLD_BACKGROUND_TASK_DISPLAY_NAME_LIMIT: usize = 255;
const EXPANDED_BACKGROUND_TASK_DISPLAY_NAME_LIMIT: usize = 512;

async fn assert_core_tables_exist(db: &DatabaseConnection, backend: DbBackend) {
    let table_names = match backend {
        DbBackend::Postgres => {
            let rows = db
                .query_all_raw(Statement::from_string(
                    backend,
                    "SELECT table_name \
                     FROM information_schema.tables \
                     WHERE table_schema = 'public' \
                       AND table_type = 'BASE TABLE'",
                ))
                .await
                .expect("postgres table list should query");
            rows.into_iter()
                .map(|row| row.try_get_by_index(0).expect("table name should exist"))
                .collect::<Vec<String>>()
        }
        DbBackend::MySql => {
            let rows = db
                .query_all_raw(Statement::from_string(
                    backend,
                    "SELECT table_name \
                     FROM information_schema.tables \
                     WHERE table_schema = DATABASE() \
                       AND table_type = 'BASE TABLE'",
                ))
                .await
                .expect("mysql table list should query");
            rows.into_iter()
                .map(|row| row.try_get_by_index(0).expect("table name should exist"))
                .collect::<Vec<String>>()
        }
        backend => panic!("unsupported test database backend: {backend:?}"),
    };

    for expected in [
        "users",
        "auth_sessions",
        "external_auth_providers",
        "external_auth_identities",
        "external_auth_login_flows",
        "system_config",
        "audit_logs",
        "background_tasks",
        "seaql_migrations",
    ] {
        assert!(
            table_names.iter().any(|name| name == expected),
            "{expected} table should exist in {table_names:?}"
        );
    }
}

async fn assert_mysql_datetime_columns(db: &DatabaseConnection) {
    let timestamp_count = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_NAME <> 'seaql_migrations' \
               AND DATA_TYPE = 'timestamp'",
        ))
        .await
        .expect("mysql timestamp count should query")
        .expect("mysql timestamp count should return one row");
    let timestamp_count: i64 = timestamp_count
        .try_get_by_index(0)
        .expect("mysql timestamp count should decode");
    assert_eq!(
        timestamp_count, 0,
        "application tables should use DATETIME instead of TIMESTAMP on MySQL"
    );

    let datetime_count = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_NAME <> 'seaql_migrations' \
               AND DATA_TYPE = 'datetime' \
               AND DATETIME_PRECISION = 6",
        ))
        .await
        .expect("mysql datetime count should query")
        .expect("mysql datetime count should return one row");
    let datetime_count: i64 = datetime_count
        .try_get_by_index(0)
        .expect("mysql datetime count should decode");
    assert!(
        datetime_count >= 20,
        "foundation timestamp columns should be datetime(6), got {datetime_count}"
    );
}

async fn assert_background_task_display_name_column_len(
    db: &DatabaseConnection,
    backend: DbBackend,
) {
    let sql = match backend {
        DbBackend::Postgres => {
            "SELECT character_maximum_length::bigint \
             FROM information_schema.columns \
             WHERE table_schema = 'public' \
               AND table_name = 'background_tasks' \
               AND column_name = 'display_name'"
        }
        DbBackend::MySql => {
            "SELECT CAST(CHARACTER_MAXIMUM_LENGTH AS SIGNED) \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_NAME = 'background_tasks' \
               AND COLUMN_NAME = 'display_name'"
        }
        backend => panic!("unsupported test database backend: {backend:?}"),
    };

    let row = db
        .query_one_raw(Statement::from_string(backend, sql))
        .await
        .expect("background_tasks.display_name length should query")
        .expect("background_tasks.display_name column should exist");
    let max_len: i64 = row
        .try_get_by_index(0)
        .expect("background_tasks.display_name max length should decode");
    assert_eq!(
        max_len,
        i64::try_from(EXPANDED_BACKGROUND_TASK_DISPLAY_NAME_LIMIT).unwrap()
    );
}

async fn assert_background_task_display_name_accepts_expanded_len(db: &DatabaseConnection) {
    let now = Utc::now();
    let display_name = "x".repeat(OLD_BACKGROUND_TASK_DISPLAY_NAME_LIMIT + 1);
    assert!(display_name.len() <= EXPANDED_BACKGROUND_TASK_DISPLAY_NAME_LIMIT);

    let task = background_task_repo::create(
        db,
        background_task::ActiveModel {
            kind: Set(BackgroundTaskKind::SystemRuntime),
            status: Set(BackgroundTaskStatus::Succeeded),
            creator_user_id: Set(None),
            display_name: Set(display_name.clone()),
            payload_json: Set(StoredTaskPayload(
                r#"{"task_name":"expanded-display-name-smoke"}"#.to_string(),
            )),
            result_json: Set(Some(StoredTaskResult(
                r#"{"duration_ms":0,"summary":"expanded display name accepted"}"#.to_string(),
            ))),
            runtime_json: Set(None),
            steps_json: Set(None),
            progress_current: Set(1),
            progress_total: Set(1),
            status_text: Set(Some("expanded display name accepted".to_string())),
            attempt_count: Set(0),
            max_attempts: Set(1),
            next_run_at: Set(now),
            processing_token: Set(0),
            processing_started_at: Set(None),
            last_heartbeat_at: Set(None),
            lease_expires_at: Set(None),
            started_at: Set(Some(now)),
            finished_at: Set(Some(now)),
            last_error: Set(None),
            failure_can_retry: Set(None),
            expires_at: Set(now + Duration::hours(1)),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        },
    )
    .await
    .expect("expanded background task display_name should insert");

    assert_eq!(task.display_name, display_name);
}

async fn exercise_foundation_api_smoke(database_url: &str, backend: DbBackend) {
    let state = common::setup_with_database_url(database_url).await;
    assert_eq!(state.writer_db().get_database_backend(), backend);

    assert_core_tables_exist(state.writer_db(), backend).await;
    if backend == DbBackend::MySql {
        assert_mysql_datetime_columns(state.writer_db()).await;
    }
    assert_background_task_display_name_column_len(state.writer_db(), backend).await;
    assert_background_task_display_name_accepts_expanded_len(state.writer_db()).await;

    let configs = system_config_repo::find_all(state.writer_db())
        .await
        .expect("system config defaults should query");
    assert!(
        configs.iter().any(|item| item.key == BRANDING_TITLE_KEY),
        "{BRANDING_TITLE_KEY} default config should be seeded"
    );
    assert_eq!(
        user_repo::count_all(state.writer_db())
            .await
            .expect("user count should query"),
        0
    );

    let app = create_test_app!(state);

    let req = test::TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["status"], "ok");

    let req = test::TestRequest::get().uri("/health/ready").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["status"], "ready");

    let token = setup_admin!(app);

    let req = test::TestRequest::get()
        .uri("/api/v1/system/info")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["data"]["name"], "AsterForge");
    assert_eq!(body["data"]["site_title"], "AsterForge");

    let req = test::TestRequest::get()
        .uri("/api/v1/examples/public")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(
        body["data"]["message"],
        "AsterForge public example API is working"
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/examples/protected")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert_eq!(
        body["data"]["message"],
        "AsterForge protected example API is working"
    );
    assert_eq!(body["data"]["user"]["username"], "admin");

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/config")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["data"]["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let req = test::TestRequest::get()
        .uri("/api/v1/admin/tasks?limit=10")
        .insert_header(common::bearer_header(&token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body: Value = test::read_body_json(resp).await;
    assert!(
        body["data"]["total"]
            .as_u64()
            .is_some_and(|total| total >= 1),
        "admin task list should include the inserted smoke task: {body}"
    );
}

#[actix_web::test]
async fn test_sqlite_transactions_are_serialized_by_single_connection_pool() {
    use sea_orm::TransactionTrait;

    let database_path = format!("/tmp/asterforge-sqlite-lock-{}.db", uuid::Uuid::new_v4());
    let database_url = format!("sqlite://{database_path}");
    let cfg = aster_forge::config::DatabaseConfig {
        url: database_url,
        pool_size: 8,
        retry_count: 0,
    };
    let db =
        aster_forge::db::connect_with_metrics(&cfg, aster_forge::metrics_core::NoopMetrics::arc())
            .await
            .expect("sqlite lock smoke database should connect");

    let txn = db
        .begin()
        .await
        .expect("first sqlite transaction should start");
    let second_begin = timeout(std::time::Duration::from_millis(100), db.begin()).await;
    assert!(
        second_begin.is_err(),
        "SQLite should serialize transactions by exposing only one pooled connection"
    );

    txn.commit()
        .await
        .expect("first sqlite transaction should commit");

    let second_txn = timeout(std::time::Duration::from_secs(1), db.begin())
        .await
        .expect("second transaction should start after the first commit")
        .expect("second sqlite transaction should start");
    second_txn
        .commit()
        .await
        .expect("second sqlite transaction should commit");

    let _ = tokio::fs::remove_file(database_path).await;
}

#[actix_web::test]
async fn test_postgres_smoke_foundation_contracts() {
    let database_url = common::postgres_test_database_url().await;

    exercise_foundation_api_smoke(&database_url, DbBackend::Postgres).await;
}

#[actix_web::test]
async fn test_mysql_smoke_foundation_contracts() {
    let database_url = common::mysql_test_database_url().await;

    exercise_foundation_api_smoke(&database_url, DbBackend::MySql).await;
}
