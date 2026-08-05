//! Versioned authenticated encryption for product-owned persisted secrets.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, Generate, KeyInit, Payload},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::{CryptoError, Result};

const ENVELOPE_VERSION: &str = "v1";
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
type AesNonce = Nonce<<Aes256Gcm as AeadCore>::NonceSize>;

/// Encrypts bytes into a versioned AES-256-GCM secret envelope.
///
/// `context` is used as HKDF info and must be a stable, non-empty product-owned purpose string.
/// `aad` is authenticated but not stored in the envelope. Both values are persistence contracts:
/// changing either one makes existing ciphertext fail authentication.
///
/// # Errors
///
/// Returns an error when `context` is empty, key derivation fails, or authenticated encryption
/// fails. Error values never contain the master key, context, AAD, plaintext, or ciphertext.
pub fn encrypt_secret(
    master_key: &[u8],
    context: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<String> {
    let nonce = AesNonce::generate();
    encrypt_secret_with_nonce(master_key, context, aad, plaintext, nonce.as_slice())
}

/// Decrypts a strict `v1:<base64url nonce>:<base64url ciphertext>` secret envelope.
///
/// # Errors
///
/// Returns a classified error for an empty context, malformed envelope, unsupported version, key
/// derivation failure, or authentication failure. Error values never contain secret material.
pub fn decrypt_secret(
    master_key: &[u8],
    context: &[u8],
    aad: &[u8],
    envelope: &str,
) -> Result<Vec<u8>> {
    if context.is_empty() {
        return Err(CryptoError::InvalidSecretEnvelopePolicy);
    }
    let (nonce, ciphertext) = parse_envelope(envelope)?;
    let cipher = cipher(master_key, context)?;
    let nonce =
        AesNonce::try_from(nonce.as_slice()).map_err(|_| CryptoError::InvalidSecretEnvelope)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::SecretEnvelopeAuthentication)
}

fn cipher(master_key: &[u8], context: &[u8]) -> Result<Aes256Gcm> {
    if context.is_empty() {
        return Err(CryptoError::InvalidSecretEnvelopePolicy);
    }
    let hkdf = Hkdf::<Sha256>::new(None, master_key);
    let mut key = [0_u8; KEY_LEN];
    hkdf.expand(context, &mut key)
        .map_err(|_| CryptoError::SecretEnvelopeKeyDerivation)?;
    Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::SecretEnvelopeKeyDerivation)
}

fn parse_envelope(envelope: &str) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let mut parts = envelope.split(':');
    let version = parts.next().ok_or(CryptoError::InvalidSecretEnvelope)?;
    let nonce = parts.next().ok_or(CryptoError::InvalidSecretEnvelope)?;
    let ciphertext = parts.next().ok_or(CryptoError::InvalidSecretEnvelope)?;
    if parts.next().is_some() || nonce.is_empty() || ciphertext.is_empty() {
        return Err(CryptoError::InvalidSecretEnvelope);
    }
    if version != ENVELOPE_VERSION {
        return Err(CryptoError::UnsupportedSecretEnvelopeVersion);
    }

    let nonce = URL_SAFE_NO_PAD
        .decode(nonce)
        .map_err(|_| CryptoError::InvalidSecretEnvelope)?;
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| CryptoError::InvalidSecretEnvelope)?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(ciphertext)
        .map_err(|_| CryptoError::InvalidSecretEnvelope)?;
    if ciphertext.is_empty() {
        return Err(CryptoError::InvalidSecretEnvelope);
    }
    Ok((nonce, ciphertext))
}

fn encrypt_secret_with_nonce(
    master_key: &[u8],
    context: &[u8],
    aad: &[u8],
    plaintext: &[u8],
    nonce: &[u8],
) -> Result<String> {
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| CryptoError::InvalidSecretEnvelopePolicy)?;
    let nonce = AesNonce::try_from(nonce.as_slice())
        .map_err(|_| CryptoError::InvalidSecretEnvelopePolicy)?;
    let cipher = cipher(master_key, context)?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::SecretEnvelopeEncryption)?;
    Ok(format!(
        "{ENVELOPE_VERSION}:{}:{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(ciphertext)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER_KEY: &[u8] = b"forge-secret-envelope-test-master-key";
    const MFA_CONTEXT: &[u8] = b"asterdrive:mfa-secret:v1";
    const STORAGE_CONTEXT: &[u8] = b"asterdrive:storage-credential-token:v1";
    const FIXED_NONCE: [u8; NONCE_LEN] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

    fn fixture(context: &[u8], aad: &[u8], plaintext: &[u8]) -> String {
        encrypt_secret_with_nonce(MASTER_KEY, context, aad, plaintext, &FIXED_NONCE)
            .expect("fixed fixture encryption should succeed")
    }

    #[test]
    fn round_trips_empty_binary_and_large_plaintext() {
        for plaintext in [Vec::new(), vec![0, 255, 1, 128, 2], vec![0x5a; 1024 * 1024]] {
            let envelope = encrypt_secret(MASTER_KEY, MFA_CONTEXT, b"factor:7", &plaintext)
                .expect("secret encryption should succeed");
            assert_eq!(
                decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"factor:7", &envelope)
                    .expect("secret decryption should succeed"),
                plaintext
            );
        }
    }

    #[test]
    fn fixed_drive_context_fixtures_are_stable_and_decryptable() {
        let mfa = fixture(MFA_CONTEXT, b"mfa_factor:7:totp", b"JBSWY3DPEHPK3PXP");
        let storage = fixture(
            STORAGE_CONTEXT,
            b"storage_policy_credential:9:microsoft_graph:access",
            b"opaque-access-token",
        );

        assert_eq!(
            mfa,
            "v1:AAECAwQFBgcICQoL:pt1VIrNAcBeWaV0OT5oopWy1VJSEeAF3WeFu3yRJ_EE"
        );
        assert_eq!(
            storage,
            "v1:AAECAwQFBgcICQoL:lVf2A9KFG97Bm8ru8l9wF-i_taTNtAbZ-MYT3Kujr95sOUE"
        );
        assert_eq!(
            decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"mfa_factor:7:totp", &mfa)
                .expect("MFA fixture should decrypt"),
            b"JBSWY3DPEHPK3PXP"
        );
        assert_eq!(
            decrypt_secret(
                MASTER_KEY,
                STORAGE_CONTEXT,
                b"storage_policy_credential:9:microsoft_graph:access",
                &storage,
            )
            .expect("storage fixture should decrypt"),
            b"opaque-access-token"
        );
    }

    #[test]
    fn rejects_wrong_key_context_and_aad() {
        let envelope = fixture(MFA_CONTEXT, b"aad-one", b"secret-value");

        for result in [
            decrypt_secret(b"wrong-key", MFA_CONTEXT, b"aad-one", &envelope),
            decrypt_secret(MASTER_KEY, STORAGE_CONTEXT, b"aad-one", &envelope),
            decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"aad-two", &envelope),
        ] {
            assert!(matches!(
                result,
                Err(CryptoError::SecretEnvelopeAuthentication)
            ));
        }
    }

    #[test]
    fn rejects_malformed_unknown_version_and_invalid_nonce() {
        for envelope in [
            "",
            "v1",
            "v1::ciphertext",
            "v1:nonce:",
            "v1:a:b:extra",
            "v1:not_base64!:ciphertext",
            "v1:AA:ciphertext",
        ] {
            assert!(matches!(
                decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"aad", envelope),
                Err(CryptoError::InvalidSecretEnvelope)
            ));
        }
        assert!(matches!(
            decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"aad", "v2:AA:AA"),
            Err(CryptoError::UnsupportedSecretEnvelopeVersion)
        ));
        assert!(matches!(
            decrypt_secret(MASTER_KEY, b"", b"aad", "v1:AA:AA"),
            Err(CryptoError::InvalidSecretEnvelopePolicy)
        ));
        assert!(matches!(
            encrypt_secret(MASTER_KEY, b"", b"aad", b"secret"),
            Err(CryptoError::InvalidSecretEnvelopePolicy)
        ));
    }

    #[test]
    fn rejects_truncated_and_tampered_ciphertext() {
        let envelope = fixture(MFA_CONTEXT, b"aad", b"secret-value");
        let (version_and_nonce, encoded_ciphertext) = envelope
            .rsplit_once(':')
            .expect("fixture should contain ciphertext");
        let ciphertext = URL_SAFE_NO_PAD
            .decode(encoded_ciphertext)
            .expect("fixture ciphertext should decode");

        for changed in [ciphertext[..ciphertext.len() - 1].to_vec(), {
            let mut tampered = ciphertext.clone();
            tampered[0] ^= 0x80;
            tampered
        }] {
            let changed = format!("{version_and_nonce}:{}", URL_SAFE_NO_PAD.encode(changed));
            assert!(matches!(
                decrypt_secret(MASTER_KEY, MFA_CONTEXT, b"aad", &changed),
                Err(CryptoError::SecretEnvelopeAuthentication)
            ));
        }
    }

    #[test]
    fn errors_do_not_expose_secret_material() {
        let secret = "SENSITIVE_MASTER_KEY_AND_PLAINTEXT";
        let envelope = fixture(MFA_CONTEXT, b"aad", secret.as_bytes());
        let error = decrypt_secret(secret.as_bytes(), MFA_CONTEXT, b"wrong-aad", &envelope)
            .expect_err("wrong key and AAD should fail authentication");

        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
        assert!(!error.to_string().contains(&envelope));
        assert!(!format!("{error:?}").contains(&envelope));
    }
}
