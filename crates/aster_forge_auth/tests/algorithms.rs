//! Fixed HMAC wire vectors shared by both backend test runs, independently checked with Python's
//! stdlib hmac/hashlib. They also pin the pre-feature JWT format (jsonwebtoken 11.1.0).

use aster_forge_auth::*;
use std::sync::Arc;

mod support;

const NOW: i64 = 1_700_000_000;
const KEY: &[u8] = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ACCESS: &str = "eyJzdWIiOiI0MiIsInNpZCI6InNlc3Npb24tMSIsInNlc3Npb25fdmVyc2lvbiI6NywidG9rZW5fdHlwZSI6ImFjY2VzcyIsInJlc3RyaWN0aW9uIjoibm9uZSIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjoxNzAwMDAwMDYwLCJpc3MiOiJpc3N1ZXIiLCJhdWQiOiJsb2NhbC13ZWIifQ";
const REFRESH: &str = "eyJzdWIiOiI0MiIsInNpZCI6InNlc3Npb24tMSIsInNlc3Npb25fdmVyc2lvbiI6NywidG9rZW5fdHlwZSI6InJlZnJlc2giLCJqdGkiOiJyZWZyZXNoLWp0aS1zZWNyZXQiLCJyZXN0cmljdGlvbiI6Im5vbmUiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6MTcwMDAwMDYwMCwiaXNzIjoiaXNzdWVyIiwiYXVkIjoibG9jYWwtd2ViIn0";

struct FixedIds;
impl IdSource for FixedIds {
    fn next_id(&self) -> Result<RefreshJti> {
        RefreshJti::new("refresh-jti-secret")
    }
}

fn policy(algorithm: JwtAlgorithm) -> JwtPolicy {
    support::initialize_provider();
    JwtPolicy::new("issuer", "local-web", 60, 600)
        .unwrap()
        .with_algorithm(algorithm)
}

fn signer(key: JwtKey, policy: JwtPolicy) -> JwtSigner {
    JwtSigner::new(
        key,
        policy,
        Arc::new(FixedClock::new(NOW)),
        Arc::new(FixedIds),
    )
}

fn issue(signer: &JwtSigner) -> Result<AuthTokenPair> {
    signer.issue(
        Subject::new("42").unwrap(),
        SessionId::new("session-1").unwrap(),
        SessionVersion::new(7),
        Restriction::None,
    )
}

#[test]
fn every_runtime_algorithm_preserves_wire_format_and_rejects_wrong_keys_and_algorithms() {
    for (algorithm, header, access_signature, refresh_signature) in [
        (
            JwtAlgorithm::Hs256,
            "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9",
            "ZswmuY4sNeweDS4IR0Uy-b9Z8oqrhrRTVtZwtsIRELk",
            "PliodjtFz14tXsxno0UD0xIVPfUkf_L5Vm7dqDhVR5w",
        ),
        (
            JwtAlgorithm::Hs384,
            "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzM4NCJ9",
            "cpZf5NBIOFn7dwyAEHgx_9cyK8-pdLTnLXbYQoDbILzlwHNoKpFtXPW3Mh6j7WBo",
            "Gr9msJ9hBCZUWHS13YmqdTcHVMfEyr1uztV9h3S_sVpJFV7RPBAMqPMZGu-JLlgB",
        ),
        (
            JwtAlgorithm::Hs512,
            "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzUxMiJ9",
            "ig988p0f6a3uEGfTJd1ZNKtTsRT4igXoN42pGRaWfyopMqGZCK6IxgqlqBsDal06FsdFTN5aom51GO_lGFE_vw",
            "q58mGmkrNm0beADSlABXFdBnVyflXO5LxUYaZy8WQqZvLBEm2FgZIztiILJyZdl5TXSKtUrHTx8O1qJu31Z7rw",
        ),
    ] {
        let policy = policy(algorithm);
        let pair = issue(&signer(JwtKey::hmac(KEY).unwrap(), policy.clone())).unwrap();
        let validator = JwtVerifier::new(
            JwtKey::hmac(KEY).unwrap(),
            policy.clone(),
            Arc::new(FixedClock::new(NOW)),
        );
        let wrong_key = JwtVerifier::new(
            JwtKey::hmac([b'z'; 64]).unwrap(),
            policy,
            Arc::new(FixedClock::new(NOW)),
        );
        for (token, payload, signature, kind) in [
            (
                &pair.access_token,
                ACCESS,
                access_signature,
                AuthTokenType::Access,
            ),
            (
                &pair.refresh_token,
                REFRESH,
                refresh_signature,
                AuthTokenType::Refresh,
            ),
        ] {
            let legacy = format!("{header}.{payload}.{signature}");
            assert_eq!(*token, legacy, "{algorithm:?} {kind:?}");
            let verified = validator.verify(&legacy, kind).unwrap();
            assert_eq!(verified.claims().sub.as_str(), "42");
            assert_eq!(wrong_key.verify(token, kind), Err(AuthError::InvalidToken));
            let tampered = format!("{header}.{payload}.a{}", &signature[1..]);
            assert_eq!(
                validator.verify(&tampered, kind),
                Err(AuthError::InvalidToken)
            );

            for other in [
                JwtAlgorithm::Hs256,
                JwtAlgorithm::Hs384,
                JwtAlgorithm::Hs512,
            ] {
                if other != algorithm {
                    let other = JwtVerifier::new(
                        JwtKey::hmac(KEY).unwrap(),
                        self::policy(other),
                        Arc::new(FixedClock::new(NOW)),
                    );
                    assert_eq!(other.verify(token, kind), Err(AuthError::AlgorithmMismatch));
                }
            }
            for header in [
                "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9", // RS256
                "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9", // ES256
                "eyJ0eXAiOiJKV1QiLCJhbGciOiJFZERTQSJ9", // EdDSA
            ] {
                let foreign = format!("{header}.{payload}.{signature}");
                assert_eq!(
                    validator.verify(&foreign, kind),
                    Err(AuthError::AlgorithmMismatch)
                );
            }
        }
    }
}

#[test]
fn runtime_algorithm_enforces_its_minimum_key_size_for_both_operations() {
    for (algorithm, minimum) in [
        (JwtAlgorithm::Hs256, 32),
        (JwtAlgorithm::Hs384, 48),
        (JwtAlgorithm::Hs512, 64),
    ] {
        let policy = policy(algorithm);
        let key = JwtKey::hmac(vec![42; minimum]).unwrap();
        let pair = issue(&signer(key.clone(), policy.clone())).unwrap();
        let verifier = JwtVerifier::new(key, policy.clone(), Arc::new(FixedClock::new(NOW)));
        assert!(
            verifier
                .verify(&pair.access_token, AuthTokenType::Access)
                .is_ok()
        );
        if minimum == 32 {
            assert!(JwtKey::hmac([42; 31]).is_err());
        } else {
            let key = JwtKey::hmac(vec![42; minimum - 1]).unwrap();
            assert_eq!(
                issue(&signer(key.clone(), policy.clone())),
                Err(AuthError::InvalidPolicy)
            );
            let verifier = JwtVerifier::new(key, policy, Arc::new(FixedClock::new(NOW)));
            assert_eq!(
                verifier.verify(&pair.access_token, AuthTokenType::Access),
                Err(AuthError::InvalidPolicy)
            );
        }
    }
}
