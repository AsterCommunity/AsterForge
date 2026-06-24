//! Integration coverage for the external-auth provider registry contract.
//!
//! These tests mirror the public contract assertions AsterDrive keeps around its provider-kind
//! administration API and storage connector registry: feature-enabled built-ins must expose stable
//! descriptors, external registrations must not silently replace existing providers, and product
//! provider configs must be gated before a runtime driver is used.

use aster_forge_external_auth::{
    ExternalAuthAuthorizationStart, ExternalAuthCallback, ExternalAuthError, ExternalAuthProfile,
    ExternalAuthProtocol, ExternalAuthProviderConfig, ExternalAuthProviderDescriptor,
    ExternalAuthProviderDriver, ExternalAuthProviderKind, ExternalAuthProviderOptions,
    ExternalAuthProviderRegistry, ExternalAuthProviderTestResult, Result, default_registry,
};
use async_trait::async_trait;

#[derive(Clone, Copy)]
struct StaticProviderDriver {
    kind: ExternalAuthProviderKind,
    descriptor_kind: ExternalAuthProviderKind,
    protocol: ExternalAuthProtocol,
}

impl StaticProviderDriver {
    const fn oidc() -> Self {
        Self {
            kind: ExternalAuthProviderKind::Oidc,
            descriptor_kind: ExternalAuthProviderKind::Oidc,
            protocol: ExternalAuthProtocol::Oidc,
        }
    }

    const fn mismatched() -> Self {
        Self {
            kind: ExternalAuthProviderKind::Oidc,
            descriptor_kind: ExternalAuthProviderKind::GenericOAuth2,
            protocol: ExternalAuthProtocol::OAuth2,
        }
    }
}

#[async_trait]
impl ExternalAuthProviderDriver for StaticProviderDriver {
    fn kind(&self) -> ExternalAuthProviderKind {
        self.kind
    }

    fn descriptor(&self) -> ExternalAuthProviderDescriptor {
        ExternalAuthProviderDescriptor {
            kind: self.descriptor_kind,
            protocol: self.protocol,
            display_name: "Static Test Provider",
            description: "External provider driver used by registry integration tests.",
            default_scopes: "openid email profile",
            issuer_url_required: self.protocol == ExternalAuthProtocol::Oidc,
            manual_endpoint_configuration_supported: self.protocol == ExternalAuthProtocol::OAuth2,
            authorization_url_required: self.protocol == ExternalAuthProtocol::OAuth2,
            token_url_required: self.protocol == ExternalAuthProtocol::OAuth2,
            userinfo_url_required: self.protocol == ExternalAuthProtocol::OAuth2,
            supports_discovery: self.protocol == ExternalAuthProtocol::Oidc,
            supports_pkce: true,
            supports_email_verified_claim: self.protocol == ExternalAuthProtocol::Oidc,
        }
    }

    async fn start_authorization(
        &self,
        _provider: &ExternalAuthProviderConfig,
        _redirect_uri: &str,
    ) -> Result<ExternalAuthAuthorizationStart> {
        Err(ExternalAuthError::internal_error(
            "integration test driver does not start authorization",
        ))
    }

    async fn exchange_callback(
        &self,
        _provider: &ExternalAuthProviderConfig,
        _callback: ExternalAuthCallback,
    ) -> Result<ExternalAuthProfile> {
        Err(ExternalAuthError::internal_error(
            "integration test driver does not exchange callbacks",
        ))
    }

    async fn test_provider(
        &self,
        _provider: &ExternalAuthProviderConfig,
    ) -> Result<ExternalAuthProviderTestResult> {
        Err(ExternalAuthError::internal_error(
            "integration test driver does not test providers",
        ))
    }
}

fn provider_config(
    kind: ExternalAuthProviderKind,
    protocol: ExternalAuthProtocol,
) -> ExternalAuthProviderConfig {
    ExternalAuthProviderConfig {
        id: 7,
        key: kind.as_str().to_string(),
        provider_kind: kind,
        protocol,
        options: ExternalAuthProviderOptions::default(),
        issuer_url: Some("https://issuer.example.test".to_string()),
        authorization_url: Some("https://issuer.example.test/authorize".to_string()),
        token_url: Some("https://issuer.example.test/token".to_string()),
        userinfo_url: Some("https://issuer.example.test/userinfo".to_string()),
        client_id: "client-id".to_string(),
        client_secret: Some("client-secret".to_string()),
        scopes: "openid email profile".to_string(),
        subject_claim: None,
        username_claim: None,
        display_name_claim: None,
        email_claim: None,
        email_verified_claim: None,
        groups_claim: None,
        avatar_url_claim: None,
        outbound_http_user_agent: None,
    }
}

fn feature_enabled_builtin_kinds() -> Vec<ExternalAuthProviderKind> {
    let mut kinds: Vec<ExternalAuthProviderKind> = [
        #[cfg(feature = "oidc")]
        ExternalAuthProviderKind::Oidc,
        #[cfg(feature = "oauth2")]
        ExternalAuthProviderKind::GenericOAuth2,
        #[cfg(feature = "github")]
        ExternalAuthProviderKind::GitHub,
        #[cfg(feature = "google")]
        ExternalAuthProviderKind::Google,
        #[cfg(feature = "microsoft")]
        ExternalAuthProviderKind::Microsoft,
        #[cfg(feature = "qq")]
        ExternalAuthProviderKind::Qq,
    ]
    .into_iter()
    .collect();
    kinds.sort_by_key(|kind| kind.as_str());
    kinds
}

#[cfg(any(
    feature = "github",
    feature = "google",
    feature = "microsoft",
    feature = "oauth2",
    feature = "oidc",
    feature = "qq"
))]
struct DescriptorExpectation {
    kind: ExternalAuthProviderKind,
    protocol: ExternalAuthProtocol,
    display_name: &'static str,
    default_scopes: &'static str,
    issuer_url_required: bool,
    manual_endpoint_configuration_supported: bool,
    authorization_url_required: bool,
    token_url_required: bool,
    userinfo_url_required: bool,
    supports_discovery: bool,
    supports_pkce: bool,
    supports_email_verified_claim: bool,
}

#[cfg(any(
    feature = "github",
    feature = "google",
    feature = "microsoft",
    feature = "oauth2",
    feature = "oidc",
    feature = "qq"
))]
fn assert_builtin_descriptor(
    descriptor: &ExternalAuthProviderDescriptor,
    expected: DescriptorExpectation,
) {
    assert_eq!(descriptor.kind, expected.kind);
    assert_eq!(descriptor.protocol, expected.protocol);
    assert_eq!(descriptor.display_name, expected.display_name);
    assert_eq!(descriptor.default_scopes, expected.default_scopes);
    assert_eq!(descriptor.issuer_url_required, expected.issuer_url_required);
    assert_eq!(
        descriptor.manual_endpoint_configuration_supported,
        expected.manual_endpoint_configuration_supported
    );
    assert_eq!(
        descriptor.authorization_url_required,
        expected.authorization_url_required
    );
    assert_eq!(descriptor.token_url_required, expected.token_url_required);
    assert_eq!(
        descriptor.userinfo_url_required,
        expected.userinfo_url_required
    );
    assert_eq!(descriptor.supports_discovery, expected.supports_discovery);
    assert_eq!(descriptor.supports_pkce, expected.supports_pkce);
    assert_eq!(
        descriptor.supports_email_verified_claim,
        expected.supports_email_verified_claim
    );
}

#[test]
fn builtin_registry_covers_feature_enabled_provider_descriptors() {
    let registry = ExternalAuthProviderRegistry::new();
    let expected_kinds = feature_enabled_builtin_kinds();
    let descriptors = registry.descriptors();

    assert_eq!(descriptors.len(), expected_kinds.len());
    for kind in expected_kinds {
        assert!(registry.contains(kind), "missing provider kind {kind:?}");
        registry
            .ensure_provider_supported(kind)
            .expect("feature-enabled provider should be supported");
        let descriptor = registry
            .descriptor_for(kind)
            .expect("feature-enabled descriptor should resolve");
        let driver = registry
            .get_driver(kind)
            .expect("feature-enabled driver should resolve");

        assert_eq!(descriptor.kind, kind);
        assert_eq!(descriptor.kind, driver.kind());
        assert_eq!(descriptor.protocol, kind.default_protocol());
    }

    for kind in ExternalAuthProviderKind::ALL {
        if !registry.contains(kind) {
            let error = registry
                .ensure_provider_supported(kind)
                .expect_err("disabled provider kind should be rejected");
            assert!(error.to_string().contains("is not registered"));
        }
    }
}

#[test]
fn builtin_descriptors_match_drive_provider_kind_contract() {
    let registry = ExternalAuthProviderRegistry::new();
    let _ = &registry;

    #[cfg(feature = "oidc")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::Oidc)
            .expect("OIDC descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::Oidc,
            protocol: ExternalAuthProtocol::Oidc,
            display_name: "OpenID Connect",
            default_scopes: "openid email profile",
            issuer_url_required: true,
            manual_endpoint_configuration_supported: false,
            authorization_url_required: false,
            token_url_required: false,
            userinfo_url_required: false,
            supports_discovery: true,
            supports_pkce: true,
            supports_email_verified_claim: true,
        },
    );

    #[cfg(feature = "oauth2")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::GenericOAuth2)
            .expect("OAuth2 descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::GenericOAuth2,
            protocol: ExternalAuthProtocol::OAuth2,
            display_name: "Generic OAuth2",
            default_scopes: "openid email profile",
            issuer_url_required: false,
            manual_endpoint_configuration_supported: true,
            authorization_url_required: true,
            token_url_required: true,
            userinfo_url_required: true,
            supports_discovery: false,
            supports_pkce: true,
            supports_email_verified_claim: true,
        },
    );

    #[cfg(feature = "github")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::GitHub)
            .expect("GitHub descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::GitHub,
            protocol: ExternalAuthProtocol::OAuth2,
            display_name: "GitHub",
            default_scopes: "read:user user:email",
            issuer_url_required: false,
            manual_endpoint_configuration_supported: false,
            authorization_url_required: false,
            token_url_required: false,
            userinfo_url_required: false,
            supports_discovery: false,
            supports_pkce: true,
            supports_email_verified_claim: false,
        },
    );

    #[cfg(feature = "google")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::Google)
            .expect("Google descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::Google,
            protocol: ExternalAuthProtocol::Oidc,
            display_name: "Google",
            default_scopes: "openid profile email",
            issuer_url_required: false,
            manual_endpoint_configuration_supported: false,
            authorization_url_required: false,
            token_url_required: false,
            userinfo_url_required: false,
            supports_discovery: true,
            supports_pkce: true,
            supports_email_verified_claim: true,
        },
    );

    #[cfg(feature = "microsoft")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::Microsoft)
            .expect("Microsoft descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::Microsoft,
            protocol: ExternalAuthProtocol::Oidc,
            display_name: "Microsoft",
            default_scopes: "openid profile email",
            issuer_url_required: false,
            manual_endpoint_configuration_supported: false,
            authorization_url_required: false,
            token_url_required: false,
            userinfo_url_required: false,
            supports_discovery: true,
            supports_pkce: true,
            supports_email_verified_claim: false,
        },
    );

    #[cfg(feature = "qq")]
    assert_builtin_descriptor(
        &registry
            .descriptor_for(ExternalAuthProviderKind::Qq)
            .expect("QQ descriptor should resolve"),
        DescriptorExpectation {
            kind: ExternalAuthProviderKind::Qq,
            protocol: ExternalAuthProtocol::OAuth2,
            display_name: "QQ",
            default_scopes: "get_user_info",
            issuer_url_required: false,
            manual_endpoint_configuration_supported: false,
            authorization_url_required: false,
            token_url_required: false,
            userinfo_url_required: false,
            supports_discovery: false,
            supports_pkce: true,
            supports_email_verified_claim: false,
        },
    );
}

#[test]
fn empty_registry_allows_external_driver_and_validates_provider_configs() {
    let mut registry = ExternalAuthProviderRegistry::empty();
    assert!(registry.descriptors().is_empty());

    registry
        .add(StaticProviderDriver::oidc())
        .expect("external OIDC driver should register");

    let provider = provider_config(ExternalAuthProviderKind::Oidc, ExternalAuthProtocol::Oidc);
    let descriptor = registry
        .validate_provider_config(&provider)
        .expect("matching provider config should validate");
    let driver = registry
        .driver_for_provider(&provider)
        .expect("matching provider config should resolve driver");

    assert_eq!(descriptor.kind, ExternalAuthProviderKind::Oidc);
    assert_eq!(driver.kind(), ExternalAuthProviderKind::Oidc);

    let mismatched_protocol =
        provider_config(ExternalAuthProviderKind::Oidc, ExternalAuthProtocol::OAuth2);
    let error = registry
        .validate_provider_config(&mismatched_protocol)
        .expect_err("protocol mismatch should be rejected");
    assert!(
        error.to_string().contains(
            "external auth provider 'oidc' is configured with protocol 'oauth2' but driver expects 'oidc'"
        )
    );

    let unsupported = provider_config(
        ExternalAuthProviderKind::GenericOAuth2,
        ExternalAuthProtocol::OAuth2,
    );
    let error = registry
        .driver_for_provider(&unsupported)
        .err()
        .expect("unsupported provider kind should be rejected");
    assert!(error.to_string().contains("generic_oauth2"));
}

#[test]
fn external_registration_hook_does_not_silently_replace_existing_provider() {
    let duplicate = ExternalAuthProviderRegistry::with_external_registrations(|registry| {
        registry.add(StaticProviderDriver::oidc())
    });

    #[cfg(feature = "oidc")]
    {
        let error = duplicate
            .err()
            .expect("external OIDC driver should not replace built-in OIDC");
        assert!(
            error
                .to_string()
                .contains("external auth provider driver 'oidc' is already registered")
        );
    }

    #[cfg(not(feature = "oidc"))]
    {
        let registry =
            duplicate.expect("external OIDC driver should register without built-in OIDC");
        assert!(registry.contains(ExternalAuthProviderKind::Oidc));
    }
}

#[test]
fn external_registration_rejects_descriptor_kind_mismatch() {
    let mut registry = ExternalAuthProviderRegistry::empty();

    let add_error = registry
        .add(StaticProviderDriver::mismatched())
        .expect_err("add should reject mismatched descriptor metadata");
    assert!(
        add_error.to_string().contains(
            "external auth provider driver 'oidc' returned descriptor for 'generic_oauth2'"
        )
    );
    assert!(!registry.contains(ExternalAuthProviderKind::Oidc));

    let register_error = registry
        .register(StaticProviderDriver::mismatched())
        .expect_err("register should reject mismatched descriptor metadata");
    assert!(
        register_error.to_string().contains(
            "external auth provider driver 'oidc' returned descriptor for 'generic_oauth2'"
        )
    );
    assert!(!registry.contains(ExternalAuthProviderKind::Oidc));
}

#[test]
fn default_registry_uses_the_same_feature_enabled_contract() {
    let registry = default_registry();
    let mut supported = registry.supported_kinds().collect::<Vec<_>>();
    supported.sort_by_key(|kind| kind.as_str());

    assert_eq!(supported, feature_enabled_builtin_kinds());
}
