//! Integration test helpers.

use aster_forge_cache::CacheConfig;

/// Builds a clean test [`AppState`](crate::runtime::AppState).
#[allow(dead_code)]
pub async fn setup() -> {{crate_name}}::runtime::AppState {
    let mut config = {{crate_name}}::config::AppConfig::default();
    config.database.url = format!("sqlite://{}?mode=rwc", unique_database_path().display());
    config.cache = CacheConfig::default();
    config.logging.file = String::new();

    {{crate_name}}::runtime::assembly::prepare_state(config)
        .await
        .expect("runtime state should prepare")
}

fn unique_database_path() -> std::path::PathBuf {
    static NEXT_DATABASE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let id = NEXT_DATABASE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}-test-{}-{id}-{nanos}.db",
        env!("CARGO_PKG_NAME"),
        std::process::id()
    ))
}

/// Creates the standard test Actix app.
{% raw %}
#[macro_export]
macro_rules! create_test_app {
    ($state:expr) => {{
        use actix_web::{App, test, web};

        let state = $state;
        test::init_service(
            App::new()
                .wrap(actix_web::middleware::Compress::default())
                .wrap(actix_web::middleware::Logger::default())
                .wrap(aster_forge_actix_middleware::request_id::RequestIdMiddleware)
                .wrap(aster_forge_actix_middleware::security_headers::default_headers())
                .wrap(aster_forge_actix_middleware::metrics::MetricsMiddleware)
                .app_data(web::Data::new(state.clone()))
                .app_data(web::Data::from(state.metrics.clone()))
                .configure({% endraw %}{{crate_name}}{% raw %}::api::configure),
        )
        .await
    }};
}{% endraw %}
