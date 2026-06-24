//! Built-in provider drivers controlled by Cargo features.
//!
//! The module tree is feature-gated so a backend can enable only the connectors it supports. The
//! default crate features expose generic OIDC and OAuth2. Dedicated providers reuse those generic
//! implementations but remain opt-in at compile time.

#[cfg(feature = "github")]
pub mod github;
#[cfg(feature = "google")]
pub mod google;
#[cfg(feature = "microsoft")]
pub mod microsoft;
#[cfg(feature = "oauth2")]
pub mod oauth2;
#[cfg(feature = "oidc")]
pub mod oidc;
#[cfg(feature = "qq")]
pub mod qq;
