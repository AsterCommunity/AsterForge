//! API layer.
//!
//! Keep product DTOs, permissions, and response semantics here. Forge middleware and API helpers
//! should be called directly when they add reusable mechanics.

pub(crate) mod common;
pub mod http;
#[cfg(all(debug_assertions, feature = "openapi"))]
pub mod openapi;
pub mod response;
pub mod routes;

use actix_web::web;

/// Registers product routes.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::scope(routes::API_V1_PREFIX).configure(routes::configure_api))
        .service(routes::health::routes());

    #[cfg(all(debug_assertions, feature = "openapi"))]
    configure_openapi(cfg);

    // Frontend fallback is intentionally last so product and health routes keep API semantics.
    cfg.service(routes::frontend::routes());
}

#[cfg(all(debug_assertions, feature = "openapi"))]
fn configure_openapi(cfg: &mut web::ServiceConfig) {
    use actix_web::HttpResponse;
    use utoipa::OpenApi;
    use utoipa_swagger_ui::SwaggerUi;

    let spec = openapi::ApiDoc::openapi();
    let spec_clone = spec.clone();
    cfg.service(web::scope("/api-docs").route(
        "/openapi.json",
        web::get().to(move || {
            let spec = spec_clone.clone();
            async move { HttpResponse::Ok().json(spec) }
        }),
    ));
    cfg.service(SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", spec));
}
