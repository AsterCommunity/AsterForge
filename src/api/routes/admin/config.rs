//! Administrator config API routes.

use crate::api::dto::SetConfigReq;
use crate::api::pagination::LimitOffsetQuery;
#[cfg(all(debug_assertions, feature = "openapi"))]
use crate::api::pagination::OffsetPage;
use crate::api::response::ApiResponse;
use crate::errors::{AsterError, Result};
use crate::runtime::AppState;
use crate::services::auth_service::AuthUserInfo;
use crate::services::{audit_service, config_service};
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};

fn current_admin_user_id(req: &HttpRequest) -> Result<i64> {
    req.extensions()
        .get::<AuthUserInfo>()
        .map(|user| user.id)
        .ok_or_else(|| AsterError::internal_error("missing authenticated user in request context"))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/admin/config",
    tag = "admin",
    operation_id = "list_config",
    params(LimitOffsetQuery),
    responses(
        (status = 200, description = "List config entries", body = inline(ApiResponse<OffsetPage<config_service::SystemConfig>>)),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer" = [])),
)]
pub async fn list_config(
    state: web::Data<AppState>,
    query: web::Query<LimitOffsetQuery>,
) -> Result<HttpResponse> {
    let configs =
        config_service::list_paginated(state.get_ref(), query.limit_or(50, 100), query.offset())
            .await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(configs)))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/admin/config/schema",
    tag = "admin",
    operation_id = "config_schema",
    responses(
        (status = 200, description = "Config schema", body = inline(ApiResponse<Vec<config_service::ConfigSchemaItem>>)),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer" = [])),
)]
pub async fn config_schema() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(ApiResponse::ok(config_service::get_schema())))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/admin/config/{key}",
    tag = "admin",
    operation_id = "get_config",
    params(("key" = String, Path, description = "Config key")),
    responses(
        (status = 200, description = "Config entry", body = inline(ApiResponse<config_service::SystemConfig>)),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Config key not found"),
    ),
    security(("bearer" = [])),
)]
pub async fn get_config(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let config = config_service::get_by_key(state.get_ref(), &path).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(config)))
}

#[api_docs_macros::path(
    put,
    path = "/api/v1/admin/config/{key}",
    tag = "admin",
    operation_id = "set_config",
    params(("key" = String, Path, description = "Config key")),
    request_body = SetConfigReq,
    responses(
        (status = 200, description = "Config value set", body = inline(ApiResponse<config_service::SystemConfig>)),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer" = [])),
)]
pub async fn set_config(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<String>,
    body: web::Json<SetConfigReq>,
) -> Result<HttpResponse> {
    let user_id = current_admin_user_id(&req)?;
    let ctx = audit_service::AuditContext::from_request(&req, user_id);
    let config = config_service::set_with_audit_and_visibility(
        state.get_ref(),
        &path,
        &body.value,
        body.visibility,
        user_id,
        &ctx,
    )
    .await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(config)))
}

#[api_docs_macros::path(
    delete,
    path = "/api/v1/admin/config/{key}",
    tag = "admin",
    operation_id = "delete_config",
    params(("key" = String, Path, description = "Config key")),
    responses(
        (status = 200, description = "Config entry deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Config key not found"),
    ),
    security(("bearer" = [])),
)]
pub async fn delete_config(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let user_id = current_admin_user_id(&req)?;
    let ctx = audit_service::AuditContext::from_request(&req, user_id);
    config_service::delete_with_audit(state.get_ref(), &path, &ctx).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::<()>::ok_empty()))
}
