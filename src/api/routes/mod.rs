//! API route registration.

pub mod admin;
pub mod auth;
pub mod auth_external_auth;
pub mod examples;
pub mod external_auth;
pub mod frontend;
pub mod health;
pub mod system;

use actix_web::web;

pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.configure(auth_external_auth::configure)
        .configure(auth::configure)
        .configure(external_auth::configure)
        .service(admin::routes(
            &crate::config::get_config().rate_limit,
            &crate::config::get_config().network_trust,
        ))
        .configure(examples::configure)
        .configure(system::configure)
        .default_service(web::to(crate::api::common::api_not_found));
}
