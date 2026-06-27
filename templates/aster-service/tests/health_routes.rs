//! HTTP health route integration tests.

use actix_web::{App, http::StatusCode, test, web};
use aster_forge_cache::CacheConfig;

#[actix_web::test]
async fn health_and_ready_routes_return_ok() {
    let state = prepare_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure({{crate_name}}::api::configure),
    )
    .await;

    let health =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;
    assert_eq!(health.status(), StatusCode::OK);

    let ready =
        test::call_service(&app, test::TestRequest::get().uri("/readyz").to_request()).await;
    assert_eq!(ready.status(), StatusCode::OK);
}

#[cfg(feature = "metrics")]
#[actix_web::test]
async fn metrics_route_exports_prometheus_text() {
    let state = prepare_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure({{crate_name}}::api::configure),
    )
    .await;

    let response =
        test::call_service(&app, test::TestRequest::get().uri("/metrics").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = test::read_body(response).await;
    let body = std::str::from_utf8(&body).expect("metrics body should be utf-8");
    assert!(body.contains("db_queries_total"));
    assert!(body.contains("health_report_status"));
    assert!(body.contains("background_tasks_pending"));
    assert!(body.contains("process_uptime_seconds"));
    assert!(body.contains("process_heap_allocated_mib"));
}

async fn prepare_state() -> {{crate_name}}::runtime::AppState {
    let mut config = {{crate_name}}::config::AppConfig::default();
    config.database.url = format!("sqlite://{}?mode=rwc", unique_database_path().display());
    config.cache = CacheConfig::default();
    config.logging.file = String::new();

    {{crate_name}}::runtime::assembly::prepare_state(config)
        .await
        .expect("runtime state should prepare")
}

fn unique_database_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{{project-name}}-health-test-{nanos}.db"
    ))
}
