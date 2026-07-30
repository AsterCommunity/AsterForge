//! Incremental backend changes, deletion tombstones, and explicit cursor resets.

use crate::{ChangeCursor, CloudItem, CloudItemId, CloudItemKey, MetadataRevision};

/// One product-neutral remote change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudChange {
    /// Creates an item or replaces its known metadata/content state.
    Upsert {
        /// Complete current item state.
        item: CloudItem,
    },
    /// Removes an item using a durable deletion tombstone.
    Delete {
        /// Fully scoped stable identity of the deleted item.
        key: CloudItemKey,
        /// Last known parent identity, when supplied by the backend or local baseline.
        previous_parent_id: Option<CloudItemId>,
        /// Deletion/metadata revision, when the backend exposes one.
        metadata_revision: Option<MetadataRevision>,
    },
}

impl CloudChange {
    /// Returns the fully scoped item identity affected by this change.
    pub const fn key(&self) -> &CloudItemKey {
        match self {
            Self::Upsert { item } => item.key(),
            Self::Delete { key, .. } => key,
        }
    }
}

/// One durable batch from an anchored backend change stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeBatch {
    changes: Vec<CloudChange>,
    next_cursor: ChangeCursor,
    has_more: bool,
}

impl ChangeBatch {
    /// Creates a change batch. The cursor becomes active only after effects are durably replayable.
    pub const fn new(changes: Vec<CloudChange>, next_cursor: ChangeCursor, has_more: bool) -> Self {
        Self {
            changes,
            next_cursor,
            has_more,
        }
    }

    /// Returns changes in backend-defined application order.
    pub fn changes(&self) -> &[CloudChange] {
        &self.changes
    }

    /// Returns the checkpoint following this batch.
    pub const fn next_cursor(&self) -> &ChangeCursor {
        &self.next_cursor
    }

    /// Returns whether another batch is immediately available.
    pub const fn has_more(&self) -> bool {
        self.has_more
    }

    /// Consumes the batch and returns its changes, next cursor, and continuation flag.
    pub fn into_parts(self) -> (Vec<CloudChange>, ChangeCursor, bool) {
        (self.changes, self.next_cursor, self.has_more)
    }
}

/// Reason an existing change checkpoint can no longer continue incrementally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeResetReason {
    /// The backend no longer retains changes after the supplied cursor.
    CursorExpired,
    /// The backend rebuilt or replaced its change-tracking state.
    BackendStateRebuilt,
    /// Stable item identifiers or their mapping can no longer be trusted.
    IdentityMappingInvalidated,
}

/// Result of requesting the next anchored change page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePage {
    /// A replayable batch and its following checkpoint.
    Batch(ChangeBatch),
    /// Incremental continuation stopped and requires reconciliation or reimport.
    ResetRequired {
        /// Product-neutral reset classification.
        reason: ChangeResetReason,
    },
}
