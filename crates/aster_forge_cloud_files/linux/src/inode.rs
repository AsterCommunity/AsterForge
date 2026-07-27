//! Stable Linux inode and generation records restored by a product-owned store.

use std::collections::HashMap;

use aster_forge_cloud_files_core::{CloudItemKey, CloudScope};

use crate::{LinuxCloudFilesError, Result};

/// Linux FUSE root inode.
pub const LINUX_ROOT_INODE: LinuxInode = LinuxInode(1);

/// Non-zero inode number scoped to one mounted filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinuxInode(u64);

impl LinuxInode {
    /// Creates a non-zero Linux inode.
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(LinuxCloudFilesError::ZeroInode);
        }
        Ok(Self(value))
    }

    /// Returns the native inode number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Non-zero generation fence paired with one restored Linux inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinuxInodeGeneration(u64);

impl LinuxInodeGeneration {
    /// Creates a non-zero inode generation fence.
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(LinuxCloudFilesError::ZeroGeneration);
        }
        Ok(Self(value))
    }

    /// Returns the native generation value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One durable mapping from a scoped stable cloud identity to a FUSE inode and generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxInodeRecord {
    key: CloudItemKey,
    inode: LinuxInode,
    generation: LinuxInodeGeneration,
}

impl LinuxInodeRecord {
    /// Creates one restored inode record.
    pub const fn new(
        key: CloudItemKey,
        inode: LinuxInode,
        generation: LinuxInodeGeneration,
    ) -> Self {
        Self {
            key,
            inode,
            generation,
        }
    }

    /// Returns the stable scoped cloud identity.
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the restored native inode.
    pub const fn inode(&self) -> LinuxInode {
        self.inode
    }

    /// Returns the restored inode generation fence.
    pub const fn generation(&self) -> LinuxInodeGeneration {
        self.generation
    }

    /// Consumes the record into its durable fields.
    pub fn into_parts(self) -> (CloudItemKey, LinuxInode, LinuxInodeGeneration) {
        (self.key, self.inode, self.generation)
    }
}

/// Immutable mapping restored before exposing a FUSE mount to the kernel.
///
/// Product code persists and restores these records. The table deliberately does not derive inode
/// values from names, paths, or hashes: a rename and a daemon restart must retain the same record.
#[derive(Debug, Clone)]
pub struct LinuxInodeTable {
    scope: CloudScope,
    root: LinuxInodeRecord,
    by_key: HashMap<CloudItemKey, LinuxInodeRecord>,
    by_inode: HashMap<LinuxInode, LinuxInodeRecord>,
}

impl LinuxInodeTable {
    /// Restores an immutable table. `records` must exclude the root record.
    pub fn new(
        root: LinuxInodeRecord,
        records: impl IntoIterator<Item = LinuxInodeRecord>,
    ) -> Result<Self> {
        if root.inode() != LINUX_ROOT_INODE {
            return Err(LinuxCloudFilesError::RootInodeMismatch);
        }
        let scope = root.key().scope().clone();
        let mut by_key = HashMap::new();
        let mut by_inode = HashMap::new();
        for record in records {
            if record.key().scope() != &scope {
                return Err(LinuxCloudFilesError::ScopeMismatch);
            }
            if record.inode() == LINUX_ROOT_INODE {
                return Err(LinuxCloudFilesError::RootInodeMismatch);
            }
            if by_key
                .insert(record.key().clone(), record.clone())
                .is_some()
            {
                return Err(LinuxCloudFilesError::DuplicateItem);
            }
            if by_inode.insert(record.inode(), record.clone()).is_some() {
                return Err(LinuxCloudFilesError::DuplicateInode {
                    inode: record.inode().get(),
                });
            }
        }
        Ok(Self {
            scope,
            root,
            by_key,
            by_inode,
        })
    }

    /// Returns the scope shared by every restored record.
    pub const fn scope(&self) -> &CloudScope {
        &self.scope
    }

    /// Returns the root record.
    pub const fn root(&self) -> &LinuxInodeRecord {
        &self.root
    }

    /// Returns the restored record for one stable key.
    pub fn by_key(&self, key: &CloudItemKey) -> Option<&LinuxInodeRecord> {
        if key == self.root.key() {
            Some(&self.root)
        } else {
            self.by_key.get(key)
        }
    }

    /// Returns the restored record for one native inode.
    pub fn by_inode(&self, inode: LinuxInode) -> Option<&LinuxInodeRecord> {
        if inode == LINUX_ROOT_INODE {
            Some(&self.root)
        } else {
            self.by_inode.get(&inode)
        }
    }

    /// Returns every non-root record in arbitrary map order for persistence checks.
    pub fn records(&self) -> impl Iterator<Item = &LinuxInodeRecord> {
        self.by_inode.values()
    }
}
