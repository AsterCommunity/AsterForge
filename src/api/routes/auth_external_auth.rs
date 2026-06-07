//! External auth aliases under `/auth/external-auth`.

use crate::api::dto::{ExternalAuthCallbackQuery, StartExternalAuthReq, validate_request};
use crate::api::response::ApiResponse;
use crate::errors::{AsterError, Result};
use crate::runtime::AppState;
use crate::services::external_auth_service;
use crate::types::ExternalAuthKind;
use actix_web::{HttpResponse, web};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/auth/external-auth")
            .route("/providers", web::get().to(list_providers))
            .route("/{kind}/providers", web::get().to(list_providers_by_kind))
            .route("/{kind}/{provider}/start", web::post().to(start_login))
            .route("/{kind}/{provider}/callback", web::get().to(finish_login)),
    );
}

fn parse_kind(value: &str) -> Result<ExternalAuthKind> {
    ExternalAuthKind::parse(value).ok_or_else(|| {
        AsterError::record_not_found(format!("external auth provider kind '{value}'"))
    })
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/external-auth/providers",
    tag = "external-auth",
    operation_id = "auth_external_auth_list_providers",
    responses(
        (status = 200, description = "Enabled external auth providers", body = inline(ApiResponse<Vec<external_auth_service::ExternalAuthPublicProvider>>)),
    ),
)]
pub async fn list_providers(state: web::Data<AppState>) -> Result<HttpResponse> {
    let providers = external_auth_service::list_public_providers(state.get_ref()).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(providers)))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/external-auth/{kind}/providers",
    tag = "external-auth",
    operation_id = "auth_external_auth_list_providers_by_kind",
    params(("kind" = ExternalAuthKind, Path, description = "External auth provider kind")),
    responses(
        (status = 200, description = "Enabled external auth providers for kind", body = inline(ApiResponse<Vec<external_auth_service::ExternalAuthPublicProvider>>)),
        (status = 404, description = "Provider kind not found"),
    ),
)]
pub async fn list_providers_by_kind(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let kind = parse_kind(&path)?;
    let providers =
        external_auth_service::list_public_providers_by_kind(state.get_ref(), kind).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(providers)))
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/external-auth/{kind}/{provider}/start",
    tag = "external-auth",
    operation_id = "auth_external_auth_start_login",
    params(
        ("kind" = ExternalAuthKind, Path, description = "External auth provider kind"),
        ("provider" = String, Path, description = "External auth provider slug"),
    ),
    request_body = StartExternalAuthReq,
    responses(
        (status = 200, description = "External auth authorization start response", body = inline(ApiResponse<external_auth_service::ExternalAuthStartLoginResponse>)),
        (status = 400, description = "Provider is misconfigured or request is invalid"),
        (status = 404, description = "Provider not found"),
    ),
)]
pub async fn start_login(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
    body: web::Json<StartExternalAuthReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let (kind, provider) = path.into_inner();
    let kind = parse_kind(&kind)?;
    ensure_provider_kind(state.get_ref(), kind, &provider).await?;
    let data =
        external_auth_service::start_login(state.get_ref(), &provider, &body.redirect_uri).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(data)))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/external-auth/{kind}/{provider}/callback",
    tag = "external-auth",
    operation_id = "auth_external_auth_finish_login",
    params(
        ("kind" = ExternalAuthKind, Path, description = "External auth provider kind"),
        ("provider" = String, Path, description = "External auth provider slug"),
        ExternalAuthCallbackQuery,
    ),
    responses(
        (status = 200, description = "External auth callback accepted", body = inline(ApiResponse<external_auth_service::ExternalAuthFinishLoginResponse>)),
        (status = 400, description = "Invalid callback or expired state"),
        (status = 404, description = "Provider not found"),
    ),
)]
pub async fn finish_login(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
    query: web::Query<ExternalAuthCallbackQuery>,
) -> Result<HttpResponse> {
    validate_request(&*query)?;
    let (kind, provider) = path.into_inner();
    let kind = parse_kind(&kind)?;
    ensure_provider_kind(state.get_ref(), kind, &provider).await?;
    let data =
        external_auth_service::finish_login(state.get_ref(), &query.state, &query.code).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(data)))
}

async fn ensure_provider_kind(
    state: &AppState,
    kind: ExternalAuthKind,
    provider: &str,
) -> Result<()> {
    let providers = external_auth_service::list_public_providers_by_kind(state, kind).await?;
    if providers.iter().any(|item| item.slug == provider) {
        return Ok(());
    }
    Err(AsterError::record_not_found(format!(
        "external auth provider {provider}"
    )))
}
