//! Error types for the cloud-files core model.

/// Result type returned by cloud-files core validation and capability operations.
pub type Result<T> = std::result::Result<T, CloudFilesCoreError>;

/// Product-neutral failures produced while validating core values and capability intersections.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudFilesCoreError {
    /// An opaque identity, revision, cursor, or digest field was empty.
    #[error("{field} must not be empty")]
    EmptyValue {
        /// Stable field classification suitable for product-side error mapping.
        field: &'static str,
    },
    /// An encoded native identity exceeded the active platform limit.
    #[error("native identity is {actual_bytes} bytes, exceeding the {max_bytes}-byte limit")]
    NativeIdentityTooLarge {
        /// Actual encoded identity size.
        actual_bytes: usize,
        /// Maximum accepted size for the active platform adapter.
        max_bytes: usize,
    },
    /// Two range-alignment constraints could not be combined without overflow.
    #[error("range alignment intersection overflowed for {left} and {right}")]
    AlignmentIntersectionOverflow {
        /// Left alignment in bytes.
        left: u64,
        /// Right alignment in bytes.
        right: u64,
    },
    /// A backend or adapter omitted an invariant required by the core identity contract.
    #[error("required cloud-files capability is missing: {capability}")]
    MissingRequiredCapability {
        /// Stable capability name suitable for diagnostics and tests.
        capability: &'static str,
    },
    /// An item combined root, parent, kind, or content fields in an invalid shape.
    #[error("invalid cloud item shape: {reason}")]
    InvalidItemShape {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A byte range used a zero length or overflowed its end position.
    #[error("invalid byte range: {reason}")]
    InvalidByteRange {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// Backend content bytes did not satisfy the revision or range request contract.
    #[error("invalid content response: {reason}")]
    InvalidContentResponse {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A mutation intent omitted required state or combined incompatible fields.
    #[error("invalid mutation intent: {reason}")]
    InvalidMutationIntent {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A durable mutation record attempted an invalid state transition.
    #[error("invalid mutation transition: {reason}")]
    InvalidMutationTransition {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A session generation used zero, which is reserved for the absence of a session.
    #[error("session generation must be non-zero")]
    InvalidSessionGeneration,
    /// A local-content generation used zero, which is reserved for the absence of a snapshot.
    #[error("local content generation must be non-zero")]
    InvalidLocalContentGeneration,
    /// Content storage ownership, range coverage, leases, or materialization state was invalid.
    #[error("invalid content storage state: {reason}")]
    InvalidContentStorageState {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// An eviction record attempted an invalid durable transition or physical effect.
    #[error("invalid content eviction transition: {reason}")]
    InvalidContentEvictionTransition {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A provider-cache write attempted an invalid durable transition.
    #[error("invalid content cache write transition: {reason}")]
    InvalidContentCacheWriteTransition {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
    /// A resumable content upload attempted an invalid request or durable transition.
    #[error("invalid content upload transition: {reason}")]
    InvalidContentUploadTransition {
        /// Stable validation reason suitable for deterministic contract tests.
        reason: &'static str,
    },
}

impl CloudFilesCoreError {
    pub(crate) const fn empty(field: &'static str) -> Self {
        Self::EmptyValue { field }
    }

    pub(crate) const fn invalid_item(reason: &'static str) -> Self {
        Self::InvalidItemShape { reason }
    }

    pub(crate) const fn invalid_byte_range(reason: &'static str) -> Self {
        Self::InvalidByteRange { reason }
    }

    pub(crate) const fn invalid_content_response(reason: &'static str) -> Self {
        Self::InvalidContentResponse { reason }
    }

    pub(crate) const fn invalid_mutation_intent(reason: &'static str) -> Self {
        Self::InvalidMutationIntent { reason }
    }

    pub(crate) const fn invalid_mutation_transition(reason: &'static str) -> Self {
        Self::InvalidMutationTransition { reason }
    }

    pub(crate) const fn invalid_content_storage(reason: &'static str) -> Self {
        Self::InvalidContentStorageState { reason }
    }

    pub(crate) const fn invalid_content_eviction(reason: &'static str) -> Self {
        Self::InvalidContentEvictionTransition { reason }
    }

    pub(crate) const fn invalid_content_cache_write(reason: &'static str) -> Self {
        Self::InvalidContentCacheWriteTransition { reason }
    }

    pub(crate) const fn invalid_content_upload(reason: &'static str) -> Self {
        Self::InvalidContentUploadTransition { reason }
    }
}
