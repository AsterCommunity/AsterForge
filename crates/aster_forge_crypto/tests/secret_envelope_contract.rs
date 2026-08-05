use aster_forge_crypto::{CryptoError, decrypt_secret, encrypt_secret};

const MASTER_KEY: &[u8] = b"forge-secret-envelope-test-master-key";
const MFA_CONTEXT: &[u8] = b"asterdrive:mfa-secret:v1";
const MFA_FIXTURE: &str = "v1:AAECAwQFBgcICQoL:pt1VIrNAcBeWaV0OT5oopWy1VJSEeAF3WeFu3yRJ_EE";

#[test]
fn public_secret_envelope_api_round_trips_and_reads_drive_fixture() {
    let envelope = encrypt_secret(MASTER_KEY, MFA_CONTEXT, b"factor:8", b"binary\0secret")
        .expect("public encryption API should succeed");
    assert_eq!(
        decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"factor:8", &envelope)
            .expect("public decryption API should succeed"),
        b"binary\0secret"
    );
    assert_eq!(
        decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"mfa_factor:7:totp", MFA_FIXTURE,)
            .expect("fixed Drive-compatible fixture should decrypt"),
        b"JBSWY3DPEHPK3PXP"
    );
}

#[test]
fn public_secret_envelope_errors_are_classified_and_redacted() {
    assert!(matches!(
        decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"aad", "v2:AA:AA"),
        Err(CryptoError::UnsupportedSecretEnvelopeVersion)
    ));
    assert!(matches!(
        decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"wrong-aad", MFA_FIXTURE),
        Err(CryptoError::SecretEnvelopeAuthentication)
    ));

    let error = decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"wrong-aad", MFA_FIXTURE)
        .expect_err("wrong AAD should fail authentication");
    assert_eq!(error.to_string(), "secret envelope authentication failed");
    assert!(!format!("{error:?}").contains(MFA_FIXTURE));
}
