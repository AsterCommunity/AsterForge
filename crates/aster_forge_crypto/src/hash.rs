//! Feature-selected password hashing, keyed authentication and digest helpers.

#[cfg(feature = "password")]
#[path = "password.rs"]
mod password;
#[cfg(feature = "password")]
pub use password::{
    PasswordHashPolicy, PasswordHashVerification, PasswordHashVerificationLimits,
    PasswordHashWorkFactor, hash_password, hash_password_with_policy, validate_password_hash,
    verify_password, verify_password_with_policy,
};

#[cfg(feature = "hmac-sha256")]
use crate::{CryptoError, Result};
#[cfg(feature = "hmac-sha256")]
use hmac::{Hmac, KeyInit, Mac};
#[cfg(feature = "sha256")]
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Computes HMAC-SHA-256 over `data` and returns lowercase hex.
///
/// Products remain responsible for providing a high-entropy, purpose-specific key and managing
/// its lifecycle. This helper is suitable for keyed cache components and lookup digests; it does
/// not replace Argon2id for human passwords.
///
/// # Errors
///
/// Returns an error when the HMAC implementation rejects the supplied key.
#[cfg(feature = "hmac-sha256")]
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> Result<String> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(CryptoError::message_authentication)?;
    mac.update(data);
    Ok(bytes_to_hex(&mac.finalize().into_bytes()))
}

/// Computes the SHA-256 digest of `data` and returns lowercase hex.
#[must_use]
#[cfg(feature = "sha256")]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    bytes_to_hex(&hasher.finalize())
}

/// Encodes arbitrary bytes as lowercase hex.
#[must_use]
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Encodes a SHA-256 digest as lowercase hex.
#[must_use]
#[cfg(feature = "sha256")]
pub fn sha256_digest_to_hex(digest: &[u8]) -> String {
    bytes_to_hex(digest)
}

/// Creates a new incremental SHA-256 hasher.
#[must_use]
#[cfg(feature = "sha256")]
pub fn new_sha256() -> Sha256 {
    Sha256::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "sha256")]
    #[test]
    fn sha256_hex_matches_known_vectors_and_binary_input() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(&[0x00, 0xff, 0x10]),
            "2da45f2cd1f9c8e69a67abf7a6b26c282533d0a7686787a9533265418680d4d2"
        );
    }

    #[cfg(feature = "hmac-sha256")]
    #[test]
    fn hmac_sha256_matches_rfc_4231_vector_and_separates_keys() {
        let data = b"Hi There";
        let first = hmac_sha256_hex(&[0x0b; 20], data).unwrap();
        let second = hmac_sha256_hex(&[0x0c; 20], data).unwrap();

        assert_eq!(
            first,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_ne!(first, second);
        assert_ne!(first, hmac_sha256_hex(&[0x0b; 20], b"Hi There!").unwrap());
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?").unwrap(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hmac_sha256_hex(b"", b"").unwrap(),
            "b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"
        );
    }

    #[test]
    fn bytes_to_hex_encodes_lowercase_and_preserves_leading_zeroes() {
        assert_eq!(bytes_to_hex(&[]), "");
        assert_eq!(bytes_to_hex(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "000f10abff");
    }

    #[cfg(feature = "sha256")]
    #[test]
    fn sha256_digest_to_hex_matches_incremental_hasher_output() {
        let mut hasher = new_sha256();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(sha256_digest_to_hex(&hasher.finalize()), sha256_hex(b"abc"));
    }
}
