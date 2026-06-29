//! API route registration.

use actix_web::web;

pub mod frontend;
pub mod health;

/// Versioned product API prefix used by generated services.
pub const API_V1_PREFIX: &str = "/api/v1";

/// Registers product API routes under [`API_V1_PREFIX`].
///
/// Add product-owned route modules here. The default service keeps unknown API paths from falling
/// through to the SPA fallback registered at `/`.
pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.default_service(web::to(crate::api::common::api_not_found));
}
