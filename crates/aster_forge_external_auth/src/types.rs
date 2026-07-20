//! Product-neutral provider kinds, protocol tags, and connector options.
//!
//! Application crates still own their external-auth tables and migrations. Products that persist
//! the shared provider kind and protocol values can enable the optional `sea-orm` feature and use
//! these enums directly as active enum fields without copying the wire contract.

#[cfg(feature = "sea-orm")]
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

/// Storage width required for persisted provider kind and protocol values.
///
/// Products creating new external-auth schema should size both enum columns to at least this
/// width. Increasing this value is a schema compatibility change for consumers.
pub const EXTERNAL_AUTH_TYPE_STORAGE_LEN: u32 = 32;

/// Built-in external authentication provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")
)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAuthProviderKind {
    /// Generic OpenID Connect provider using discovery.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "oidc"))]
    Oidc,
    /// Generic OAuth2 authorization-code provider with manually configured endpoints.
    #[serde(rename = "generic_oauth2", alias = "oauth2")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "generic_oauth2"))]
    GenericOAuth2,
    /// GitHub OAuth App sign-in.
    #[serde(rename = "github")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "github"))]
    GitHub,
    /// Google OpenID Connect sign-in.
    #[serde(rename = "google")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "google"))]
    Google,
    /// Microsoft Entra ID / Microsoft Account OpenID Connect sign-in.
    #[serde(rename = "microsoft")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "microsoft"))]
    Microsoft,
    /// QQ Connect OAuth2 sign-in.
    #[serde(rename = "qq")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "qq"))]
    Qq,
}

impl ExternalAuthProviderKind {
    /// All known provider kinds, independent of which connector features are enabled.
    pub const ALL: [Self; 6] = [
        Self::Oidc,
        Self::GenericOAuth2,
        Self::GitHub,
        Self::Google,
        Self::Microsoft,
        Self::Qq,
    ];

    /// Returns the stable serialized provider kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oidc => "oidc",
            Self::GenericOAuth2 => "generic_oauth2",
            Self::GitHub => "github",
            Self::Google => "google",
            Self::Microsoft => "microsoft",
            Self::Qq => "qq",
        }
    }

    /// Parses a provider kind from a persisted or API-facing string.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "oidc" => Some(Self::Oidc),
            "oauth2" | "generic_oauth2" => Some(Self::GenericOAuth2),
            "github" => Some(Self::GitHub),
            "google" => Some(Self::Google),
            "microsoft" => Some(Self::Microsoft),
            "qq" => Some(Self::Qq),
            _ => None,
        }
    }

    /// Returns the default protocol used by this provider kind.
    pub fn default_protocol(self) -> ExternalAuthProtocol {
        match self {
            Self::Oidc => ExternalAuthProtocol::Oidc,
            Self::GenericOAuth2 | Self::GitHub | Self::Qq => ExternalAuthProtocol::OAuth2,
            Self::Google | Self::Microsoft => ExternalAuthProtocol::Oidc,
        }
    }
}

impl std::fmt::Display for ExternalAuthProviderKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ExternalAuthProviderKind {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(value).ok_or(())
    }
}

impl AsRef<str> for ExternalAuthProviderKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// External authentication protocol families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")
)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAuthProtocol {
    /// OpenID Connect over OAuth2 authorization code flow.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "oidc"))]
    Oidc,
    /// OAuth2 authorization code flow without ID token validation.
    #[serde(rename = "oauth2")]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "oauth2"))]
    OAuth2,
}

impl ExternalAuthProtocol {
    /// Returns the stable serialized protocol tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oidc => "oidc",
            Self::OAuth2 => "oauth2",
        }
    }
}

/// Connector-specific runtime options decoded from application-owned storage.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ExternalAuthProviderOptions {
    /// Microsoft tenant selector options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microsoft: Option<MicrosoftExternalAuthProviderOptions>,
}

impl ExternalAuthProviderOptions {
    /// Returns a copy with empty connector-specific options removed and strings canonicalized.
    pub fn normalized(mut self) -> Self {
        if let Some(microsoft) = self.microsoft.take() {
            self.microsoft = microsoft.normalized();
        }
        self
    }
}

/// Microsoft connector options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct MicrosoftExternalAuthProviderOptions {
    /// Tenant selector: `common`, `organizations`, `consumers`, or a tenant UUID.
    pub tenant: String,
}

impl MicrosoftExternalAuthProviderOptions {
    /// Creates Microsoft provider options from a tenant selector.
    pub fn new(tenant: impl Into<String>) -> Self {
        Self {
            tenant: tenant.into(),
        }
    }

    fn normalized(self) -> Option<Self> {
        let tenant = self.tenant.trim().to_string();
        (!tenant.is_empty()).then_some(Self { tenant })
    }
}

/// Parses provider options JSON and falls back to empty options for invalid input.
pub fn parse_external_auth_provider_options(options: &str) -> ExternalAuthProviderOptions {
    serde_json::from_str::<ExternalAuthProviderOptions>(options)
        .unwrap_or_else(|error| {
            if !options.is_empty() && options != "{}" {
                tracing::warn!("invalid external auth provider options JSON '{options}': {error}");
            }
            ExternalAuthProviderOptions::default()
        })
        .normalized()
}

/// Serializes normalized provider options to JSON for application-owned storage.
pub fn serialize_external_auth_provider_options(
    options: &ExternalAuthProviderOptions,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string(&options.clone().normalized())
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sea-orm")]
    use super::EXTERNAL_AUTH_TYPE_STORAGE_LEN;
    use super::{ExternalAuthProtocol, ExternalAuthProviderKind};

    #[test]
    fn provider_kind_parses_canonical_and_legacy_values() {
        assert_eq!(
            "generic_oauth2".parse(),
            Ok(ExternalAuthProviderKind::GenericOAuth2)
        );
        assert_eq!(
            "oauth2".parse(),
            Ok(ExternalAuthProviderKind::GenericOAuth2)
        );
        assert!("unknown".parse::<ExternalAuthProviderKind>().is_err());
    }

    #[test]
    fn provider_kind_default_protocols_match_connector_contract() {
        assert_eq!(
            ExternalAuthProviderKind::Google.default_protocol(),
            ExternalAuthProtocol::Oidc
        );
        assert_eq!(
            ExternalAuthProviderKind::GitHub.default_protocol(),
            ExternalAuthProtocol::OAuth2
        );
    }

    #[cfg(feature = "sea-orm")]
    #[test]
    fn shared_external_auth_types_have_stable_sea_orm_values() {
        use sea_orm::ActiveEnum;

        for kind in ExternalAuthProviderKind::ALL {
            assert!(kind.to_value().len() <= EXTERNAL_AUTH_TYPE_STORAGE_LEN as usize);
            assert_eq!(
                ExternalAuthProviderKind::try_from_value(&kind.to_value()),
                Ok(kind)
            );
        }
        for protocol in [ExternalAuthProtocol::Oidc, ExternalAuthProtocol::OAuth2] {
            assert!(protocol.to_value().len() <= EXTERNAL_AUTH_TYPE_STORAGE_LEN as usize);
            assert_eq!(
                ExternalAuthProtocol::try_from_value(&protocol.to_value()),
                Ok(protocol)
            );
        }
        assert!(ExternalAuthProviderKind::try_from_value(&"oauth2".to_string()).is_err());
        assert!(ExternalAuthProtocol::try_from_value(&"saml".to_string()).is_err());
    }
}
