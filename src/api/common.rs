use super::error_code::AsterErrorCode;
use super::response::ApiResponse;
use actix_web::HttpResponse;

pub(super) async fn api_not_found() -> HttpResponse {
    HttpResponse::NotFound().json(ApiResponse::<()>::error(
        AsterErrorCode::EndpointNotFound,
        "endpoint not found",
    ))
}
