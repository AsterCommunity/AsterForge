//! External authentication routes.

use crate::api::dto::{ExternalAuthCallbackQuery, StartExternalAuthReq, validate_request};
use crate::api::response::ApiResponse;
use crate::errors::Result;
use crate::runtime::AppState;
use crate::services::external_auth_service;
use actix_web::{HttpResponse, web};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/external-auth")
            .route("/providers", web::get().to(list_providers))
            .route("/{provider}/start", web::post().to(start_login))
            .route("/{provider}/callback", web::get().to(finish_login)),
    );
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/external-auth/providers",
    tag = "external-auth",
    operation_id = "list_external_auth_providers",
    responses(
        (status = 200, description = "Enabled external auth providers", body = inline(ApiResponse<Vec<external_auth_service::ExternalAuthPublicProvider>>)),
    ),
)]
pub async fn list_providers(state: web::Data<AppState>) -> Result<HttpResponse> {
    let providers = external_auth_service::list_public_providers(state.get_ref()).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(providers)))
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/external-auth/{provider}/start",
    tag = "external-auth",
    operation_id = "start_external_auth_login",
    params(("provider" = String, Path, description = "External auth provider slug")),
    request_body = StartExternalAuthReq,
    responses(
        (status = 200, description = "External auth authorization start response", body = inline(ApiResponse<external_auth_service::ExternalAuthStartLoginResponse>)),
        (status = 400, description = "Provider is misconfigured or request is invalid"),
        (status = 404, description = "Provider not found"),
    ),
)]
pub async fn start_login(
    state: web::Data<AppState>,
    provider: web::Path<String>,
    body: web::Json<StartExternalAuthReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let data =
        external_auth_service::start_login(state.get_ref(), &provider, &body.redirect_uri).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(data)))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/external-auth/{provider}/callback",
    tag = "external-auth",
    operation_id = "finish_external_auth_login",
    params(
        ("provider" = String, Path, description = "External auth provider slug"),
        ExternalAuthCallbackQuery,
    ),
    responses(
        (status = 200, description = "External auth callback accepted"),
        (status = 400, description = "Invalid callback or expired state"),
    ),
)]
pub async fn finish_login(
    state: web::Data<AppState>,
    _provider: web::Path<String>,
    query: web::Query<ExternalAuthCallbackQuery>,
) -> Result<HttpResponse> {
    validate_request(&*query)?;
    let data =
        external_auth_service::finish_login(state.get_ref(), &query.state, &query.code).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(data)))
}
