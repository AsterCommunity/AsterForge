//! Stable, product-mappable authentication failures.

/// Result returned by authentication primitives.
pub type Result<T> = std::result::Result<T, AuthError>;

/// Errors produced by the product-neutral authentication mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// The product entrypoint has not successfully initialized the process-wide JWT provider.
    #[error("JWT crypto provider has not been initialized")]
    CryptoProviderNotInitialized,
    /// The upstream JWT provider was already installed; Forge never overwrites or adopts it.
    #[error("JWT crypto provider was already installed")]
    CryptoProviderAlreadyInstalled,
    /// The HMAC key has fewer bytes than required by the configured algorithm.
    #[error("authentication signing key is too short for the configured algorithm")]
    KeyTooShort,
    /// The caller supplied an invalid policy, identifier, or claim value.
    #[error("invalid authentication policy")]
    InvalidPolicy,
    /// The token exceeded the configured parser budget.
    #[error("authentication token exceeds the configured size limit")]
    TokenTooLarge,
    /// The token could not be decoded or its signature was not accepted.
    #[error("invalid authentication token")]
    InvalidToken,
    /// The token signature was valid, but it is no longer valid at the current clock value.
    #[error("authentication token expired")]
    Expired,
    /// The token's issuance or optional not-before time is beyond the accepted clock skew.
    #[error("authentication token is not yet valid")]
    NotYetValid,
    /// A required claim is absent or malformed.
    #[error("authentication token is missing a required claim")]
    MissingClaim,
    /// The token type does not match the operation.
    #[error("authentication token type mismatch")]
    WrongTokenType,
    /// The token was signed with a disallowed algorithm.
    #[error("authentication token algorithm is not allowed")]
    AlgorithmMismatch,
    /// The issuer does not match the configured policy.
    #[error("authentication token issuer mismatch")]
    IssuerMismatch,
    /// The audience does not match the configured policy.
    #[error("authentication token audience mismatch")]
    AudienceMismatch,
    /// Token creation failed without exposing secret material.
    #[error("authentication token creation failed")]
    Encoding,
    /// The instance's bounded refresh request budget is exhausted.
    #[error("refresh request budget exhausted")]
    ResourceLimit,
    /// Request cancellation or instance shutdown occurred before persistence started.
    #[error("refresh request cancelled")]
    Cancelled,
}
