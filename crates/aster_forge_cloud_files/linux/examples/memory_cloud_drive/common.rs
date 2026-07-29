use std::{
    collections::HashMap,
    error::Error,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use aster_forge_cloud_files_core::{
    Alignment, BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudContentUploadBackend, CloudFilesBackend,
    CloudFilesCapabilities, CloudFilesStoreError, CloudFilesStoreErrorKind, CloudItem, CloudItemId,
    CloudItemKey, CloudItemKind, CloudItemPage, CloudMetadataBackend, CloudMutationBackend,
    CloudNamespaceId, CloudRootId, CloudScope, ContentCacheKey, ContentLeaseId, ContentReadRange,
    ContentReadRequest, ContentReadResponse, ContentRevision, ContentUploadChunk,
    ContentUploadChunkAck, ContentUploadIntent, ContentUploadRecord, ContentUploadRecordTransition,
    ContentUploadRunOutcome, ContentUploadRunner, ContentUploadSession, ContentUploadSessionId,
    ContentUploadState, ContentUploadStore, DesiredMutation, EnumerationCapabilities,
    HydrationCapabilities, IdempotencyKey, IdentityCapabilities, LocalContentGeneration,
    LocalContentReference, LocalContentSnapshot, LocalContentSnapshotReader, MetadataRevision,
    MutationCapabilities, MutationIntent, MutationJournalStore, MutationOrigin,
    MutationPreconditions, MutationRecord, MutationRecordTransition, MutationRemoteOutcome,
    MutationRunOutcome, MutationRunner, MutationState, OperationId, PageCursor,
    RangeHydrationCapabilities, RecoveryPage, RevisionCapabilities, SessionGeneration,
    SessionState, StoreResult, StoreWriteStatus,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCreateDirectoryRequest, LinuxCreateFileAcceptance,
    LinuxCreateFileRequest, LinuxCreatedFile, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord,
    LinuxInodeTable, LinuxMountConfig, LinuxNamespaceItem, LinuxNamespaceMutationStore,
    LinuxNamespaceMutationStoreError, LinuxNamespaceMutationStoreErrorKind, LinuxNamespaceOverlay,
    LinuxNamespaceStoreResult, LinuxNamespaceTombstone, LinuxNodeKind, LinuxReadOnlyEngine,
    LinuxRemoveRequest, LinuxRenameAcceptance, LinuxRenameRequest, LinuxWritableEngine,
    LinuxWritableFilesystem, LinuxWriteCommit, LinuxWriteOpenRequest, LinuxWriteSession,
    LinuxWriteSessionId, LinuxWritebackStore, mount_writable,
};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, watch};

pub type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// The synthetic fixture keeps a small diagnostic tail without turning completed journals into
/// an unbounded in-memory and on-disk history.
const TERMINAL_RECORD_LIMIT: usize = 128;
const RECOVERY_BATCH_LIMIT: usize = 128;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn store_error(kind: CloudFilesStoreErrorKind, context: impl Into<String>) -> CloudFilesStoreError {
    CloudFilesStoreError::new(kind, context)
}

fn namespace_store_error(
    kind: LinuxNamespaceMutationStoreErrorKind,
    context: impl Into<String>,
) -> LinuxNamespaceMutationStoreError {
    LinuxNamespaceMutationStoreError::new(kind, context)
}

#[derive(Clone)]
struct StagedFile {
    bytes: Arc<[u8]>,
    snapshot: Option<LocalContentSnapshot>,
    base_revision: ContentRevision,
}

#[derive(Clone)]
struct ImmutableSnapshotBytes {
    reference: LocalContentReference,
    bytes: Arc<[u8]>,
}

#[derive(Clone)]
struct WriteSessionBinding {
    key: CloudItemKey,
    generation: SessionGeneration,
}

#[derive(Clone, Default)]
struct MemoryWritebackState {
    active_generation: Option<SessionGeneration>,
    active_session_state: Option<SessionState>,
    next_session: u64,
    next_generation: u64,
    next_item: u64,
    next_inode: u64,
    files: HashMap<CloudItemKey, StagedFile>,
    immutable_snapshots: HashMap<(CloudItemKey, LocalContentGeneration), ImmutableSnapshotBytes>,
    sessions: HashMap<LinuxWriteSessionId, WriteSessionBinding>,
    created: HashMap<CloudItemKey, LinuxCreatedFile>,
    created_names: HashMap<(CloudItemId, String), CloudItemKey>,
    namespace_items: HashMap<CloudItemKey, LinuxNamespaceItem>,
    namespace_names: HashMap<(CloudItemId, String), CloudItemKey>,
    tombstones: HashMap<(CloudItemId, String), LinuxNamespaceTombstone>,
    mutations: HashMap<OperationId, MutationRecord>,
    idempotency_keys: HashMap<IdempotencyKey, OperationId>,
    uploads: HashMap<OperationId, ContentUploadRecord>,
    upload_idempotency_keys: HashMap<IdempotencyKey, OperationId>,
}

struct MemoryWritebackStore {
    scope: CloudScope,
    state: Mutex<MemoryWritebackState>,
    state_file: Option<PathBuf>,
    mutation_notify: Notify,
    upload_notify: Notify,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct DurableWritebackDocument {
    active_generation: Option<u64>,
    active_session_state: Option<DurableSessionState>,
    next_generation: u64,
    next_item: u64,
    next_inode: u64,
    files: Vec<DurableStagedFile>,
    immutable_snapshots: Vec<DurableImmutableSnapshot>,
    created: Vec<DurableCreatedFile>,
    namespace_items: Vec<DurableNamespaceItem>,
    tombstones: Vec<DurableNamespaceTombstone>,
    mutations: Vec<DurableMutationRecord>,
    uploads: Vec<DurableUploadRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableStagedFile {
    namespace_id: String,
    root_id: String,
    item_id: String,
    bytes: Vec<u8>,
    generation: Option<u64>,
    local_reference: Option<String>,
    base_revision: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableImmutableSnapshot {
    namespace_id: String,
    root_id: String,
    item_id: String,
    generation: u64,
    local_reference: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableCreatedFile {
    namespace_id: String,
    root_id: String,
    item_id: String,
    parent_id: String,
    name: String,
    metadata_revision: Vec<u8>,
    content_revision: Vec<u8>,
    inode: u64,
    inode_generation: u64,
    operation_id: String,
    idempotency_key: String,
    accepted_generation: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DurableItemKind {
    File,
    Directory,
}

impl From<CloudItemKind> for DurableItemKind {
    fn from(value: CloudItemKind) -> Self {
        match value {
            CloudItemKind::File => Self::File,
            CloudItemKind::Directory => Self::Directory,
        }
    }
}

impl From<LinuxNodeKind> for DurableItemKind {
    fn from(value: LinuxNodeKind) -> Self {
        match value {
            LinuxNodeKind::File => Self::File,
            LinuxNodeKind::Directory => Self::Directory,
        }
    }
}

impl From<DurableItemKind> for CloudItemKind {
    fn from(value: DurableItemKind) -> Self {
        match value {
            DurableItemKind::File => Self::File,
            DurableItemKind::Directory => Self::Directory,
        }
    }
}

impl From<DurableItemKind> for LinuxNodeKind {
    fn from(value: DurableItemKind) -> Self {
        match value {
            DurableItemKind::File => Self::File,
            DurableItemKind::Directory => Self::Directory,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DurableDesiredMutation {
    Create {
        parent_id: String,
        name: String,
        item_kind: DurableItemKind,
    },
    ModifyMetadata {
        item_id: String,
        parent_id: Option<String>,
        name: Option<String>,
    },
    Delete {
        item_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableNamespaceIntent {
    operation_id: String,
    idempotency_key: String,
    accepted_generation: u64,
    desired: DurableDesiredMutation,
    metadata_precondition: Option<Vec<u8>>,
    content_precondition: Option<Vec<u8>>,
}

impl DurableNamespaceIntent {
    fn from_intent(intent: &MutationIntent) -> StoreResult<Self> {
        let desired = match intent.desired() {
            DesiredMutation::Create {
                parent_id,
                name,
                kind,
                ..
            } => DurableDesiredMutation::Create {
                parent_id: parent_id.as_str().to_owned(),
                name: name.clone(),
                item_kind: (*kind).into(),
            },
            DesiredMutation::ModifyMetadata {
                key,
                parent_id,
                name,
            } => DurableDesiredMutation::ModifyMetadata {
                item_id: key.item_id().as_str().to_owned(),
                parent_id: parent_id.as_ref().map(|value| value.as_str().to_owned()),
                name: name.clone(),
            },
            DesiredMutation::Delete { key } => DurableDesiredMutation::Delete {
                item_id: key.item_id().as_str().to_owned(),
            },
            DesiredMutation::ModifyContent { .. } => {
                return Err(store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic namespace state cannot persist a content mutation",
                ));
            }
        };
        Ok(Self {
            operation_id: intent.operation_id().as_str().to_owned(),
            idempotency_key: intent.idempotency_key().as_str().to_owned(),
            accepted_generation: intent.session_generation().get(),
            desired,
            metadata_precondition: intent
                .preconditions()
                .metadata_revision()
                .map(|value| value.as_bytes().to_vec()),
            content_precondition: intent
                .preconditions()
                .content_revision()
                .map(|value| value.as_bytes().to_vec()),
        })
    }

    fn into_intent(self, scope: &CloudScope) -> ExampleResult<MutationIntent> {
        let desired = match self.desired {
            DurableDesiredMutation::Create {
                parent_id,
                name,
                item_kind,
            } => DesiredMutation::create(
                scope.clone(),
                CloudItemId::new(parent_id)?,
                name,
                item_kind.into(),
            )?,
            DurableDesiredMutation::ModifyMetadata {
                item_id,
                parent_id,
                name,
            } => DesiredMutation::modify_metadata(
                CloudItemKey::new(scope.clone(), CloudItemId::new(item_id)?),
                parent_id.map(CloudItemId::new).transpose()?,
                name,
            )?,
            DurableDesiredMutation::Delete { item_id } => DesiredMutation::delete(
                CloudItemKey::new(scope.clone(), CloudItemId::new(item_id)?),
            ),
        };
        Ok(MutationIntent::new(
            OperationId::new(self.operation_id)?,
            IdempotencyKey::new(self.idempotency_key)?,
            MutationOrigin::PlatformCommand,
            SessionGeneration::new(self.accepted_generation)?,
            desired,
            MutationPreconditions::new(
                self.metadata_precondition
                    .map(|value| MetadataRevision::from_slice(&value))
                    .transpose()?,
                self.content_precondition
                    .map(|value| ContentRevision::from_slice(&value))
                    .transpose()?,
            ),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableNamespaceItem {
    namespace_id: String,
    root_id: String,
    item_id: String,
    parent_id: String,
    name: String,
    item_kind: DurableItemKind,
    metadata_revision: Vec<u8>,
    content_revision: Option<Vec<u8>>,
    size: u64,
    inode: u64,
    inode_generation: u64,
    intent: DurableNamespaceIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableNamespaceTombstone {
    namespace_id: String,
    root_id: String,
    item_id: String,
    inode_generation: u64,
    parent_id: String,
    name: String,
    item_kind: DurableItemKind,
    intent: DurableNamespaceIntent,
}

impl DurableNamespaceItem {
    fn from_item(item: &LinuxNamespaceItem) -> StoreResult<Self> {
        let cloud_item = item.item();
        let parent_id = cloud_item.parent_id().ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic namespace item omitted its parent",
            )
        })?;
        Ok(Self {
            namespace_id: cloud_item.key().scope().namespace_id().as_str().to_owned(),
            root_id: cloud_item.key().scope().root_id().as_str().to_owned(),
            item_id: cloud_item.key().item_id().as_str().to_owned(),
            parent_id: parent_id.as_str().to_owned(),
            name: cloud_item.name().to_owned(),
            item_kind: cloud_item.kind().into(),
            metadata_revision: cloud_item.metadata_revision().as_bytes().to_vec(),
            content_revision: cloud_item
                .content()
                .map(|content| content.revision().as_bytes().to_vec()),
            size: cloud_item.content().map_or(0, CloudContentMetadata::size),
            inode: item.inode_record().inode().get(),
            inode_generation: item.inode_record().generation().get(),
            intent: DurableNamespaceIntent::from_intent(item.mutation_intent())?,
        })
    }

    fn into_item(self, scope: &CloudScope) -> ExampleResult<LinuxNamespaceItem> {
        let item_scope = CloudScope::new(
            CloudNamespaceId::new(self.namespace_id)?,
            CloudRootId::new(self.root_id)?,
        );
        if &item_scope != scope {
            return Err("synthetic namespace item belonged to another scope".into());
        }
        let key = CloudItemKey::new(item_scope, CloudItemId::new(self.item_id)?);
        let parent = CloudItemId::new(self.parent_id)?;
        let item = match self.item_kind {
            DurableItemKind::File => CloudItem::file(
                key.clone(),
                parent,
                self.name,
                MetadataRevision::from_slice(&self.metadata_revision)?,
                CloudContentMetadata::new(
                    ContentRevision::from_slice(
                        self.content_revision
                            .as_deref()
                            .ok_or("synthetic durable file omitted content revision")?,
                    )?,
                    None,
                    self.size,
                ),
            )?,
            DurableItemKind::Directory => CloudItem::directory(
                key.clone(),
                Some(parent),
                self.name,
                MetadataRevision::from_slice(&self.metadata_revision)?,
            )?,
        };
        let record = LinuxInodeRecord::new(
            key,
            LinuxInode::new(self.inode)?,
            LinuxInodeGeneration::new(self.inode_generation)?,
        );
        Ok(LinuxNamespaceItem::new(
            item,
            record,
            self.intent.into_intent(scope)?,
        ))
    }
}

impl DurableNamespaceTombstone {
    fn from_tombstone(tombstone: &LinuxNamespaceTombstone) -> StoreResult<Self> {
        Ok(Self {
            namespace_id: tombstone.key().scope().namespace_id().as_str().to_owned(),
            root_id: tombstone.key().scope().root_id().as_str().to_owned(),
            item_id: tombstone.key().item_id().as_str().to_owned(),
            inode_generation: tombstone.inode_generation().get(),
            parent_id: tombstone.parent_key().item_id().as_str().to_owned(),
            name: tombstone.name().to_owned(),
            item_kind: tombstone.kind().into(),
            intent: DurableNamespaceIntent::from_intent(tombstone.mutation_intent())?,
        })
    }

    fn into_tombstone(self, scope: &CloudScope) -> ExampleResult<LinuxNamespaceTombstone> {
        let item_scope = CloudScope::new(
            CloudNamespaceId::new(self.namespace_id)?,
            CloudRootId::new(self.root_id)?,
        );
        if &item_scope != scope {
            return Err("synthetic namespace tombstone belonged to another scope".into());
        }
        Ok(LinuxNamespaceTombstone::new(
            CloudItemKey::new(item_scope.clone(), CloudItemId::new(self.item_id)?),
            LinuxInodeGeneration::new(self.inode_generation)?,
            CloudItemKey::new(item_scope, CloudItemId::new(self.parent_id)?),
            self.name,
            self.item_kind.into(),
            self.intent.into_intent(scope)?,
        )?)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DurableSessionState {
    Accepting,
    Closing,
    Draining,
    Closed,
}

impl From<SessionState> for DurableSessionState {
    fn from(value: SessionState) -> Self {
        match value {
            SessionState::Accepting => Self::Accepting,
            SessionState::Closing => Self::Closing,
            SessionState::Draining => Self::Draining,
            SessionState::Closed => Self::Closed,
        }
    }
}

impl From<DurableSessionState> for SessionState {
    fn from(value: DurableSessionState) -> Self {
        match value {
            DurableSessionState::Accepting => Self::Accepting,
            DurableSessionState::Closing => Self::Closing,
            DurableSessionState::Draining => Self::Draining,
            DurableSessionState::Closed => Self::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DurableMutationState {
    IntentPersisted,
    RemoteApplying,
    RemoteOutcomeUnknown,
    RemoteOutcomeKnown,
    PlatformReconciled,
    Completed,
}

impl From<MutationState> for DurableMutationState {
    fn from(value: MutationState) -> Self {
        match value {
            MutationState::IntentPersisted => Self::IntentPersisted,
            MutationState::RemoteApplying => Self::RemoteApplying,
            MutationState::RemoteOutcomeUnknown => Self::RemoteOutcomeUnknown,
            MutationState::RemoteOutcomeKnown => Self::RemoteOutcomeKnown,
            MutationState::PlatformReconciled => Self::PlatformReconciled,
            MutationState::Completed => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableMutationFile {
    namespace_id: String,
    root_id: String,
    item_id: String,
    parent_id: Option<String>,
    name: String,
    metadata_revision: Vec<u8>,
    #[serde(default)]
    item_kind: Option<DurableItemKind>,
    #[serde(default)]
    content_revision: Option<Vec<u8>>,
    size: u64,
}

impl DurableMutationFile {
    fn from_item(item: &CloudItem) -> StoreResult<Self> {
        Ok(Self {
            namespace_id: item.key().scope().namespace_id().as_str().to_owned(),
            root_id: item.key().scope().root_id().as_str().to_owned(),
            item_id: item.key().item_id().as_str().to_owned(),
            parent_id: item.parent_id().map(|parent| parent.as_str().to_owned()),
            name: item.name().to_owned(),
            metadata_revision: item.metadata_revision().as_bytes().to_vec(),
            item_kind: Some(item.kind().into()),
            content_revision: item
                .content()
                .map(|content| content.revision().as_bytes().to_vec()),
            size: item.content().map_or(0, CloudContentMetadata::size),
        })
    }

    fn into_item(self) -> aster_forge_cloud_files_core::Result<CloudItem> {
        let key = CloudItemKey::new(
            CloudScope::new(
                CloudNamespaceId::new(self.namespace_id)?,
                CloudRootId::new(self.root_id)?,
            ),
            CloudItemId::new(self.item_id)?,
        );
        let parent = self.parent_id.map(CloudItemId::new).transpose()?;
        let metadata = MetadataRevision::from_slice(&self.metadata_revision)?;
        match self.item_kind.unwrap_or(DurableItemKind::File) {
            DurableItemKind::File => CloudItem::file(
                key,
                parent.ok_or(
                    aster_forge_cloud_files_core::CloudFilesCoreError::InvalidContentResponse {
                        reason: "durable remote file omitted its parent",
                    },
                )?,
                self.name,
                metadata,
                CloudContentMetadata::new(
                    ContentRevision::from_slice(self.content_revision.as_deref().ok_or(
                        aster_forge_cloud_files_core::CloudFilesCoreError::InvalidContentResponse {
                            reason: "durable remote file omitted content revision",
                        },
                    )?)?,
                    None,
                    self.size,
                ),
            ),
            DurableItemKind::Directory => CloudItem::directory(key, parent, self.name, metadata),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DurableMutationOutcome {
    Committed {
        item: Option<DurableMutationFile>,
    },
    AlreadyCommitted {
        item: Option<DurableMutationFile>,
    },
    PreconditionFailed {
        metadata_revision: Option<Vec<u8>>,
        content_revision: Option<Vec<u8>>,
    },
    RemoteOutcomeUnknown,
}

impl DurableMutationOutcome {
    fn from_outcome(outcome: &MutationRemoteOutcome) -> StoreResult<Self> {
        Ok(match outcome {
            MutationRemoteOutcome::Committed { item } => Self::Committed {
                item: item
                    .as_ref()
                    .map(DurableMutationFile::from_item)
                    .transpose()?,
            },
            MutationRemoteOutcome::AlreadyCommitted { item } => Self::AlreadyCommitted {
                item: item
                    .as_ref()
                    .map(DurableMutationFile::from_item)
                    .transpose()?,
            },
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision,
                content_revision,
            } => Self::PreconditionFailed {
                metadata_revision: metadata_revision
                    .as_ref()
                    .map(|revision| revision.as_bytes().to_vec()),
                content_revision: content_revision
                    .as_ref()
                    .map(|revision| revision.as_bytes().to_vec()),
            },
            MutationRemoteOutcome::RemoteOutcomeUnknown => Self::RemoteOutcomeUnknown,
        })
    }

    fn into_outcome(self) -> aster_forge_cloud_files_core::Result<MutationRemoteOutcome> {
        Ok(match self {
            Self::Committed { item } => MutationRemoteOutcome::Committed {
                item: item.map(DurableMutationFile::into_item).transpose()?,
            },
            Self::AlreadyCommitted { item } => MutationRemoteOutcome::AlreadyCommitted {
                item: item.map(DurableMutationFile::into_item).transpose()?,
            },
            Self::PreconditionFailed {
                metadata_revision,
                content_revision,
            } => MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: metadata_revision
                    .map(|revision| MetadataRevision::from_slice(&revision))
                    .transpose()?,
                content_revision: content_revision
                    .map(|revision| ContentRevision::from_slice(&revision))
                    .transpose()?,
            },
            Self::RemoteOutcomeUnknown => MutationRemoteOutcome::RemoteOutcomeUnknown,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableMutationRecord {
    operation_id: String,
    #[serde(default)]
    intent: Option<DurableNamespaceIntent>,
    state: DurableMutationState,
    remote_outcome: Option<DurableMutationOutcome>,
}

impl DurableMutationRecord {
    fn from_record(record: &MutationRecord) -> StoreResult<Self> {
        Ok(Self {
            operation_id: record.intent().operation_id().as_str().to_owned(),
            intent: Some(DurableNamespaceIntent::from_intent(record.intent())?),
            state: record.state().into(),
            remote_outcome: record
                .remote_outcome()
                .map(DurableMutationOutcome::from_outcome)
                .transpose()?,
        })
    }

    fn into_record(
        self,
        scope: &CloudScope,
        legacy_intent: Option<MutationIntent>,
    ) -> ExampleResult<MutationRecord> {
        let intent = match self.intent {
            Some(intent) => intent.into_intent(scope)?,
            None => legacy_intent.ok_or_else(|| {
                format!(
                    "synthetic mutation {} had no durable namespace intent",
                    self.operation_id
                )
            })?,
        };
        if self.operation_id != intent.operation_id().as_str() {
            return Err("synthetic mutation record operation identity drifted".into());
        }
        let mut record = MutationRecord::persist(intent)?;
        if matches!(self.state, DurableMutationState::IntentPersisted) {
            if self.remote_outcome.is_some() {
                return Err("intent-persisted mutation unexpectedly stored an outcome".into());
            }
            return Ok(record);
        }
        record.begin_remote_apply()?;
        if matches!(self.state, DurableMutationState::RemoteApplying) {
            if self.remote_outcome.is_some() {
                return Err("remote-applying mutation unexpectedly stored an outcome".into());
            }
            return Ok(record);
        }
        let outcome = self
            .remote_outcome
            .ok_or("advanced synthetic mutation omitted its remote outcome")?
            .into_outcome()?;
        record.record_remote_outcome(outcome)?;
        match self.state {
            DurableMutationState::RemoteOutcomeUnknown
            | DurableMutationState::RemoteOutcomeKnown => {}
            DurableMutationState::PlatformReconciled => {
                record.mark_platform_reconciled()?;
            }
            DurableMutationState::Completed => {
                record.mark_platform_reconciled()?;
                record.complete()?;
            }
            DurableMutationState::IntentPersisted | DurableMutationState::RemoteApplying => {
                return Err("synthetic mutation replay state was inconsistent".into());
            }
        }
        if record.state()
            != match self.state {
                DurableMutationState::IntentPersisted => MutationState::IntentPersisted,
                DurableMutationState::RemoteApplying => MutationState::RemoteApplying,
                DurableMutationState::RemoteOutcomeUnknown => MutationState::RemoteOutcomeUnknown,
                DurableMutationState::RemoteOutcomeKnown => MutationState::RemoteOutcomeKnown,
                DurableMutationState::PlatformReconciled => MutationState::PlatformReconciled,
                DurableMutationState::Completed => MutationState::Completed,
            }
        {
            return Err("synthetic mutation outcome did not match its durable state".into());
        }
        Ok(record)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DurableUploadState {
    IntentPersisted,
    Uploading,
    RemoteCommitting,
    RemoteOutcomeUnknown,
    RemoteOutcomeKnown,
    MetadataReconciled,
    Completed,
}

impl From<ContentUploadState> for DurableUploadState {
    fn from(value: ContentUploadState) -> Self {
        match value {
            ContentUploadState::IntentPersisted => Self::IntentPersisted,
            ContentUploadState::Uploading => Self::Uploading,
            ContentUploadState::RemoteCommitting => Self::RemoteCommitting,
            ContentUploadState::RemoteOutcomeUnknown => Self::RemoteOutcomeUnknown,
            ContentUploadState::RemoteOutcomeKnown => Self::RemoteOutcomeKnown,
            ContentUploadState::MetadataReconciled => Self::MetadataReconciled,
            ContentUploadState::Completed => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableUploadSession {
    id: String,
    accepted_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableUploadRecord {
    operation_id: String,
    idempotency_key: String,
    namespace_id: String,
    root_id: String,
    item_id: String,
    base_revision: Vec<u8>,
    local_generation: u64,
    local_reference: String,
    size: u64,
    upload_lease_id: String,
    accepted_generation: u64,
    state: DurableUploadState,
    session: Option<DurableUploadSession>,
    remote_outcome: Option<DurableMutationOutcome>,
}

impl DurableUploadRecord {
    fn from_record(record: &ContentUploadRecord) -> StoreResult<Self> {
        let intent = record.intent();
        Ok(Self {
            operation_id: intent.operation_id().as_str().to_owned(),
            idempotency_key: intent.idempotency_key().as_str().to_owned(),
            namespace_id: intent
                .base_cache_key()
                .item_key()
                .scope()
                .namespace_id()
                .as_str()
                .to_owned(),
            root_id: intent
                .base_cache_key()
                .item_key()
                .scope()
                .root_id()
                .as_str()
                .to_owned(),
            item_id: intent
                .base_cache_key()
                .item_key()
                .item_id()
                .as_str()
                .to_owned(),
            base_revision: intent.base_cache_key().revision().as_bytes().to_vec(),
            local_generation: intent.snapshot().generation().get(),
            local_reference: intent.snapshot().reference().as_str().to_owned(),
            size: intent.snapshot().size(),
            upload_lease_id: intent.upload_lease_id().as_str().to_owned(),
            accepted_generation: intent.session_generation().get(),
            state: record.state().into(),
            session: record.session().map(|session| DurableUploadSession {
                id: session.id().as_str().to_owned(),
                accepted_offset: session.accepted_offset(),
            }),
            remote_outcome: record
                .remote_outcome()
                .map(DurableMutationOutcome::from_outcome)
                .transpose()?,
        })
    }

    fn into_record(self, scope: &CloudScope) -> ExampleResult<ContentUploadRecord> {
        let item_scope = CloudScope::new(
            CloudNamespaceId::new(self.namespace_id)?,
            CloudRootId::new(self.root_id)?,
        );
        if &item_scope != scope {
            return Err("synthetic upload state belonged to another cloud scope".into());
        }
        let item_key = CloudItemKey::new(item_scope, CloudItemId::new(self.item_id)?);
        let snapshot = LocalContentSnapshot::new(
            item_key.clone(),
            LocalContentGeneration::new(self.local_generation)?,
            LocalContentReference::new(self.local_reference)?,
            self.size,
            None,
        );
        let intent = ContentUploadIntent::new(
            OperationId::new(self.operation_id)?,
            IdempotencyKey::new(self.idempotency_key)?,
            ContentCacheKey::new(item_key, ContentRevision::from_slice(&self.base_revision)?),
            snapshot,
            ContentLeaseId::new(self.upload_lease_id)?,
            SessionGeneration::new(self.accepted_generation)?,
        )?;
        let mut record = ContentUploadRecord::persist(intent);
        if matches!(self.state, DurableUploadState::IntentPersisted) {
            if self.session.is_some() || self.remote_outcome.is_some() {
                return Err("intent-persisted upload stored advanced fields".into());
            }
            return Ok(record);
        }
        let session = self
            .session
            .ok_or("advanced synthetic upload omitted its backend session")?;
        record.record_session(ContentUploadSession::new(
            ContentUploadSessionId::new(session.id)?,
            session.accepted_offset,
        ))?;
        if matches!(self.state, DurableUploadState::Uploading) {
            if self.remote_outcome.is_some() {
                return Err("uploading record unexpectedly stored a remote outcome".into());
            }
            return Ok(record);
        }
        record.begin_remote_commit()?;
        if matches!(self.state, DurableUploadState::RemoteCommitting) {
            if self.remote_outcome.is_some() {
                return Err("remote-committing upload unexpectedly stored an outcome".into());
            }
            return Ok(record);
        }
        let outcome = self
            .remote_outcome
            .ok_or("advanced synthetic upload omitted its remote outcome")?
            .into_outcome()?;
        record.record_remote_outcome(outcome)?;
        match self.state {
            DurableUploadState::RemoteOutcomeUnknown | DurableUploadState::RemoteOutcomeKnown => {}
            DurableUploadState::MetadataReconciled => {
                record.mark_metadata_reconciled()?;
            }
            DurableUploadState::Completed => {
                record.mark_metadata_reconciled()?;
                record.complete()?;
            }
            DurableUploadState::IntentPersisted
            | DurableUploadState::Uploading
            | DurableUploadState::RemoteCommitting => {
                return Err("synthetic upload replay state was inconsistent".into());
            }
        }
        Ok(record)
    }
}
