//! Every initialization scenario runs in its own process, isolating upstream global state.

use aster_forge_auth::{
    AuthError, AuthTokenType, JwtKey, JwtPolicy, JwtSigner, JwtVerifier, Restriction, SessionId,
    SessionVersion, Subject, initialize_jwt_crypto_provider,
};
use std::process::Command;

const SCENARIO: &str = "ASTER_AUTH_PROVIDER_SCENARIO";

#[test]
fn initialization_contracts_are_process_isolated() {
    for scenario in [
        "missing",
        "initialized",
        "external",
        "automatic",
        "concurrent",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "provider_child", "--ignored", "--nocapture"])
            .env(SCENARIO, scenario)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "invoked in fresh processes by initialization_contracts_are_process_isolated"]
fn provider_child() {
    let scenario = std::env::var(SCENARIO).unwrap();
    match scenario.as_str() {
        "missing" => assert_unavailable(),
        "initialized" => {
            initialize_jwt_crypto_provider().unwrap();
            assert_available();
            assert_eq!(
                initialize_jwt_crypto_provider(),
                Err(AuthError::CryptoProviderAlreadyInstalled)
            );
            assert_available();
        }
        "external" => {
            #[cfg(feature = "aws-lc-rs")]
            let provider = &jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER;
            #[cfg(feature = "rust-crypto")]
            let provider = &jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
            provider.install_default().unwrap();
            assert_conflict();
        }
        "automatic" => {
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &serde_json::json!({"sub": "upstream-initialized-first"}),
                &jsonwebtoken::EncodingKey::from_secret(&[42; 32]),
            )
            .unwrap();
            assert_conflict();
        }
        "concurrent" => {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        initialize_jwt_crypto_provider()
                    })
                })
                .collect();
            let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
            assert_eq!(
                results
                    .iter()
                    .filter(|r| **r == Err(AuthError::CryptoProviderAlreadyInstalled))
                    .count(),
                1
            );
            assert_available();
        }
        _ => panic!("unknown provider test scenario"),
    }
}

fn signer() -> JwtSigner {
    JwtSigner::with_system_clock(
        JwtKey::hmac([42; 32]).unwrap(),
        JwtPolicy::new("product", "local-web", 60, 600).unwrap(),
    )
}

fn issue() -> aster_forge_auth::Result<aster_forge_auth::AuthTokenPair> {
    signer().issue(
        Subject::new("42").unwrap(),
        SessionId::new("session-1").unwrap(),
        SessionVersion::new(1),
        Restriction::None,
    )
}

fn verifier() -> JwtVerifier {
    JwtVerifier::with_system_clock(
        JwtKey::hmac([42; 32]).unwrap(),
        JwtPolicy::new("product", "local-web", 60, 600).unwrap(),
    )
}

fn assert_available() {
    let pair = issue().unwrap();
    assert!(
        verifier()
            .verify(&pair.access_token, AuthTokenType::Access)
            .is_ok()
    );
}

fn assert_unavailable() {
    assert_eq!(issue(), Err(AuthError::CryptoProviderNotInitialized));
    assert_eq!(
        verifier().verify("invalid", AuthTokenType::Access),
        Err(AuthError::CryptoProviderNotInitialized)
    );
}

fn assert_conflict() {
    assert_eq!(
        initialize_jwt_crypto_provider(),
        Err(AuthError::CryptoProviderAlreadyInstalled)
    );
    assert_unavailable();
}
