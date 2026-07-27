//! Provisional read-only backend ports and revision-bound content read values.
//!
//! These traits intentionally cover Phase 1 metadata, enumeration, change discovery, and content
//! reads. Resumable content mutation uses the separate upload port and durable journal model. The
//! async dispatch shape remains provisional until synthetic and platform PoCs exercise hot paths.

use std::num::NonZeroU64;

use async_trait::async_trait;
use bytes::Bytes;

use crate::{
    BackendResult, ChangeCursor, ChangePage, CloudFilesCapabilities, CloudFilesCoreError,
    CloudItem, CloudItemKey, CloudItemPage, CloudScope, ContentRevision, PageCursor, Result,
};

/// Non-empty half-open byte range `[offset, end_exclusive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteRange {
    offset: u64,
    length: NonZeroU64,
}

impl ByteRange {
    /// Creates a non-empty range and rejects end-position overflow.
    pub fn new(offset: u64, length: u64) -> Result<Self> {
        let Some(length) = NonZeroU64::new(length) else {
            return Err(CloudFilesCoreError::invalid_byte_range(
                "range length must be greater than zero",
            ));
        };
        if offset.checked_add(length.get()).is_none() {
            return Err(CloudFilesCoreError::invalid_byte_range(
                "range end exceeds u64",
            ));
        }
        Ok(Self { offset, length })
    }

    /// Returns the first requested byte offset.
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Returns the requested byte length.
    pub const fn length(self) -> u64 {
        self.length.get()
    }

    /// Returns the exclusive range end. Construction guarantees this addition cannot overflow.
    pub const fn end_exclusive(self) -> u64 {
        self.offset + self.length.get()
    }
}

/// Requested content extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentReadRange {
    /// Read the complete file.
    Whole,
    /// Read one exact logical range, truncated only at end-of-file.
    Range(ByteRange),
}

/// Revision-bound content read request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentReadRequest {
    key: CloudItemKey,
    revision: ContentRevision,
    expected_size: u64,
    range: ContentReadRange,
}

impl ContentReadRequest {
    /// Creates a complete-file read request.
    pub const fn whole(key: CloudItemKey, revision: ContentRevision, expected_size: u64) -> Self {
        Self {
            key,
            revision,
            expected_size,
            range: ContentReadRange::Whole,
        }
    }

    /// Creates a range read request.
    pub const fn range(
        key: CloudItemKey,
        revision: ContentRevision,
        expected_size: u64,
        range: ByteRange,
    ) -> Self {
        Self {
            key,
            revision,
            expected_size,
            range: ContentReadRange::Range(range),
        }
    }

    /// Returns the fully scoped item identity.
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the exact expected content revision.
    pub const fn revision(&self) -> &ContentRevision {
        &self.revision
    }

    /// Returns the logical size reported by metadata for this exact revision.
    pub const fn expected_size(&self) -> u64 {
        self.expected_size
    }

    /// Returns the requested extent.
    pub const fn read_range(&self) -> ContentReadRange {
        self.range
    }

    /// Validates that a backend response satisfies this exact request.
    pub fn validate_response(&self, response: &ContentReadResponse) -> Result<()> {
        if response.revision() != &self.revision {
            return Err(CloudFilesCoreError::invalid_content_response(
                "response revision does not match the requested revision",
            ));
        }
        if response.total_size() != self.expected_size {
            return Err(CloudFilesCoreError::invalid_content_response(
                "response size does not match metadata for the requested revision",
            ));
        }
        match self.range {
            ContentReadRange::Whole => {
                if response.offset() != 0
                    || response.byte_len() != response.total_size()
                    || !response.is_complete_file()
                {
                    return Err(CloudFilesCoreError::invalid_content_response(
                        "whole-file response does not contain the complete file",
                    ));
                }
            }
            ContentReadRange::Range(range) => {
                if response.offset() != range.offset() {
                    return Err(CloudFilesCoreError::invalid_content_response(
                        "range response starts at a different offset",
                    ));
                }
                let expected_len = if range.offset() >= response.total_size() {
                    0
                } else {
                    std::cmp::min(
                        range.length(),
                        response.total_size().saturating_sub(range.offset()),
                    )
                };
                if response.byte_len() != expected_len {
                    return Err(CloudFilesCoreError::invalid_content_response(
                        "range response length does not match the requested extent",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Owned content bytes returned for one exact revision and logical offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentReadResponse {
    revision: ContentRevision,
    offset: u64,
    bytes: Bytes,
    byte_len: u64,
    total_size: u64,
}

impl ContentReadResponse {
    /// Creates a response and verifies that its bytes remain within the logical file size.
    pub fn new(
        revision: ContentRevision,
        offset: u64,
        bytes: Bytes,
        total_size: u64,
    ) -> Result<Self> {
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            CloudFilesCoreError::invalid_content_response(
                "response byte length cannot be represented as u64",
            )
        })?;
        let Some(end) = offset.checked_add(byte_len) else {
            return Err(CloudFilesCoreError::invalid_content_response(
                "response byte range exceeds u64",
            ));
        };
        if offset > total_size || end > total_size {
            return Err(CloudFilesCoreError::invalid_content_response(
                "response bytes exceed the logical file size",
            ));
        }
        Ok(Self {
            revision,
            offset,
            bytes,
            byte_len,
            total_size,
        })
    }

    /// Returns the exact content revision represented by these bytes.
    pub const fn revision(&self) -> &ContentRevision {
        &self.revision
    }

    /// Returns the logical byte offset.
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the content bytes.
    pub const fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Returns the number of content bytes.
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Returns the complete logical file size for this revision.
    pub const fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns whether the response contains the complete file, including an empty file.
    pub fn is_complete_file(&self) -> bool {
        self.offset == 0 && self.byte_len() == self.total_size
    }

    /// Consumes the response and returns its revision, offset, bytes, and total size.
    pub fn into_parts(self) -> (ContentRevision, u64, Bytes, u64) {
        (self.revision, self.offset, self.bytes, self.total_size)
    }
}

/// Product-owned metadata, enumeration, and change-discovery adapter.
#[async_trait]
pub trait CloudMetadataBackend: Send + Sync {
    /// Loads one item by stable scoped identity.
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem>;

    /// Lists one directory page. A page cursor is valid only for the same parent enumeration.
    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage>;

    /// Loads the next anchored change batch or an explicit reset result.
    async fn changes_since(
        &self,
        scope: &CloudScope,
        cursor: Option<&ChangeCursor>,
    ) -> BackendResult<ChangePage>;
}

/// Product-owned revision-bound content adapter.
#[async_trait]
pub trait CloudContentBackend: Send + Sync {
    /// Reads complete or ranged content for one exact revision.
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse>;
}

/// Complete provisional read-only backend contract used by Phase 1 executable models.
pub trait CloudFilesBackend: CloudMetadataBackend + CloudContentBackend {
    /// Returns backend constraints. Dimensions not owned by the backend use `unconstrained()`.
    fn capabilities(&self) -> CloudFilesCapabilities;
}
