//! Shared API route helpers.

use actix_web::HttpResponse;

use crate::api::response::ErrorResponse;

pub(super) async fn api_not_found() -> HttpResponse {
    match ErrorResponse::endpoint_not_found() {
        Ok(error) => HttpResponse::NotFound().json(error),
        Err(error) => {
            tracing::error!(%error, "invalid product response code classification");
            HttpResponse::InternalServerError().finish()
        }
    }
}
