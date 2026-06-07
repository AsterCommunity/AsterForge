//! Generic administrator API route registration.

use crate::api::middleware::{admin::RequireAdmin, auth::JwtAuth, rate_limit};
use crate::config::{NetworkTrustConfig, RateLimitConfig};
use actix_governor::Governor;
use actix_web::middleware::Condition;
use actix_web::web;

pub mod audit_logs;
pub mod config;
pub mod external_auth;
pub mod tasks;

pub use audit_logs::list_audit_logs;
pub use config::{config_schema, delete_config, get_config, list_config, set_config};
pub use external_auth::{
    create_external_auth_provider, delete_external_auth_provider, get_external_auth_provider,
    list_external_auth_provider_kinds, list_external_auth_providers, test_external_auth_provider,
    test_external_auth_provider_params, update_external_auth_provider,
};
pub use tasks::{cleanup_tasks, list_tasks, retry_task};

pub fn routes(
    rl: &RateLimitConfig,
    network_trust: &NetworkTrustConfig,
) -> impl actix_web::dev::HttpServiceFactory + use<> {
    let limiter = rate_limit::build_governor(&rl.write, &network_trust.trusted_proxies);

    web::scope("/admin")
        .wrap(Condition::new(rl.enabled, Governor::new(&limiter)))
        .service(
            web::scope("").wrap(JwtAuth).service(
                web::scope("")
                    .wrap(RequireAdmin)
                    .route("/audit-logs", web::get().to(list_audit_logs))
                    .route("/config", web::get().to(list_config))
                    .route("/config/schema", web::get().to(config_schema))
                    .route("/config/{key}", web::get().to(get_config))
                    .route("/config/{key}", web::put().to(set_config))
                    .route("/config/{key}", web::delete().to(delete_config))
                    .route("/tasks", web::get().to(list_tasks))
                    .route("/tasks/cleanup", web::post().to(cleanup_tasks))
                    .route("/tasks/{id}/retry", web::post().to(retry_task))
                    .route(
                        "/external-auth/provider-kinds",
                        web::get().to(list_external_auth_provider_kinds),
                    )
                    .route(
                        "/external-auth/providers",
                        web::get().to(list_external_auth_providers),
                    )
                    .route(
                        "/external-auth/providers",
                        web::post().to(create_external_auth_provider),
                    )
                    .route(
                        "/external-auth/providers/test",
                        web::post().to(test_external_auth_provider_params),
                    )
                    .route(
                        "/external-auth/providers/{id}",
                        web::get().to(get_external_auth_provider),
                    )
                    .route(
                        "/external-auth/providers/{id}",
                        web::patch().to(update_external_auth_provider),
                    )
                    .route(
                        "/external-auth/providers/{id}",
                        web::delete().to(delete_external_auth_provider),
                    )
                    .route(
                        "/external-auth/providers/{id}/test",
                        web::post().to(test_external_auth_provider),
                    ),
            ),
        )
}
