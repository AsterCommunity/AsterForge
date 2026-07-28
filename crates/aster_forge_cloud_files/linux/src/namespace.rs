//! Durable Linux namespace mutation acceptance for native file creation.

use aster_forge_cloud_files_core::{
    CloudItem, CloudItemKey, CloudScope, MutationIntent, SessionGeneration,
};
use async_trait::async_trait;

use crate::{LinuxFileAccess, LinuxInodeRecord, LinuxWriteSession, Result, validate_linux_name};

/// Result returned by the product-owned Linux namespace store.
pub type LinuxNamespaceStoreResult<T> = std::result::Result<T, LinuxNamespaceMutationStoreError>;

/// Stable classification of a Linux namespace persistence failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxNamespaceMutationStoreErrorKind {
    /// The requested parent, item, or durable staging record does not exist.
    NotFound,
    /// The requested parent/name pair already exists.
    AlreadyExists,
    /// A newer mount generation rejected the request.
    Fenced,
    /// Durable identity or namespace state conflicts with the requested transition.
    Conflict,
    /// The product store did not durably complete the requested transaction.
    PersistenceFailure,
}

/// Product-store failure returned at the Linux namespace boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("linux namespace store {kind:?}: {context}")]
pub struct LinuxNamespaceMutationStoreError {
    kind: LinuxNamespaceMutationStoreErrorKind,
    context: String,
}

impl LinuxNamespaceMutationStoreError {
    /// Creates a classified store error with adapter-owned diagnostic context.
    pub fn new(kind: LinuxNamespaceMutationStoreErrorKind, context: impl Into<String>) -> Self {
        Self {
            kind,
            context: context.into(),
        }
    }

    /// Returns the stable store-error classification.
    pub const fn kind(&self) -> LinuxNamespaceMutationStoreErrorKind {
        self.kind
    }

    /// Returns implementation diagnostic context. Product layers own user-facing text.
    pub fn context(&self) -> &str {
        &self.context
    }
}

/// Native request facts for one regular-file create operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCreateFileRequest {
    parent_key: CloudItemKey,
    name: String,
    mode: u32,
    umask: u32,
    access: LinuxFileAccess,
    session_generation: SessionGeneration,
}

impl LinuxCreateFileRequest {
    /// Creates a request without allocating any product identity.
    pub fn new(
        parent_key: CloudItemKey,
        name: impl Into<String>,
        mode: u32,
        umask: u32,
        access: LinuxFileAccess,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self {
            parent_key,
            name,
            mode,
            umask,
            access,
            session_generation,
        })
    }

    /// Returns the stable parent identity resolved from the requested inode.
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    /// Returns the exact requested Linux directory-component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the raw create mode supplied by FUSE.
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Returns the caller umask supplied by FUSE.
    pub const fn umask(&self) -> u32 {
        self.umask
    }

    /// Returns the access mode requested for the newly opened handle.
    pub const fn access(&self) -> LinuxFileAccess {
        self.access
    }

    /// Returns the active mount generation accepting this create.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Durable product allocation restored into the local Linux namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCreatedFile {
    item: CloudItem,
    inode_record: LinuxInodeRecord,
    mutation_intent: MutationIntent,
}

impl LinuxCreatedFile {
    /// Creates a durable local namespace record from product-allocated values.
    pub const fn new(
        item: CloudItem,
        inode_record: LinuxInodeRecord,
        mutation_intent: MutationIntent,
    ) -> Self {
        Self {
            item,
            inode_record,
            mutation_intent,
        }
    }

    /// Returns the product-neutral local item state.
    pub const fn item(&self) -> &CloudItem {
        &self.item
    }

    /// Returns the stable Linux inode/generation allocation.
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

    /// Returns the create mutation intent persisted in the same product transaction.
    pub const fn mutation_intent(&self) -> &MutationIntent {
        &self.mutation_intent
    }

    /// Consumes the record into its durable fields.
    pub fn into_parts(self) -> (CloudItem, LinuxInodeRecord, MutationIntent) {
        (self.item, self.inode_record, self.mutation_intent)
    }
}

/// Complete durable acceptance returned before FUSE reports create success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCreateFileAcceptance {
    created: LinuxCreatedFile,
    session: LinuxWriteSession,
}

impl LinuxCreateFileAcceptance {
    /// Creates an acceptance containing durable namespace, mutation, and staging state.
    pub const fn new(created: LinuxCreatedFile, session: LinuxWriteSession) -> Self {
        Self { created, session }
    }

    /// Returns the durable local namespace allocation.
    pub const fn created(&self) -> &LinuxCreatedFile {
        &self.created
    }

    /// Returns the empty local staging session opened by the transaction.
    pub const fn session(&self) -> &LinuxWriteSession {
        &self.session
    }

    /// Consumes the acceptance into the durable record and opened staging session.
    pub fn into_parts(self) -> (LinuxCreatedFile, LinuxWriteSession) {
        (self.created, self.session)
    }
}

/// Product-owned durable namespace transaction used by native Linux create.
///
/// `create_file` must atomically persist the stable `CloudItemKey`, inode/generation record,
/// complete core mutation intent, empty staging state, and mount-generation comparison before it
/// returns. The port never asks Forge to derive identities from a name, path, hash, or callback.
#[async_trait]
pub trait LinuxNamespaceMutationStore: Send + Sync {
    /// Activates a mount generation and returns non-terminal local creates for startup recovery.
    async fn activate_namespace(
        &self,
        scope: &CloudScope,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<Vec<LinuxCreatedFile>>;

    /// Atomically accepts a create and opens its empty local staging session.
    async fn create_file(
        &self,
        request: &LinuxCreateFileRequest,
    ) -> LinuxNamespaceStoreResult<LinuxCreateFileAcceptance>;

    /// Opens local staging for an already accepted create that is not remotely materialized yet.
    async fn open_created_file(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<LinuxWriteSession>;
}
