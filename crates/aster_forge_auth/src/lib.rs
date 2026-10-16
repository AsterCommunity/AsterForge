//! Product-neutral local session authentication mechanics.
//!
//! `aster_forge_auth` owns JWT token contracts, validation policy, token-pair issuance, and the
//! refresh-rotation coordinator. Product repositories own account state, database transactions,
//! session tables, cookies, permissions, audit events, and error presentation. Persistence is
//! accessed through [`RefreshSessionStore`], so this crate does not depend on `SeaORM` or a product
//! schema.
//!
//! Select exactly one Cargo backend feature: `aws-lc-rs` (default) or `rust-crypto`.
//! The product entrypoint must call [`initialize_jwt_crypto_provider`] before any JWT use,
//! including direct `jsonwebtoken` calls. This installs a process-wide provider, not an
//! instance-specific backend. Signing algorithms remain a runtime [`JwtPolicy`] choice.
//!
//! ```
//! use aster_forge_auth::{AuthTokenType, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
//!     Restriction, SessionId, SessionVersion, Subject};
//!
//! # fn main() -> aster_forge_auth::Result<()> {
//! aster_forge_auth::initialize_jwt_crypto_provider()?;
//! let key = JwtKey::hmac([42; 32])?; // Product loads a dedicated random signing key.
//! let policy = JwtPolicy::new("product", "local-web", 900, 604_800)?;
//! let signer = JwtSigner::with_system_clock(key.clone(), policy.clone());
//! let verifier = JwtVerifier::with_system_clock(key, policy);
//! let pair = signer.issue(Subject::new("42")?, SessionId::new("session-1")?,
//!     SessionVersion::new(1), Restriction::None)?;
//! let verified = verifier.verify(&pair.access_token, AuthTokenType::Access)?;
//! assert_eq!(verified.claims().sub.as_str(), "42");
//! // Product persists the session before publishing pair, and checks current authorization.
//! # Ok(())
//! # }
//! ```
#![deny(missing_docs)]
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

#[cfg(all(feature = "aws-lc-rs", feature = "rust-crypto"))]
compile_error!(
    "aster_forge_auth: features `aws-lc-rs` and `rust-crypto` are mutually exclusive; disable default features to select `rust-crypto`"
);
#[cfg(not(any(feature = "aws-lc-rs", feature = "rust-crypto")))]
compile_error!(
    "aster_forge_auth: select exactly one JWT backend feature: `aws-lc-rs` or `rust-crypto`"
);

mod coordinator;
mod error;
mod ids;
mod jwt;
mod provider;
mod refresh;
mod time;

pub use coordinator::RefreshCoordinator;
pub use error::{AuthError, Result};
pub use ids::{IdSource, RandomIdSource, RefreshJti, SessionId, Subject};
pub use jwt::{
    AuthTokenPair, AuthTokenType, JwtAlgorithm, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
    Restriction, SessionVersion, TokenClaims, VerifiedToken,
};
pub use provider::initialize_jwt_crypto_provider;
pub use refresh::{
    ClientContext, RefreshDecision, RefreshError, RefreshOutcome, RefreshPolicy, RefreshRejection,
    RefreshRotationRequest, RefreshSession, RefreshSessionStore, RefreshStoreOutcome,
    RefreshTokenService, classify_refresh,
};
pub use time::{Clock, FixedClock, SystemClock};

#[cfg(feature = "password")]
mod password;
#[cfg(feature = "password")]
pub use password::*;
