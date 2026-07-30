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
    #[must_use]
    pub const fn new(item: CloudItem, inode_record: LinuxInodeRecord) -> Self {
        Self { item, inode_record }
    }

    #[must_use]
    pub const fn item(&self) -> &CloudItem {
        &self.item
    }

    #[must_use]
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

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

    #[must_use]
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

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

    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    #[must_use]
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

    #[must_use]
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

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

    #[must_use]
    pub const fn entry(&self) -> &LinuxRemoteEntry {
        &self.entry
    }

    #[must_use]
    pub const fn previous(&self) -> Option<&LinuxRemoteLocation> {
        self.previous.as_ref()
    }

    #[must_use]
    pub const fn replaced(&self) -> Option<&LinuxRemoteDelete> {
        self.replaced.as_ref()
    }

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
    Upsert(LinuxRemoteUpsert),
    Delete(LinuxRemoteDelete),
}

/// One native kernel-cache notification produced after an engine transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxInvalidation {
    Entry {
        parent: LinuxInode,
        name: String,
    },
    Inode {
        inode: LinuxInode,
    },
    Delete {
        parent: LinuxInode,
        child: LinuxInode,
        name: String,
    },
}
