//! Owned File Provider item data independent from Objective-C/Swift object lifetimes.

use aster_forge_cloud_files_core::{CloudItem, CloudItemKey, CloudItemKind};

use crate::{MacosBridgeError, MacosFileProviderIdentifier, MacosFileProviderItemVersion, Result};

/// Portable item kind mapped by Swift to `NSFileProviderItemCapabilities` and content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosFileProviderItemKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
}

/// Fully owned File Provider item snapshot returned across the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosFileProviderItem {
    identifier: MacosFileProviderIdentifier,
    parent_identifier: MacosFileProviderIdentifier,
    filename: String,
    kind: MacosFileProviderItemKind,
    size: u64,
    version: MacosFileProviderItemVersion,
}

impl MacosFileProviderItem {
    /// Converts a core item into an owned File Provider snapshot scoped to one exact root.
    ///
    /// `root_key` determines both the root item role and whether a direct child's parent must use
    /// Apple's root system identifier instead of an ordinary encoded item identifier.
    pub fn from_cloud_item(item: &CloudItem, root_key: &CloudItemKey) -> Result<Self> {
        if item.key().scope() != root_key.scope() {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "item escaped the File Provider root scope",
            });
        }
        if item.is_root() != (item.key() == root_key) {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "item root shape did not match the File Provider root role",
            });
        }
        if item.name().contains('/') || item.name().contains('\0') {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "item filename is not one File Provider path component",
            });
        }
        let identifier = if item.is_root() {
            MacosFileProviderIdentifier::System(crate::MacosFileProviderSystemContainer::Root)
        } else {
            MacosFileProviderIdentifier::encode(item.key())?
        };
        let parent_identifier = match item.parent_id() {
            Some(parent_id) => {
                let parent_key = CloudItemKey::new(item.key().scope().clone(), parent_id.clone());
                if &parent_key == root_key {
                    MacosFileProviderIdentifier::System(
                        crate::MacosFileProviderSystemContainer::Root,
                    )
                } else {
                    MacosFileProviderIdentifier::encode(&parent_key)?
                }
            }
            None => {
                MacosFileProviderIdentifier::System(crate::MacosFileProviderSystemContainer::Root)
            }
        };
        let (kind, size, version) = match item.kind() {
            CloudItemKind::Directory => (
                MacosFileProviderItemKind::Directory,
                0,
                MacosFileProviderItemVersion::directory(item.metadata_revision().clone())?,
            ),
            CloudItemKind::File => {
                let content = item
                    .content()
                    .ok_or(MacosBridgeError::InvalidBackendResponse {
                        reason: "regular file omitted content metadata",
                    })?;
                (
                    MacosFileProviderItemKind::File,
                    content.size(),
                    MacosFileProviderItemVersion::new(
                        item.metadata_revision().clone(),
                        content.revision().clone(),
                    )?,
                )
            }
        };
        Ok(Self {
            identifier,
            parent_identifier,
            filename: item.name().to_owned(),
            kind,
            size,
            version,
        })
    }

    /// Returns the persistent item identifier.
    pub const fn identifier(&self) -> &MacosFileProviderIdentifier {
        &self.identifier
    }

    /// Returns the current parent identifier.
    pub const fn parent_identifier(&self) -> &MacosFileProviderIdentifier {
        &self.parent_identifier
    }

    /// Returns the current display filename.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Returns the item kind.
    pub const fn kind(&self) -> MacosFileProviderItemKind {
        self.kind
    }

    /// Returns the logical content size, or zero for directories.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns separate metadata/content versions for regular files.
    pub const fn version(&self) -> &MacosFileProviderItemVersion {
        &self.version
    }
}
