//! Signed-wire, resource, namespace, time and redaction contracts; no wall-clock sleeps.
use aster_forge_auth::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::sync::Arc;

mod support;

const NOW: i64 = 1_700_000_000;
const KEY: &[u8] = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
struct TestIds;
impl IdSource for TestIds {
    fn next_id(&self) -> Result<RefreshJti> {
        RefreshJti::new("refresh-jti-secret")
    }
}
fn policy() -> JwtPolicy {
    support::initialize_provider();
    JwtPolicy::new("issuer", "local-web", 60, 600).unwrap()
}
fn signer(at: i64) -> JwtSigner {
    JwtSigner::new(
        JwtKey::hmac(KEY).unwrap(),
        policy(),
        Arc::new(FixedClock::new(at)),
        Arc::new(TestIds),
    )
}
fn verifier(at: i64) -> JwtVerifier {
    JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy(),
        Arc::new(FixedClock::new(at)),
    )
}
fn pair() -> AuthTokenPair {
    signer(NOW)
        .issue(
            Subject::new("42").unwrap(),
            SessionId::new("session-1").unwrap(),
            SessionVersion::new(7),
            Restriction::None,
        )
        .unwrap()
}
fn claims() -> Value {
    json!({"sub":"42","sid":"session-1","session_version":7,"token_type":"refresh","jti":"refresh-jti-secret","restriction":"none","iat":NOW,"exp":NOW+600,"iss":"issuer","aud":"local-web"})
}
fn signed(value: &Value) -> String {
    support::initialize_provider();
    encode(
        &Header::new(Algorithm::HS256),
        value,
        &EncodingKey::from_secret(KEY),
    )
    .unwrap()
}
fn verify(value: &Value) -> Result<VerifiedToken> {
    verifier(NOW).verify(&signed(value), AuthTokenType::Refresh)
}

#[test]
fn pairs_bind_integer_and_uuid_subjects_to_one_session_and_distinct_purposes() {
    for subject in ["42", "dbf9a5ac-a36a-4bfb-8d65-4e9a8954b741"] {
        for restriction in [Restriction::None, Restriction::MustChangePassword] {
            let pair = signer(NOW)
                .issue(
                    Subject::new(subject).unwrap(),
                    SessionId::new("session-1").unwrap(),
                    SessionVersion::new(7),
                    restriction,
                )
                .unwrap();
            let access = verifier(NOW)
                .verify(&pair.access_token, AuthTokenType::Access)
                .unwrap()
                .into_claims();
            let refresh = verifier(NOW)
                .verify(&pair.refresh_token, AuthTokenType::Refresh)
                .unwrap()
                .into_claims();
            assert_eq!(access.sub.as_str(), subject);
            assert_eq!(access.sid, refresh.sid);
            assert_eq!(access.session_version.value(), 7);
            assert_eq!(access.restriction, restriction);
            assert_eq!(access.exp - access.iat, 60);
            assert_eq!(refresh.exp - refresh.iat, 600);
            assert!(access.jti.is_none());
            assert_eq!(refresh.jti, Some(pair.refresh_jti));
            assert_eq!(
                verifier(NOW).verify(&pair.access_token, AuthTokenType::Refresh),
                Err(AuthError::WrongTokenType)
            );
            assert_eq!(
                verifier(NOW).verify(&pair.refresh_token, AuthTokenType::Access),
                Err(AuthError::WrongTokenType)
            );
        }
    }
}

#[test]
fn all_required_claims_are_rejected_when_absent() {
    for field in [
        "sub",
        "sid",
        "session_version",
        "token_type",
        "jti",
        "restriction",
        "iat",
        "exp",
        "iss",
        "aud",
    ] {
        let mut value = claims();
        value.as_object_mut().unwrap().remove(field);
        assert!(verify(&value).is_err(), "missing {field}");
    }
}

#[test]
fn signed_invalid_identifiers_cannot_bypass_newtype_constructors() {
    for field in ["sub", "sid", "jti"] {
        for bad in [
            String::new(),
            " ".into(),
            "line\nbreak".into(),
            "x".repeat(257),
        ] {
            let mut value = claims();
            value[field] = json!(bad);
            assert!(verify(&value).is_err(), "invalid {field}");
        }
    }
    assert!(Subject::new("x".repeat(256)).is_ok());
    assert!(SessionId::new("valid/session").is_ok());
}

#[test]
fn strict_namespace_and_claim_types_prevent_cross_product_or_cross_protocol_use() {
    for (field, bad, error) in [
        ("iss", json!("other"), AuthError::IssuerMismatch),
        ("aud", json!("oauth-api"), AuthError::AudienceMismatch),
        ("aud", json!(["local-web"]), AuthError::InvalidToken),
        ("token_type", json!("id_token"), AuthError::InvalidToken),
        (
            "restriction",
            json!("administrator"),
            AuthError::InvalidToken,
        ),
        ("session_version", json!(-1), AuthError::InvalidToken),
        ("session_version", json!("7"), AuthError::InvalidToken),
        ("iat", json!(u64::MAX), AuthError::InvalidToken),
        ("exp", json!("tomorrow"), AuthError::InvalidToken),
    ] {
        let mut value = claims();
        value[field] = bad;
        assert_eq!(verify(&value), Err(error), "{field}");
    }
    let old = json!({"sub":"42","user_id":42,"session_version":7,"token_type":"refresh","password_change":false,"jti":"old","exp":NOW+600});
    assert!(verify(&old).is_err());
}

#[test]
fn key_and_algorithm_are_checked_even_for_validly_signed_tokens() {
    let token = signed(&claims());
    let wrong = JwtVerifier::new(
        JwtKey::hmac([b'z'; 64]).unwrap(),
        policy(),
        Arc::new(FixedClock::new(NOW)),
    );
    assert_eq!(
        wrong.verify(&token, AuthTokenType::Refresh),
        Err(AuthError::InvalidToken)
    );
    for (algorithm, upstream) in [
        (JwtAlgorithm::Hs256, Algorithm::HS256),
        (JwtAlgorithm::Hs384, Algorithm::HS384),
        (JwtAlgorithm::Hs512, Algorithm::HS512),
    ] {
        let token = encode(
            &Header::new(upstream),
            &claims(),
            &EncodingKey::from_secret(KEY),
        )
        .unwrap();
        let validator = JwtVerifier::new(
            JwtKey::hmac(KEY).unwrap(),
            policy().with_algorithm(algorithm),
            Arc::new(FixedClock::new(NOW)),
        );
        assert!(validator.verify(&token, AuthTokenType::Refresh).is_ok());
        if upstream != Algorithm::HS256 {
            assert_eq!(
                verifier(NOW).verify(&token, AuthTokenType::Refresh),
                Err(AuthError::AlgorithmMismatch)
            );
        }
    }
    let mut changed = token.into_bytes();
    let last = changed.len() - 2;
    changed[last] = if changed[last] == b'a' { b'b' } else { b'a' };
    assert!(
        verifier(NOW)
            .verify(
                std::str::from_utf8(&changed).unwrap(),
                AuthTokenType::Refresh
            )
            .is_err()
    );
}

#[test]
fn exclusive_expiry_inclusive_not_before_and_future_issuance_boundaries() {
    let pair = pair();
    assert!(
        verifier(NOW + 59)
            .verify(&pair.access_token, AuthTokenType::Access)
            .is_ok()
    );
    assert_eq!(
        verifier(NOW + 60).verify(&pair.access_token, AuthTokenType::Access),
        Err(AuthError::Expired)
    );
    let skew = JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy().with_leeway(5).unwrap(),
        Arc::new(FixedClock::new(NOW + 64)),
    );
    assert!(
        skew.verify(&pair.access_token, AuthTokenType::Access)
            .is_ok()
    );
    let skew_end = JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy().with_leeway(5).unwrap(),
        Arc::new(FixedClock::new(NOW + 65)),
    );
    assert_eq!(
        skew_end.verify(&pair.access_token, AuthTokenType::Access),
        Err(AuthError::Expired)
    );
    let mut value = claims();
    value["nbf"] = json!(NOW + 1);
    assert_eq!(verify(&value), Err(AuthError::NotYetValid));
    assert!(
        verifier(NOW + 1)
            .verify(&signed(&value), AuthTokenType::Refresh)
            .is_ok()
    );
    value = claims();
    value["iat"] = json!(NOW + 1);
    assert_eq!(verify(&value), Err(AuthError::NotYetValid));
}

#[test]
fn negative_inconsistent_and_overflowing_times_fail_without_panics() {
    for (field, bad) in [
        ("iat", -1),
        ("exp", NOW),
        ("exp", -1),
        ("nbf", -1),
        ("nbf", NOW + 600),
    ] {
        let mut value = claims();
        value[field] = json!(bad);
        assert_eq!(verify(&value), Err(AuthError::InvalidToken));
    }
    assert_eq!(
        signer(-1).issue(
            Subject::new("42").unwrap(),
            SessionId::new("s").unwrap(),
            SessionVersion::new(1),
            Restriction::None
        ),
        Err(AuthError::InvalidPolicy)
    );
    assert_eq!(
        signer(i64::MAX - 10).issue(
            Subject::new("42").unwrap(),
            SessionId::new("s").unwrap(),
            SessionVersion::new(1),
            Restriction::None
        ),
        Err(AuthError::InvalidPolicy)
    );
    let mut value = claims();
    value["iat"] = json!(i64::MAX - 100);
    value["exp"] = json!(i64::MAX);
    let edge = JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy().with_leeway(10).unwrap(),
        Arc::new(FixedClock::new(i64::MAX)),
    );
    assert!(edge.verify(&signed(&value), AuthTokenType::Refresh).is_ok());
}

#[test]
fn malformed_and_oversized_tokens_fail_before_unbounded_parsing() {
    for token in [
        "",
        ".",
        "a.b.c",
        "a.b.c.d",
        "eyJhbGciOiJub25lIn0.e30.",
        "💥",
    ] {
        assert!(verifier(NOW).verify(token, AuthTokenType::Access).is_err());
    }
    assert_eq!(
        verifier(NOW).verify(&"x".repeat(16385), AuthTokenType::Access),
        Err(AuthError::TokenTooLarge)
    );
    let pair = pair();
    let boundary = pair.refresh_token.len();
    let exact = JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy().with_max_token_bytes(boundary).unwrap(),
        Arc::new(FixedClock::new(NOW)),
    );
    assert!(
        exact
            .verify(&pair.refresh_token, AuthTokenType::Refresh)
            .is_ok()
    );
    let too_small = JwtVerifier::new(
        JwtKey::hmac(KEY).unwrap(),
        policy().with_max_token_bytes(boundary - 1).unwrap(),
        Arc::new(FixedClock::new(NOW)),
    );
    assert_eq!(
        too_small.verify(&pair.refresh_token, AuthTokenType::Refresh),
        Err(AuthError::TokenTooLarge)
    );
}

#[test]
fn policy_key_budget_and_version_limits_are_checked() {
    assert!(JwtPolicy::new("", "web", 1, 1).is_err());
    assert!(JwtPolicy::new("issuer", " ", 1, 1).is_err());
    assert!(JwtPolicy::new("issuer", "web", 0, 1).is_err());
    assert!(JwtPolicy::new("issuer", "web", u64::MAX, 1).is_err());
    assert!(policy().with_leeway(u64::MAX).is_err());
    assert!(policy().with_max_token_bytes(0).is_err());
    assert!(JwtKey::hmac([0; 31]).is_err());
    assert!(JwtKey::hmac([0; 32]).is_ok());
    let short = JwtVerifier::new(
        JwtKey::hmac([0; 32]).unwrap(),
        policy().with_algorithm(JwtAlgorithm::Hs512),
        Arc::new(FixedClock::new(NOW)),
    );
    assert_eq!(
        short.verify(&signed(&claims()), AuthTokenType::Refresh),
        Err(AuthError::InvalidPolicy)
    );
    assert!(SessionVersion::new(u64::MAX).next().is_err());
    assert_eq!(SessionVersion::new(7).next().unwrap().value(), 8);
}

#[test]
fn access_cannot_carry_a_refresh_identity_and_generation_failures_do_not_leak_pairs() {
    struct FailedIds;
    impl IdSource for FailedIds {
        fn next_id(&self) -> Result<RefreshJti> {
            Err(AuthError::Encoding)
        }
    }
    let mut value = claims();
    value["token_type"] = json!("access");
    assert_eq!(
        verifier(NOW).verify(&signed(&value), AuthTokenType::Access),
        Err(AuthError::InvalidToken)
    );
    let failed = JwtSigner::new(
        JwtKey::hmac(KEY).unwrap(),
        policy(),
        Arc::new(FixedClock::new(NOW)),
        Arc::new(FailedIds),
    );
    assert_eq!(
        failed.issue(
            Subject::new("42").unwrap(),
            SessionId::new("s").unwrap(),
            SessionVersion::new(7),
            Restriction::None
        ),
        Err(AuthError::Encoding)
    );
}

#[test]
fn all_secret_bearing_debug_and_error_paths_are_redacted() {
    let pair = pair();
    let verified = verifier(NOW)
        .verify(&pair.refresh_token, AuthTokenType::Refresh)
        .unwrap();
    let debug = format!(
        "{pair:?} {verified:?} {:?} {:?}",
        JwtKey::hmac(KEY).unwrap(),
        RefreshError::Store("sensitive db input")
    );
    for secret in [
        &pair.access_token,
        &pair.refresh_token,
        &pair.refresh_jti.as_str().to_string(),
        &"sensitive db input".to_string(),
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(!format!("{}", RefreshError::Store("sensitive db input")).contains("sensitive"));
}
