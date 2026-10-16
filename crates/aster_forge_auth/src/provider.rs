//! Explicit initialization of the upstream process-wide JWT crypto provider.

use crate::{AuthError, Result};
use std::sync::atomic::{AtomicBool, Ordering};

// Only a successful explicit installation makes Forge JWT operations available. The upstream
// provider is immutable once installed; this flag neither selects nor replaces that provider.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Installs the backend selected by this crate's Cargo features for the entire process.
///
/// Call exactly once from the product composition root before starting workers or making any
/// JWT calls, including direct `jsonwebtoken` calls. Features are additive: another dependency
/// may enable the other upstream backend, but this explicit selection remains deterministic.
/// Custom providers and separate providers per signer/verifier are not supported.
///
/// # Errors
/// Returns [`AuthError::CryptoProviderAlreadyInstalled`] if any caller has already initialized
/// the upstream provider, including upstream automatic initialization. No provider is replaced
/// or silently accepted. A duplicate call after Forge's successful initialization returns this
/// error while existing JWT operations remain available. If another caller installed first,
/// Forge stays unavailable; the product must treat that as a startup configuration failure.
pub fn initialize_jwt_crypto_provider() -> Result<()> {
    #[cfg(feature = "aws-lc-rs")]
    let provider = &jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER;
    #[cfg(all(feature = "rust-crypto", not(feature = "aws-lc-rs")))]
    let provider = &jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;

    #[cfg(any(feature = "aws-lc-rs", feature = "rust-crypto"))]
    provider
        .install_default()
        .map_err(|_| AuthError::CryptoProviderAlreadyInstalled)?;

    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn ensure_initialized() -> Result<()> {
    if INITIALIZED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(AuthError::CryptoProviderNotInitialized)
    }
}
