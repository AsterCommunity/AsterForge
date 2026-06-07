//! Generic external authentication service.

mod providers;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ExternalAuthKind;

pub use providers::{
    cleanup_expired_flows, create_provider, delete_provider, finish_login, get_admin_provider,
    list_admin_providers, list_provider_kinds, list_public_providers,
    list_public_providers_by_kind, start_login, test_provider, test_provider_params,
    update_provider,
};

pub const DEFAULT_OIDC_SCOPES: &str = "openid email profile";
pub const DEFAULT_OAUTH2_SCOPES: &str = "email profile";
pub const FLOW_TTL_SECS: i64 = 300;
pub const REDACTED_SECRET: &str = "***REDACTED***";

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthPublicProvider {
    pub slug: String,
    pub key: String,
    pub display_name: String,
    pub kind: ExternalAuthKind,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderKindInfo {
    pub kind: ExternalAuthKind,
    pub protocol: String,
    pub display_name: String,
    pub description: String,
    pub default_scopes: String,
    pub issuer_url_required: bool,
    pub authorization_url_required: bool,
    pub token_url_required: bool,
    pub userinfo_url_required: bool,
    pub supports_discovery: bool,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct AdminExternalAuthProviderInfo {
    pub id: i64,
    pub key: String,
    pub slug: String,
    pub kind: ExternalAuthKind,
    pub provider_kind: ExternalAuthKind,
    pub protocol: String,
    pub display_name: String,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_secret_configured: bool,
    pub scopes: String,
    pub enabled: bool,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct CreateExternalAuthProviderInput {
    pub key: Option<String>,
    pub slug: Option<String>,
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    pub display_name: String,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct UpdateExternalAuthProviderInput {
    pub key: Option<String>,
    pub slug: Option<String>,
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    pub display_name: Option<String>,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderTestParamsInput {
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderTestCheck {
    pub name: String,
    pub success: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderTestResult {
    pub provider: String,
    pub issuer: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    pub checks: Vec<ExternalAuthProviderTestCheck>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthStartLoginResponse {
    pub authorize_url: String,
    pub authorization_url: String,
    pub state: String,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthFinishLoginResponse {
    pub message: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExternalAuthProviderAuditDetails<'a> {
    pub key: &'a str,
    pub slug: &'a str,
    pub kind: ExternalAuthKind,
    pub issuer_url: Option<&'a str>,
    pub enabled: bool,
}
