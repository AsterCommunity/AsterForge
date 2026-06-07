use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, IntoActiveModel};
use url::Url;

use crate::api::pagination::{OffsetPage, load_offset_page};
use crate::db::repository::external_auth_repo;
use crate::entities::external_auth_provider;
use crate::errors::{AsterError, Result};
use crate::runtime::SharedRuntimeState;
use crate::types::ExternalAuthKind;
use crate::utils::id;

use super::{
    AdminExternalAuthProviderInfo, CreateExternalAuthProviderInput, DEFAULT_OAUTH2_SCOPES,
    DEFAULT_OIDC_SCOPES, ExternalAuthFinishLoginResponse, ExternalAuthProviderKindInfo,
    ExternalAuthProviderTestCheck, ExternalAuthProviderTestParamsInput,
    ExternalAuthProviderTestResult, ExternalAuthPublicProvider, ExternalAuthStartLoginResponse,
    FLOW_TTL_SECS, REDACTED_SECRET, UpdateExternalAuthProviderInput,
};

fn default_scopes(kind: ExternalAuthKind) -> &'static str {
    match kind {
        ExternalAuthKind::Oidc => DEFAULT_OIDC_SCOPES,
        ExternalAuthKind::Oauth2 => DEFAULT_OAUTH2_SCOPES,
    }
}

fn protocol(kind: ExternalAuthKind) -> &'static str {
    kind.as_str()
}

fn provider_kind_info(kind: ExternalAuthKind) -> ExternalAuthProviderKindInfo {
    match kind {
        ExternalAuthKind::Oidc => ExternalAuthProviderKindInfo {
            kind,
            protocol: protocol(kind).to_string(),
            display_name: "OpenID Connect".to_string(),
            description: "Generic OpenID Connect provider".to_string(),
            default_scopes: default_scopes(kind).to_string(),
            issuer_url_required: false,
            authorization_url_required: true,
            token_url_required: true,
            userinfo_url_required: false,
            supports_discovery: true,
        },
        ExternalAuthKind::Oauth2 => ExternalAuthProviderKindInfo {
            kind,
            protocol: protocol(kind).to_string(),
            display_name: "OAuth 2.0".to_string(),
            description: "Generic OAuth 2.0 provider".to_string(),
            default_scopes: default_scopes(kind).to_string(),
            issuer_url_required: false,
            authorization_url_required: true,
            token_url_required: true,
            userinfo_url_required: false,
            supports_discovery: false,
        },
    }
}

fn provider_to_public(model: external_auth_provider::Model) -> ExternalAuthPublicProvider {
    ExternalAuthPublicProvider {
        key: model.slug.clone(),
        slug: model.slug,
        display_name: model.display_name,
        kind: model.kind,
    }
}

fn provider_to_admin(model: external_auth_provider::Model) -> AdminExternalAuthProviderInfo {
    let secret_configured = !model.client_secret.is_empty();
    AdminExternalAuthProviderInfo {
        id: model.id,
        key: model.slug.clone(),
        slug: model.slug,
        kind: model.kind,
        provider_kind: model.kind,
        protocol: protocol(model.kind).to_string(),
        display_name: model.display_name,
        issuer_url: model.issuer_url,
        authorization_url: model.authorize_url.clone(),
        authorize_url: model.authorize_url,
        token_url: model.token_url,
        userinfo_url: model.userinfo_url,
        client_id: model.client_id,
        client_secret: secret_configured.then(|| REDACTED_SECRET.to_string()),
        client_secret_configured: secret_configured,
        scopes: model.scopes,
        enabled: model.enabled,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn resolve_kind(
    kind: Option<ExternalAuthKind>,
    provider_kind: Option<ExternalAuthKind>,
) -> ExternalAuthKind {
    kind.or(provider_kind).unwrap_or(ExternalAuthKind::Oidc)
}

fn required_string(value: &str, field: &str, max_len: usize) -> Result<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(AsterError::validation_error(format!("{field} is required")));
    }
    if normalized.len() > max_len {
        return Err(AsterError::validation_error(format!(
            "{field} must not exceed {max_len} bytes"
        )));
    }
    Ok(normalized.to_string())
}

fn optional_url(value: Option<String>, field: &str) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = Url::parse(value).map_err(|error| {
        AsterError::validation_error(format!("{field} must be a valid URL: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AsterError::validation_error(format!(
            "{field} must use http or https"
        )));
    }
    Ok(Some(parsed.to_string()))
}

fn normalize_slug(value: &str) -> Result<String> {
    let value = required_string(value, "slug", 96)?;
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(AsterError::validation_error(
            "slug may only contain ASCII letters, digits, '.', '_' and '-'",
        ));
    }
    Ok(value)
}

fn normalize_scopes(value: Option<String>, kind: ExternalAuthKind) -> String {
    value
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_scopes(kind).to_string())
}

async fn generate_unique_slug(state: &impl SharedRuntimeState) -> Result<String> {
    let uuid = id::new_best_effort_uuid("external auth provider slug", |candidate| {
        let db = state.reader_db();
        async move {
            let slug = candidate.simple().to_string();
            external_auth_repo::find_provider_by_slug(db, &slug)
                .await
                .map(|provider| provider.is_some())
        }
    })
    .await?;
    Ok(uuid.simple().to_string())
}

pub async fn list_public_providers(
    state: &impl SharedRuntimeState,
) -> Result<Vec<ExternalAuthPublicProvider>> {
    Ok(
        external_auth_repo::list_enabled_providers(state.reader_db())
            .await?
            .into_iter()
            .map(provider_to_public)
            .collect(),
    )
}

pub async fn list_public_providers_by_kind(
    state: &impl SharedRuntimeState,
    kind: ExternalAuthKind,
) -> Result<Vec<ExternalAuthPublicProvider>> {
    Ok(
        external_auth_repo::list_enabled_providers_by_kind(state.reader_db(), kind)
            .await?
            .into_iter()
            .map(provider_to_public)
            .collect(),
    )
}

pub async fn list_admin_providers(
    state: &impl SharedRuntimeState,
    limit: u64,
    offset: u64,
) -> Result<OffsetPage<AdminExternalAuthProviderInfo>> {
    let page = load_offset_page(limit, offset, 100, |limit, offset| async move {
        external_auth_repo::list_providers_paginated(state.reader_db(), limit, offset).await
    })
    .await?;
    let items = page.items.into_iter().map(provider_to_admin).collect();
    Ok(OffsetPage::new(items, page.total, page.limit, page.offset))
}

pub fn list_provider_kinds() -> Vec<ExternalAuthProviderKindInfo> {
    ExternalAuthKind::ALL
        .into_iter()
        .map(provider_kind_info)
        .collect()
}

pub async fn get_admin_provider(
    state: &impl SharedRuntimeState,
    id: i64,
) -> Result<AdminExternalAuthProviderInfo> {
    external_auth_repo::find_provider_by_id(state.reader_db(), id)
        .await
        .map(provider_to_admin)
}

pub async fn create_provider(
    state: &impl SharedRuntimeState,
    input: CreateExternalAuthProviderInput,
) -> Result<AdminExternalAuthProviderInfo> {
    let kind = resolve_kind(input.kind, input.provider_kind);
    let slug = match input.slug.or(input.key) {
        Some(slug) => normalize_slug(&slug)?,
        None => generate_unique_slug(state).await?,
    };
    let now = Utc::now();
    let active = external_auth_provider::ActiveModel {
        slug: Set(slug),
        display_name: Set(required_string(&input.display_name, "display_name", 128)?),
        kind: Set(kind),
        enabled: Set(input.enabled.unwrap_or(true)),
        issuer_url: Set(optional_url(input.issuer_url, "issuer_url")?),
        authorize_url: Set(optional_url(
            input.authorization_url.or(input.authorize_url),
            "authorization_url",
        )?),
        token_url: Set(optional_url(input.token_url, "token_url")?),
        userinfo_url: Set(optional_url(input.userinfo_url, "userinfo_url")?),
        client_id: Set(required_string(&input.client_id, "client_id", 255)?),
        client_secret: Set(input.client_secret.unwrap_or_default()),
        scopes: Set(normalize_scopes(input.scopes, kind)),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    };
    external_auth_repo::insert_provider(state.writer_db(), active)
        .await
        .map(provider_to_admin)
}

pub async fn update_provider(
    state: &impl SharedRuntimeState,
    id: i64,
    input: UpdateExternalAuthProviderInput,
) -> Result<AdminExternalAuthProviderInfo> {
    let existing = external_auth_repo::find_provider_by_id(state.reader_db(), id).await?;
    let mut active = existing.into_active_model();
    if let Some(slug) = input.slug.or(input.key) {
        active.slug = Set(normalize_slug(&slug)?);
    }
    if let Some(kind) = input.kind.or(input.provider_kind) {
        active.kind = Set(kind);
    }
    if let Some(display_name) = input.display_name {
        active.display_name = Set(required_string(&display_name, "display_name", 128)?);
    }
    if input.issuer_url.is_some() {
        active.issuer_url = Set(optional_url(input.issuer_url, "issuer_url")?);
    }
    let authorization_url = input.authorization_url.or(input.authorize_url);
    if authorization_url.is_some() {
        active.authorize_url = Set(optional_url(authorization_url, "authorization_url")?);
    }
    if input.token_url.is_some() {
        active.token_url = Set(optional_url(input.token_url, "token_url")?);
    }
    if input.userinfo_url.is_some() {
        active.userinfo_url = Set(optional_url(input.userinfo_url, "userinfo_url")?);
    }
    if let Some(client_id) = input.client_id {
        active.client_id = Set(required_string(&client_id, "client_id", 255)?);
    }
    if let Some(client_secret) = input.client_secret
        && client_secret != REDACTED_SECRET
    {
        active.client_secret = Set(client_secret);
    }
    if let Some(scopes) = input.scopes {
        let kind = active.kind.clone().unwrap();
        active.scopes = Set(normalize_scopes(Some(scopes), kind));
    }
    if let Some(enabled) = input.enabled {
        active.enabled = Set(enabled);
    }
    active.updated_at = Set(Utc::now());
    active
        .update(state.writer_db())
        .await
        .map_err(AsterError::from)
        .map(provider_to_admin)
}

pub async fn delete_provider(state: &impl SharedRuntimeState, id: i64) -> Result<()> {
    external_auth_repo::delete_provider(state.writer_db(), id).await
}

fn build_authorize_url(
    provider: &external_auth_provider::Model,
    redirect_uri: &str,
    state: &str,
) -> Result<String> {
    let authorize_url = provider.authorize_url.as_deref().ok_or_else(|| {
        AsterError::external_auth_error("provider authorization_url is not configured")
    })?;
    let mut url = Url::parse(authorize_url).map_err(|error| {
        AsterError::external_auth_error(format!("invalid authorization_url: {error}"))
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &provider.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &provider.scopes)
        .append_pair("state", state);
    Ok(url.to_string())
}

pub async fn start_login(
    state: &impl SharedRuntimeState,
    provider_slug: &str,
    redirect_uri: &str,
) -> Result<ExternalAuthStartLoginResponse> {
    let redirect_uri = required_string(redirect_uri, "redirect_uri", 1024)?;
    optional_url(Some(redirect_uri.clone()), "redirect_uri")?;
    let provider =
        external_auth_repo::find_enabled_provider_by_slug(state.reader_db(), provider_slug).await?;
    let flow_state = id::new_short_token();
    let expires_at = Utc::now() + Duration::seconds(FLOW_TTL_SECS);
    external_auth_repo::create_login_flow(
        state.writer_db(),
        provider.id,
        &flow_state,
        &redirect_uri,
        expires_at,
    )
    .await?;
    let authorize_url = build_authorize_url(&provider, &redirect_uri, &flow_state)?;
    Ok(ExternalAuthStartLoginResponse {
        authorization_url: authorize_url.clone(),
        authorize_url,
        state: flow_state,
        expires_at,
    })
}

pub async fn finish_login(
    state: &impl SharedRuntimeState,
    flow_state: &str,
    code: &str,
) -> Result<ExternalAuthFinishLoginResponse> {
    required_string(code, "code", 4096)?;
    let flow = external_auth_repo::consume_login_flow(state.writer_db(), flow_state).await?;
    Ok(ExternalAuthFinishLoginResponse {
        state: flow.state,
        message:
            "external auth callback accepted; implement provider token exchange in your project"
                .to_string(),
    })
}

pub async fn cleanup_expired_flows(state: &impl SharedRuntimeState) -> Result<u64> {
    external_auth_repo::cleanup_expired_login_flows(state.writer_db(), Utc::now()).await
}

fn endpoint_check(
    name: &str,
    value: Option<&str>,
    required: bool,
) -> ExternalAuthProviderTestCheck {
    match value {
        Some(value) if Url::parse(value).is_ok() => ExternalAuthProviderTestCheck {
            name: name.to_string(),
            success: true,
            message: "configured".to_string(),
        },
        Some(_) => ExternalAuthProviderTestCheck {
            name: name.to_string(),
            success: false,
            message: "invalid URL".to_string(),
        },
        None if required => ExternalAuthProviderTestCheck {
            name: name.to_string(),
            success: false,
            message: "missing required endpoint".to_string(),
        },
        None => ExternalAuthProviderTestCheck {
            name: name.to_string(),
            success: true,
            message: "not configured".to_string(),
        },
    }
}

pub async fn test_provider_params(
    _state: &impl SharedRuntimeState,
    input: ExternalAuthProviderTestParamsInput,
) -> Result<ExternalAuthProviderTestResult> {
    let kind = resolve_kind(input.kind, input.provider_kind);
    required_string(&input.client_id, "client_id", 255)?;
    let authorization_url = optional_url(
        input.authorization_url.or(input.authorize_url),
        "authorization_url",
    )?;
    let token_url = optional_url(input.token_url, "token_url")?;
    let userinfo_url = optional_url(input.userinfo_url, "userinfo_url")?;
    let issuer_url = optional_url(input.issuer_url, "issuer_url")?;
    Ok(ExternalAuthProviderTestResult {
        provider: kind.as_str().to_string(),
        issuer: issuer_url.clone(),
        authorization_endpoint: authorization_url.clone(),
        token_endpoint: token_url.clone(),
        userinfo_endpoint: userinfo_url.clone(),
        checks: vec![
            endpoint_check("issuer_url", issuer_url.as_deref(), false),
            endpoint_check("authorization_url", authorization_url.as_deref(), true),
            endpoint_check("token_url", token_url.as_deref(), true),
            endpoint_check("userinfo_url", userinfo_url.as_deref(), false),
        ],
    })
}

pub async fn test_provider(
    state: &impl SharedRuntimeState,
    id: i64,
) -> Result<ExternalAuthProviderTestResult> {
    let provider = external_auth_repo::find_provider_by_id(state.reader_db(), id).await?;
    test_provider_params(
        state,
        ExternalAuthProviderTestParamsInput {
            kind: Some(provider.kind),
            provider_kind: None,
            issuer_url: provider.issuer_url,
            authorization_url: provider.authorize_url,
            authorize_url: None,
            token_url: provider.token_url,
            userinfo_url: provider.userinfo_url,
            client_id: provider.client_id,
            client_secret: Some(provider.client_secret),
            scopes: Some(provider.scopes),
        },
    )
    .await
}
