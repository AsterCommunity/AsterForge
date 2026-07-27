//! Product-neutral contracts for native cloud-files integrations.
//!
//! This crate defines the operating-system-independent value model shared by Windows Cloud Files,
//! Apple File Provider, and Linux FUSE adapters. Product crates provide authentication, remote API
//! DTO mapping, permissions, root/account policy, and user-visible conflict behavior.
//!
//! The current Phase 1 surface includes scoped identities, opaque revisions and cursors,
//! capability negotiation, read-only backend ports, durable checkpoint/mutation store contracts,
//! runtime-neutral hydration work sharing with waiter-scoped cancellation, and revision-bound
//! content-storage ownership with guarded eviction, recoverable provider-cache writes, immutable
//! local generations, and resumable upload recovery. Native platform bindings follow after
//! executable models prove their failure and recovery boundaries.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

pub mod backend;
pub mod backend_error;
pub mod cache_write;
pub mod capabilities;
pub mod change;
pub mod content_storage;
pub mod cursor;
pub mod error;
pub mod hydration;
pub mod identity;
pub mod item;
pub mod local_content;
pub mod mutation;
pub mod revision;
pub mod store;
pub mod upload;

pub use backend::{
    ByteRange, CloudContentBackend, CloudFilesBackend, CloudMetadataBackend, ContentReadRange,
    ContentReadRequest, ContentReadResponse,
};
pub use backend_error::{BackendResult, CloudBackendError, CloudBackendErrorKind, RetryAdvice};
pub use cache_write::{
    ContentCacheWriteExtent, ContentCacheWriteIntent, ContentCacheWriteOperationId,
    ContentCacheWriteRecord, ContentCacheWriteRecordTransition, ContentCacheWriteState,
};
pub use capabilities::{
    Alignment, AnchoredChangeCapabilities, CancellationLevel, ChangeCapabilities,
    CloudFilesCapabilities, CloudFilesLimits, ContentStorageModes, EnumerationCapabilities,
    EvictionCapabilities, HydrationCapabilities, IdentityCapabilities, MaterializationCapabilities,
    MutationCapabilities, NamingCapabilities, RangeHydrationCapabilities, RevisionCapabilities,
};
pub use change::{ChangeBatch, ChangePage, ChangeResetReason, CloudChange};
pub use content_storage::{
    ContentCacheKey, ContentEvictionBegin, ContentEvictionBlocker, ContentEvictionDecision,
    ContentEvictionIntent, ContentEvictionOperationId, ContentEvictionPhysicalEffect,
    ContentEvictionPlan, ContentEvictionRecord, ContentEvictionRecordTransition,
    ContentEvictionState, ContentEvictionTarget, ContentLeaseCounts, ContentLeaseId,
    ContentLeaseKind, ContentRangeSet, ContentStorageEntry, ContentStorageMode,
    DirtyContentTransition, PlatformMaterializationState,
};
pub use cursor::{ChangeCursor, DirectoryCookie, PageCursor};
pub use error::{CloudFilesCoreError, Result};
pub use hydration::{
    HydrationCancellationHandle, HydrationCancellationOutcome, HydrationCoordinator,
    HydrationError, HydrationRequest, HydrationResult, HydrationWaiter,
};
pub use identity::{CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope};
pub use item::{CloudContentMetadata, CloudItem, CloudItemKind, CloudItemPage};
pub use local_content::{LocalContentGeneration, LocalContentReference, LocalContentSnapshot};
pub use mutation::{
    DesiredMutation, IdempotencyKey, MutationIntent, MutationOrigin, MutationPreconditions,
    MutationRecord, MutationRecordTransition, MutationRemoteOutcome, MutationRetryMetadata,
    MutationState, OperationId, PlatformRequestCorrelation, RemoteReconciliationKey,
    SessionGeneration, SessionState,
};
pub use revision::{ContentDigest, ContentDigestAlgorithm, ContentRevision, MetadataRevision};
pub use store::{
    ChangeBatchId, ChangeCheckpointStore, ChangeCursorCheckpoint, CloudFilesStoreError,
    CloudFilesStoreErrorKind, ContentCacheWriteStore, ContentStorageStore, ContentUploadStore,
    MutationJournalStore, PersistedChangeBatch, PersistedChangeBatchState, StoreResult,
    StoreWriteStatus,
};
pub use upload::{
    CloudContentUploadBackend, ContentUploadChunk, ContentUploadChunkAck, ContentUploadIntent,
    ContentUploadRecord, ContentUploadRecordTransition, ContentUploadSession,
    ContentUploadSessionId, ContentUploadState,
};
