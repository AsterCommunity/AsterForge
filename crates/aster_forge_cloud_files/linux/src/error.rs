//! Linux adapter errors and their portable errno classifications.

use aster_forge_cloud_files_core::{
    CloudBackendError, CloudBackendErrorKind, CloudFilesStoreError, CloudFilesStoreErrorKind,
};

/// Result returned by the Linux cloud-files adapter.
pub type Result<T> = std::result::Result<T, LinuxCloudFilesError>;

/// Portable errno classification used before the native FUSE boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxErrorCode {
    /// No matching inode, handle, item, or directory entry exists.
    NotFound,
    /// A namespace create collided with an existing parent/name entry.
    AlreadyExists,
    /// A directory removal was rejected because children still exist.
    DirectoryNotEmpty,
    /// A regular-file operation resolved a directory.
    IsDirectory,
    /// A directory operation resolved another item kind.
    NotDirectory,
    /// The caller lacks access or must authenticate before access can proceed.
    AccessDenied,
    /// The read-only adapter rejected a mutation attempt.
    ReadOnlyFilesystem,
    /// The bounded dispatcher is temporarily saturated.
    TryAgain,
    /// A file handle or revision snapshot is stale.
    Stale,
    /// The requested operation is not implemented by the active backend or adapter.
    NotSupported,
    /// The request shape is invalid at the native boundary.
    InvalidArgument,
    /// An internal, contract, or transport failure occurred.
    Io,
}

/// Errors produced by Linux inode, handle, and read-only FUSE mechanics.
#[derive(Debug, thiserror::Error)]
pub enum LinuxCloudFilesError {
    /// Inode zero is reserved by the FUSE protocol.
    #[error("linux inode must be non-zero")]
    ZeroInode,
    /// Generation zero is rejected so stale inode records cannot silently bypass fencing.
    #[error("linux inode generation must be non-zero")]
    ZeroGeneration,
    /// The root record did not use inode one.
    #[error("linux root inode must be one")]
    RootInodeMismatch,
    /// A supplied record did not belong to the active root scope.
    #[error("linux inode record belongs to another cloud scope")]
    ScopeMismatch,
    /// Two persisted records attempted to reuse one inode.
    #[error("duplicate linux inode {inode}")]
    DuplicateInode {
        /// Conflicting inode number.
        inode: u64,
    },
    /// Two persisted records attempted to map one stable key to different inode records.
    #[error("duplicate linux inode record for cloud item")]
    DuplicateItem,
    /// A backend item did not have a restored inode mapping.
    #[error("missing restored linux inode record for cloud item")]
    MissingInodeRecord,
    /// An inode was not known to the active mount session.
    #[error("unknown linux inode {inode}")]
    UnknownInode {
        /// Unknown inode number.
        inode: u64,
    },
    /// An open file or directory handle no longer belongs to the requested inode.
    #[error("stale linux file handle")]
    StaleHandle,
    /// A file-handle operation did not match the access mode selected at open time.
    #[error("linux file handle access mode does not permit this operation")]
    AccessModeMismatch,
    /// In-memory handle allocation exhausted the native handle domain.
    #[error("linux file handle space is exhausted")]
    HandleExhausted,
    /// A mount or attribute policy used an invalid value.
    #[error("invalid linux cloud-files configuration: {reason}")]
    InvalidConfiguration {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A file name cannot be represented by this string-based backend contract.
    #[error("invalid linux filename: {reason}")]
    InvalidName {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// An operation required a directory but received a file.
    #[error("cloud item is not a directory")]
    NotDirectory,
    /// An operation required a regular file but received a directory.
    #[error("cloud item is not a regular file")]
    NotFile,
    /// A product backend returned metadata that violated its scoped item contract.
    #[error("invalid cloud backend response: {reason}")]
    InvalidBackendResponse {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// The product-owned backend reported a classified failure.
    #[error(transparent)]
    Backend(#[from] CloudBackendError),
    /// The product-owned durable writeback store reported a classified failure.
    #[error(transparent)]
    WritebackStore(#[from] CloudFilesStoreError),
    /// The product-owned durable namespace store reported a classified failure.
    #[error(transparent)]
    NamespaceStore(#[from] crate::LinuxNamespaceMutationStoreError),
    /// Native create was requested without a product namespace transaction port.
    #[error("linux namespace mutation store is not configured")]
    NamespaceMutationNotConfigured,
}

impl LinuxCloudFilesError {
    /// Returns the portable errno classification expected by the FUSE adapter.
    #[must_use]
    pub const fn error_code(&self) -> LinuxErrorCode {
        match self {
            Self::UnknownInode { .. } => LinuxErrorCode::NotFound,
            Self::StaleHandle => LinuxErrorCode::Stale,
            Self::AccessModeMismatch => LinuxErrorCode::AccessDenied,
            Self::InvalidName { .. } => LinuxErrorCode::InvalidArgument,
            Self::NotDirectory => LinuxErrorCode::NotDirectory,
            Self::NotFile => LinuxErrorCode::IsDirectory,
            Self::Backend(error) => backend_error_code(error.kind()),
            Self::WritebackStore(error) => writeback_store_error_code(error.kind()),
            Self::NamespaceStore(error) => namespace_store_error_code(error.kind()),
            Self::NamespaceMutationNotConfigured => LinuxErrorCode::NotSupported,
            Self::ZeroInode
            | Self::ZeroGeneration
            | Self::RootInodeMismatch
            | Self::ScopeMismatch
            | Self::DuplicateInode { .. }
            | Self::DuplicateItem
            | Self::MissingInodeRecord
            | Self::HandleExhausted
            | Self::InvalidConfiguration { .. }
            | Self::InvalidBackendResponse { .. } => LinuxErrorCode::Io,
        }
    }
}

/// Maps durable Linux namespace failures to FUSE-visible classifications.
#[must_use]
pub const fn namespace_store_error_code(
    kind: crate::LinuxNamespaceMutationStoreErrorKind,
) -> LinuxErrorCode {
    match kind {
        crate::LinuxNamespaceMutationStoreErrorKind::NotFound => LinuxErrorCode::NotFound,
        crate::LinuxNamespaceMutationStoreErrorKind::AlreadyExists => LinuxErrorCode::AlreadyExists,
        crate::LinuxNamespaceMutationStoreErrorKind::DirectoryNotEmpty => {
            LinuxErrorCode::DirectoryNotEmpty
        }
        crate::LinuxNamespaceMutationStoreErrorKind::IsDirectory => LinuxErrorCode::IsDirectory,
        crate::LinuxNamespaceMutationStoreErrorKind::NotDirectory => LinuxErrorCode::NotDirectory,
        crate::LinuxNamespaceMutationStoreErrorKind::Unsupported => LinuxErrorCode::NotSupported,
        crate::LinuxNamespaceMutationStoreErrorKind::Fenced => LinuxErrorCode::Stale,
        crate::LinuxNamespaceMutationStoreErrorKind::Conflict
        | crate::LinuxNamespaceMutationStoreErrorKind::PersistenceFailure => LinuxErrorCode::Io,
    }
}

/// Maps durable local-write failures to FUSE-visible classifications.
#[must_use]
pub const fn writeback_store_error_code(kind: CloudFilesStoreErrorKind) -> LinuxErrorCode {
    match kind {
        CloudFilesStoreErrorKind::NotFound => LinuxErrorCode::NotFound,
        CloudFilesStoreErrorKind::Conflict => LinuxErrorCode::Stale,
        CloudFilesStoreErrorKind::InvalidTransition
        | CloudFilesStoreErrorKind::PersistenceFailure => LinuxErrorCode::Io,
    }
}

/// Maps product-neutral backend failures to FUSE-visible classifications.
#[must_use]
pub const fn backend_error_code(kind: CloudBackendErrorKind) -> LinuxErrorCode {
    match kind {
        CloudBackendErrorKind::NotFound => LinuxErrorCode::NotFound,
        CloudBackendErrorKind::AuthenticationRequired | CloudBackendErrorKind::PermissionDenied => {
            LinuxErrorCode::AccessDenied
        }
        CloudBackendErrorKind::Conflict | CloudBackendErrorKind::PreconditionFailed => {
            LinuxErrorCode::Stale
        }
        CloudBackendErrorKind::InvalidRequest => LinuxErrorCode::InvalidArgument,
        CloudBackendErrorKind::RateLimited | CloudBackendErrorKind::TemporarilyUnavailable => {
            LinuxErrorCode::TryAgain
        }
        CloudBackendErrorKind::Unsupported => LinuxErrorCode::NotSupported,
        CloudBackendErrorKind::InvalidResponse | CloudBackendErrorKind::Internal => {
            LinuxErrorCode::Io
        }
    }
}
