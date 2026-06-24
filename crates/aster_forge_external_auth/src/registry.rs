//! Runtime registry for feature-enabled external authentication provider drivers.
//!
//! The default registry registers only drivers compiled into the crate through Cargo features.
//! Applications can create their own registry and call [`ExternalAuthProviderRegistry::add`] to
//! append product or plugin-provided drivers without replacing built-ins. Tests and advanced
//! application code can still call [`ExternalAuthProviderRegistry::register`] when intentional
//! replacement is required.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::driver::{
    ExternalAuthProviderConfig, ExternalAuthProviderDescriptor, ExternalAuthProviderDriver,
};
#[cfg(feature = "github")]
use super::providers::github::GitHubProviderDriver;
#[cfg(feature = "google")]
use super::providers::google::GoogleProviderDriver;
#[cfg(feature = "microsoft")]
use super::providers::microsoft::MicrosoftProviderDriver;
#[cfg(feature = "oauth2")]
use super::providers::oauth2::OAuth2ProviderDriver;
#[cfg(feature = "oidc")]
use super::providers::oidc::OidcProviderDriver;
#[cfg(feature = "qq")]
use super::providers::qq::QqProviderDriver;
use crate::types::ExternalAuthProviderKind;
use crate::{ExternalAuthError, Result};

/// Registry of external authentication provider drivers keyed by provider kind.
///
/// The registry is the shared capability boundary for application services. Callers can use it to
/// list descriptors for admin surfaces, gate stored provider configs before a login flow starts,
/// and retrieve the runtime driver for a validated provider. The registry deliberately does not
/// know how providers are stored in a product database; applications adapt their own rows into
/// [`ExternalAuthProviderConfig`] immediately before using this type.
pub struct ExternalAuthProviderRegistry {
    drivers: HashMap<ExternalAuthProviderKind, Arc<dyn ExternalAuthProviderDriver>>,
}

impl ExternalAuthProviderRegistry {
    /// Creates an empty registry with no built-in provider drivers.
    ///
    /// This is useful for applications that want a fully explicit provider list, tests that need
    /// deterministic registration behavior across Cargo feature sets, or plugin hosts that build a
    /// registry from externally supplied drivers.
    pub fn empty() -> Self {
        Self {
            drivers: HashMap::new(),
        }
    }

    /// Creates a registry populated with all feature-enabled built-in drivers.
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut registry = Self::empty();
        #[cfg(feature = "oidc")]
        registry.register_builtin(OidcProviderDriver::new());
        #[cfg(feature = "oauth2")]
        registry.register_builtin(OAuth2ProviderDriver::new());
        #[cfg(feature = "github")]
        registry.register_builtin(GitHubProviderDriver::new());
        #[cfg(feature = "google")]
        registry.register_builtin(GoogleProviderDriver::new());
        #[cfg(feature = "microsoft")]
        registry.register_builtin(MicrosoftProviderDriver::new());
        #[cfg(feature = "qq")]
        registry.register_builtin(QqProviderDriver::new());
        registry
    }

    /// Creates a built-in registry and lets an external system append registrations.
    ///
    /// This is the intended integration point for application-level extension systems: the caller
    /// receives a mutable registry, calls [`ExternalAuthProviderRegistry::add`] for each external
    /// driver it wants to expose, and returns any setup error. Built-in drivers are registered
    /// before the callback runs, so external systems cannot accidentally replace them through the
    /// non-replacing add API.
    pub fn with_external_registrations<F>(configure: F) -> Result<Self>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        let mut registry = Self::new();
        configure(&mut registry)?;
        Ok(registry)
    }

    /// Adds a driver if its provider kind is not already registered.
    ///
    /// Use this for product-specific or plugin-provided drivers. Duplicate provider kinds return a
    /// configuration error instead of replacing the existing driver, which keeps built-in behavior
    /// stable when external systems are enabled.
    pub fn add<D>(&mut self, driver: D) -> Result<()>
    where
        D: ExternalAuthProviderDriver + 'static,
    {
        self.add_arc(Arc::new(driver))
    }

    /// Adds an already shared driver if its provider kind is not already registered.
    pub fn add_arc(&mut self, driver: Arc<dyn ExternalAuthProviderDriver>) -> Result<()> {
        let kind = driver.kind();
        Self::validate_driver_descriptor(kind, driver.descriptor())?;
        if self.drivers.contains_key(&kind) {
            return Err(ExternalAuthError::config_error(format!(
                "external auth provider driver '{}' is already registered",
                kind.as_str()
            )));
        }
        self.drivers.insert(kind, driver);
        Ok(())
    }

    /// Registers or replaces a driver for its provider kind.
    ///
    /// Use this only when replacement is intentional, such as tests or product-level overrides.
    /// The driver must still report a descriptor for the same provider kind that it registers.
    pub fn register<D>(&mut self, driver: D) -> Result<()>
    where
        D: ExternalAuthProviderDriver + 'static,
    {
        self.register_arc(Arc::new(driver))
    }

    /// Registers or replaces an already shared driver for its provider kind.
    pub fn register_arc(&mut self, driver: Arc<dyn ExternalAuthProviderDriver>) -> Result<()> {
        let kind = driver.kind();
        Self::validate_driver_descriptor(kind, driver.descriptor())?;
        self.drivers.insert(kind, driver);
        Ok(())
    }

    /// Iterates over registered provider kinds.
    pub fn supported_kinds(&self) -> impl Iterator<Item = ExternalAuthProviderKind> + '_ {
        self.drivers.keys().copied()
    }

    /// Returns whether a driver for `kind` is registered.
    pub fn contains(&self, kind: ExternalAuthProviderKind) -> bool {
        self.drivers.contains_key(&kind)
    }

    /// Returns registered provider descriptors sorted by provider kind.
    pub fn descriptors(&self) -> Vec<ExternalAuthProviderDescriptor> {
        let mut descriptors = self
            .drivers
            .values()
            .map(|driver| driver.descriptor())
            .collect::<Vec<_>>();
        descriptors.sort_by_key(|descriptor| descriptor.kind.as_str());
        descriptors
    }

    /// Returns the descriptor for a registered provider kind.
    pub fn descriptor_for(
        &self,
        kind: ExternalAuthProviderKind,
    ) -> Result<ExternalAuthProviderDescriptor> {
        Ok(self.get_driver(kind)?.descriptor())
    }

    /// Ensures that a provider kind is enabled in this registry.
    ///
    /// This is useful for service-layer guards that need to reject disabled provider kinds without
    /// constructing a login flow or exposing the underlying driver.
    pub fn ensure_provider_supported(&self, kind: ExternalAuthProviderKind) -> Result<()> {
        if self.contains(kind) {
            return Ok(());
        }
        Err(ExternalAuthError::config_error(format!(
            "external auth provider driver '{}' is not registered",
            kind.as_str()
        )))
    }

    /// Validates that a product-owned provider config matches a registered driver descriptor.
    ///
    /// This catches configuration drift before provider-specific network calls run. The method
    /// checks that the provider kind is enabled and that the stored protocol matches the driver's
    /// declared protocol. It returns the descriptor so callers can keep using the capability data
    /// without another registry lookup.
    pub fn validate_provider_config(
        &self,
        provider: &ExternalAuthProviderConfig,
    ) -> Result<ExternalAuthProviderDescriptor> {
        let descriptor = self.descriptor_for(provider.provider_kind)?;
        if provider.protocol != descriptor.protocol {
            return Err(ExternalAuthError::validation_error(format!(
                "external auth provider '{}' is configured with protocol '{}' but driver expects '{}'",
                provider.provider_kind.as_str(),
                provider.protocol.as_str(),
                descriptor.protocol.as_str()
            )));
        }
        Ok(descriptor)
    }

    /// Returns the registered driver for a provider config after validating the config boundary.
    ///
    /// Product services should prefer this method when starting authorization, exchanging a
    /// callback, or testing a provider because it applies the same registry-level gates for every
    /// flow.
    pub fn driver_for_provider(
        &self,
        provider: &ExternalAuthProviderConfig,
    ) -> Result<Arc<dyn ExternalAuthProviderDriver>> {
        self.validate_provider_config(provider)?;
        self.get_driver(provider.provider_kind)
    }

    /// Returns a registered driver by provider kind.
    pub fn get_driver(
        &self,
        kind: ExternalAuthProviderKind,
    ) -> Result<Arc<dyn ExternalAuthProviderDriver>> {
        self.drivers.get(&kind).cloned().ok_or_else(|| {
            ExternalAuthError::config_error(format!(
                "external auth provider driver '{}' is not registered",
                kind.as_str()
            ))
        })
    }

    /// Returns the OIDC driver from this registry.
    pub fn oidc(&self) -> Result<Arc<dyn ExternalAuthProviderDriver>> {
        self.get_driver(ExternalAuthProviderKind::Oidc)
    }

    fn validate_driver_descriptor(
        kind: ExternalAuthProviderKind,
        descriptor: ExternalAuthProviderDescriptor,
    ) -> Result<()> {
        if descriptor.kind == kind {
            return Ok(());
        }
        Err(ExternalAuthError::config_error(format!(
            "external auth provider driver '{}' returned descriptor for '{}'",
            kind.as_str(),
            descriptor.kind.as_str()
        )))
    }

    #[cfg(any(
        feature = "github",
        feature = "google",
        feature = "microsoft",
        feature = "oauth2",
        feature = "oidc",
        feature = "qq"
    ))]
    fn register_builtin<D>(&mut self, driver: D)
    where
        D: ExternalAuthProviderDriver + 'static,
    {
        let kind = driver.kind();
        self.drivers.insert(kind, Arc::new(driver));
    }
}

impl Default for ExternalAuthProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns a process-wide default registry populated with feature-enabled drivers.
pub fn default_registry() -> &'static ExternalAuthProviderRegistry {
    static REGISTRY: OnceLock<ExternalAuthProviderRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ExternalAuthProviderRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExternalAuthAuthorizationStart, ExternalAuthCallback, ExternalAuthProfile,
        ExternalAuthProviderConfig, ExternalAuthProviderTestResult,
    };
    use async_trait::async_trait;

    #[derive(Default)]
    struct TestOidcDriver;

    #[async_trait]
    impl ExternalAuthProviderDriver for TestOidcDriver {
        fn kind(&self) -> ExternalAuthProviderKind {
            ExternalAuthProviderKind::Oidc
        }

        fn descriptor(&self) -> ExternalAuthProviderDescriptor {
            ExternalAuthProviderDescriptor {
                kind: ExternalAuthProviderKind::Oidc,
                protocol: crate::types::ExternalAuthProtocol::Oidc,
                display_name: "Test OIDC",
                description: "Test OIDC driver",
                default_scopes: "openid email profile",
                issuer_url_required: true,
                manual_endpoint_configuration_supported: false,
                authorization_url_required: false,
                token_url_required: false,
                userinfo_url_required: false,
                supports_discovery: true,
                supports_pkce: true,
                supports_email_verified_claim: true,
            }
        }

        async fn start_authorization(
            &self,
            _provider: &ExternalAuthProviderConfig,
            _redirect_uri: &str,
        ) -> Result<ExternalAuthAuthorizationStart> {
            unreachable!("registry tests only inspect driver registration")
        }

        async fn exchange_callback(
            &self,
            _provider: &ExternalAuthProviderConfig,
            _callback: ExternalAuthCallback,
        ) -> Result<ExternalAuthProfile> {
            unreachable!("registry tests only inspect driver registration")
        }

        async fn test_provider(
            &self,
            _provider: &ExternalAuthProviderConfig,
        ) -> Result<ExternalAuthProviderTestResult> {
            unreachable!("registry tests only inspect driver registration")
        }
    }

    #[derive(Default)]
    struct MismatchedDescriptorDriver;

    #[async_trait]
    impl ExternalAuthProviderDriver for MismatchedDescriptorDriver {
        fn kind(&self) -> ExternalAuthProviderKind {
            ExternalAuthProviderKind::Oidc
        }

        fn descriptor(&self) -> ExternalAuthProviderDescriptor {
            ExternalAuthProviderDescriptor {
                kind: ExternalAuthProviderKind::GenericOAuth2,
                protocol: crate::types::ExternalAuthProtocol::OAuth2,
                display_name: "Mismatched driver",
                description: "Driver with inconsistent registry metadata",
                default_scopes: "email",
                issuer_url_required: false,
                manual_endpoint_configuration_supported: true,
                authorization_url_required: true,
                token_url_required: true,
                userinfo_url_required: true,
                supports_discovery: false,
                supports_pkce: true,
                supports_email_verified_claim: false,
            }
        }

        async fn start_authorization(
            &self,
            _provider: &ExternalAuthProviderConfig,
            _redirect_uri: &str,
        ) -> Result<ExternalAuthAuthorizationStart> {
            unreachable!("registry tests only inspect driver registration")
        }

        async fn exchange_callback(
            &self,
            _provider: &ExternalAuthProviderConfig,
            _callback: ExternalAuthCallback,
        ) -> Result<ExternalAuthProfile> {
            unreachable!("registry tests only inspect driver registration")
        }

        async fn test_provider(
            &self,
            _provider: &ExternalAuthProviderConfig,
        ) -> Result<ExternalAuthProviderTestResult> {
            unreachable!("registry tests only inspect driver registration")
        }
    }

    fn oidc_provider_config() -> ExternalAuthProviderConfig {
        ExternalAuthProviderConfig {
            id: 1,
            key: "test-oidc".to_string(),
            provider_kind: ExternalAuthProviderKind::Oidc,
            protocol: crate::types::ExternalAuthProtocol::Oidc,
            options: crate::types::ExternalAuthProviderOptions::default(),
            issuer_url: Some("https://issuer.example.com".to_string()),
            authorization_url: None,
            token_url: None,
            userinfo_url: None,
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

    #[cfg(feature = "oidc")]
    #[test]
    fn registry_returns_oidc_driver_by_kind() {
        let registry = ExternalAuthProviderRegistry::new();
        let driver = registry
            .get_driver(ExternalAuthProviderKind::Oidc)
            .expect("OIDC driver should be registered");

        assert_eq!(driver.kind(), ExternalAuthProviderKind::Oidc);
    }

    #[test]
    fn registry_allows_driver_replacement_by_kind() {
        let mut registry = ExternalAuthProviderRegistry::new();
        registry
            .register(TestOidcDriver)
            .expect("replacement driver should register");

        assert!(registry.contains(ExternalAuthProviderKind::Oidc));
        #[cfg(feature = "oauth2")]
        assert!(registry.contains(ExternalAuthProviderKind::GenericOAuth2));
        #[cfg(feature = "github")]
        assert!(registry.contains(ExternalAuthProviderKind::GitHub));
        #[cfg(feature = "google")]
        assert!(registry.contains(ExternalAuthProviderKind::Google));
        #[cfg(feature = "microsoft")]
        assert!(registry.contains(ExternalAuthProviderKind::Microsoft));
        #[cfg(feature = "qq")]
        assert!(registry.contains(ExternalAuthProviderKind::Qq));
    }

    #[test]
    fn registry_register_rejects_driver_descriptor_kind_mismatch() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };

        let error = registry
            .register(MismatchedDescriptorDriver)
            .expect_err("mismatched descriptor should fail");

        assert!(error.to_string().contains(
            "external auth provider driver 'oidc' returned descriptor for 'generic_oauth2'"
        ));
        assert!(!registry.contains(ExternalAuthProviderKind::Oidc));
    }

    #[test]
    fn registry_add_rejects_duplicate_kind_without_replacing_existing_driver() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };
        registry
            .add(TestOidcDriver)
            .expect("initial add should work");

        let error = registry
            .add(TestOidcDriver)
            .expect_err("duplicate add should fail");

        assert!(
            error
                .to_string()
                .contains("external auth provider driver 'oidc' is already registered")
        );
    }

    #[test]
    fn registry_add_rejects_driver_descriptor_kind_mismatch() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };

        let error = registry
            .add(MismatchedDescriptorDriver)
            .expect_err("mismatched descriptor should fail");

        assert!(error.to_string().contains(
            "external auth provider driver 'oidc' returned descriptor for 'generic_oauth2'"
        ));
        assert!(!registry.contains(ExternalAuthProviderKind::Oidc));
    }

    #[test]
    fn registry_descriptor_for_returns_registered_descriptor() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };
        registry
            .add(TestOidcDriver)
            .expect("test driver should register");

        let descriptor = registry
            .descriptor_for(ExternalAuthProviderKind::Oidc)
            .expect("descriptor should exist");

        assert_eq!(descriptor.kind, ExternalAuthProviderKind::Oidc);
        assert_eq!(
            descriptor.protocol,
            crate::types::ExternalAuthProtocol::Oidc
        );
    }

    #[test]
    fn registry_validate_provider_config_rejects_protocol_mismatch() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };
        registry
            .add(TestOidcDriver)
            .expect("test driver should register");
        let mut provider = oidc_provider_config();
        provider.protocol = crate::types::ExternalAuthProtocol::OAuth2;

        let error = registry
            .validate_provider_config(&provider)
            .expect_err("protocol mismatch should fail");

        assert!(error.to_string().contains(
            "external auth provider 'oidc' is configured with protocol 'oauth2' but driver expects 'oidc'"
        ));
    }

    #[test]
    fn registry_driver_for_provider_returns_validated_driver() {
        let mut registry = ExternalAuthProviderRegistry {
            drivers: HashMap::new(),
        };
        registry
            .add(TestOidcDriver)
            .expect("test driver should register");
        let provider = oidc_provider_config();

        let driver = registry
            .driver_for_provider(&provider)
            .expect("valid provider should resolve driver");

        assert_eq!(driver.kind(), ExternalAuthProviderKind::Oidc);
    }

    #[test]
    fn registry_with_external_registrations_exposes_configure_hook() {
        let registry = ExternalAuthProviderRegistry::with_external_registrations(|registry| {
            if registry.contains(ExternalAuthProviderKind::Oidc) {
                Ok(())
            } else {
                registry.add(TestOidcDriver)
            }
        })
        .expect("external registration hook should add driver");

        assert!(registry.contains(ExternalAuthProviderKind::Oidc));
    }

    #[test]
    fn default_registry_is_singleton() {
        let first = default_registry() as *const ExternalAuthProviderRegistry;
        let second = default_registry() as *const ExternalAuthProviderRegistry;

        assert_eq!(first, second);
    }
}
