//! Durable provider-cache byte installation and coverage reconciliation.

use std::fmt;

use crate::{ByteRange, CloudFilesCoreError, ContentCacheKey, Result, SessionGeneration};

/// Stable identity of one provider-cache write operation.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentCacheWriteOperationId(String);

impl ContentCacheWriteOperationId {
    /// Creates a non-empty opaque write identity.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty(
                "content cache write operation id",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the opaque operation identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContentCacheWriteOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentCacheWriteOperationId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Logical bytes installed by one atomic provider-cache write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentCacheWriteExtent {
    /// Installs the complete file, including a zero-byte file.
    Whole,
    /// Installs one non-empty sparse byte range.
    Range(ByteRange),
}

/// Durable write intent captured before temporary-file or sparse-byte effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCacheWriteIntent {
    operation_id: ContentCacheWriteOperationId,
    cache_key: ContentCacheKey,
    expected_size: u64,
    extent: ContentCacheWriteExtent,
    session_generation: SessionGeneration,
}

impl ContentCacheWriteIntent {
    /// Creates and validates a revision-bound provider-cache write intent.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(
        operation_id: ContentCacheWriteOperationId,
        cache_key: ContentCacheKey,
        expected_size: u64,
        extent: ContentCacheWriteExtent,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        if let ContentCacheWriteExtent::Range(range) = extent
            && range.end_exclusive() > expected_size
        {
            return Err(CloudFilesCoreError::invalid_content_cache_write(
                "cache write range exceeds the expected content size",
            ));
        }
        Ok(Self {
            operation_id,
            cache_key,
            expected_size,
            extent,
            session_generation,
        })
    }

    /// Returns the durable operation identity.
    #[must_use]
    pub const fn operation_id(&self) -> &ContentCacheWriteOperationId {
        &self.operation_id
    }

    /// Returns the exact revision-bound cache identity.
    #[must_use]
    pub const fn cache_key(&self) -> &ContentCacheKey {
        &self.cache_key
    }

    /// Returns the expected size of the exact content revision.
    #[must_use]
    pub const fn expected_size(&self) -> u64 {
        self.expected_size
    }

    /// Returns the logical extent whose physical bytes are installed.
    #[must_use]
    pub const fn extent(&self) -> ContentCacheWriteExtent {
        self.extent
    }

    /// Returns the platform session generation that created the write.
    #[must_use]
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Durable provider-cache write state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentCacheWriteState {
    /// Intent and entry reservation are durable; physical bytes may be absent.
    IntentPersisted,
    /// Temporary/sparse bytes were atomically installed and can be observed after restart.
    PhysicalBytesCommitted,
    /// Provider range coverage reflects the installed physical bytes.
    CoverageCommitted,
    /// The operation is terminal and its cache-write reservation was released.
    Completed,
}

/// Result of an idempotent cache-write transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentCacheWriteRecordTransition {
    /// Durable state changed.
    Applied,
    /// Equivalent or later durable state already contains the transition.
    AlreadyApplied,
}

/// Recoverable provider-cache write journal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCacheWriteRecord {
    intent: ContentCacheWriteIntent,
    state: ContentCacheWriteState,
}

impl ContentCacheWriteRecord {
    /// Creates the first recoverable state after the entry write reservation is durable.
    #[must_use]
    pub const fn persist(intent: ContentCacheWriteIntent) -> Self {
        Self {
            intent,
            state: ContentCacheWriteState::IntentPersisted,
        }
    }

    /// Returns the immutable write intent.
    #[must_use]
    pub const fn intent(&self) -> &ContentCacheWriteIntent {
        &self.intent
    }

    /// Returns the current durable state.
    #[must_use]
    pub const fn state(&self) -> ContentCacheWriteState {
        self.state
    }

    /// Records that physical bytes were atomically installed.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn mark_physical_bytes_committed(&mut self) -> Result<ContentCacheWriteRecordTransition> {
        match self.state {
            ContentCacheWriteState::IntentPersisted => {
                self.state = ContentCacheWriteState::PhysicalBytesCommitted;
                Ok(ContentCacheWriteRecordTransition::Applied)
            }
            ContentCacheWriteState::PhysicalBytesCommitted
            | ContentCacheWriteState::CoverageCommitted
            | ContentCacheWriteState::Completed => {
                Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
            }
        }
    }

    /// Marks sparse coverage committed after physical bytes are observable.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn mark_coverage_committed(&mut self) -> Result<ContentCacheWriteRecordTransition> {
        match self.state {
            ContentCacheWriteState::PhysicalBytesCommitted => {
                self.state = ContentCacheWriteState::CoverageCommitted;
                Ok(ContentCacheWriteRecordTransition::Applied)
            }
            ContentCacheWriteState::CoverageCommitted | ContentCacheWriteState::Completed => {
                Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
            }
            ContentCacheWriteState::IntentPersisted => {
                Err(CloudFilesCoreError::invalid_content_cache_write(
                    "coverage commit requires observable physical bytes",
                ))
            }
        }
    }

    /// Marks the write terminal after coverage commit.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn complete(&mut self) -> Result<ContentCacheWriteRecordTransition> {
        match self.state {
            ContentCacheWriteState::CoverageCommitted => {
                self.state = ContentCacheWriteState::Completed;
                Ok(ContentCacheWriteRecordTransition::Applied)
            }
            ContentCacheWriteState::Completed => {
                Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
            }
            _ => Err(CloudFilesCoreError::invalid_content_cache_write(
                "cache write completion requires committed coverage",
            )),
        }
    }
}
