//! Provider driver trait and shared external-auth flow value objects.
//!
//! The driver contract is intentionally expressed in runtime DTOs rather than application database
//! models. Product crates adapt their stored provider rows into [`ExternalAuthProviderConfig`],
//! then call a registered driver to start authorization, exchange callbacks, or validate provider
//! configuration.

use crate::types::{ExternalAuthProtocol, ExternalAuthProviderKind, ExternalAuthProviderOptions};
use crate::{ExternalAuthError, Result};
use async_trait::async_trait;
use serde::Serialize;
use std::fmt;

/// Static metadata that describes a provider driver's capabilities and configuration needs.
#[derive(Clone, Debug)]
pub struct ExternalAuthProviderDescriptor {
    /// Provider kind handled by the driver.
    pub kind: ExternalAuthProviderKind,
    /// Protocol family used by the driver.
    pub protocol: ExternalAuthProtocol,
    /// Human-readable provider name.
    pub display_name: &'static str,
    /// Short capability summary suitable for admin UI surfaces.
    pub description: &'static str,
    /// Default scopes used when a provider config leaves scopes empty.
    pub default_scopes: &'static str,
    /// Whether an issuer URL must be supplied by administrators.
    pub issuer_url_required: bool,
    /// Whether administrators may manually configure OAuth/OIDC endpoints.
    pub manual_endpoint_configuration_supported: bool,
    /// Whether the authorization endpoint is required.
    pub authorization_url_required: bool,
    /// Whether the token endpoint is required.
    pub token_url_required: bool,
    /// Whether the userinfo endpoint is required.
    pub userinfo_url_required: bool,
    /// Whether provider discovery is supported.
    pub supports_discovery: bool,
    /// Whether the authorization flow uses PKCE.
    pub supports_pkce: bool,
    /// Whether profile extraction can use an email-verified claim.
    pub supports_email_verified_claim: bool,
}

/// Runtime configuration used by provider drivers.
///
/// The value is intentionally independent from persistence. Application crates can keep their own
/// schema, encrypted secret handling, OpenAPI shape, and migration behavior, then construct this
/// config immediately before invoking a provider driver.
#[derive(Clone)]
pub struct ExternalAuthProviderConfig {
    /// Product-owned provider id, carried through for logging and app-level correlation.
    pub id: i64,
    /// Product-owned stable provider key.
    pub key: String,
    /// Provider kind selected by the application.
    pub provider_kind: ExternalAuthProviderKind,
    /// Protocol selected by the application.
    pub protocol: ExternalAuthProtocol,
    /// Connector-specific decoded options.
    pub options: ExternalAuthProviderOptions,
    /// Issuer URL for OIDC-style providers.
    pub issuer_url: Option<String>,
    /// Authorization endpoint for manual OAuth2 providers.
    pub authorization_url: Option<String>,
    /// Token endpoint for manual OAuth2 providers.
    pub token_url: Option<String>,
    /// Userinfo endpoint for manual OAuth2 providers.
    pub userinfo_url: Option<String>,
    /// OAuth/OIDC client id.
    pub client_id: String,
    /// Optional OAuth/OIDC client secret.
    pub client_secret: Option<String>,
    /// Space-separated scope list.
    pub scopes: String,
    /// Optional profile claim name or JSON pointer used as the subject.
    pub subject_claim: Option<String>,
    /// Optional profile claim name or JSON pointer used as the preferred username.
    pub username_claim: Option<String>,
    /// Optional profile claim name or JSON pointer used as the display name.
    pub display_name_claim: Option<String>,
    /// Optional profile claim name or JSON pointer used as the email address.
    pub email_claim: Option<String>,
    /// Optional profile claim name or JSON pointer used as the email verification flag.
    pub email_verified_claim: Option<String>,
    /// Optional profile claim name reserved for group extraction by application crates.
    pub groups_claim: Option<String>,
    /// Optional profile claim name reserved for avatar URL extraction by application crates.
    pub avatar_url_claim: Option<String>,
}

impl fmt::Debug for ExternalAuthProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalAuthProviderConfig")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("provider_kind", &self.provider_kind)
            .field("protocol", &self.protocol)
            .field("options", &self.options)
            .field("issuer_url", &self.issuer_url)
            .field("authorization_url", &self.authorization_url)
            .field("token_url", &self.token_url)
            .field("userinfo_url", &self.userinfo_url)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "***REDACTED***"),
            )
            .field("scopes", &self.scopes)
            .field("subject_claim", &self.subject_claim)
            .field("username_claim", &self.username_claim)
            .field("display_name_claim", &self.display_name_claim)
            .field("email_claim", &self.email_claim)
            .field("email_verified_claim", &self.email_verified_claim)
            .field("groups_claim", &self.groups_claim)
            .field("avatar_url_claim", &self.avatar_url_claim)
            .finish()
    }
}

impl ExternalAuthProviderConfig {
    /// Returns a non-empty issuer URL or a validation error.
    pub fn require_issuer_url(&self) -> Result<&str> {
        self.issuer_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ExternalAuthError::validation_error("external auth provider missing issuer_url")
            })
    }
}

/// Result of starting an authorization flow.
#[derive(Clone, Debug)]
pub struct ExternalAuthAuthorizationStart {
    /// Provider authorization URL to redirect the browser to.
    pub authorization_url: String,
    /// CSRF state stored by the application for callback validation.
    pub state: String,
    /// Optional OIDC nonce stored by the application for callback validation.
    pub nonce: Option<String>,
    /// Optional PKCE verifier stored by the application for callback exchange.
    pub pkce_verifier: Option<String>,
}

/// Callback payload needed by a provider driver to exchange an authorization code.
#[derive(Clone, Debug)]
pub struct ExternalAuthCallback {
    /// Authorization code returned by the provider.
    pub code: String,
    /// Stored OIDC nonce, when the provider uses one.
    pub nonce: Option<String>,
    /// Stored PKCE verifier.
    pub pkce_verifier: Option<String>,
    /// Redirect URI used for this login flow.
    pub redirect_uri: String,
}

/// Normalized profile returned by an external authentication provider.
#[derive(Clone, Debug)]
pub struct ExternalAuthProfile {
    /// Provider-scoped namespace used to avoid subject collisions across issuers and connectors.
    pub identity_namespace: String,
    /// Provider subject identifier.
    pub subject: String,
    /// Optional email address.
    pub email: Option<String>,
    /// Whether the provider asserted the email as verified.
    pub email_verified: bool,
    /// Optional display name snapshot.
    pub display_name: Option<String>,
    /// Optional preferred username snapshot.
    pub preferred_username: Option<String>,
}

/// Single provider health/test check.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderTestCheck {
    /// Machine-readable check name.
    pub name: String,
    /// Whether the check passed.
    pub success: bool,
    /// Human-readable check result.
    pub message: String,
}

/// Provider health/test result returned to admin tooling.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ExternalAuthProviderTestResult {
    /// Display name of the tested provider driver.
    pub provider: String,
    /// Effective issuer URL, when applicable.
    pub issuer: Option<String>,
    /// Effective authorization endpoint, when applicable.
    pub authorization_endpoint: Option<String>,
    /// Effective token endpoint, when applicable.
    pub token_endpoint: Option<String>,
    /// Effective userinfo endpoint, when applicable.
    pub userinfo_endpoint: Option<String>,
    /// Number of discovered JWKS keys, when applicable.
    pub jwks_key_count: Option<usize>,
    /// Individual test checks.
    pub checks: Vec<ExternalAuthProviderTestCheck>,
}

/// External authentication provider driver.
#[async_trait]
pub trait ExternalAuthProviderDriver: Send + Sync {
    /// Returns the provider kind handled by this driver.
    fn kind(&self) -> ExternalAuthProviderKind;

    /// Returns static provider metadata and capability flags.
    fn descriptor(&self) -> ExternalAuthProviderDescriptor;

    /// Builds an authorization URL and state values for a browser redirect.
    async fn start_authorization(
        &self,
        provider: &ExternalAuthProviderConfig,
        redirect_uri: &str,
    ) -> Result<ExternalAuthAuthorizationStart>;

    /// Exchanges a callback authorization code and returns a normalized profile.
    async fn exchange_callback(
        &self,
        provider: &ExternalAuthProviderConfig,
        callback: ExternalAuthCallback,
    ) -> Result<ExternalAuthProfile>;

    /// Performs configuration/discovery checks suitable for admin validation.
    async fn test_provider(
        &self,
        provider: &ExternalAuthProviderConfig,
    ) -> Result<ExternalAuthProviderTestResult>;
}
