//! Opaque metadata/content revisions and optional content integrity digests.

use std::fmt;

use crate::{CloudFilesCoreError, Result};

macro_rules! opaque_revision {
    ($name:ident, $field:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(Vec<u8>);

        impl $name {
            /// Creates a non-empty opaque revision token.
            /// # Errors
            ///
            /// Returns an error when validation fails or an underlying backend, store, or platform
            /// operation fails.
            pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() {
                    return Err(CloudFilesCoreError::empty($field));
                }
                Ok(Self(value))
            }

            /// Copies a non-empty opaque revision token from a byte slice.
            /// # Errors
            ///
            /// Returns an error when validation fails or an underlying backend, store, or platform
            /// operation fails.
            pub fn from_slice(value: &[u8]) -> Result<Self> {
                Self::new(value.to_vec())
            }

            /// Returns the opaque revision bytes.
            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            /// Consumes the revision and returns its opaque bytes.
            pub fn into_bytes(self) -> Vec<u8> {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("byte_len", &self.0.len())
                    .finish()
            }
        }
    };
}

opaque_revision!(
    MetadataRevision,
    "metadata revision",
    "Opaque equality/precondition token for metadata state."
);
opaque_revision!(
    ContentRevision,
    "content revision",
    "Opaque equality/precondition token for content state."
);

/// Algorithm identifier attached to a real content integrity digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentDigestAlgorithm(String);

impl ContentDigestAlgorithm {
    /// Creates a non-empty algorithm identifier without normalizing its spelling.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("content digest algorithm"));
        }
        Ok(Self(value))
    }

    /// Returns the algorithm identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the identifier and returns its string value.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Optional algorithm-tagged content integrity digest.
///
/// A content revision or strong `ETag` is not automatically a digest. Callers construct this value
/// only when the remote backend supplies bytes produced by the named digest algorithm.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentDigest {
    algorithm: ContentDigestAlgorithm,
    value: Vec<u8>,
}

impl ContentDigest {
    /// Creates a non-empty algorithm-tagged digest.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(algorithm: ContentDigestAlgorithm, value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("content digest value"));
        }
        Ok(Self { algorithm, value })
    }

    /// Returns the digest algorithm identifier.
    #[must_use]
    pub const fn algorithm(&self) -> &ContentDigestAlgorithm {
        &self.algorithm
    }

    /// Returns the digest bytes.
    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// Consumes the digest and returns its algorithm and bytes.
    #[must_use]
    pub fn into_parts(self) -> (ContentDigestAlgorithm, Vec<u8>) {
        (self.algorithm, self.value)
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentDigest")
            .field("algorithm", &self.algorithm)
            .field("byte_len", &self.value.len())
            .finish()
    }
}
