//! Product-neutral content storage ownership, sparse coverage, leases, and eviction recovery.

use std::{collections::HashMap, fmt};

use crate::{
    ByteRange, CloudFilesCoreError, CloudItemKey, ContentRevision, LocalContentGeneration,
    LocalContentSnapshot, Result, SessionGeneration,
};

/// Active physical ownership model selected from effective materialization capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentStorageMode {
    /// The operating system owns the materialized local copy.
    PlatformManaged,
    /// The provider owns the backing content cache.
    ProviderManaged,
    /// Platform and provider own distinct physical content layers.
    Hybrid,
}

/// Revision-bound identity of one logical cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentCacheKey {
    item_key: CloudItemKey,
    revision: ContentRevision,
}

impl ContentCacheKey {
    /// Creates a path-independent cache key for one exact content revision.
    pub const fn new(item_key: CloudItemKey, revision: ContentRevision) -> Self {
        Self { item_key, revision }
    }

    /// Returns the fully scoped stable item identity.
    pub const fn item_key(&self) -> &CloudItemKey {
        &self.item_key
    }

    /// Returns the exact content revision represented by this entry.
    pub const fn revision(&self) -> &ContentRevision {
        &self.revision
    }
}

/// Normalized non-overlapping provider-cache byte coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentRangeSet {
    ranges: Vec<ByteRange>,
}

impl ContentRangeSet {
    /// Maximum number of disjoint ranges retained for one sparse entry.
    pub const MAX_RANGES: usize = 8_192;

    /// Creates an empty sparse coverage set.
    pub const fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    /// Returns normalized ranges in ascending order.
    pub fn ranges(&self) -> &[ByteRange] {
        &self.ranges
    }

    /// Inserts one range and merges all overlapping or adjacent ranges.
    pub fn insert(&mut self, range: ByteRange, total_size: u64) -> Result<bool> {
        if range.end_exclusive() > total_size {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "cached range exceeds the content size",
            ));
        }
        if self.covers(range) {
            return Ok(false);
        }

        let mut start = range.offset();
        let mut end = range.end_exclusive();
        let mut merged = Vec::with_capacity(self.ranges.len().saturating_add(1));
        let mut inserted = false;
        for existing in &self.ranges {
            if existing.end_exclusive() < start {
                merged.push(*existing);
            } else if end < existing.offset() {
                if !inserted {
                    merged.push(ByteRange::new(start, end.saturating_sub(start))?);
                }
                inserted = true;
                merged.push(*existing);
            } else {
                start = start.min(existing.offset());
                end = end.max(existing.end_exclusive());
            }
        }
        if !inserted {
            merged.push(ByteRange::new(start, end.saturating_sub(start))?);
        }
        if merged.len() > Self::MAX_RANGES {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "provider cache range fragmentation exceeds the supported limit",
            ));
        }
        self.ranges = merged;
        Ok(true)
    }

    /// Returns whether every byte in one range is cached.
    pub fn covers(&self, range: ByteRange) -> bool {
        let index = self
            .ranges
            .partition_point(|existing| existing.end_exclusive() <= range.offset());
        self.ranges.get(index).is_some_and(|existing| {
            existing.offset() <= range.offset() && existing.end_exclusive() >= range.end_exclusive()
        })
    }

    /// Returns whether the complete logical file is cached.
    pub fn is_complete(&self, total_size: u64) -> bool {
        if total_size == 0 {
            return true;
        }
        matches!(
            self.ranges.as_slice(),
            [range] if range.offset() == 0 && range.end_exclusive() == total_size
        )
    }

    /// Clears all provider-owned cached ranges.
    pub fn clear(&mut self) {
        self.ranges.clear();
    }
}

/// Platform-observed materialization state. It does not imply provider-cache ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformMaterializationState {
    /// The adapter has not established current platform state.
    Unknown,
    /// No platform-managed content bytes are materialized.
    Dataless,
    /// The platform reports partial materialization.
    Partial,
    /// The platform reports a complete materialized copy.
    Complete,
}

/// Runtime activity that must protect content from eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentLeaseKind {
    /// A native or product handle keeps the item open.
    Open,
    /// A reader is consuming cached or hydrated bytes.
    Read,
    /// A writer may modify local content.
    Write,
    /// A hydration operation is filling content ranges.
    Hydration,
    /// An upload operation depends on local content.
    Upload,
    /// Integrity verification is reading or finalizing content.
    Verification,
}

/// Stable identity of one runtime content lease.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentLeaseId(String);

impl ContentLeaseId {
    /// Creates a non-empty opaque lease identity.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("content lease id"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque lease identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContentLeaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentLeaseId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Deterministic runtime lease counts exposed by a content entry snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentLeaseCounts {
    /// Open-handle leases.
    pub open: u32,
    /// Active read leases.
    pub read: u32,
    /// Active write leases.
    pub write: u32,
    /// Active hydration leases.
    pub hydration: u32,
    /// Active upload leases.
    pub upload: u32,
    /// Active verification leases.
    pub verification: u32,
}

impl ContentLeaseCounts {
    fn increment(&mut self, kind: ContentLeaseKind) -> Result<()> {
        let value = self.value_mut(kind);
        *value = value.checked_add(1).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_storage("content lease count overflowed")
        })?;
        Ok(())
    }

    fn decrement(&mut self, kind: ContentLeaseKind) -> Result<()> {
        let value = self.value_mut(kind);
        *value = value.checked_sub(1).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_storage("content lease count was already zero")
        })?;
        Ok(())
    }

    fn value_mut(&mut self, kind: ContentLeaseKind) -> &mut u32 {
        match kind {
            ContentLeaseKind::Open => &mut self.open,
            ContentLeaseKind::Read => &mut self.read,
            ContentLeaseKind::Write => &mut self.write,
            ContentLeaseKind::Hydration => &mut self.hydration,
            ContentLeaseKind::Upload => &mut self.upload,
            ContentLeaseKind::Verification => &mut self.verification,
        }
    }
}

/// Stable reason an eviction attempt is currently blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentEvictionBlocker {
    /// Product or native policy pins the content.
    Pinned,
    /// Local content contains uncommitted changes.
    Dirty,
    /// An open handle lease exists.
    OpenLease,
    /// An active reader lease exists.
    ReadLease,
    /// An active writer lease exists.
    WriteLease,
    /// Hydration is filling one or more ranges.
    HydrationLease,
    /// Upload depends on the local content.
    UploadLease,
    /// Integrity verification is active.
    VerificationLease,
    /// A durable provider-cache write has not reached completion.
    CacheWriteInProgress,
    /// Another eviction intent already reserves the entry.
    EvictionInProgress,
}

/// Physical layer selected for an eviction operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentEvictionTarget {
    /// Remove only provider-owned backing content.
    ProviderCache,
    /// Ask the platform to evict or dehydrate its materialized copy.
    PlatformMaterialization,
    /// Evict every physical layer owned by the selected storage mode.
    AllManagedContent,
}

/// Required idempotent effects for one eviction target and ownership mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentEvictionPlan {
    remove_provider_cache: bool,
    evict_platform_materialization: bool,
    invalidate_platform_content: bool,
}

impl ContentEvictionPlan {
    /// Builds the required physical effects for a storage mode and target.
    pub fn for_target(mode: ContentStorageMode, target: ContentEvictionTarget) -> Result<Self> {
        let plan = match (mode, target) {
            (ContentStorageMode::PlatformManaged, ContentEvictionTarget::ProviderCache)
            | (
                ContentStorageMode::ProviderManaged,
                ContentEvictionTarget::PlatformMaterialization,
            ) => {
                return Err(CloudFilesCoreError::invalid_content_storage(
                    "eviction target is not owned by the selected storage mode",
                ));
            }
            (ContentStorageMode::PlatformManaged, _) => Self {
                remove_provider_cache: false,
                evict_platform_materialization: true,
                invalidate_platform_content: false,
            },
            (ContentStorageMode::ProviderManaged, _) => Self {
                remove_provider_cache: true,
                evict_platform_materialization: false,
                invalidate_platform_content: true,
            },
            (ContentStorageMode::Hybrid, ContentEvictionTarget::ProviderCache) => Self {
                remove_provider_cache: true,
                evict_platform_materialization: false,
                invalidate_platform_content: true,
            },
            (ContentStorageMode::Hybrid, ContentEvictionTarget::PlatformMaterialization) => Self {
                remove_provider_cache: false,
                evict_platform_materialization: true,
                invalidate_platform_content: false,
            },
            (ContentStorageMode::Hybrid, ContentEvictionTarget::AllManagedContent) => Self {
                remove_provider_cache: true,
                evict_platform_materialization: true,
                invalidate_platform_content: true,
            },
        };
        Ok(plan)
    }

    /// Returns whether provider-owned bytes must be removed.
    pub const fn remove_provider_cache(self) -> bool {
        self.remove_provider_cache
    }

    /// Returns whether the platform materialized copy must be evicted.
    pub const fn evict_platform_materialization(self) -> bool {
        self.evict_platform_materialization
    }

    /// Returns whether native/kernel content caches must be invalidated after provider removal.
    pub const fn invalidate_platform_content(self) -> bool {
        self.invalidate_platform_content
    }
}

/// Eligibility result produced before persisting an eviction intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentEvictionDecision {
    /// The entry is reserved and the supplied physical plan may begin.
    Reserved {
        /// Idempotent effects required for this target.
        plan: ContentEvictionPlan,
    },
    /// One or more guards currently prohibit eviction.
    Blocked {
        /// Deterministically ordered blocker classifications.
        blockers: Vec<ContentEvictionBlocker>,
    },
}

/// Result of recording or clearing one item-local dirty snapshot generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirtyContentTransition {
    /// Dirty state changed.
    Applied,
    /// The same state was already present.
    AlreadyApplied,
    /// A newer local generation already superseded the requested transition.
    Fenced,
}

/// Result of durably beginning an eviction through a content-storage store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentEvictionBegin {
    /// A new intent and entry reservation became durable.
    Persisted {
        /// Idempotent physical effects required by the selected target.
        plan: ContentEvictionPlan,
    },
    /// The same durable intent already exists.
    AlreadyPersisted {
        /// Existing physical effect plan.
        plan: ContentEvictionPlan,
    },
    /// Current guards prevented reservation and no intent was persisted.
    Blocked {
        /// Deterministically ordered blockers.
        blockers: Vec<ContentEvictionBlocker>,
    },
}

/// Product-neutral content storage snapshot for one item revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentStorageEntry {
    key: ContentCacheKey,
    mode: ContentStorageMode,
    total_size: u64,
    provider_ranges: Option<ContentRangeSet>,
    platform_materialization: Option<PlatformMaterializationState>,
    pinned: bool,
    dirty_snapshot: Option<LocalContentSnapshot>,
    leases: HashMap<ContentLeaseId, ContentLeaseKind>,
    lease_counts: ContentLeaseCounts,
    active_cache_writes: u32,
    eviction_reserved: bool,
}

impl ContentStorageEntry {
    /// Creates an empty content entry consistent with the selected ownership mode.
    pub fn new(key: ContentCacheKey, mode: ContentStorageMode, total_size: u64) -> Self {
        let provider_ranges = matches!(
            mode,
            ContentStorageMode::ProviderManaged | ContentStorageMode::Hybrid
        )
        .then(ContentRangeSet::new);
        let platform_materialization = matches!(
            mode,
            ContentStorageMode::PlatformManaged | ContentStorageMode::Hybrid
        )
        .then_some(PlatformMaterializationState::Unknown);
        Self {
            key,
            mode,
            total_size,
            provider_ranges,
            platform_materialization,
            pinned: false,
            dirty_snapshot: None,
            leases: HashMap::new(),
            lease_counts: ContentLeaseCounts::default(),
            active_cache_writes: 0,
            eviction_reserved: false,
        }
    }

    /// Returns the revision-bound cache identity.
    pub const fn key(&self) -> &ContentCacheKey {
        &self.key
    }

    /// Returns the selected ownership mode.
    pub const fn mode(&self) -> ContentStorageMode {
        self.mode
    }

    /// Returns the logical size for this exact revision.
    pub const fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns provider-owned sparse range coverage, when the mode owns such a cache.
    pub const fn provider_ranges(&self) -> Option<&ContentRangeSet> {
        self.provider_ranges.as_ref()
    }

    /// Returns platform-observed materialization without claiming provider ownership.
    pub const fn platform_materialization(&self) -> Option<PlatformMaterializationState> {
        self.platform_materialization
    }

    /// Returns whether policy pins this content.
    pub const fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Returns whether local content has uncommitted changes.
    pub const fn is_dirty(&self) -> bool {
        self.dirty_snapshot.is_some()
    }

    /// Returns the immutable local snapshot currently protecting this entry from eviction.
    pub const fn dirty_snapshot(&self) -> Option<&LocalContentSnapshot> {
        self.dirty_snapshot.as_ref()
    }

    /// Returns runtime lease counts.
    pub const fn lease_counts(&self) -> ContentLeaseCounts {
        self.lease_counts
    }

    /// Returns the number of durable provider-cache writes protecting this entry.
    pub const fn active_cache_writes(&self) -> u32 {
        self.active_cache_writes
    }

    /// Returns whether an eviction intent currently reserves this entry.
    pub const fn is_eviction_reserved(&self) -> bool {
        self.eviction_reserved
    }

    /// Records provider-owned cached bytes. Platform-managed mode rejects this operation.
    pub fn record_provider_range(&mut self, range: ByteRange) -> Result<bool> {
        let Some(ranges) = self.provider_ranges.as_mut() else {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "platform-managed content has no provider cache ranges",
            ));
        };
        ranges.insert(range, self.total_size)
    }

    /// Updates platform-observed materialization. Provider-managed mode has no such owned state.
    pub fn observe_platform_materialization(
        &mut self,
        state: PlatformMaterializationState,
    ) -> Result<bool> {
        let Some(current) = self.platform_materialization.as_mut() else {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "provider-managed content has no platform materialization state",
            ));
        };
        if *current == state {
            return Ok(false);
        }
        *current = state;
        Ok(true)
    }

    /// Updates the product/native pin guard and reports a late blocker transition.
    pub fn set_pinned(&mut self, pinned: bool) -> Result<bool> {
        if pinned && self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "content cannot become pinned during eviction",
            ));
        }
        let changed = self.pinned != pinned;
        self.pinned = pinned;
        Ok(changed)
    }

    /// Records an immutable dirty snapshot and fences older local generations.
    pub fn mark_dirty(&mut self, snapshot: LocalContentSnapshot) -> Result<DirtyContentTransition> {
        if self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "dirty content cannot start during eviction",
            ));
        }
        if snapshot.item_key() != self.key.item_key() {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "dirty snapshot item does not match the content entry",
            ));
        }
        let Some(current) = self.dirty_snapshot.as_ref() else {
            self.dirty_snapshot = Some(snapshot);
            return Ok(DirtyContentTransition::Applied);
        };
        if current == &snapshot {
            return Ok(DirtyContentTransition::AlreadyApplied);
        }
        if snapshot.generation() < current.generation() {
            return Ok(DirtyContentTransition::Fenced);
        }
        if snapshot.generation() == current.generation() {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "one local generation identifies different immutable snapshots",
            ));
        }
        self.dirty_snapshot = Some(snapshot);
        Ok(DirtyContentTransition::Applied)
    }

    /// Clears dirty state only when the completed upload still owns the active generation.
    pub fn clear_dirty(
        &mut self,
        generation: LocalContentGeneration,
    ) -> Result<DirtyContentTransition> {
        let Some(current) = self.dirty_snapshot.as_ref() else {
            return Ok(DirtyContentTransition::AlreadyApplied);
        };
        if generation < current.generation() {
            return Ok(DirtyContentTransition::Fenced);
        }
        if generation > current.generation() {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "dirty clear generation was never recorded",
            ));
        }
        self.dirty_snapshot = None;
        Ok(DirtyContentTransition::Applied)
    }

    /// Acquires one idempotently identified runtime lease.
    pub fn acquire_lease(
        &mut self,
        lease_id: ContentLeaseId,
        kind: ContentLeaseKind,
    ) -> Result<bool> {
        if self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "content lease cannot start during eviction",
            ));
        }
        if let Some(existing) = self.leases.get(&lease_id) {
            if *existing == kind {
                return Ok(false);
            }
            return Err(CloudFilesCoreError::invalid_content_storage(
                "content lease id already has another kind",
            ));
        }
        self.lease_counts.increment(kind)?;
        self.leases.insert(lease_id, kind);
        Ok(true)
    }

    /// Releases one runtime lease.
    pub fn release_lease(&mut self, lease_id: &ContentLeaseId) -> Result<bool> {
        let Some(kind) = self.leases.remove(lease_id) else {
            return Ok(false);
        };
        self.lease_counts.decrement(kind)?;
        Ok(true)
    }

    /// Reserves one provider-cache write against concurrent eviction.
    pub fn begin_cache_write(&mut self) -> Result<()> {
        if self.provider_ranges.is_none() {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "platform-managed content has no provider cache write target",
            ));
        }
        if self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "provider cache write cannot start during eviction",
            ));
        }
        self.active_cache_writes = self.active_cache_writes.checked_add(1).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_storage("active cache write count overflowed")
        })?;
        Ok(())
    }

    /// Releases one completed provider-cache write reservation.
    pub fn complete_cache_write(&mut self) -> Result<()> {
        self.active_cache_writes = self.active_cache_writes.checked_sub(1).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_storage(
                "completed cache write has no active reservation",
            )
        })?;
        Ok(())
    }

    /// Computes blockers without changing reservation state.
    pub fn eviction_blockers(&self) -> Vec<ContentEvictionBlocker> {
        let mut blockers = Vec::new();
        if self.pinned {
            blockers.push(ContentEvictionBlocker::Pinned);
        }
        if self.dirty_snapshot.is_some() {
            blockers.push(ContentEvictionBlocker::Dirty);
        }
        if self.lease_counts.open > 0 {
            blockers.push(ContentEvictionBlocker::OpenLease);
        }
        if self.lease_counts.read > 0 {
            blockers.push(ContentEvictionBlocker::ReadLease);
        }
        if self.lease_counts.write > 0 {
            blockers.push(ContentEvictionBlocker::WriteLease);
        }
        if self.lease_counts.hydration > 0 {
            blockers.push(ContentEvictionBlocker::HydrationLease);
        }
        if self.lease_counts.upload > 0 {
            blockers.push(ContentEvictionBlocker::UploadLease);
        }
        if self.lease_counts.verification > 0 {
            blockers.push(ContentEvictionBlocker::VerificationLease);
        }
        if self.active_cache_writes > 0 {
            blockers.push(ContentEvictionBlocker::CacheWriteInProgress);
        }
        if self.eviction_reserved {
            blockers.push(ContentEvictionBlocker::EvictionInProgress);
        }
        blockers
    }

    /// Atomically evaluates guards and reserves this entry for an eviction intent.
    pub fn reserve_eviction(
        &mut self,
        target: ContentEvictionTarget,
    ) -> Result<ContentEvictionDecision> {
        let plan = ContentEvictionPlan::for_target(self.mode, target)?;
        let blockers = self.eviction_blockers();
        if !blockers.is_empty() {
            return Ok(ContentEvictionDecision::Blocked { blockers });
        }
        self.eviction_reserved = true;
        Ok(ContentEvictionDecision::Reserved { plan })
    }

    /// Applies reconciled physical effects to metadata while retaining the reservation.
    pub fn reconcile_eviction(&mut self, plan: ContentEvictionPlan) -> Result<()> {
        if !self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "eviction metadata reconciliation requires a reservation",
            ));
        }
        if plan.remove_provider_cache {
            let Some(ranges) = self.provider_ranges.as_mut() else {
                return Err(CloudFilesCoreError::invalid_content_storage(
                    "eviction plan removes an unowned provider cache",
                ));
            };
            ranges.clear();
        }
        if plan.evict_platform_materialization {
            let Some(state) = self.platform_materialization.as_mut() else {
                return Err(CloudFilesCoreError::invalid_content_storage(
                    "eviction plan evicts an unowned platform materialization",
                ));
            };
            *state = PlatformMaterializationState::Dataless;
        }
        Ok(())
    }

    /// Releases the reservation after the durable eviction record reaches completion.
    pub fn complete_eviction(&mut self) -> Result<()> {
        if !self.eviction_reserved {
            return Err(CloudFilesCoreError::invalid_content_storage(
                "completed eviction has no active reservation",
            ));
        }
        self.eviction_reserved = false;
        Ok(())
    }
}

/// Stable identity of one durable eviction operation.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentEvictionOperationId(String);

impl ContentEvictionOperationId {
    /// Creates a non-empty opaque eviction operation identity.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("content eviction operation id"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque operation identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContentEvictionOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentEvictionOperationId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Durable eviction intent captured before any physical cache or platform effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentEvictionIntent {
    operation_id: ContentEvictionOperationId,
    cache_key: ContentCacheKey,
    target: ContentEvictionTarget,
    session_generation: SessionGeneration,
}

impl ContentEvictionIntent {
    /// Creates a product-neutral eviction intent.
    pub const fn new(
        operation_id: ContentEvictionOperationId,
        cache_key: ContentCacheKey,
        target: ContentEvictionTarget,
        session_generation: SessionGeneration,
    ) -> Self {
        Self {
            operation_id,
            cache_key,
            target,
            session_generation,
        }
    }

    /// Returns the durable operation identity.
    pub const fn operation_id(&self) -> &ContentEvictionOperationId {
        &self.operation_id
    }

    /// Returns the revision-bound content key.
    pub const fn cache_key(&self) -> &ContentCacheKey {
        &self.cache_key
    }

    /// Returns the selected physical eviction target.
    pub const fn target(&self) -> ContentEvictionTarget {
        self.target
    }

    /// Returns the platform session generation that created the intent.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// One idempotent physical effect observed during eviction recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentEvictionPhysicalEffect {
    /// Provider-owned backing bytes are absent.
    ProviderCacheRemoved,
    /// Platform-owned materialized bytes are absent.
    PlatformMaterializationEvicted,
    /// Native/kernel content caches were invalidated after provider removal.
    PlatformContentInvalidated,
}

/// Durable eviction journal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentEvictionState {
    /// Intent and required physical plan are durable.
    IntentPersisted,
    /// Every required physical effect has been observed and recorded.
    PhysicalEffectsApplied,
    /// Content metadata reflects the physical result.
    MetadataReconciled,
    /// The operation is terminal and the entry reservation is released.
    Completed,
}

/// Result of an idempotent eviction record transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentEvictionRecordTransition {
    /// Durable state changed.
    Applied,
    /// Equivalent state was already durable.
    AlreadyApplied,
}

/// Recoverable durable eviction record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentEvictionRecord {
    intent: ContentEvictionIntent,
    plan: ContentEvictionPlan,
    state: ContentEvictionState,
    provider_cache_removed: bool,
    platform_materialization_evicted: bool,
    platform_content_invalidated: bool,
}

impl ContentEvictionRecord {
    /// Creates the first recoverable state after the entry reservation is durable.
    pub const fn persist(intent: ContentEvictionIntent, plan: ContentEvictionPlan) -> Self {
        Self {
            intent,
            plan,
            state: ContentEvictionState::IntentPersisted,
            provider_cache_removed: false,
            platform_materialization_evicted: false,
            platform_content_invalidated: false,
        }
    }

    /// Returns the immutable eviction intent.
    pub const fn intent(&self) -> &ContentEvictionIntent {
        &self.intent
    }

    /// Returns the required physical effect plan.
    pub const fn plan(&self) -> ContentEvictionPlan {
        self.plan
    }

    /// Returns the current durable state.
    pub const fn state(&self) -> ContentEvictionState {
        self.state
    }

    /// Returns whether one required physical effect has been observed.
    pub const fn has_observed_effect(&self, effect: ContentEvictionPhysicalEffect) -> bool {
        match effect {
            ContentEvictionPhysicalEffect::ProviderCacheRemoved => self.provider_cache_removed,
            ContentEvictionPhysicalEffect::PlatformMaterializationEvicted => {
                self.platform_materialization_evicted
            }
            ContentEvictionPhysicalEffect::PlatformContentInvalidated => {
                self.platform_content_invalidated
            }
        }
    }

    /// Records one idempotent physical effect and advances when the complete plan is observed.
    pub fn record_physical_effect(
        &mut self,
        effect: ContentEvictionPhysicalEffect,
    ) -> Result<ContentEvictionRecordTransition> {
        let required = match effect {
            ContentEvictionPhysicalEffect::ProviderCacheRemoved => self.plan.remove_provider_cache,
            ContentEvictionPhysicalEffect::PlatformMaterializationEvicted => {
                self.plan.evict_platform_materialization
            }
            ContentEvictionPhysicalEffect::PlatformContentInvalidated => {
                self.plan.invalidate_platform_content
            }
        };
        if !required {
            return Err(CloudFilesCoreError::invalid_content_eviction(
                "physical effect is not required by the eviction plan",
            ));
        }
        let observed = match effect {
            ContentEvictionPhysicalEffect::ProviderCacheRemoved => &mut self.provider_cache_removed,
            ContentEvictionPhysicalEffect::PlatformMaterializationEvicted => {
                &mut self.platform_materialization_evicted
            }
            ContentEvictionPhysicalEffect::PlatformContentInvalidated => {
                &mut self.platform_content_invalidated
            }
        };
        if *observed {
            return Ok(ContentEvictionRecordTransition::AlreadyApplied);
        }
        if !matches!(
            self.state,
            ContentEvictionState::IntentPersisted | ContentEvictionState::PhysicalEffectsApplied
        ) {
            return Err(CloudFilesCoreError::invalid_content_eviction(
                "physical effect must precede metadata reconciliation",
            ));
        }
        *observed = true;
        if self.all_required_effects_observed() {
            self.state = ContentEvictionState::PhysicalEffectsApplied;
        }
        Ok(ContentEvictionRecordTransition::Applied)
    }

    /// Marks content metadata reconciled after all physical effects are durable or observable.
    pub fn mark_metadata_reconciled(&mut self) -> Result<ContentEvictionRecordTransition> {
        match self.state {
            ContentEvictionState::PhysicalEffectsApplied => {
                self.state = ContentEvictionState::MetadataReconciled;
                Ok(ContentEvictionRecordTransition::Applied)
            }
            ContentEvictionState::MetadataReconciled | ContentEvictionState::Completed => {
                Ok(ContentEvictionRecordTransition::AlreadyApplied)
            }
            _ => Err(CloudFilesCoreError::invalid_content_eviction(
                "metadata reconciliation requires all physical effects",
            )),
        }
    }

    /// Marks the eviction terminal after metadata reconciliation.
    pub fn complete(&mut self) -> Result<ContentEvictionRecordTransition> {
        match self.state {
            ContentEvictionState::MetadataReconciled => {
                self.state = ContentEvictionState::Completed;
                Ok(ContentEvictionRecordTransition::Applied)
            }
            ContentEvictionState::Completed => Ok(ContentEvictionRecordTransition::AlreadyApplied),
            _ => Err(CloudFilesCoreError::invalid_content_eviction(
                "eviction completion requires metadata reconciliation",
            )),
        }
    }

    const fn all_required_effects_observed(&self) -> bool {
        (!self.plan.remove_provider_cache || self.provider_cache_removed)
            && (!self.plan.evict_platform_materialization || self.platform_materialization_evicted)
            && (!self.plan.invalidate_platform_content || self.platform_content_invalidated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CloudItemId, CloudNamespaceId, CloudRootId, CloudScope, ContentRevision};

    fn cache_key() -> ContentCacheKey {
        ContentCacheKey::new(
            CloudItemKey::new(
                CloudScope::new(
                    CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
                    CloudRootId::new("root").expect("root fixture should be valid"),
                ),
                CloudItemId::new("item").expect("item fixture should be valid"),
            ),
            ContentRevision::from_slice(b"content-v1")
                .expect("content revision fixture should be valid"),
        )
    }

    #[test]
    fn sparse_range_insertion_between_existing_ranges_takes_the_middle_branch() {
        let mut ranges = ContentRangeSet::new();
        ranges
            .insert(ByteRange::new(0, 2).expect("range should be valid"), 16)
            .expect("prefix range should insert");
        ranges
            .insert(ByteRange::new(10, 2).expect("range should be valid"), 16)
            .expect("suffix range should insert");
        ranges
            .insert(ByteRange::new(4, 2).expect("range should be valid"), 16)
            .expect("middle range should insert");

        assert_eq!(
            ranges.ranges(),
            &[
                ByteRange::new(0, 2).expect("range should be valid"),
                ByteRange::new(4, 2).expect("range should be valid"),
                ByteRange::new(10, 2).expect("range should be valid"),
            ]
        );
    }

    #[test]
    fn private_lease_and_cache_write_counters_reject_overflow_and_underflow() {
        let mut counts = ContentLeaseCounts {
            open: u32::MAX,
            ..ContentLeaseCounts::default()
        };
        assert_eq!(
            counts.increment(ContentLeaseKind::Open),
            Err(CloudFilesCoreError::InvalidContentStorageState {
                reason: "content lease count overflowed",
            })
        );

        let mut counts = ContentLeaseCounts::default();
        assert_eq!(
            counts.decrement(ContentLeaseKind::Read),
            Err(CloudFilesCoreError::InvalidContentStorageState {
                reason: "content lease count was already zero",
            })
        );

        let mut entry =
            ContentStorageEntry::new(cache_key(), ContentStorageMode::ProviderManaged, 16);
        entry.active_cache_writes = u32::MAX;
        assert_eq!(
            entry.begin_cache_write(),
            Err(CloudFilesCoreError::InvalidContentStorageState {
                reason: "active cache write count overflowed",
            })
        );
    }

    #[test]
    fn corrupted_eviction_record_rejects_late_unobserved_physical_effect() {
        let target = ContentEvictionTarget::ProviderCache;
        let intent = ContentEvictionIntent::new(
            ContentEvictionOperationId::new("eviction").expect("operation fixture should be valid"),
            cache_key(),
            target,
            SessionGeneration::new(1).expect("generation fixture should be valid"),
        );
        let plan = ContentEvictionPlan::for_target(ContentStorageMode::ProviderManaged, target)
            .expect("eviction plan should be valid");
        let mut record = ContentEvictionRecord::persist(intent, plan);
        record.state = ContentEvictionState::MetadataReconciled;

        assert_eq!(
            record.record_physical_effect(ContentEvictionPhysicalEffect::ProviderCacheRemoved),
            Err(CloudFilesCoreError::InvalidContentEvictionTransition {
                reason: "physical effect must precede metadata reconciliation",
            })
        );
    }
}
