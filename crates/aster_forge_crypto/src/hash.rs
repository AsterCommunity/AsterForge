//! Password hashing and digest helpers.
//!
//! Passwords are hashed with Argon2 and a fresh salt for storage, while SHA-256 helpers cover the
//! deterministic digest cases used by services. The module keeps formatting helpers centralized so
//! hex output stays lowercase and stable across call sites.

use crate::{CryptoError, Result};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core_06::OsRng;
use sha2::{Digest, Sha256};
use std::fmt::Write;

fn password_hasher() -> Result<Argon2<'static>> {
    Ok(Argon2::default())
}

/// Hashes a password with Argon2 and a fresh random salt.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    password_hasher()?
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(CryptoError::password_hash)
}

/// Verifies a password against a stored Argon2 password hash.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(CryptoError::password_hash)?;
    Ok(password_hasher()?
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Computes the SHA-256 digest of `data` and returns lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    bytes_to_hex(&hasher.finalize())
}

/// Encodes arbitrary bytes as lowercase hex.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Encodes a SHA-256 digest as lowercase hex.
pub fn sha256_digest_to_hex(digest: &[u8]) -> String {
    bytes_to_hex(digest)
}

/// Creates a new incremental SHA-256 hasher.
pub fn new_sha256() -> Sha256 {
    Sha256::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn bytes_to_hex_encodes_lowercase_and_preserves_leading_zeroes() {
        assert_eq!(bytes_to_hex(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "000f10abff");
    }

    #[test]
    fn sha256_digest_to_hex_matches_incremental_hasher_output() {
        let mut hasher = new_sha256();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(sha256_digest_to_hex(&hasher.finalize()), sha256_hex(b"abc"));
    }

    #[test]
    fn password_hash_verifies_matching_password_and_rejects_wrong_password() {
        let hash = hash_password("correct horse battery staple").unwrap();

        assert!(verify_password("correct horse battery staple", &hash).unwrap());
        assert!(!verify_password("wrong password", &hash).unwrap());
        assert!(hash.starts_with("$argon2"));
    }

    #[test]
    fn password_hash_uses_fresh_salt() {
        let first = hash_password("same password").unwrap();
        let second = hash_password("same password").unwrap();

        assert_ne!(first, second);
        assert!(verify_password("same password", &first).unwrap());
        assert!(verify_password("same password", &second).unwrap());
    }

    #[test]
    fn malformed_password_hash_returns_error() {
        let error = verify_password("password", "not-a-password-hash").unwrap_err();

        assert!(matches!(error, CryptoError::PasswordHash(_)));
    }
}
