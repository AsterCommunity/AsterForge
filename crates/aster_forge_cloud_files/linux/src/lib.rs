//! Product-neutral Linux FUSE bindings for Forge cloud-files core.
//!
//! This crate owns stable inode/generation mappings, directory-handle snapshots, file-handle
//! revision fences, direct-I/O writeback, durable namespace mutation acceptance, remote-change
//! overlays, kernel invalidation plans, mount-generation recovery, FUSE reply mapping, and bounded
//! callback-to-async dispatch. Product crates retain identity allocation, backend adapters,
//! durable inode/content storage, change cursors, remote workers, daemon/service packaging, mount
//! UX, authentication, permissions, and user-visible errors.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod dispatch;
mod engine;
mod error;
mod inode;
mod namespace;
mod remote;
mod writeback;

#[cfg(target_os = "linux")]
mod native;

pub use dispatch::{
    LinuxDispatchMetrics, LinuxDispatchRejection, LinuxDispatchReservation, LinuxRequestDispatcher,
};
pub use engine::{
    LinuxAttributePolicy, LinuxDirectoryEntry, LinuxDirectoryHandle, LinuxDirectorySnapshot,
    LinuxFileAttributes, LinuxFileHandle, LinuxNode, LinuxNodeKind, LinuxReadOnlyEngine,
    validate_linux_name,
};
pub use error::{
    LinuxCloudFilesError, LinuxErrorCode, Result, backend_error_code, namespace_store_error_code,
    writeback_store_error_code,
};
pub use inode::{
    LINUX_ROOT_INODE, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord, LinuxInodeTable,
};
pub use namespace::{
    LinuxCreateDirectoryRequest, LinuxCreateFileAcceptance, LinuxCreateFileRequest,
    LinuxCreatedFile, LinuxNamespaceItem, LinuxNamespaceMutationStore,
    LinuxNamespaceMutationStoreError, LinuxNamespaceMutationStoreErrorKind, LinuxNamespaceOverlay,
    LinuxNamespaceStoreResult, LinuxNamespaceTombstone, LinuxRemoveRequest, LinuxRenameAcceptance,
    LinuxRenameDestination, LinuxRenameRequest,
};
#[cfg(target_os = "linux")]
pub use native::{
    LinuxBackgroundSession, LinuxKernelNotifier, LinuxReadOnlyFilesystem, LinuxWritableFilesystem,
    mount_read_only, mount_writable, spawn_mount_read_only, spawn_mount_writable,
};
pub use remote::{
    LinuxInvalidation, LinuxRemoteChange, LinuxRemoteDelete, LinuxRemoteEntry, LinuxRemoteLocation,
    LinuxRemoteUpsert,
};
pub use writeback::{
    LinuxFileAccess, LinuxWritableEngine, LinuxWriteCommit, LinuxWriteOpenRequest,
    LinuxWriteSession, LinuxWriteSessionId, LinuxWritebackStore,
};

/// Native mount-session configuration kept independent from product daemon policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxMountConfig {
    filesystem_name: String,
    subtype: Option<String>,
    kernel_threads: Option<usize>,
    clone_fd: bool,
}

impl LinuxMountConfig {
    /// Creates a read-only mount configuration with one fuser event-loop thread.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(filesystem_name: impl Into<String>) -> Result<Self> {
        let filesystem_name = filesystem_name.into();
        validate_mount_text(&filesystem_name, "filesystem name")?;
        Ok(Self {
            filesystem_name,
            subtype: None,
            kernel_threads: None,
            clone_fd: false,
        })
    }

    /// Sets the optional FUSE filesystem subtype.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn with_subtype(mut self, subtype: impl Into<String>) -> Result<Self> {
        let subtype = subtype.into();
        validate_mount_text(&subtype, "filesystem subtype")?;
        self.subtype = Some(subtype);
        Ok(self)
    }

    /// Sets native event-loop concurrency and Linux `FUSE_DEV_IOC_CLONE` use.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn with_kernel_threads(mut self, threads: usize, clone_fd: bool) -> Result<Self> {
        if threads == 0 {
            return Err(LinuxCloudFilesError::InvalidConfiguration {
                reason: "kernel event-loop thread count must be greater than zero",
            });
        }
        self.kernel_threads = Some(threads);
        self.clone_fd = clone_fd;
        Ok(self)
    }

    /// Returns the filesystem name reported by the mount.
    #[must_use]
    pub fn filesystem_name(&self) -> &str {
        &self.filesystem_name
    }

    /// Returns the optional filesystem subtype.
    #[must_use]
    pub fn subtype(&self) -> Option<&str> {
        self.subtype.as_deref()
    }

    /// Returns native fuser event-loop concurrency.
    #[must_use]
    pub const fn kernel_threads(&self) -> Option<usize> {
        self.kernel_threads
    }

    /// Returns whether Linux should clone the FUSE device fd for event-loop workers.
    #[must_use]
    pub const fn clone_fd(&self) -> bool {
        self.clone_fd
    }
}

fn validate_mount_text(value: &str, _field: &'static str) -> Result<()> {
    if value.is_empty() {
        return Err(LinuxCloudFilesError::InvalidConfiguration {
            reason: "mount identity text must not be empty",
        });
    }
    if value.contains('\0') || value.contains(',') {
        return Err(LinuxCloudFilesError::InvalidConfiguration {
            reason: "mount identity text must not contain NUL or comma",
        });
    }
    Ok(())
}
