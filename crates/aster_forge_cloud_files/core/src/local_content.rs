//! Immutable local-content snapshots used by dirty tracking and upload recovery.

use std::{fmt, num::NonZeroU64};

use crate::{CloudFilesCoreError, CloudItemKey, ContentDigest, Result};

/// Opaque product-neutral reference to immutable local bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LocalContentReference(String);

impl LocalContentReference {
    /// Creates a non-empty local-content reference and preserves it exactly.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("local content reference"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque reference.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the opaque reference.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for LocalContentReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalContentReference")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Monotonic item-local generation of an immutable local-content snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalContentGeneration(NonZeroU64);

impl LocalContentGeneration {
    /// Creates a non-zero local generation.
    pub const fn new(value: u64) -> Result<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(CloudFilesCoreError::InvalidLocalContentGeneration),
        }
    }

    /// Returns the numeric generation fence value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Immutable local bytes captured for dirty tracking and upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalContentSnapshot {
    item_key: CloudItemKey,
    generation: LocalContentGeneration,
    reference: LocalContentReference,
    size: u64,
    digest: Option<ContentDigest>,
}

impl LocalContentSnapshot {
    /// Creates a snapshot. The referenced bytes must remain immutable for this generation.
    pub const fn new(
        item_key: CloudItemKey,
        generation: LocalContentGeneration,
        reference: LocalContentReference,
        size: u64,
        digest: Option<ContentDigest>,
    ) -> Self {
        Self {
            item_key,
            generation,
            reference,
            size,
            digest,
        }
    }

    /// Returns the stable item identity whose local bytes were captured.
    pub const fn item_key(&self) -> &CloudItemKey {
        &self.item_key
    }

    /// Returns the item-local generation used to fence stale upload completion.
    pub const fn generation(&self) -> LocalContentGeneration {
        self.generation
    }

    /// Returns the opaque reference used by the platform/host content reader.
    pub const fn reference(&self) -> &LocalContentReference {
        &self.reference
    }

    /// Returns the immutable byte length.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the optional real integrity digest.
    pub const fn digest(&self) -> Option<&ContentDigest> {
        self.digest.as_ref()
    }
}
