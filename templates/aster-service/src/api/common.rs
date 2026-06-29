//! Shared API route helpers.

use actix_web::HttpResponse;

use crate::api::response::ErrorResponse;

pub(super) async fn api_not_found() -> HttpResponse {
    HttpResponse::NotFound().json(ErrorResponse {
        service: env!("CARGO_PKG_NAME"),
        code: "endpoint_not_found",
        message: "endpoint not found",
    })
}
