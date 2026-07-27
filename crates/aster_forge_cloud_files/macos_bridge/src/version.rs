//! Exact mapping to the two-part `NSFileProviderItemVersion` contract.

use aster_forge_cloud_files_core::{ContentRevision, MetadataRevision};

use crate::{MacosBridgeError, Result};

/// Maximum size of either `NSFileProviderItemVersion` component.
pub const FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES: usize = 128;

const DIRECTORY_CONTENT_VERSION: &[u8] = b"aster-forge-directory-v1";

/// Owned metadata/content versions for one File Provider item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosFileProviderItemVersion {
    metadata: MetadataRevision,
    content: ContentRevision,
}

impl MacosFileProviderItemVersion {
    /// Creates an item version while preserving both opaque revision values exactly.
    pub fn new(metadata: MetadataRevision, content: ContentRevision) -> Result<Self> {
        validate_component(metadata.as_bytes(), "metadata version")?;
        validate_component(content.as_bytes(), "content version")?;
        Ok(Self { metadata, content })
    }

    /// Creates a directory version with a fixed content component and exact metadata revision.
    pub fn directory(metadata: MetadataRevision) -> Result<Self> {
        let content = ContentRevision::from_slice(DIRECTORY_CONTENT_VERSION).map_err(|_| {
            MacosBridgeError::InvalidItemVersion {
                reason: "directory content version could not be represented",
            }
        })?;
        Self::new(metadata, content)
    }

    /// Returns the opaque metadata version bytes.
    pub const fn metadata(&self) -> &MetadataRevision {
        &self.metadata
    }

    /// Returns the opaque content version bytes.
    pub const fn content(&self) -> &ContentRevision {
        &self.content
    }

    /// Consumes the version into metadata and content components.
    pub fn into_parts(self) -> (MetadataRevision, ContentRevision) {
        (self.metadata, self.content)
    }
}

fn validate_component(value: &[u8], _field: &'static str) -> Result<()> {
    if value.len() > FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES {
        return Err(MacosBridgeError::InvalidItemVersion {
            reason: "item version component exceeds the File Provider 128-byte limit",
        });
    }
    Ok(())
}
