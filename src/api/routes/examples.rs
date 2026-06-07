//! Template example API routes.

use crate::api::response::ApiResponse;
use crate::errors::Result;
use crate::runtime::AppState;
use crate::services::{auth_service, example_service};
use actix_web::{HttpRequest, HttpResponse, web};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/examples")
            .route("/public", web::get().to(public_example))
            .route("/protected", web::get().to(protected_example)),
    );
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/examples/public",
    tag = "examples",
    operation_id = "public_example",
    responses(
        (status = 200, description = "Public example response", body = inline(ApiResponse<example_service::ExampleMessage>)),
    ),
)]
pub async fn public_example() -> HttpResponse {
    HttpResponse::Ok().json(ApiResponse::ok(example_service::public_message()))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/examples/protected",
    tag = "examples",
    operation_id = "protected_example",
    responses(
        (status = 200, description = "Protected example response", body = inline(ApiResponse<example_service::ProtectedExampleMessage>)),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "User is disabled"),
    ),
    security(("bearer" = [])),
)]
pub async fn protected_example(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse> {
    let user = auth_service::current_user(state.get_ref(), &req).await?;
    Ok(
        HttpResponse::Ok().json(ApiResponse::ok(example_service::protected_message(
            auth_service::AuthUserInfo::from(user),
        ))),
    )
}
