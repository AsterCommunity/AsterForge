//! API layer.

mod common;
pub mod dto;
pub mod error_code;
pub mod middleware;
#[cfg(all(debug_assertions, feature = "openapi"))]
pub mod openapi;
pub mod pagination;
pub mod request_auth;
pub mod response;
pub mod routes;

use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::scope("/api/v1").configure(routes::configure_api))
        .service(routes::health::routes());

    #[cfg(all(debug_assertions, feature = "openapi"))]
    configure_openapi(cfg);

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
