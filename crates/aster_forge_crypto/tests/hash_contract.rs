use aster_forge_crypto::{
    CryptoError, PasswordHashPolicy, PasswordHashVerificationLimits, PasswordHashWorkFactor,
    hash_password, hash_password_with_policy, hmac_sha256_hex, verify_password,
    verify_password_with_policy,
};

fn lightweight_policy() -> PasswordHashPolicy {
    PasswordHashPolicy::new(
        PasswordHashWorkFactor::new(8, 1, 1, 32).unwrap(),
        PasswordHashVerificationLimits::new(8, 1, 1, 32).unwrap(),
    )
    .unwrap()
}

#[test]
fn public_default_api_uses_rfc_profile_and_verifies() {
    let hash = hash_password("public default contract").unwrap();

    assert!(hash.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"));
    assert!(verify_password("public default contract", &hash).unwrap());
    assert!(!verify_password("wrong password", &hash).unwrap());
}

#[test]
fn public_custom_policy_controls_hashing_and_every_verification_limit() {
    let policy = lightweight_policy();
    let hash = hash_password_with_policy("custom policy", &policy).unwrap();

    assert!(hash.starts_with("$argon2id$v=19$m=8,t=1,p=1$"));
    let verification = verify_password_with_policy("custom policy", &hash, &policy).unwrap();
    assert!(verification.is_valid);
    assert!(!verification.needs_rehash);

    for (parameter, over_limit) in [
        ("m", hash.replacen("m=8", "m=9", 1)),
        ("t", hash.replacen("t=1", "t=2", 1)),
    ] {
        assert!(matches!(
            verify_password_with_policy("custom policy", &over_limit, &policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: actual_parameter,
                ..
            }) if actual_parameter == parameter
        ));
    }

    let parallel_policy = PasswordHashPolicy::new(
        PasswordHashWorkFactor::new(16, 1, 1, 32).unwrap(),
        PasswordHashVerificationLimits::new(16, 1, 1, 32).unwrap(),
    )
    .unwrap();
    let parallel_hash = hash_password_with_policy("custom policy", &parallel_policy).unwrap();
    let over_parallelism = parallel_hash.replacen("p=1", "p=2", 1);
    assert!(matches!(
        verify_password_with_policy("custom policy", &over_parallelism, &parallel_policy),
        Err(CryptoError::PasswordHashVerificationLimit { parameter: "p", .. })
    ));

    let output_policy = PasswordHashPolicy::new(
        PasswordHashWorkFactor::new(8, 1, 1, 33).unwrap(),
        PasswordHashVerificationLimits::new(8, 1, 1, 33).unwrap(),
    )
    .unwrap();
    let output_hash = hash_password_with_policy("custom policy", &output_policy).unwrap();
    assert!(matches!(
        verify_password_with_policy("custom policy", &output_hash, &policy),
        Err(CryptoError::PasswordHashVerificationLimit {
            parameter: "output_length",
            actual: 33,
            maximum: 32,
        })
    ));
}

#[test]
fn public_detailed_verification_marks_old_default_for_rehash() {
    let legacy_policy = PasswordHashPolicy::new(
        PasswordHashWorkFactor::new(19 * 1024, 2, 1, 32).unwrap(),
        PasswordHashVerificationLimits::new(19 * 1024, 2, 1, 32).unwrap(),
    )
    .unwrap();
    let hash = hash_password_with_policy("legacy password", &legacy_policy).unwrap();

    let verification =
        verify_password_with_policy("legacy password", &hash, &PasswordHashPolicy::default())
            .unwrap();
    assert!(verification.is_valid);
    assert!(verification.needs_rehash);
}

#[test]
fn public_error_and_hmac_contracts_remain_stable() {
    assert!(matches!(
        verify_password("password", "not-a-phc-string"),
        Err(CryptoError::PasswordHash(_))
    ));
    assert_eq!(
        hmac_sha256_hex(&[0x0b; 20], b"Hi There").unwrap(),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}
