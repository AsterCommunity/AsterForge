//! Product-neutral acceptance values for durable remote namespace changes.

use aster_forge_cloud_files_core::{CloudItem, CloudItemKey};

use crate::{LinuxInode, LinuxInodeRecord, LinuxNodeKind, Result, validate_linux_name};

/// One durable remote item and its product-allocated stable Linux mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRemoteEntry {
    item: CloudItem,
    inode_record: LinuxInodeRecord,
}

impl LinuxRemoteEntry {
    /// Creates an entry from a durable backend item and its restored inode record.
    #[must_use]
    pub const fn new(item: CloudItem, inode_record: LinuxInodeRecord) -> Self {
        Self { item, inode_record }
    }

    /// Returns the durable backend item.
    #[must_use]
    pub const fn item(&self) -> &CloudItem {
        &self.item
    }

    /// Returns the product-allocated stable inode record for this item.
    #[must_use]
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

    /// Consumes the entry and returns its backend item and inode record.
    #[must_use]
    pub fn into_parts(self) -> (CloudItem, LinuxInodeRecord) {
        (self.item, self.inode_record)
    }
}

/// Last durable directory location of an upserted stable item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRemoteLocation {
    parent_key: CloudItemKey,
    name: String,
}

impl LinuxRemoteLocation {
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(parent_key: CloudItemKey, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self { parent_key, name })
    }

    /// Returns the durable parent item key.
    #[must_use]
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    /// Returns the validated remote name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Exact durable facts for one remote deletion or destination replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRemoteDelete {
    key: CloudItemKey,
    inode_record: LinuxInodeRecord,
    parent_key: CloudItemKey,
    name: String,
    kind: LinuxNodeKind,
}

impl LinuxRemoteDelete {
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(
        key: CloudItemKey,
        inode_record: LinuxInodeRecord,
        parent_key: CloudItemKey,
        name: impl Into<String>,
        kind: LinuxNodeKind,
    ) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self {
            key,
            inode_record,
            parent_key,
            name,
            kind,
        })
    }

    /// Returns the stable key of the deleted or replaced item.
    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the inode record released by this deletion or replacement.
    #[must_use]
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

    /// Returns the durable parent item key.
    #[must_use]
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    /// Returns the validated remote name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the node kind of the deleted or replaced item.
    #[must_use]
    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }
}

/// Durable product transaction result for one remote create, metadata update, or move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRemoteUpsert {
    entry: LinuxRemoteEntry,
    previous: Option<LinuxRemoteLocation>,
    replaced: Option<LinuxRemoteDelete>,
}

impl LinuxRemoteUpsert {
    /// Creates an upsert result from the durable entry, its prior location, and any replaced item.
    #[must_use]
    pub const fn new(
        entry: LinuxRemoteEntry,
        previous: Option<LinuxRemoteLocation>,
        replaced: Option<LinuxRemoteDelete>,
    ) -> Self {
        Self {
            entry,
            previous,
            replaced,
        }
    }

    /// Returns the durable entry describing the new remote state.
    #[must_use]
    pub const fn entry(&self) -> &LinuxRemoteEntry {
        &self.entry
    }

    /// Returns the prior durable location for a move or rename, if one existed.
    #[must_use]
    pub const fn previous(&self) -> Option<&LinuxRemoteLocation> {
        self.previous.as_ref()
    }

    /// Returns the exact facts of a replaced destination item, if any.
    #[must_use]
    pub const fn replaced(&self) -> Option<&LinuxRemoteDelete> {
        self.replaced.as_ref()
    }

    /// Consumes the result and returns the entry, prior location, and replaced item.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        LinuxRemoteEntry,
        Option<LinuxRemoteLocation>,
        Option<LinuxRemoteDelete>,
    ) {
        (self.entry, self.previous, self.replaced)
    }
}

/// One product-persisted remote namespace transition ready for engine exposure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "remote changes are low-frequency control values and should not require heap allocation"
)]
pub enum LinuxRemoteChange {
    /// A durable create, metadata update, or move.
    Upsert(LinuxRemoteUpsert),
    /// A durable deletion or destination replacement.
    Delete(LinuxRemoteDelete),
}

/// One native kernel-cache notification produced after an engine transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxInvalidation {
    /// Invalidate one directory entry by parent inode and name.
    Entry {
        /// Parent directory inode containing the entry.
        parent: LinuxInode,
        /// Entry name to invalidate.
        name: String,
    },
    /// Invalidate all kernel state for one inode.
    Inode {
        /// Inode whose cached attributes and contents become invalid.
        inode: LinuxInode,
    },
    /// Remove one deleted child from the kernel dcache by parent and name.
    Delete {
        /// Parent directory inode whose child was removed.
        parent: LinuxInode,
        /// Inode of the removed child.
        child: LinuxInode,
        /// Removed child name.
        name: String,
    },
}
