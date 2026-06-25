//! Feature-gated external authentication provider drivers for Aster services.
//!
//! This crate contains the provider-neutral external authentication contract plus reusable
//! implementations for OpenID Connect, generic OAuth2, and selected fixed-endpoint providers. It
//! deliberately avoids application database models, account entities, and HTTP handler concerns:
//! product crates map their stored provider rows into [`ExternalAuthProviderConfig`] and map
//! [`ExternalAuthError`] into their own API error type. Built-in connectors are controlled by Cargo
//! features so each backend can compile only the providers it supports. The default feature set
//! enables `oidc` and generic `oauth2`; dedicated connectors such as `github`, `google`,
//! `microsoft`, and `qq` must be enabled explicitly.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

pub mod driver;
mod error;
pub mod normalize;
pub mod providers;
pub mod registry;
pub mod types;

pub use driver::{
    ExternalAuthAuthorizationStart, ExternalAuthCallback, ExternalAuthProfile,
    ExternalAuthProviderConfig, ExternalAuthProviderDescriptor, ExternalAuthProviderDriver,
    ExternalAuthProviderTestCheck, ExternalAuthProviderTestResult,
};
pub use error::{ExternalAuthError, MapExternalAuthErr, Result};
pub use registry::{ExternalAuthProviderRegistry, default_registry};
pub use types::{
    ExternalAuthProtocol, ExternalAuthProviderKind, ExternalAuthProviderOptions,
    MicrosoftExternalAuthProviderOptions, parse_external_auth_provider_options,
    serialize_external_auth_provider_options,
};

#[cfg(any(feature = "oauth2", feature = "oidc"))]
pub(crate) const OUTBOUND_HTTP_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[cfg(any(feature = "oauth2", feature = "oidc"))]
pub(crate) fn outbound_http_user_agent(provider: &driver::ExternalAuthProviderConfig) -> &str {
    provider
        .outbound_http_user_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(OUTBOUND_HTTP_USER_AGENT)
}
