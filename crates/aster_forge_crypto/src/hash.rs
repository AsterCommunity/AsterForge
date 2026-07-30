//! Password hashing, keyed authentication, and digest helpers.
//!
//! Passwords are hashed with an explicit Argon2id policy and a fresh salt for storage. Stored PHC
//! strings are checked against finite verification limits before Argon2 allocates its work memory.
//! SHA-256 helpers cover deterministic digest cases, while HMAC-SHA-256 covers keyed cache and
//! lookup components that must not expose a fast, reusable digest of a low-entropy secret.

use crate::{CryptoError, Result};
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{
        Error as PasswordHashError, Output, PasswordHash, PasswordHasher, PasswordVerifier,
        SaltString,
    },
};
use hmac::{Hmac, KeyInit, Mac};
use rand_core_06::OsRng;
use sha2::{Digest, Sha256};
use std::fmt::Write;

const ARGON2_VERSION: u32 = 19;
const PASSWORD_SALT_LENGTH: usize = 16;
const RFC_9106_SECOND_MEMORY_KIB: u32 = 64 * 1024;
const RFC_9106_SECOND_ITERATIONS: u32 = 3;
const RFC_9106_SECOND_PARALLELISM: u32 = 4;
const DEFAULT_PASSWORD_HASH_OUTPUT_LENGTH: usize = 32;

/// Argon2id work factor used when creating new password hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordHashWorkFactor {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_length: usize,
}

impl PasswordHashWorkFactor {
    /// Returns the RFC 9106 second recommended Argon2id profile.
    ///
    /// This uses 64 MiB of memory, three iterations, four lanes, and a 32-byte output.
    #[must_use]
    pub const fn rfc_9106_second_recommended() -> Self {
        Self {
            memory_kib: RFC_9106_SECOND_MEMORY_KIB,
            iterations: RFC_9106_SECOND_ITERATIONS,
            parallelism: RFC_9106_SECOND_PARALLELISM,
            output_length: DEFAULT_PASSWORD_HASH_OUTPUT_LENGTH,
        }
    }

    /// Creates a custom Argon2id work factor.
    ///
    /// # Errors
    ///
    /// Returns an error when the output length or Argon2 cost parameters fall outside the ranges
    /// accepted by the Argon2 implementation.
    pub fn new(
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
        output_length: usize,
    ) -> Result<Self> {
        let work_factor = Self {
            memory_kib,
            iterations,
            parallelism,
            output_length,
        };
        if !(Output::MIN_LENGTH..=Output::MAX_LENGTH).contains(&output_length) {
            return Err(CryptoError::password_hash_policy(format_args!(
                "output length must be between {} and {} bytes",
                Output::MIN_LENGTH,
                Output::MAX_LENGTH
            )));
        }
        work_factor.params()?;
        Ok(work_factor)
    }

    /// Memory cost in KiB.
    #[must_use]
    pub const fn memory_kib(self) -> u32 {
        self.memory_kib
    }

    /// Number of Argon2 passes.
    #[must_use]
    pub const fn iterations(self) -> u32 {
        self.iterations
    }

    /// Number of Argon2 lanes.
    #[must_use]
    pub const fn parallelism(self) -> u32 {
        self.parallelism
    }

    /// Password-hash output length in bytes.
    #[must_use]
    pub const fn output_length(self) -> usize {
        self.output_length
    }

    fn params(self) -> Result<Params> {
        Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            Some(self.output_length),
        )
        .map_err(CryptoError::password_hash_policy)
    }
}

impl Default for PasswordHashWorkFactor {
    fn default() -> Self {
        Self::rfc_9106_second_recommended()
    }
}

/// Absolute resource limits accepted when verifying a stored Argon2id PHC string.
///
/// Values below the current work factor remain valid for compatibility and are reported through
/// [`PasswordHashVerification::needs_rehash`]. Values above these limits are rejected before the
/// Argon2 work-memory allocation begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_field_names,
    reason = "The max prefix distinguishes verification ceilings from password hashing work factors."
)]
pub struct PasswordHashVerificationLimits {
    max_memory_kib: u32,
    max_iterations: u32,
    max_parallelism: u32,
    max_output_length: usize,
}

impl PasswordHashVerificationLimits {
    /// Creates custom absolute verification limits.
    ///
    /// # Errors
    ///
    /// Returns an error when any limit is below the Argon2 minimum or the output-length limit is
    /// outside the range accepted by the password-hash encoding.
    pub fn new(
        max_memory_kib: u32,
        max_iterations: u32,
        max_parallelism: u32,
        max_output_length: usize,
    ) -> Result<Self> {
        if max_memory_kib < Params::MIN_M_COST
            || max_iterations < Params::MIN_T_COST
            || max_parallelism < Params::MIN_P_COST
        {
            return Err(CryptoError::password_hash_policy(
                "verification limits are below the Argon2 minimum",
            ));
        }
        if !(Output::MIN_LENGTH..=Output::MAX_LENGTH).contains(&max_output_length) {
            return Err(CryptoError::password_hash_policy(format_args!(
                "maximum output length must be between {} and {} bytes",
                Output::MIN_LENGTH,
                Output::MAX_LENGTH
            )));
        }

        Ok(Self {
            max_memory_kib,
            max_iterations,
            max_parallelism,
            max_output_length,
        })
    }

    /// Maximum accepted memory cost in KiB.
    #[must_use]
    pub const fn max_memory_kib(self) -> u32 {
        self.max_memory_kib
    }

    /// Maximum accepted number of Argon2 passes.
    #[must_use]
    pub const fn max_iterations(self) -> u32 {
        self.max_iterations
    }

    /// Maximum accepted number of Argon2 lanes.
    #[must_use]
    pub const fn max_parallelism(self) -> u32 {
        self.max_parallelism
    }

    /// Maximum accepted password-hash output length in bytes.
    #[must_use]
    pub const fn max_output_length(self) -> usize {
        self.max_output_length
    }
}

impl Default for PasswordHashVerificationLimits {
    fn default() -> Self {
        let work_factor = PasswordHashWorkFactor::rfc_9106_second_recommended();
        Self {
            max_memory_kib: work_factor.memory_kib,
            max_iterations: work_factor.iterations,
            max_parallelism: work_factor.parallelism,
            max_output_length: work_factor.output_length,
        }
    }
}

/// Password hashing and verification policy.
///
/// The work factor controls newly created hashes. Verification limits are a separate absolute
/// resource budget so products can accept bounded legacy or stronger hashes without allowing a
/// stored PHC string to request arbitrary memory or CPU time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordHashPolicy {
    work_factor: PasswordHashWorkFactor,
    verification_limits: PasswordHashVerificationLimits,
}

impl PasswordHashPolicy {
    /// Creates a validated password-hash policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the work factor is invalid or exceeds any configured verification
    /// limit.
    pub fn new(
        work_factor: PasswordHashWorkFactor,
        verification_limits: PasswordHashVerificationLimits,
    ) -> Result<Self> {
        work_factor.params()?;
        ensure_policy_value_within_limit(
            "m",
            u64::from(work_factor.memory_kib),
            u64::from(verification_limits.max_memory_kib),
        )?;
        ensure_policy_value_within_limit(
            "t",
            u64::from(work_factor.iterations),
            u64::from(verification_limits.max_iterations),
        )?;
        ensure_policy_value_within_limit(
            "p",
            u64::from(work_factor.parallelism),
            u64::from(verification_limits.max_parallelism),
        )?;
        ensure_policy_value_within_limit(
            "output_length",
            usize_to_u64(work_factor.output_length),
            usize_to_u64(verification_limits.max_output_length),
        )?;

        Ok(Self {
            work_factor,
            verification_limits,
        })
    }

    /// Work factor used for newly created hashes.
    #[must_use]
    pub const fn work_factor(self) -> PasswordHashWorkFactor {
        self.work_factor
    }

    /// Absolute limits used before verifying stored hashes.
    #[must_use]
    pub const fn verification_limits(self) -> PasswordHashVerificationLimits {
        self.verification_limits
    }
}

impl Default for PasswordHashPolicy {
    fn default() -> Self {
        Self {
            work_factor: PasswordHashWorkFactor::rfc_9106_second_recommended(),
            verification_limits: PasswordHashVerificationLimits {
                max_memory_kib: RFC_9106_SECOND_MEMORY_KIB,
                max_iterations: RFC_9106_SECOND_ITERATIONS,
                max_parallelism: RFC_9106_SECOND_PARALLELISM,
                max_output_length: DEFAULT_PASSWORD_HASH_OUTPUT_LENGTH,
            },
        }
    }
}

/// Detailed result of password verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordHashVerification {
    /// Whether the password matched the stored hash.
    pub is_valid: bool,
    /// Whether a matching password should be hashed again with the current work factor.
    pub needs_rehash: bool,
}

/// Hashes a password with the default RFC 9106 second recommended policy.
///
/// # Errors
///
/// Returns an error when the default Argon2 parameters cannot be constructed or hashing fails.
pub fn hash_password(password: &str) -> Result<String> {
    hash_password_with_policy(password, &PasswordHashPolicy::default())
}

/// Hashes a password with an explicit policy and a fresh random salt.
///
/// # Errors
///
/// Returns an error when the policy's Argon2 parameters are invalid or the password-hash operation
/// fails.
pub fn hash_password_with_policy(password: &str, policy: &PasswordHashPolicy) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    password_hasher(policy.work_factor)?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(CryptoError::password_hash)
}

/// Verifies a password with the default policy.
///
/// Malformed, unsupported, or over-budget hashes return an error. Only a genuine password
/// mismatch returns `Ok(false)`.
///
/// # Errors
///
/// Returns an error when the stored PHC string is malformed, uses unsupported parameters, exceeds
/// the default verification budget, or the Argon2 verifier fails for a reason other than password
/// mismatch.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    verify_password_with_policy(password, hash, &PasswordHashPolicy::default())
        .map(|verification| verification.is_valid)
}

/// Verifies a password with an explicit policy and reports whether a matching hash needs upgrade.
///
/// # Errors
///
/// Returns an error when the stored PHC string is malformed, unsupported, outside `policy`'s
/// verification limits, or the Argon2 verifier fails for a reason other than password mismatch.
pub fn verify_password_with_policy(
    password: &str,
    hash: &str,
    policy: &PasswordHashPolicy,
) -> Result<PasswordHashVerification> {
    let parsed = PasswordHash::new(hash).map_err(CryptoError::password_hash)?;
    let params = validate_stored_password_hash(&parsed, policy.verification_limits)?;
    let needs_rehash = password_hash_needs_rehash(&parsed, &params, policy.work_factor)?;

    match password_hasher(policy.work_factor)?.verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(PasswordHashVerification {
            is_valid: true,
            needs_rehash,
        }),
        Err(PasswordHashError::Password) => Ok(PasswordHashVerification {
            is_valid: false,
            needs_rehash: false,
        }),
        Err(error) => Err(CryptoError::password_hash(error)),
    }
}

fn password_hasher(work_factor: PasswordHashWorkFactor) -> Result<Argon2<'static>> {
    Ok(Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        work_factor.params()?,
    ))
}

fn validate_stored_password_hash(
    parsed: &PasswordHash<'_>,
    limits: PasswordHashVerificationLimits,
) -> Result<Params> {
    if parsed.algorithm.as_str() != "argon2id" {
        return Err(CryptoError::password_hash(format_args!(
            "unsupported password hash algorithm {}",
            parsed.algorithm
        )));
    }
    if parsed.version != Some(ARGON2_VERSION) {
        return Err(CryptoError::password_hash(
            "unsupported or missing Argon2 version",
        ));
    }
    if parsed.salt.is_none() {
        return Err(CryptoError::password_hash(
            "password hash is missing a salt",
        ));
    }
    if parsed.hash.is_none() {
        return Err(CryptoError::password_hash(
            "password hash is missing an output",
        ));
    }

    let params = Params::try_from(parsed).map_err(CryptoError::password_hash)?;
    if !params.keyid().is_empty() || !params.data().is_empty() {
        return Err(CryptoError::password_hash(
            "Argon2 keyid and associated data are not supported",
        ));
    }

    ensure_within_limit(
        "m",
        u64::from(params.m_cost()),
        u64::from(limits.max_memory_kib),
    )?;
    ensure_within_limit(
        "t",
        u64::from(params.t_cost()),
        u64::from(limits.max_iterations),
    )?;
    ensure_within_limit(
        "p",
        u64::from(params.p_cost()),
        u64::from(limits.max_parallelism),
    )?;
    ensure_within_limit(
        "output_length",
        usize_to_u64(
            params
                .output_len()
                .unwrap_or(DEFAULT_PASSWORD_HASH_OUTPUT_LENGTH),
        ),
        usize_to_u64(limits.max_output_length),
    )?;

    Ok(params)
}

fn password_hash_needs_rehash(
    parsed: &PasswordHash<'_>,
    params: &Params,
    current: PasswordHashWorkFactor,
) -> Result<bool> {
    let salt = parsed
        .salt
        .ok_or_else(|| CryptoError::password_hash("password hash is missing a salt"))?;
    let mut salt_bytes = [0_u8; 64];
    let salt_length = salt
        .decode_b64(&mut salt_bytes)
        .map_err(CryptoError::password_hash)?
        .len();
    let output_length = params
        .output_len()
        .unwrap_or(DEFAULT_PASSWORD_HASH_OUTPUT_LENGTH);

    Ok(params.m_cost() < current.memory_kib
        || params.t_cost() < current.iterations
        || params.p_cost() < current.parallelism
        || output_length < current.output_length
        || salt_length < PASSWORD_SALT_LENGTH)
}

fn ensure_within_limit(parameter: &'static str, actual: u64, maximum: u64) -> Result<()> {
    if actual > maximum {
        Err(CryptoError::PasswordHashVerificationLimit {
            parameter,
            actual,
            maximum,
        })
    } else {
        Ok(())
    }
}

fn ensure_policy_value_within_limit(
    parameter: &'static str,
    actual: u64,
    maximum: u64,
) -> Result<()> {
    if actual > maximum {
        Err(CryptoError::password_hash_policy(format_args!(
            "work factor {parameter}={actual} exceeds verification limit {maximum}"
        )))
    } else {
        Ok(())
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Computes HMAC-SHA-256 over `data` and returns lowercase hex.
///
/// Products remain responsible for providing a high-entropy, purpose-specific key and managing
/// its lifecycle. This helper is suitable for keyed cache components and lookup digests; it does
/// not replace Argon2id for human passwords.
///
/// # Errors
///
/// Returns an error when the HMAC implementation rejects the supplied key.
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> Result<String> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(CryptoError::message_authentication)?;
    mac.update(data);
    Ok(bytes_to_hex(&mac.finalize().into_bytes()))
}

/// Computes the SHA-256 digest of `data` and returns lowercase hex.
#[must_use]
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
pub fn sha256_digest_to_hex(digest: &[u8]) -> String {
    bytes_to_hex(digest)
}

/// Creates a new incremental SHA-256 hasher.
#[must_use]
pub fn new_sha256() -> Sha256 {
    Sha256::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::SaltString;
    use sha2::Digest;

    fn lightweight_policy() -> PasswordHashPolicy {
        let work_factor = PasswordHashWorkFactor::new(8, 1, 1, 32).unwrap();
        let limits = PasswordHashVerificationLimits::new(8, 1, 1, 32).unwrap();
        PasswordHashPolicy::new(work_factor, limits).unwrap()
    }

    fn hash_with_work_factor(password: &str, work_factor: PasswordHashWorkFactor) -> String {
        hash_with_work_factor_and_salt(password, work_factor, &[7_u8; PASSWORD_SALT_LENGTH])
    }

    fn hash_with_work_factor_and_salt(
        password: &str,
        work_factor: PasswordHashWorkFactor,
        salt: &[u8],
    ) -> String {
        password_hasher(work_factor)
            .unwrap()
            .hash_password(password.as_bytes(), &SaltString::encode_b64(salt).unwrap())
            .unwrap()
            .to_string()
    }

    #[test]
    fn default_policy_uses_rfc_9106_second_recommended_profile() {
        let policy = PasswordHashPolicy::default();
        let work_factor = policy.work_factor();
        let limits = policy.verification_limits();

        assert_eq!(work_factor.memory_kib(), 64 * 1024);
        assert_eq!(work_factor.iterations(), 3);
        assert_eq!(work_factor.parallelism(), 4);
        assert_eq!(work_factor.output_length(), 32);
        assert_eq!(limits.max_memory_kib(), work_factor.memory_kib());
        assert_eq!(limits.max_iterations(), work_factor.iterations());
        assert_eq!(limits.max_parallelism(), work_factor.parallelism());
        assert_eq!(limits.max_output_length(), work_factor.output_length());
    }

    #[test]
    fn default_hash_encodes_rfc_9106_second_recommended_profile() {
        let hash = hash_password("default policy password").unwrap();

        assert!(hash.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"));
        assert!(verify_password("default policy password", &hash).unwrap());
    }

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

    #[test]
    fn sha256_digest_to_hex_matches_incremental_hasher_output() {
        let mut hasher = new_sha256();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(sha256_digest_to_hex(&hasher.finalize()), sha256_hex(b"abc"));
    }

    #[test]
    fn password_hash_verifies_matching_password_and_rejects_wrong_password() {
        let policy = lightweight_policy();
        let hash = hash_password_with_policy("correct horse battery staple", &policy).unwrap();

        assert!(
            verify_password_with_policy("correct horse battery staple", &hash, &policy)
                .unwrap()
                .is_valid
        );
        assert!(
            !verify_password_with_policy("wrong password", &hash, &policy)
                .unwrap()
                .is_valid
        );
        assert!(hash.starts_with("$argon2id$v=19$m=8,t=1,p=1$"));
    }

    #[test]
    fn password_hash_uses_fresh_salt() {
        let policy = lightweight_policy();
        let first = hash_password_with_policy("same password", &policy).unwrap();
        let second = hash_password_with_policy("same password", &policy).unwrap();

        assert_ne!(first, second);
        assert!(
            verify_password_with_policy("same password", &first, &policy)
                .unwrap()
                .is_valid
        );
        assert!(
            verify_password_with_policy("same password", &second, &policy)
                .unwrap()
                .is_valid
        );
    }

    #[test]
    fn malformed_and_unsupported_password_hashes_return_errors() {
        assert!(matches!(
            verify_password("password", "not-a-password-hash"),
            Err(CryptoError::PasswordHash(_))
        ));

        let policy = lightweight_policy();
        let hash = hash_password_with_policy("password", &policy).unwrap();
        let unsupported = hash.replacen("$argon2id$", "$notargon$", 1);
        assert!(matches!(
            verify_password_with_policy("password", &unsupported, &policy),
            Err(CryptoError::PasswordHash(_))
        ));

        let unsupported_version = hash.replacen("v=19", "v=16", 1);
        assert!(matches!(
            verify_password_with_policy("password", &unsupported_version, &policy),
            Err(CryptoError::PasswordHash(_))
        ));

        let missing_version = hash.replacen("$v=19$", "$", 1);
        assert!(matches!(
            verify_password_with_policy("password", &missing_version, &policy),
            Err(CryptoError::PasswordHash(_))
        ));

        assert!(matches!(
            verify_password_with_policy("password", "$argon2id$v=19$m=8,t=1,p=1", &policy,),
            Err(CryptoError::PasswordHash(_))
        ));
        assert!(matches!(
            verify_password_with_policy(
                "password",
                "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ",
                &policy,
            ),
            Err(CryptoError::PasswordHash(_))
        ));

        let unknown_parameter = hash.replacen("p=1", "p=1,x=1", 1);
        assert!(matches!(
            verify_password_with_policy("password", &unknown_parameter, &policy),
            Err(CryptoError::PasswordHash(_))
        ));
    }

    #[test]
    fn verification_rejects_over_budget_work_factors_before_argon2_runs() {
        let policy = lightweight_policy();
        let hash = hash_password_with_policy("password", &policy).unwrap();
        let over_memory = hash.replacen("m=8", "m=9", 1);
        let over_iterations = hash.replacen("t=1", "t=2", 1);
        let over_output = hash_with_work_factor(
            "password",
            PasswordHashWorkFactor::new(8, 1, 1, 33).unwrap(),
        );
        let parallel_work_factor = PasswordHashWorkFactor::new(16, 1, 1, 32).unwrap();
        let parallel_limits = PasswordHashVerificationLimits::new(16, 1, 1, 32).unwrap();
        let parallel_policy =
            PasswordHashPolicy::new(parallel_work_factor, parallel_limits).unwrap();
        let over_parallelism =
            hash_with_work_factor("password", parallel_work_factor).replacen("p=1", "p=2", 1);

        assert!(matches!(
            verify_password_with_policy("password", &over_memory, &policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: "m",
                actual: 9,
                maximum: 8,
            })
        ));
        assert!(matches!(
            verify_password_with_policy("password", &over_iterations, &policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: "t",
                actual: 2,
                maximum: 1,
            })
        ));
        assert!(matches!(
            verify_password_with_policy("password", &over_parallelism, &parallel_policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: "p",
                actual: 2,
                maximum: 1,
            })
        ));
        assert!(matches!(
            verify_password_with_policy("password", &over_output, &policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: "output_length",
                actual: 33,
                maximum: 32,
            })
        ));

        let extreme_memory = hash.replacen("m=8", "m=4294967295", 1);
        assert!(matches!(
            verify_password_with_policy("password", &extreme_memory, &policy),
            Err(CryptoError::PasswordHashVerificationLimit {
                parameter: "m",
                actual: 4_294_967_295,
                maximum: 8,
            })
        ));
    }

    #[test]
    fn legacy_work_factor_verifies_and_requests_rehash() {
        let legacy = PasswordHashWorkFactor::new(19 * 1024, 2, 1, 32).unwrap();
        let hash = hash_with_work_factor("password", legacy);
        let verification =
            verify_password_with_policy("password", &hash, &PasswordHashPolicy::default()).unwrap();

        assert!(verification.is_valid);
        assert!(verification.needs_rehash);

        let wrong =
            verify_password_with_policy("wrong password", &hash, &PasswordHashPolicy::default())
                .unwrap();
        assert!(!wrong.is_valid);
        assert!(!wrong.needs_rehash);
    }

    #[test]
    fn current_work_factor_does_not_request_rehash() {
        let policy = lightweight_policy();
        let hash = hash_password_with_policy("password", &policy).unwrap();
        let verification = verify_password_with_policy("password", &hash, &policy).unwrap();

        assert!(verification.is_valid);
        assert!(!verification.needs_rehash);
    }

    #[test]
    fn short_legacy_salt_verifies_and_requests_rehash() {
        let policy = lightweight_policy();
        let hash = hash_with_work_factor_and_salt("password", policy.work_factor(), &[5_u8; 8]);
        let verification = verify_password_with_policy("password", &hash, &policy).unwrap();

        assert!(verification.is_valid);
        assert!(verification.needs_rehash);
    }

    #[test]
    fn custom_limits_accept_stronger_hash_without_rehashing_down() {
        let current = PasswordHashWorkFactor::new(8, 1, 1, 32).unwrap();
        let limits = PasswordHashVerificationLimits::new(16, 2, 2, 32).unwrap();
        let policy = PasswordHashPolicy::new(current, limits).unwrap();
        let stronger = PasswordHashWorkFactor::new(16, 2, 2, 32).unwrap();
        let hash = hash_with_work_factor("password", stronger);
        let verification = verify_password_with_policy("password", &hash, &policy).unwrap();

        assert!(verification.is_valid);
        assert!(!verification.needs_rehash);
    }

    #[test]
    fn policy_rejects_work_factor_above_its_verification_limits() {
        let work_factor = PasswordHashWorkFactor::new(64, 2, 1, 32).unwrap();
        let limits = PasswordHashVerificationLimits::new(32, 2, 1, 32).unwrap();

        assert!(matches!(
            PasswordHashPolicy::new(work_factor, limits),
            Err(CryptoError::PasswordHashPolicy(_))
        ));

        assert!(matches!(
            PasswordHashWorkFactor::new(8, 1, 1, 4),
            Err(CryptoError::PasswordHashPolicy(_))
        ));
    }

    #[test]
    fn work_factor_and_limit_constructors_reject_every_invalid_boundary() {
        for result in [
            PasswordHashWorkFactor::new(7, 1, 1, 32),
            PasswordHashWorkFactor::new(15, 1, 2, 32),
            PasswordHashWorkFactor::new(8, 0, 1, 32),
            PasswordHashWorkFactor::new(8, 1, 0, 32),
            PasswordHashWorkFactor::new(8, 1, 1, Output::MIN_LENGTH - 1),
            PasswordHashWorkFactor::new(8, 1, 1, Output::MAX_LENGTH + 1),
        ] {
            assert!(matches!(result, Err(CryptoError::PasswordHashPolicy(_))));
        }

        for result in [
            PasswordHashVerificationLimits::new(Params::MIN_M_COST - 1, 1, 1, 32),
            PasswordHashVerificationLimits::new(8, Params::MIN_T_COST - 1, 1, 32),
            PasswordHashVerificationLimits::new(8, 1, Params::MIN_P_COST - 1, 32),
            PasswordHashVerificationLimits::new(8, 1, 1, Output::MIN_LENGTH - 1),
            PasswordHashVerificationLimits::new(8, 1, 1, Output::MAX_LENGTH + 1),
        ] {
            assert!(matches!(result, Err(CryptoError::PasswordHashPolicy(_))));
        }
    }

    #[test]
    fn policy_rejects_each_work_factor_dimension_above_its_limit() {
        let cases = [
            (
                PasswordHashWorkFactor::new(16, 1, 1, 32).unwrap(),
                PasswordHashVerificationLimits::new(8, 1, 1, 32).unwrap(),
            ),
            (
                PasswordHashWorkFactor::new(8, 2, 1, 32).unwrap(),
                PasswordHashVerificationLimits::new(8, 1, 1, 32).unwrap(),
            ),
            (
                PasswordHashWorkFactor::new(16, 1, 2, 32).unwrap(),
                PasswordHashVerificationLimits::new(16, 1, 1, 32).unwrap(),
            ),
            (
                PasswordHashWorkFactor::new(8, 1, 1, 33).unwrap(),
                PasswordHashVerificationLimits::new(8, 1, 1, 32).unwrap(),
            ),
        ];

        for (work_factor, limits) in cases {
            assert!(matches!(
                PasswordHashPolicy::new(work_factor, limits),
                Err(CryptoError::PasswordHashPolicy(_))
            ));
        }
    }
}
