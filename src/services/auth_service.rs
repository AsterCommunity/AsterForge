//! Local authentication and session service.

use crate::db::repository::{auth_session_repo, user_repo};
use crate::entities::{auth_session, user};
use crate::errors::{AsterError, MapAsterErr, Result};
use crate::runtime::SharedRuntimeState;
use crate::services::audit_service;
use crate::types::{UserRole, UserStatus};
use crate::utils::email::normalize_email;
use crate::utils::hash::{hash_password, sha256_hex, verify_password};
use actix_web::HttpRequest;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuthUserInfo {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub role: UserRole,
    pub status: UserStatus,
}

impl From<user::Model> for AuthUserInfo {
    fn from(value: user::Model) -> Self {
        Self {
            id: value.id,
            username: value.username,
            email: value.email,
            role: value.role,
            status: value.status,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuthTokenResponse {
    pub expires_in: u64,
}

#[derive(Debug, Clone)]
pub struct AuthTokenBundle {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: AuthUserInfo,
}

impl AuthTokenBundle {
    pub fn response(&self) -> AuthTokenResponse {
        AuthTokenResponse {
            expires_in: self.expires_in,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AccessClaims {
    pub sub: i64,
    pub username: String,
    pub role: UserRole,
    pub session_version: i64,
    pub exp: usize,
    pub iat: usize,
}

pub async fn setup_first_admin<S: SharedRuntimeState>(
    state: &S,
    username: &str,
    email: &str,
    password: &str,
    req: &HttpRequest,
) -> Result<AuthTokenBundle> {
    if user_repo::count_all(state.writer_db()).await? > 0 {
        return Err(AsterError::validation_error(
            "system is already initialized",
        ));
    }
    let user = create_user(
        state.writer_db(),
        username,
        email,
        password,
        UserRole::Admin,
    )
    .await?;
    let response = issue_tokens(state, user.clone(), req).await?;
    let audit_ctx = audit_service::AuditContext::from_request(req, user.id);
    audit_service::log(
        state,
        &audit_ctx,
        audit_service::AuditAction::SystemSetup,
        audit_service::AuditEntityType::User,
        Some(user.id),
        Some(&user.username),
        None,
    )
    .await;
    Ok(response)
}

pub async fn register<S: SharedRuntimeState>(
    state: &S,
    username: &str,
    email: &str,
    password: &str,
    req: &HttpRequest,
) -> Result<AuthTokenBundle> {
    let auth_policy =
        crate::config::auth_runtime::RuntimeAuthPolicy::from_runtime_config(state.runtime_config());
    if !auth_policy.allow_user_registration {
        return Err(AsterError::auth_forbidden("registration is disabled"));
    }
    crate::config::local_email_policy::LocalEmailPolicy::from_runtime_config(
        state.runtime_config(),
    )
    .check(email)?;

    let role = if user_repo::count_all(state.writer_db()).await? == 0 {
        UserRole::Admin
    } else {
        UserRole::User
    };
    let user = create_user(state.writer_db(), username, email, password, role).await?;
    let response = issue_tokens(state, user.clone(), req).await?;
    let audit_ctx = audit_service::AuditContext::from_request(req, user.id);
    audit_service::log(
        state,
        &audit_ctx,
        audit_service::AuditAction::UserRegister,
        audit_service::AuditEntityType::User,
        Some(user.id),
        Some(&user.username),
        None,
    )
    .await;
    Ok(response)
}

pub async fn login<S: SharedRuntimeState>(
    state: &S,
    identifier: &str,
    password: &str,
    req: &HttpRequest,
) -> Result<AuthTokenBundle> {
    let Some(user) = user_repo::find_by_identifier(state.reader_db(), identifier).await? else {
        return Err(AsterError::auth_invalid_credentials("invalid credentials"));
    };
    if !user.status.is_active() {
        return Err(AsterError::auth_forbidden("user is disabled"));
    }
    if !verify_password(password, &user.password_hash)? {
        return Err(AsterError::auth_invalid_credentials("invalid credentials"));
    }
    let response = issue_tokens(state, user.clone(), req).await?;
    let audit_ctx = audit_service::AuditContext::from_request(req, user.id);
    audit_service::log(
        state,
        &audit_ctx,
        audit_service::AuditAction::UserLogin,
        audit_service::AuditEntityType::AuthSession,
        None,
        Some(&user.username),
        audit_service::details(audit_service::LoginAuditDetails { identifier }),
    )
    .await;
    Ok(response)
}

pub async fn refresh<S: SharedRuntimeState>(
    state: &S,
    refresh_token: &str,
    req: &HttpRequest,
) -> Result<AuthTokenBundle> {
    let hash = sha256_hex(refresh_token.as_bytes());
    let Some(session) =
        auth_session_repo::find_active_by_refresh_hash(state.writer_db(), &hash).await?
    else {
        return Err(AsterError::auth_token_invalid("invalid refresh token"));
    };
    if session.expires_at <= Utc::now() {
        return Err(AsterError::auth_token_expired("refresh token expired"));
    }

    auth_session_repo::revoke_by_refresh_hash(state.writer_db(), &hash).await?;
    let user = user_repo::find_by_id(state.reader_db(), session.user_id).await?;
    if session.session_version != user.session_version {
        return Err(AsterError::auth_token_invalid("refresh token is stale"));
    }
    let response = issue_tokens(state, user.clone(), req).await?;
    let audit_ctx = audit_service::AuditContext::from_request(req, user.id);
    audit_service::log(
        state,
        &audit_ctx,
        audit_service::AuditAction::UserRefreshToken,
        audit_service::AuditEntityType::AuthSession,
        Some(session.id),
        Some(&user.username),
        None,
    )
    .await;
    Ok(response)
}

pub async fn logout<S: SharedRuntimeState>(
    state: &S,
    refresh_token: &str,
    req: &HttpRequest,
) -> Result<bool> {
    let hash = sha256_hex(refresh_token.as_bytes());
    let Some(session) =
        auth_session_repo::find_active_by_refresh_hash(state.writer_db(), &hash).await?
    else {
        return Ok(false);
    };

    let revoked = auth_session_repo::revoke_by_refresh_hash(state.writer_db(), &hash).await?;
    if revoked {
        let audit_ctx = audit_service::AuditContext::from_request(req, session.user_id);
        audit_service::log(
            state,
            &audit_ctx,
            audit_service::AuditAction::UserLogout,
            audit_service::AuditEntityType::AuthSession,
            Some(session.id),
            None,
            None,
        )
        .await;
    }
    Ok(revoked)
}

pub async fn current_user<S: SharedRuntimeState>(
    state: &S,
    req: &HttpRequest,
) -> Result<user::Model> {
    let token = crate::api::request_auth::access_token(req)
        .ok_or_else(|| AsterError::auth_token_invalid("missing access token"))?;
    current_user_from_token(state, &token).await
}

pub async fn current_user_from_token<S: SharedRuntimeState>(
    state: &S,
    token: &str,
) -> Result<user::Model> {
    let claims = decode_access_claims(state, token)?;
    let user = user_repo::find_by_id(state.reader_db(), claims.sub).await?;
    if !user.status.is_active() {
        return Err(AsterError::auth_forbidden("user is disabled"));
    }
    if user.session_version != claims.session_version {
        return Err(AsterError::auth_token_invalid("session is stale"));
    }
    Ok(user)
}

pub async fn list_sessions<S: SharedRuntimeState>(
    state: &S,
    user_id: i64,
) -> Result<Vec<auth_session::Model>> {
    auth_session_repo::list_by_user(state.reader_db(), user_id).await
}

pub async fn cleanup_expired_auth_sessions<S: SharedRuntimeState>(state: &S) -> Result<u64> {
    auth_session_repo::delete_expired(state.writer_db(), Utc::now()).await
}

async fn create_user<C: ConnectionTrait>(
    db: &C,
    username: &str,
    email: &str,
    password: &str,
    role: UserRole,
) -> Result<user::Model> {
    let email = validate_identity_input(username, email, password)?;
    let password_hash = hash_password(password)?;
    user_repo::create(db, username, &email, &password_hash, role).await
}

pub fn validate_username(username: &str) -> Result<()> {
    if username.trim().len() < 3 {
        return Err(AsterError::validation_error(
            "username must contain at least 3 characters",
        ));
    }
    Ok(())
}

pub fn validate_email(email: &str) -> Result<()> {
    crate::utils::email::normalize_email(email).map(|_| ())
}

pub fn validate_password(password: &str) -> Result<()> {
    if password.len() < 10 {
        return Err(AsterError::validation_error(
            "password must contain at least 10 characters",
        ));
    }
    Ok(())
}

fn validate_identity_input(username: &str, email: &str, password: &str) -> Result<String> {
    validate_username(username)?;
    let email = normalize_email(email)?;
    validate_password(password)?;
    Ok(email)
}

async fn issue_tokens<S: SharedRuntimeState>(
    state: &S,
    user: user::Model,
    req: &HttpRequest,
) -> Result<AuthTokenBundle> {
    let now = Utc::now();
    let auth_policy =
        crate::config::auth_runtime::RuntimeAuthPolicy::from_runtime_config(state.runtime_config());
    let access_expires = now
        + Duration::seconds(
            i64::try_from(auth_policy.access_token_ttl_secs)
                .map_err(|_| AsterError::config_error("access token ttl is too large"))?,
        );
    let refresh_expires = now
        + Duration::seconds(
            i64::try_from(auth_policy.refresh_token_ttl_secs)
                .map_err(|_| AsterError::config_error("refresh token ttl is too large"))?,
        );

    let claims = AccessClaims {
        sub: user.id,
        username: user.username.clone(),
        role: user.role,
        session_version: user.session_version,
        exp: usize::try_from(access_expires.timestamp())
            .map_err(|_| AsterError::internal_error("access token timestamp overflow"))?,
        iat: usize::try_from(now.timestamp())
            .map_err(|_| AsterError::internal_error("issued-at timestamp overflow"))?,
    };
    let access_token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.config().auth.jwt_secret.as_bytes()),
    )
    .map_aster_err(AsterError::auth_token_invalid)?;

    let refresh_token = Uuid::new_v4().to_string();
    auth_session_repo::create(
        state.writer_db(),
        user.id,
        &sha256_hex(refresh_token.as_bytes()),
        user.session_version,
        refresh_expires,
        user_agent(req),
        peer_ip(req),
    )
    .await?;

    Ok(AuthTokenBundle {
        access_token,
        refresh_token,
        expires_in: auth_policy.access_token_ttl_secs,
        user: AuthUserInfo::from(user),
    })
}

fn decode_access_claims<S: SharedRuntimeState>(state: &S, token: &str) -> Result<AccessClaims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    decode::<AccessClaims>(
        token,
        &DecodingKey::from_secret(state.config().auth.jwt_secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(AsterError::from)
}

fn user_agent(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get(actix_web::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn peer_ip(req: &HttpRequest) -> Option<String> {
    req.peer_addr().map(|addr| addr.ip().to_string())
}
