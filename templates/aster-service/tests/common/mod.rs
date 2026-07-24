//! Integration test helpers.

use aster_forge_cache::CacheConfig;
use aster_forge_test::temp::SqliteTestDatabase;

/// Builds a clean test [`AppState`](crate::runtime::AppState).
#[allow(dead_code)]
pub async fn setup() -> ({{crate_name}}::runtime::AppState, SqliteTestDatabase) {
    let database = SqliteTestDatabase::new("service-state");
    let mut config = {{crate_name}}::config::AppConfig::default();
    config.database.url = database.url().to_string();
    config.cache = CacheConfig::default();
    config.logging.file = String::new();

    let state = {{crate_name}}::runtime::assembly::prepare_state(config)
        .await
        .expect("runtime state should prepare");
    (state, database)
}

/// Creates the standard test Actix app.{% raw %}
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
