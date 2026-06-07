//! Administrator background task API routes.

use crate::api::dto::{
    AdminTaskCleanupReq, AdminTaskListQuery, RemovedCountResponse, validate_request,
};
use crate::api::pagination::LimitOffsetQuery;
#[cfg(all(debug_assertions, feature = "openapi"))]
use crate::api::pagination::OffsetPage;
use crate::api::response::ApiResponse;
use crate::errors::{AsterError, Result};
use crate::runtime::AppState;
use crate::services::auth_service::AuthUserInfo;
use crate::services::{audit_service, task_service};
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};

fn current_admin_user_id(req: &HttpRequest) -> Result<i64> {
    req.extensions()
        .get::<AuthUserInfo>()
        .map(|user| user.id)
        .ok_or_else(|| AsterError::internal_error("missing authenticated user in request context"))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/admin/tasks",
    tag = "admin",
    operation_id = "admin_list_tasks",
    params(LimitOffsetQuery, AdminTaskListQuery),
    responses(
        (status = 200, description = "Background tasks", body = inline(ApiResponse<OffsetPage<task_service::types::TaskInfo>>)),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer" = [])),
)]
pub async fn list_tasks(
    state: web::Data<AppState>,
    page: web::Query<LimitOffsetQuery>,
    query: web::Query<AdminTaskListQuery>,
) -> Result<HttpResponse> {
    let page = task_service::list_tasks_paginated_for_admin(
        state.get_ref(),
        page.limit_or(20, 100),
        page.offset(),
        task_service::AdminTaskListFilters {
            kind: query.kind,
            status: query.status,
        },
        query.sort_by(),
        query.sort_order(),
    )
    .await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(page)))
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/admin/tasks/{id}/retry",
    tag = "admin",
    operation_id = "admin_retry_task",
    params(("id" = i64, Path, description = "Background task ID")),
    responses(
        (status = 200, description = "Task reset for retry", body = inline(ApiResponse<task_service::types::TaskInfo>)),
        (status = 400, description = "Task cannot be retried"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Task not found"),
    ),
    security(("bearer" = [])),
)]
pub async fn retry_task(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<i64>,
) -> Result<HttpResponse> {
    let ctx = audit_service::AuditContext::from_request(&req, current_admin_user_id(&req)?);
    let task = task_service::retry_task_for_admin_with_audit(state.get_ref(), *path, &ctx).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(task)))
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/admin/tasks/cleanup",
    tag = "admin",
    operation_id = "admin_cleanup_tasks",
    request_body = AdminTaskCleanupReq,
    responses(
        (status = 200, description = "Completed tasks cleaned up", body = inline(ApiResponse<RemovedCountResponse>)),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer" = [])),
)]
pub async fn cleanup_tasks(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<AdminTaskCleanupReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let removed = task_service::cleanup_tasks_for_admin(
        state.get_ref(),
        task_service::AdminTaskCleanupFilters {
            finished_before: body.finished_before,
            kind: body.kind,
            status: body.status,
        },
    )
    .await?;
    let ctx = audit_service::AuditContext::from_request(&req, current_admin_user_id(&req)?);
    audit_service::log_with_details(
        state.get_ref(),
        &ctx,
        audit_service::AuditAction::AdminCleanupTasks,
        audit_service::AuditEntityType::Task,
        None,
        None,
        || {
            audit_service::details(audit_service::AdminTaskCleanupAuditDetails {
                removed,
                finished_before: body.finished_before,
                kind: body.kind,
                status: body.status,
            })
        },
    )
    .await;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(RemovedCountResponse { removed })))
}
