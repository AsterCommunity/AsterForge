//! Standalone consumer: its workspace and dependency resolution are independent of Forge's.
use aster_forge_auth::{
    AuthError, AuthTokenType, FixedClock, JwtAlgorithm, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
    RandomIdSource, Restriction, SessionId, SessionVersion, Subject,
    initialize_jwt_crypto_provider,
};
use std::sync::Arc;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("missing") => check_tokens(Some(AuthError::CryptoProviderNotInitialized)),
        Some("conflict") => {
            // Deliberately install the opposite backend when upstream features were merged.
            #[cfg(feature = "merge")]
            {
                #[cfg(feature = "aws-lc-rs")]
                let provider = &jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
                #[cfg(feature = "rust-crypto")]
                let provider = &jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER;
                provider.install_default().unwrap();
            }
            assert_eq!(
                initialize_jwt_crypto_provider(),
                Err(AuthError::CryptoProviderAlreadyInstalled)
            );
            check_tokens(Some(AuthError::CryptoProviderNotInitialized));
        }
        _ => {
            initialize_jwt_crypto_provider().unwrap();
            check_tokens(None);
            assert_eq!(
                initialize_jwt_crypto_provider(),
                Err(AuthError::CryptoProviderAlreadyInstalled)
            );
            check_tokens(None);
        }
    }
}

fn check_tokens(error: Option<AuthError>) {
    for algorithm in [
        JwtAlgorithm::Hs256,
        JwtAlgorithm::Hs384,
        JwtAlgorithm::Hs512,
    ] {
        let key = JwtKey::hmac([42; 64]).unwrap();
        let policy = JwtPolicy::new("product", "local-web", 60, 600)
            .unwrap()
            .with_algorithm(algorithm);
        let clock = Arc::new(FixedClock::new(1_700_000_000));
        let signer = JwtSigner::new(
            key.clone(),
            policy.clone(),
            clock.clone(),
            Arc::new(RandomIdSource),
        );
        let verifier = JwtVerifier::new(key, policy, clock);
        let result = signer.issue(
            Subject::new("42").unwrap(),
            SessionId::new("s").unwrap(),
            SessionVersion::new(7),
            Restriction::None,
        );
        if let Some(error) = error {
            assert_eq!(result, Err(error));
            assert_eq!(
                verifier.verify("invalid", AuthTokenType::Access),
                Err(error)
            );
        } else {
            let pair = result.unwrap();
            assert!(
                verifier
                    .verify(&pair.access_token, AuthTokenType::Access)
                    .is_ok()
            );
            assert!(
                verifier
                    .verify(&pair.refresh_token, AuthTokenType::Refresh)
                    .is_ok()
            );
        }
    }
}
