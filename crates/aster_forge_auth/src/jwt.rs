//! Strict local-session JWT issuance and verification, backed by `jsonwebtoken`.

use crate::{
    AuthError, Clock, IdSource, RandomIdSource, RefreshJti, Result, SessionId, Subject, SystemClock,
};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};

/// Local session credential purpose; OAuth access and OIDC ID tokens use other contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthTokenType {
    /// Short-lived request credential.
    Access,
    /// Single-use server-side session rotation credential.
    Refresh,
}

/// Authentication restriction. Products decide which operations a restricted session may perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restriction {
    /// Normal login.
    None,
    /// Only product-approved password-change operations are permitted.
    MustChangePassword,
}

/// Account-wide generation; revocation must fail on overflow rather than reuse a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionVersion(u64);
impl SessionVersion {
    /// Wraps a nonnegative product generation value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    /// Returns the generation for product persistence.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
    /// Advances without saturating or wrapping.
    ///
    /// # Errors
    /// Returns `InvalidPolicy` when the generation is exhausted.
    pub fn next(self) -> Result<Self> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(AuthError::InvalidPolicy)
    }
}

/// Required local web-session claims. No product entities or permissions are embedded. Claims are
/// signed, not encrypted, and must never carry secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenClaims {
    /// Stable product subject.
    pub sub: Subject,
    /// Server-side session identity, shared by access and refresh credentials.
    pub sid: SessionId,
    /// Account-wide generation fence checked against current product state.
    pub session_version: SessionVersion,
    /// Credential purpose; verification always requires an expected purpose.
    pub token_type: AuthTokenType,
    /// Single-use refresh identity. Access credentials must omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<RefreshJti>,
    /// Explicit login restriction interpreted by product authorization.
    pub restriction: Restriction,
    /// Nonnegative Unix issuance timestamp in seconds.
    pub iat: i64,
    /// Exclusive Unix expiry timestamp, strictly after issuance.
    pub exp: i64,
    /// Optional inclusive not-before time, strictly before expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    /// Required product signing-domain issuer.
    pub iss: String,
    /// Required local-session audience.
    pub aud: String,
}

/// Signature/policy-verified claims. Private construction prevents labeling unchecked JSON as
/// verified. Current account, session, and permissions still require product checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken(TokenClaims);
impl VerifiedToken {
    /// Borrows verified claims without exposing the original credential.
    #[must_use]
    pub const fn claims(&self) -> &TokenClaims {
        &self.0
    }
    /// Transfers claims to a product adapter.
    #[must_use]
    pub fn into_claims(self) -> TokenClaims {
        self.0
    }
}

/// Supported HMAC algorithms; headers cannot select a different key family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtAlgorithm {
    /// HMAC-SHA-256, at least 32 key bytes.
    Hs256,
    /// HMAC-SHA-384, at least 48 key bytes.
    Hs384,
    /// HMAC-SHA-512, at least 64 key bytes.
    Hs512,
}
impl JwtAlgorithm {
    const fn upstream(self) -> Algorithm {
        match self {
            Self::Hs256 => Algorithm::HS256,
            Self::Hs384 => Algorithm::HS384,
            Self::Hs512 => Algorithm::HS512,
        }
    }
    /// Minimum HMAC key length in bytes. Length alone does not establish entropy.
    #[must_use]
    pub const fn minimum_key_bytes(self) -> usize {
        match self {
            Self::Hs256 => 32,
            Self::Hs384 => 48,
            Self::Hs512 => 64,
        }
    }
}

/// HMAC key wrapper. Products own entropy, loading, rotation, and isolation from AEAD keys.
#[derive(Clone)]
pub struct JwtKey(Arc<[u8]>);
impl JwtKey {
    /// Creates a key with the minimum HS256 size.
    ///
    /// # Errors
    /// Returns `KeyTooShort` for fewer than 32 bytes; this check cannot prove entropy.
    pub fn hmac(secret: impl AsRef<[u8]>) -> Result<Self> {
        let secret = secret.as_ref();
        if secret.len() < JwtAlgorithm::Hs256.minimum_key_bytes() {
            return Err(AuthError::KeyTooShort);
        }
        Ok(Self(Arc::from(secret)))
    }
    fn check(&self, algorithm: JwtAlgorithm) -> Result<()> {
        if self.0.len() < algorithm.minimum_key_bytes() {
            return Err(AuthError::KeyTooShort);
        }
        Ok(())
    }
}
impl fmt::Debug for JwtKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JwtKey(<redacted>)")
    }
}

/// Issuance lifetimes and strict namespace. Namespace/session checks cannot be disabled. Old JWT
/// formats require reauthentication rather than an implicit fallback parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtPolicy {
    issuer: String,
    audience: String,
    access_ttl_secs: u64,
    refresh_ttl_secs: u64,
    algorithm: JwtAlgorithm,
    leeway_secs: u64,
    max_token_bytes: usize,
}
impl JwtPolicy {
    /// Creates a strict HS256 policy with no skew and a 16 KiB token budget.
    ///
    /// # Errors
    /// Returns `InvalidPolicy` for invalid namespaces or zero/unrepresentable lifetimes.
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        access_ttl_secs: u64,
        refresh_ttl_secs: u64,
    ) -> Result<Self> {
        let issuer = issuer.into();
        let audience = audience.into();
        crate::ids::validate_identifier(&issuer)?;
        crate::ids::validate_identifier(&audience)?;
        for ttl in [access_ttl_secs, refresh_ttl_secs] {
            if ttl == 0 || i64::try_from(ttl).is_err() {
                return Err(AuthError::InvalidPolicy);
            }
        }
        Ok(Self {
            issuer,
            audience,
            access_ttl_secs,
            refresh_ttl_secs,
            algorithm: JwtAlgorithm::Hs256,
            leeway_secs: 0,
            max_token_bytes: 16 * 1024,
        })
    }
    /// Selects the sole accepted algorithm with its corresponding minimum key size.
    #[must_use]
    pub const fn with_algorithm(mut self, algorithm: JwtAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }
    /// Sets maximum accepted clock skew.
    ///
    /// # Errors
    /// Returns `InvalidPolicy` for unrepresentable skew.
    pub fn with_leeway(mut self, seconds: u64) -> Result<Self> {
        i64::try_from(seconds).map_err(|_| AuthError::InvalidPolicy)?;
        self.leeway_secs = seconds;
        Ok(self)
    }
    /// Sets a parser/issuance budget checked before header parsing.
    ///
    /// # Errors
    /// Returns `InvalidPolicy` for a zero budget.
    pub fn with_max_token_bytes(mut self, bytes: usize) -> Result<Self> {
        if bytes == 0 {
            return Err(AuthError::InvalidPolicy);
        }
        self.max_token_bytes = bytes;
        Ok(self)
    }
    /// Returns the issuer.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    /// Returns the audience.
    #[must_use]
    pub fn audience(&self) -> &str {
        &self.audience
    }
    /// Returns access lifetime in seconds.
    #[must_use]
    pub const fn access_ttl_secs(&self) -> u64 {
        self.access_ttl_secs
    }
    /// Returns refresh lifetime in seconds.
    #[must_use]
    pub const fn refresh_ttl_secs(&self) -> u64 {
        self.refresh_ttl_secs
    }
    /// Returns skew in seconds.
    #[must_use]
    pub const fn leeway_secs(&self) -> u64 {
        self.leeway_secs
    }
    /// Returns parser budget in bytes.
    #[must_use]
    pub const fn max_token_bytes(&self) -> usize {
        self.max_token_bytes
    }
}

/// Candidate credentials; commit their refresh identity before releasing token strings.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthTokenPair {
    /// Access credential; never log it.
    pub access_token: String,
    /// Refresh credential; never log it.
    pub refresh_token: String,
    /// Refresh identity to commit into the store.
    pub refresh_jti: RefreshJti,
    /// Exclusive access expiry.
    pub access_expires_at: i64,
    /// Exclusive refresh expiry.
    pub refresh_expires_at: i64,
}
impl fmt::Debug for AuthTokenPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthTokenPair")
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish_non_exhaustive()
    }
}

/// Product-configured signer with injectable clock and ID source.
#[derive(Clone)]
pub struct JwtSigner {
    key: JwtKey,
    policy: JwtPolicy,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
}
impl fmt::Debug for JwtSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtSigner")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}
impl JwtSigner {
    /// Creates an issuer with explicit policy, clock, and ID source.
    #[must_use]
    pub fn new(
        key: JwtKey,
        policy: JwtPolicy,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdSource>,
    ) -> Self {
        Self {
            key,
            policy,
            clock,
            ids,
        }
    }
    /// Creates a production issuer using system time and UUID v4.
    #[must_use]
    pub fn with_system_clock(key: JwtKey, policy: JwtPolicy) -> Self {
        Self::new(key, policy, Arc::new(SystemClock), Arc::new(RandomIdSource))
    }
    /// Issues a pair bound to one subject, session, generation, and restriction.
    ///
    /// # Errors
    /// Returns a mechanism error for invalid key size, clock, expiry overflow, ID generation,
    /// encoding, or token size. Returns `CryptoProviderNotInitialized` until the product has
    /// initialized the JWT backend at startup. Issuing alone does not create a persisted session.
    pub fn issue(
        &self,
        subject: Subject,
        session: SessionId,
        version: SessionVersion,
        restriction: Restriction,
    ) -> Result<AuthTokenPair> {
        crate::provider::ensure_initialized()?;
        self.key.check(self.policy.algorithm)?;
        let now = self.clock.now_unix_seconds()?;
        if now < 0 {
            return Err(AuthError::InvalidPolicy);
        }
        let expiry = |ttl| {
            now.checked_add(i64::try_from(ttl).map_err(|_| AuthError::InvalidPolicy)?)
                .ok_or(AuthError::InvalidPolicy)
        };
        let access_expires_at = expiry(self.policy.access_ttl_secs)?;
        let refresh_expires_at = expiry(self.policy.refresh_ttl_secs)?;
        let refresh_jti = self.ids.next_id()?;
        let mut claims = TokenClaims {
            sub: subject,
            sid: session,
            session_version: version,
            token_type: AuthTokenType::Access,
            jti: None,
            restriction,
            iat: now,
            exp: access_expires_at,
            nbf: None,
            iss: self.policy.issuer.clone(),
            aud: self.policy.audience.clone(),
        };
        let header = Header::new(self.policy.algorithm.upstream());
        let key = EncodingKey::from_secret(&self.key.0);
        let access_token = encode(&header, &claims, &key).map_err(|_| AuthError::Encoding)?;
        claims.token_type = AuthTokenType::Refresh;
        claims.jti = Some(refresh_jti.clone());
        claims.exp = refresh_expires_at;
        let refresh_token = encode(&header, &claims, &key).map_err(|_| AuthError::Encoding)?;
        if access_token.len() > self.policy.max_token_bytes
            || refresh_token.len() > self.policy.max_token_bytes
        {
            return Err(AuthError::TokenTooLarge);
        }
        Ok(AuthTokenPair {
            access_token,
            refresh_token,
            refresh_jti,
            access_expires_at,
            refresh_expires_at,
        })
    }
}

/// Strict verifier: only signature-verified claims cross the validation boundary.
#[derive(Clone)]
pub struct JwtVerifier {
    key: JwtKey,
    policy: JwtPolicy,
    clock: Arc<dyn Clock>,
}
impl fmt::Debug for JwtVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtVerifier")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}
impl JwtVerifier {
    pub(crate) const fn leeway_secs(&self) -> u64 {
        self.policy.leeway_secs
    }
    /// Creates a verifier with an injectable clock.
    #[must_use]
    pub fn new(key: JwtKey, policy: JwtPolicy, clock: Arc<dyn Clock>) -> Self {
        Self { key, policy, clock }
    }
    /// Creates a production verifier using system time.
    #[must_use]
    pub fn with_system_clock(key: JwtKey, policy: JwtPolicy) -> Self {
        Self::new(key, policy, Arc::new(SystemClock))
    }
    /// Verifies signature, namespace, purpose, identifiers, and time boundaries.
    ///
    /// # Errors
    /// Returns a classified error without token/key material. Products additionally check live
    /// account/version state and live `sid` state when promising immediate session revocation.
    /// Returns `CryptoProviderNotInitialized` until the product initializes the JWT backend.
    pub fn verify(&self, token: &str, expected: AuthTokenType) -> Result<VerifiedToken> {
        crate::provider::ensure_initialized()?;
        self.key.check(self.policy.algorithm)?;
        if token.len() > self.policy.max_token_bytes {
            return Err(AuthError::TokenTooLarge);
        }
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        if header.alg != self.policy.algorithm.upstream() {
            return Err(AuthError::AlgorithmMismatch);
        }
        let mut validation = Validation::new(self.policy.algorithm.upstream());
        // Upstream verifies signatures; these checks use an injected clock and stable classifications.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        let claims =
            decode::<TokenClaims>(token, &DecodingKey::from_secret(&self.key.0), &validation)
                .map_err(|_| AuthError::InvalidToken)?
                .claims;
        if claims.iss != self.policy.issuer {
            return Err(AuthError::IssuerMismatch);
        }
        if claims.aud != self.policy.audience {
            return Err(AuthError::AudienceMismatch);
        }
        if claims.token_type != expected {
            return Err(AuthError::WrongTokenType);
        }
        match (expected, &claims.jti) {
            (AuthTokenType::Refresh, None) => return Err(AuthError::MissingClaim),
            (AuthTokenType::Access, Some(_)) => return Err(AuthError::InvalidToken),
            _ => {}
        }
        if claims.iat < 0
            || claims.exp <= claims.iat
            || claims.nbf.is_some_and(|nbf| nbf < 0 || nbf >= claims.exp)
        {
            return Err(AuthError::InvalidToken);
        }
        let now = self.clock.now_unix_seconds()?;
        if now < 0 {
            return Err(AuthError::InvalidPolicy);
        }
        let leeway =
            i64::try_from(self.policy.leeway_secs).map_err(|_| AuthError::InvalidPolicy)?;
        // Widen arithmetic: expiry near i64::MAX cannot wrap when combined with clock skew.
        let latest = i128::from(now) + i128::from(leeway);
        if i128::from(claims.iat) > latest || claims.nbf.is_some_and(|nbf| i128::from(nbf) > latest)
        {
            return Err(AuthError::NotYetValid);
        }
        if i128::from(now) >= i128::from(claims.exp) + i128::from(leeway) {
            return Err(AuthError::Expired);
        }
        Ok(VerifiedToken(claims))
    }
}
