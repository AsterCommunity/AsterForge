//! Authentication routes.

mod cookies;

use crate::api::dto::{
    CheckResp, LoginReq, LogoutReq, LogoutResp, RefreshReq, RegisterReq, SetupReq, validate_request,
};
use crate::api::middleware::csrf::{self, RequestSourceMode};
use crate::api::request_auth::access_cookie_token;
use crate::api::response::ApiResponse;
use crate::config::auth_runtime::RuntimeAuthPolicy;
use crate::errors::{AsterError, Result};
use crate::runtime::{AppState, SharedRuntimeState};
use crate::services::auth_service;
use crate::utils::numbers::u64_to_i64;
use actix_web::{HttpRequest, HttpResponse, web};

use self::cookies::{
    REFRESH_COOKIE, build_access_cookie, build_csrf_cookie, build_refresh_cookie,
    clear_access_cookie, clear_csrf_cookie, clear_refresh_cookie,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/auth")
            .route("/check", web::get().to(check))
            .route("/setup", web::post().to(setup))
            .route("/register", web::post().to(register))
            .route("/login", web::post().to(login))
            .route("/refresh", web::post().to(refresh))
            .route("/logout", web::post().to(logout))
            .route("/me", web::get().to(me))
            .route("/sessions", web::get().to(sessions)),
    );
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/check",
    tag = "auth",
    operation_id = "check_auth_state",
    responses(
        (status = 200, description = "Authentication bootstrap state", body = inline(ApiResponse<CheckResp>)),
    ),
)]
pub async fn check(state: web::Data<AppState>) -> Result<HttpResponse> {
    let initialized =
        crate::db::repository::user_repo::count_all(state.get_ref().reader_db()).await? > 0;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(CheckResp { initialized })))
}

fn authenticated_response(
    state: &AppState,
    session: auth_service::AuthTokenBundle,
) -> Result<HttpResponse> {
    let auth_policy = RuntimeAuthPolicy::from_runtime_config(state.runtime_config());
    let secure = auth_policy.cookie_secure;
    let csrf_token = csrf::build_csrf_token();
    let access_ttl = u64_to_i64(auth_policy.access_token_ttl_secs, "access token ttl")?;
    let refresh_ttl = u64_to_i64(auth_policy.refresh_token_ttl_secs, "refresh token ttl")?;

    Ok(HttpResponse::Ok()
        .cookie(build_access_cookie(
            &session.access_token,
            access_ttl,
            secure,
        ))
        .cookie(build_refresh_cookie(
            &session.refresh_token,
            refresh_ttl,
            secure,
        ))
        .cookie(build_csrf_cookie(&csrf_token, refresh_ttl, secure))
        .json(ApiResponse::ok(session.response())))
}

fn refresh_token_from_request(
    req: &HttpRequest,
    body: Option<&web::Json<RefreshReq>>,
) -> Result<String> {
    req.cookie(REFRESH_COOKIE)
        .map(|cookie| cookie.value().to_string())
        .or_else(|| body.map(|body| body.refresh_token.clone()))
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| AsterError::auth_token_invalid("missing refresh token"))
}

fn logout_token_from_request(
    req: &HttpRequest,
    body: Option<&web::Json<LogoutReq>>,
) -> Option<String> {
    req.cookie(REFRESH_COOKIE)
        .map(|cookie| cookie.value().to_string())
        .or_else(|| body.map(|body| body.refresh_token.clone()))
        .filter(|token| !token.trim().is_empty())
}

fn ensure_cookie_write_allowed(state: &AppState, req: &HttpRequest) -> Result<()> {
    csrf::ensure_request_source_allowed(
        req,
        state.runtime_config(),
        RequestSourceMode::OptionalWhenPresent,
    )?;
    csrf::ensure_double_submit_token(req)
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/setup",
    tag = "auth",
    operation_id = "setup_first_admin",
    request_body = SetupReq,
    responses(
        (status = 200, description = "First admin account created and session cookies issued", body = inline(ApiResponse<auth_service::AuthTokenResponse>)),
        (status = 400, description = "System is already initialized or input is invalid"),
    ),
)]
pub async fn setup(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<SetupReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let data = auth_service::setup_first_admin(
        state.get_ref(),
        &body.username,
        &body.email,
        &body.password,
        &req,
    )
    .await?;
    authenticated_response(state.get_ref(), data)
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/register",
    tag = "auth",
    operation_id = "register",
    request_body = RegisterReq,
    responses(
        (status = 200, description = "User account created and session cookies issued", body = inline(ApiResponse<auth_service::AuthTokenResponse>)),
        (status = 400, description = "Input is invalid"),
        (status = 403, description = "Registration is disabled"),
    ),
)]
pub async fn register(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<RegisterReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let data = auth_service::register(
        state.get_ref(),
        &body.username,
        &body.email,
        &body.password,
        &req,
    )
    .await?;
    authenticated_response(state.get_ref(), data)
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    operation_id = "login",
    request_body = LoginReq,
    responses(
        (status = 200, description = "Session cookies issued", body = inline(ApiResponse<auth_service::AuthTokenResponse>)),
        (status = 401, description = "Invalid credentials"),
        (status = 403, description = "User is disabled"),
    ),
)]
pub async fn login(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<LoginReq>,
) -> Result<HttpResponse> {
    validate_request(&*body)?;
    let data = auth_service::login(state.get_ref(), &body.identifier, &body.password, &req).await?;
    authenticated_response(state.get_ref(), data)
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "auth",
    operation_id = "refresh_token",
    responses(
        (status = 200, description = "Fresh session cookies issued", body = inline(ApiResponse<auth_service::AuthTokenResponse>)),
        (status = 401, description = "Refresh token is invalid, expired, or stale"),
    ),
)]
pub async fn refresh(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: Option<web::Json<RefreshReq>>,
) -> Result<HttpResponse> {
    if let Some(body) = body.as_ref() {
        validate_request(&**body)?;
    }
    if req.cookie(REFRESH_COOKIE).is_some() {
        ensure_cookie_write_allowed(state.get_ref(), &req)?;
    }
    let refresh_token = refresh_token_from_request(&req, body.as_ref())?;
    let data = auth_service::refresh(state.get_ref(), &refresh_token, &req).await?;
    authenticated_response(state.get_ref(), data)
}

#[api_docs_macros::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    operation_id = "logout",
    responses(
        (status = 200, description = "Refresh token revocation result and auth cookies cleared", body = inline(ApiResponse<LogoutResp>)),
    ),
)]
pub async fn logout(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: Option<web::Json<LogoutReq>>,
) -> Result<HttpResponse> {
    if let Some(body) = body.as_ref() {
        validate_request(&**body)?;
    }
    if access_cookie_token(&req).is_some() || req.cookie(REFRESH_COOKIE).is_some() {
        ensure_cookie_write_allowed(state.get_ref(), &req)?;
    }

    let revoked = if let Some(refresh_token) = logout_token_from_request(&req, body.as_ref()) {
        auth_service::logout(state.get_ref(), &refresh_token, &req).await?
    } else {
        false
    };
    let secure =
        RuntimeAuthPolicy::from_runtime_config(state.get_ref().runtime_config()).cookie_secure;
    Ok(HttpResponse::Ok()
        .cookie(clear_access_cookie(secure))
        .cookie(clear_refresh_cookie(secure))
        .cookie(clear_csrf_cookie(secure))
        .json(ApiResponse::ok(LogoutResp { revoked })))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    operation_id = "get_current_user",
    responses(
        (status = 200, description = "Current authenticated user", body = inline(ApiResponse<auth_service::AuthUserInfo>)),
        (status = 401, description = "Missing or invalid access token"),
        (status = 403, description = "User is disabled"),
    ),
    security(("bearer" = [])),
)]
pub async fn me(state: web::Data<AppState>, req: HttpRequest) -> Result<HttpResponse> {
    let user = auth_service::current_user(state.get_ref(), &req).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(auth_service::AuthUserInfo::from(user))))
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/auth/sessions",
    tag = "auth",
    operation_id = "list_auth_sessions",
    responses(
        (status = 200, description = "Current user's sessions", body = inline(ApiResponse<Vec<crate::entities::auth_session::Model>>)),
        (status = 401, description = "Missing or invalid access token"),
        (status = 403, description = "User is disabled"),
    ),
    security(("bearer" = [])),
)]
pub async fn sessions(state: web::Data<AppState>, req: HttpRequest) -> Result<HttpResponse> {
    let user = auth_service::current_user(state.get_ref(), &req).await?;
    let sessions = auth_service::list_sessions(state.get_ref(), user.id).await?;
    Ok(HttpResponse::Ok().json(ApiResponse::ok(sessions)))
}
