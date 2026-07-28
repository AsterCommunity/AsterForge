//! Durable Linux namespace mutation acceptance for native file creation.

use aster_forge_cloud_files_core::{
    CloudItem, CloudItemKey, CloudScope, MutationIntent, SessionGeneration,
};
use async_trait::async_trait;

use crate::{
    LinuxFileAccess, LinuxInodeGeneration, LinuxInodeRecord, LinuxNodeKind, LinuxWriteSession,
    Result, validate_linux_name,
};

/// Result returned by the product-owned Linux namespace store.
pub type LinuxNamespaceStoreResult<T> = std::result::Result<T, LinuxNamespaceMutationStoreError>;

/// Stable classification of a Linux namespace persistence failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxNamespaceMutationStoreErrorKind {
    /// The requested parent, item, or durable staging record does not exist.
    NotFound,
    /// The requested parent/name pair already exists.
    AlreadyExists,
    /// A directory removal was rejected because durable children still exist.
    DirectoryNotEmpty,
    /// The requested operation expected a regular file but resolved a directory.
    IsDirectory,
    /// The requested operation expected a directory but resolved another item kind.
    NotDirectory,
    /// The product store does not implement this namespace operation.
    Unsupported,
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

/// Stable namespace item materialized by a product transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxNamespaceItem {
    item: CloudItem,
    inode_record: LinuxInodeRecord,
    mutation_intent: MutationIntent,
}

impl LinuxNamespaceItem {
    /// Creates a durable namespace item with its stable native mapping and mutation intent.
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

    /// Returns current product-neutral metadata for the local namespace overlay.
    pub const fn item(&self) -> &CloudItem {
        &self.item
    }

    /// Returns the stable inode/generation record preserved across rename and restart.
    pub const fn inode_record(&self) -> &LinuxInodeRecord {
        &self.inode_record
    }

    /// Returns the durable core mutation that produced this local overlay state.
    pub const fn mutation_intent(&self) -> &MutationIntent {
        &self.mutation_intent
    }

    /// Consumes the item into its durable fields.
    pub fn into_parts(self) -> (CloudItem, LinuxInodeRecord, MutationIntent) {
        (self.item, self.inode_record, self.mutation_intent)
    }
}

impl From<LinuxCreatedFile> for LinuxNamespaceItem {
    fn from(value: LinuxCreatedFile) -> Self {
        let (item, inode_record, mutation_intent) = value.into_parts();
        Self::new(item, inode_record, mutation_intent)
    }
}

/// Durable local deletion marker retained until remote mutation reconciliation completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxNamespaceTombstone {
    key: CloudItemKey,
    inode_generation: LinuxInodeGeneration,
    parent_key: CloudItemKey,
    name: String,
    kind: LinuxNodeKind,
    mutation_intent: MutationIntent,
}

impl LinuxNamespaceTombstone {
    /// Creates a tombstone from the exact removed namespace facts.
    pub fn new(
        key: CloudItemKey,
        inode_generation: LinuxInodeGeneration,
        parent_key: CloudItemKey,
        name: impl Into<String>,
        kind: LinuxNodeKind,
        mutation_intent: MutationIntent,
    ) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self {
            key,
            inode_generation,
            parent_key,
            name,
            kind,
            mutation_intent,
        })
    }

    /// Returns the stable removed item identity.
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the removed inode generation used to reject substituted tombstones.
    pub const fn inode_generation(&self) -> LinuxInodeGeneration {
        self.inode_generation
    }

    /// Returns the exact former parent identity.
    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    /// Returns the exact former directory-component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether the removed entry was a regular file or directory.
    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }

    /// Returns the durable delete intent.
    pub const fn mutation_intent(&self) -> &MutationIntent {
        &self.mutation_intent
    }
}

/// Restored local namespace state not represented by the legacy regular-file create list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxNamespaceOverlay {
    items: Vec<LinuxNamespaceItem>,
    tombstones: Vec<LinuxNamespaceTombstone>,
}

impl LinuxNamespaceOverlay {
    /// Creates a restart overlay from current local entries and deletion markers.
    pub const fn new(
        items: Vec<LinuxNamespaceItem>,
        tombstones: Vec<LinuxNamespaceTombstone>,
    ) -> Self {
        Self { items, tombstones }
    }

    /// Returns current local namespace entries.
    pub fn items(&self) -> &[LinuxNamespaceItem] {
        &self.items
    }

    /// Returns current local deletion markers.
    pub fn tombstones(&self) -> &[LinuxNamespaceTombstone] {
        &self.tombstones
    }

    /// Consumes the overlay into its durable collections.
    pub fn into_parts(self) -> (Vec<LinuxNamespaceItem>, Vec<LinuxNamespaceTombstone>) {
        (self.items, self.tombstones)
    }
}

/// Native facts for one durable directory create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCreateDirectoryRequest {
    parent_key: CloudItemKey,
    name: String,
    mode: u32,
    umask: u32,
    session_generation: SessionGeneration,
}

impl LinuxCreateDirectoryRequest {
    /// Creates a directory request without allocating product identity.
    pub fn new(
        parent_key: CloudItemKey,
        name: impl Into<String>,
        mode: u32,
        umask: u32,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self {
            parent_key,
            name,
            mode,
            umask,
            session_generation,
        })
    }

    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn mode(&self) -> u32 {
        self.mode
    }

    pub const fn umask(&self) -> u32 {
        self.umask
    }

    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Native facts for one rename or move that preserves stable item identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRenameRequest {
    key: CloudItemKey,
    inode_generation: LinuxInodeGeneration,
    kind: LinuxNodeKind,
    old_parent_key: CloudItemKey,
    old_name: String,
    new_parent_key: CloudItemKey,
    new_name: String,
    destination: Option<LinuxRenameDestination>,
    no_replace: bool,
    session_generation: SessionGeneration,
}

impl LinuxRenameRequest {
    #[expect(
        clippy::too_many_arguments,
        reason = "the request preserves exact native source, destination, and generation facts"
    )]
    pub fn new(
        key: CloudItemKey,
        inode_generation: LinuxInodeGeneration,
        kind: LinuxNodeKind,
        old_parent_key: CloudItemKey,
        old_name: impl Into<String>,
        new_parent_key: CloudItemKey,
        new_name: impl Into<String>,
        destination: Option<LinuxRenameDestination>,
        no_replace: bool,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        let old_name = old_name.into();
        let new_name = new_name.into();
        validate_linux_name(&old_name)?;
        validate_linux_name(&new_name)?;
        Ok(Self {
            key,
            inode_generation,
            kind,
            old_parent_key,
            old_name,
            new_parent_key,
            new_name,
            destination,
            no_replace,
            session_generation,
        })
    }

    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    pub const fn inode_generation(&self) -> LinuxInodeGeneration {
        self.inode_generation
    }

    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }

    pub const fn old_parent_key(&self) -> &CloudItemKey {
        &self.old_parent_key
    }

    pub fn old_name(&self) -> &str {
        &self.old_name
    }

    pub const fn new_parent_key(&self) -> &CloudItemKey {
        &self.new_parent_key
    }

    pub fn new_name(&self) -> &str {
        &self.new_name
    }

    pub const fn destination(&self) -> Option<&LinuxRenameDestination> {
        self.destination.as_ref()
    }

    pub const fn no_replace(&self) -> bool {
        self.no_replace
    }

    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Existing destination resolved before a rename transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRenameDestination {
    key: CloudItemKey,
    inode_generation: LinuxInodeGeneration,
    kind: LinuxNodeKind,
}

impl LinuxRenameDestination {
    pub const fn new(
        key: CloudItemKey,
        inode_generation: LinuxInodeGeneration,
        kind: LinuxNodeKind,
    ) -> Self {
        Self {
            key,
            inode_generation,
            kind,
        }
    }

    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    pub const fn inode_generation(&self) -> LinuxInodeGeneration {
        self.inode_generation
    }

    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }
}

/// Durable rename result, including a destination replacement tombstone when applicable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRenameAcceptance {
    item: LinuxNamespaceItem,
    source: LinuxNamespaceTombstone,
    replaced: Option<LinuxNamespaceTombstone>,
}

impl LinuxRenameAcceptance {
    pub const fn new(
        item: LinuxNamespaceItem,
        source: LinuxNamespaceTombstone,
        replaced: Option<LinuxNamespaceTombstone>,
    ) -> Self {
        Self {
            item,
            source,
            replaced,
        }
    }

    pub const fn item(&self) -> &LinuxNamespaceItem {
        &self.item
    }

    pub const fn source(&self) -> &LinuxNamespaceTombstone {
        &self.source
    }

    pub const fn replaced(&self) -> Option<&LinuxNamespaceTombstone> {
        self.replaced.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        LinuxNamespaceItem,
        LinuxNamespaceTombstone,
        Option<LinuxNamespaceTombstone>,
    ) {
        (self.item, self.source, self.replaced)
    }
}

/// Native facts for one unlink or rmdir transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRemoveRequest {
    key: CloudItemKey,
    inode_generation: LinuxInodeGeneration,
    parent_key: CloudItemKey,
    name: String,
    kind: LinuxNodeKind,
    session_generation: SessionGeneration,
}

impl LinuxRemoveRequest {
    pub fn new(
        key: CloudItemKey,
        inode_generation: LinuxInodeGeneration,
        parent_key: CloudItemKey,
        name: impl Into<String>,
        kind: LinuxNodeKind,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        let name = name.into();
        validate_linux_name(&name)?;
        Ok(Self {
            key,
            inode_generation,
            parent_key,
            name,
            kind,
            session_generation,
        })
    }

    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    pub const fn inode_generation(&self) -> LinuxInodeGeneration {
        self.inode_generation
    }

    pub const fn parent_key(&self) -> &CloudItemKey {
        &self.parent_key
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }

    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
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

    /// Restores durable local directory creates, renames, and tombstones for a newer mount.
    async fn activate_namespace_overlay(
        &self,
        _scope: &CloudScope,
        _session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceOverlay> {
        Ok(LinuxNamespaceOverlay::default())
    }

    /// Atomically accepts a durable directory create.
    async fn create_directory(
        &self,
        _request: &LinuxCreateDirectoryRequest,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceItem> {
        Err(LinuxNamespaceMutationStoreError::new(
            LinuxNamespaceMutationStoreErrorKind::Unsupported,
            "directory create is not implemented by this namespace store",
        ))
    }

    /// Atomically accepts a stable-identity rename or move.
    async fn rename(
        &self,
        _request: &LinuxRenameRequest,
    ) -> LinuxNamespaceStoreResult<LinuxRenameAcceptance> {
        Err(LinuxNamespaceMutationStoreError::new(
            LinuxNamespaceMutationStoreErrorKind::Unsupported,
            "rename is not implemented by this namespace store",
        ))
    }

    /// Atomically accepts unlink or rmdir and returns its durable tombstone.
    async fn remove(
        &self,
        _request: &LinuxRemoveRequest,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceTombstone> {
        Err(LinuxNamespaceMutationStoreError::new(
            LinuxNamespaceMutationStoreErrorKind::Unsupported,
            "remove is not implemented by this namespace store",
        ))
    }
}
