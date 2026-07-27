use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesCoreError, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
    CloudScope, ContentCacheKey, ContentCacheWriteExtent, ContentCacheWriteIntent,
    ContentCacheWriteOperationId, ContentCacheWriteRecord, ContentCacheWriteRecordTransition,
    ContentCacheWriteState, ContentEvictionBlocker, ContentEvictionDecision, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionPhysicalEffect, ContentEvictionPlan,
    ContentEvictionRecord, ContentEvictionRecordTransition, ContentEvictionState,
    ContentEvictionTarget, ContentLeaseCounts, ContentLeaseId, ContentLeaseKind, ContentRangeSet,
    ContentRevision, ContentStorageEntry, ContentStorageMode, DirtyContentTransition,
    LocalContentGeneration, LocalContentReference, LocalContentSnapshot,
    PlatformMaterializationState, SessionGeneration,
};

fn scope() -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
        CloudRootId::new("root").expect("root fixture should be valid"),
    )
}

fn item_key(item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope(),
        CloudItemId::new(item).expect("item fixture should be valid"),
    )
}

fn cache_key(item: &str) -> ContentCacheKey {
    ContentCacheKey::new(
        item_key(item),
        ContentRevision::from_slice(b"content-v1").expect("revision fixture should be valid"),
    )
}

fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("range fixture should be valid")
}

fn snapshot(item: &str, generation: u64, reference: &str) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        item_key(item),
        LocalContentGeneration::new(generation).expect("generation fixture should be valid"),
        LocalContentReference::new(reference).expect("reference fixture should be valid"),
        16,
        None,
    )
}

#[test]
fn cache_and_operation_identities_expose_exact_values_and_empty_fields() {
    let cache = cache_key("file");
    assert_eq!(cache.item_key(), &item_key("file"));
    assert_eq!(cache.revision().as_bytes(), b"content-v1");

    assert_eq!(
        ContentLeaseId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content lease id",
        })
    );
    assert_eq!(
        ContentEvictionOperationId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content eviction operation id",
        })
    );
    assert_eq!(
        ContentCacheWriteOperationId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content cache write operation id",
        })
    );

    let lease = ContentLeaseId::new(" lease ").expect("lease fixture should be valid");
    let eviction = ContentEvictionOperationId::new(" eviction ")
        .expect("eviction operation fixture should be valid");
    let write = ContentCacheWriteOperationId::new(" write ")
        .expect("write operation fixture should be valid");
    assert_eq!(lease.as_str(), " lease ");
    assert_eq!(eviction.as_str(), " eviction ");
    assert_eq!(write.as_str(), " write ");
    assert_eq!(format!("{lease:?}"), "ContentLeaseId { byte_len: 7 }");
    assert_eq!(
        format!("{eviction:?}"),
        "ContentEvictionOperationId { byte_len: 10 }"
    );
    assert_eq!(
        format!("{write:?}"),
        "ContentCacheWriteOperationId { byte_len: 7 }"
    );
}

#[test]
fn sparse_range_set_normalizes_before_after_duplicate_contained_covering_and_bridge_inserts() {
    let mut ranges = ContentRangeSet::new();
    assert!(ranges.ranges().is_empty());
    assert!(
        ranges
            .insert(range(10, 2), 20)
            .expect("insert should succeed")
    );
    assert!(
        ranges
            .insert(range(2, 2), 20)
            .expect("insert should succeed")
    );
    assert_eq!(ranges.ranges(), &[range(2, 2), range(10, 2)]);

    assert!(
        !ranges
            .insert(range(2, 2), 20)
            .expect("duplicate should succeed")
    );
    assert!(
        !ranges
            .insert(range(3, 1), 20)
            .expect("contained should succeed")
    );
    assert!(
        ranges
            .insert(range(1, 5), 20)
            .expect("covering insert should succeed")
    );
    assert_eq!(ranges.ranges(), &[range(1, 5), range(10, 2)]);
    assert!(
        ranges
            .insert(range(6, 4), 20)
            .expect("bridge insert should succeed")
    );
    assert_eq!(ranges.ranges(), &[range(1, 11)]);
    assert!(ranges.covers(range(2, 9)));
    assert!(!ranges.covers(range(0, 1)));
    assert!(!ranges.is_complete(20));

    assert!(
        ranges
            .insert(range(0, 1), 20)
            .expect("prefix insert should succeed")
    );
    assert!(
        ranges
            .insert(range(12, 8), 20)
            .expect("suffix insert should succeed")
    );
    assert_eq!(ranges.ranges(), &[range(0, 20)]);
    assert!(ranges.is_complete(20));
    ranges.clear();
    assert!(ranges.ranges().is_empty());
    assert!(ranges.is_complete(0));
}

#[test]
fn sparse_range_set_rejects_exactly_one_byte_beyond_total_size() {
    let mut ranges = ContentRangeSet::new();
    assert!(ranges.insert(range(9, 1), 10).is_ok());
    assert_eq!(
        ranges.insert(range(10, 1), 10),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "cached range exceeds the content size",
        })
    );
}

#[test]
fn storage_entry_initial_shape_and_owned_state_match_each_mode() {
    let platform = ContentStorageEntry::new(
        cache_key("platform"),
        ContentStorageMode::PlatformManaged,
        8,
    );
    assert_eq!(platform.mode(), ContentStorageMode::PlatformManaged);
    assert_eq!(platform.total_size(), 8);
    assert!(platform.provider_ranges().is_none());
    assert_eq!(
        platform.platform_materialization(),
        Some(PlatformMaterializationState::Unknown)
    );
    assert!(!platform.is_pinned());
    assert!(!platform.is_dirty());
    assert!(platform.dirty_snapshot().is_none());
    assert_eq!(platform.lease_counts(), ContentLeaseCounts::default());
    assert_eq!(platform.active_cache_writes(), 0);
    assert!(!platform.is_eviction_reserved());

    let provider = ContentStorageEntry::new(
        cache_key("provider"),
        ContentStorageMode::ProviderManaged,
        8,
    );
    assert!(provider.provider_ranges().is_some());
    assert!(provider.platform_materialization().is_none());

    let hybrid = ContentStorageEntry::new(cache_key("hybrid"), ContentStorageMode::Hybrid, 8);
    assert!(hybrid.provider_ranges().is_some());
    assert_eq!(
        hybrid.platform_materialization(),
        Some(PlatformMaterializationState::Unknown)
    );
}

#[test]
fn owned_range_and_platform_observation_paths_are_idempotent_and_mode_checked() {
    let mut platform = ContentStorageEntry::new(
        cache_key("platform"),
        ContentStorageMode::PlatformManaged,
        8,
    );
    assert_eq!(
        platform.record_provider_range(range(0, 1)),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "platform-managed content has no provider cache ranges",
        })
    );
    assert!(
        platform
            .observe_platform_materialization(PlatformMaterializationState::Partial)
            .expect("platform observation should succeed")
    );
    assert!(
        !platform
            .observe_platform_materialization(PlatformMaterializationState::Partial)
            .expect("duplicate observation should succeed")
    );
    assert!(
        platform
            .observe_platform_materialization(PlatformMaterializationState::Complete)
            .expect("platform observation should succeed")
    );

    let mut provider = ContentStorageEntry::new(
        cache_key("provider"),
        ContentStorageMode::ProviderManaged,
        8,
    );
    assert!(
        provider
            .record_provider_range(range(0, 8))
            .expect("provider range should succeed")
    );
    assert!(
        !provider
            .record_provider_range(range(2, 2))
            .expect("contained range should succeed")
    );
    assert_eq!(
        provider.observe_platform_materialization(PlatformMaterializationState::Complete),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "provider-managed content has no platform materialization state",
        })
    );
}

#[test]
fn pin_and_dirty_transitions_are_idempotent_monotonic_and_item_scoped() {
    let mut entry =
        ContentStorageEntry::new(cache_key("file"), ContentStorageMode::ProviderManaged, 16);
    assert!(entry.set_pinned(true));
    assert!(!entry.set_pinned(true));
    assert!(entry.set_pinned(false));

    assert_eq!(
        entry.mark_dirty(snapshot("other", 1, "local://other/1")),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "dirty snapshot item does not match the content entry",
        })
    );
    let generation_two = snapshot("file", 2, "local://file/2");
    assert_eq!(
        entry.mark_dirty(generation_two.clone()),
        Ok(DirtyContentTransition::Applied)
    );
    assert_eq!(entry.dirty_snapshot(), Some(&generation_two));
    assert_eq!(
        entry.mark_dirty(generation_two),
        Ok(DirtyContentTransition::AlreadyApplied)
    );
    assert_eq!(
        entry.mark_dirty(snapshot("file", 1, "local://file/1")),
        Ok(DirtyContentTransition::Fenced)
    );
    assert_eq!(
        entry.mark_dirty(snapshot("file", 2, "local://file/different")),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "one local generation identifies different immutable snapshots",
        })
    );
    assert_eq!(
        entry.mark_dirty(snapshot("file", 3, "local://file/3")),
        Ok(DirtyContentTransition::Applied)
    );
}

#[test]
fn dirty_clear_handles_absent_stale_exact_and_future_generations() {
    let mut entry =
        ContentStorageEntry::new(cache_key("file"), ContentStorageMode::ProviderManaged, 16);
    assert_eq!(
        entry.clear_dirty(LocalContentGeneration::new(1).expect("generation should be valid")),
        Ok(DirtyContentTransition::AlreadyApplied)
    );
    entry
        .mark_dirty(snapshot("file", 2, "local://file/2"))
        .expect("dirty snapshot should persist");
    assert_eq!(
        entry.clear_dirty(LocalContentGeneration::new(1).expect("generation should be valid")),
        Ok(DirtyContentTransition::Fenced)
    );
    assert_eq!(
        entry.clear_dirty(LocalContentGeneration::new(3).expect("generation should be valid")),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "dirty clear generation was never recorded",
        })
    );
    assert_eq!(
        entry.clear_dirty(LocalContentGeneration::new(2).expect("generation should be valid")),
        Ok(DirtyContentTransition::Applied)
    );
    assert!(!entry.is_dirty());
}

#[test]
fn every_lease_kind_increments_and_decrements_its_exact_counter() {
    let mut entry =
        ContentStorageEntry::new(cache_key("file"), ContentStorageMode::ProviderManaged, 16);
    let kinds = [
        (
            ContentLeaseKind::Open,
            ContentLeaseCounts {
                open: 1,
                ..ContentLeaseCounts::default()
            },
        ),
        (
            ContentLeaseKind::Read,
            ContentLeaseCounts {
                read: 1,
                ..ContentLeaseCounts::default()
            },
        ),
        (
            ContentLeaseKind::Write,
            ContentLeaseCounts {
                write: 1,
                ..ContentLeaseCounts::default()
            },
        ),
        (
            ContentLeaseKind::Hydration,
            ContentLeaseCounts {
                hydration: 1,
                ..ContentLeaseCounts::default()
            },
        ),
        (
            ContentLeaseKind::Upload,
            ContentLeaseCounts {
                upload: 1,
                ..ContentLeaseCounts::default()
            },
        ),
        (
            ContentLeaseKind::Verification,
            ContentLeaseCounts {
                verification: 1,
                ..ContentLeaseCounts::default()
            },
        ),
    ];

    for (index, (kind, expected)) in kinds.into_iter().enumerate() {
        let lease =
            ContentLeaseId::new(format!("lease-{index}")).expect("lease fixture should be valid");
        assert!(
            entry
                .acquire_lease(lease.clone(), kind)
                .expect("lease should acquire")
        );
        assert_eq!(entry.lease_counts(), expected);
        assert!(
            !entry
                .acquire_lease(lease.clone(), kind)
                .expect("duplicate should succeed")
        );
        let other_kind = if kind == ContentLeaseKind::Open {
            ContentLeaseKind::Read
        } else {
            ContentLeaseKind::Open
        };
        assert_eq!(
            entry.acquire_lease(lease.clone(), other_kind),
            Err(CloudFilesCoreError::InvalidContentStorageState {
                reason: "content lease id already has another kind",
            })
        );
        assert!(entry.release_lease(&lease).expect("lease should release"));
        assert!(
            !entry
                .release_lease(&lease)
                .expect("missing release should succeed")
        );
        assert_eq!(entry.lease_counts(), ContentLeaseCounts::default());
    }
}

#[test]
fn cache_write_reservations_count_independently_and_block_eviction_until_all_complete() {
    let mut entry =
        ContentStorageEntry::new(cache_key("file"), ContentStorageMode::ProviderManaged, 16);
    entry
        .begin_cache_write()
        .expect("first write should reserve");
    entry
        .begin_cache_write()
        .expect("second write should reserve");
    assert_eq!(entry.active_cache_writes(), 2);
    assert_eq!(
        entry.eviction_blockers(),
        vec![ContentEvictionBlocker::CacheWriteInProgress]
    );
    entry
        .complete_cache_write()
        .expect("first write should complete");
    assert_eq!(entry.active_cache_writes(), 1);
    assert_eq!(
        entry.eviction_blockers(),
        vec![ContentEvictionBlocker::CacheWriteInProgress]
    );
    entry
        .complete_cache_write()
        .expect("second write should complete");
    assert!(entry.eviction_blockers().is_empty());
    assert_eq!(
        entry.complete_cache_write(),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "completed cache write has no active reservation",
        })
    );

    let mut platform = ContentStorageEntry::new(
        cache_key("platform"),
        ContentStorageMode::PlatformManaged,
        16,
    );
    assert_eq!(
        platform.begin_cache_write(),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "platform-managed content has no provider cache write target",
        })
    );
}

#[test]
fn all_blockers_are_reported_once_in_stable_order_and_reservation_fences_new_work() {
    let mut entry = ContentStorageEntry::new(cache_key("file"), ContentStorageMode::Hybrid, 16);
    entry.set_pinned(true);
    entry
        .mark_dirty(snapshot("file", 1, "local://file/1"))
        .expect("dirty snapshot should persist");
    let kinds = [
        ContentLeaseKind::Open,
        ContentLeaseKind::Read,
        ContentLeaseKind::Write,
        ContentLeaseKind::Hydration,
        ContentLeaseKind::Upload,
        ContentLeaseKind::Verification,
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        entry
            .acquire_lease(
                ContentLeaseId::new(format!("lease-{index}"))
                    .expect("lease fixture should be valid"),
                kind,
            )
            .expect("lease should acquire");
    }
    entry.begin_cache_write().expect("write should reserve");
    assert_eq!(
        entry.eviction_blockers(),
        vec![
            ContentEvictionBlocker::Pinned,
            ContentEvictionBlocker::Dirty,
            ContentEvictionBlocker::OpenLease,
            ContentEvictionBlocker::ReadLease,
            ContentEvictionBlocker::WriteLease,
            ContentEvictionBlocker::HydrationLease,
            ContentEvictionBlocker::UploadLease,
            ContentEvictionBlocker::VerificationLease,
            ContentEvictionBlocker::CacheWriteInProgress,
        ]
    );

    let mut clean =
        ContentStorageEntry::new(cache_key("clean"), ContentStorageMode::ProviderManaged, 16);
    assert!(matches!(
        clean
            .reserve_eviction(ContentEvictionTarget::ProviderCache)
            .expect("reservation should succeed"),
        ContentEvictionDecision::Reserved { .. }
    ));
    assert_eq!(
        clean.eviction_blockers(),
        vec![ContentEvictionBlocker::EvictionInProgress]
    );
    assert_eq!(
        clean.acquire_lease(
            ContentLeaseId::new("late").expect("lease fixture should be valid"),
            ContentLeaseKind::Read,
        ),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "content lease cannot start during eviction",
        })
    );
    assert_eq!(
        clean.begin_cache_write(),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "provider cache write cannot start during eviction",
        })
    );
    assert_eq!(
        clean
            .reserve_eviction(ContentEvictionTarget::ProviderCache)
            .expect("second reservation should report blocker"),
        ContentEvictionDecision::Blocked {
            blockers: vec![ContentEvictionBlocker::EvictionInProgress],
        }
    );
}

#[test]
fn every_storage_mode_and_eviction_target_has_the_exact_physical_plan() {
    let cases = [
        (
            ContentStorageMode::PlatformManaged,
            ContentEvictionTarget::PlatformMaterialization,
            (false, true, false),
        ),
        (
            ContentStorageMode::PlatformManaged,
            ContentEvictionTarget::AllManagedContent,
            (false, true, false),
        ),
        (
            ContentStorageMode::ProviderManaged,
            ContentEvictionTarget::ProviderCache,
            (true, false, true),
        ),
        (
            ContentStorageMode::ProviderManaged,
            ContentEvictionTarget::AllManagedContent,
            (true, false, true),
        ),
        (
            ContentStorageMode::Hybrid,
            ContentEvictionTarget::ProviderCache,
            (true, false, true),
        ),
        (
            ContentStorageMode::Hybrid,
            ContentEvictionTarget::PlatformMaterialization,
            (false, true, false),
        ),
        (
            ContentStorageMode::Hybrid,
            ContentEvictionTarget::AllManagedContent,
            (true, true, true),
        ),
    ];
    for (mode, target, expected) in cases {
        let plan = ContentEvictionPlan::for_target(mode, target)
            .expect("owned target should produce a plan");
        assert_eq!(
            (
                plan.remove_provider_cache(),
                plan.evict_platform_materialization(),
                plan.invalidate_platform_content(),
            ),
            expected
        );
    }
    assert_eq!(
        ContentEvictionPlan::for_target(
            ContentStorageMode::PlatformManaged,
            ContentEvictionTarget::ProviderCache,
        ),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "eviction target is not owned by the selected storage mode",
        })
    );
    assert_eq!(
        ContentEvictionPlan::for_target(
            ContentStorageMode::ProviderManaged,
            ContentEvictionTarget::PlatformMaterialization,
        ),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "eviction target is not owned by the selected storage mode",
        })
    );
}

#[test]
fn entry_eviction_reconciliation_requires_reservation_and_owned_plan() {
    let mut provider = ContentStorageEntry::new(
        cache_key("provider"),
        ContentStorageMode::ProviderManaged,
        8,
    );
    let provider_plan = ContentEvictionPlan::for_target(
        ContentStorageMode::ProviderManaged,
        ContentEvictionTarget::ProviderCache,
    )
    .expect("plan fixture should be valid");
    assert_eq!(
        provider.reconcile_eviction(provider_plan),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "eviction metadata reconciliation requires a reservation",
        })
    );
    assert_eq!(
        provider.complete_eviction(),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "completed eviction has no active reservation",
        })
    );
    provider
        .reserve_eviction(ContentEvictionTarget::ProviderCache)
        .expect("reservation should succeed");
    let platform_plan = ContentEvictionPlan::for_target(
        ContentStorageMode::PlatformManaged,
        ContentEvictionTarget::PlatformMaterialization,
    )
    .expect("plan fixture should be valid");
    assert_eq!(
        provider.reconcile_eviction(platform_plan),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "eviction plan evicts an unowned platform materialization",
        })
    );
    provider
        .reconcile_eviction(provider_plan)
        .expect("owned provider plan should reconcile");
    provider
        .complete_eviction()
        .expect("reservation should release");

    let mut platform = ContentStorageEntry::new(
        cache_key("platform"),
        ContentStorageMode::PlatformManaged,
        8,
    );
    platform
        .reserve_eviction(ContentEvictionTarget::PlatformMaterialization)
        .expect("reservation should succeed");
    assert_eq!(
        platform.reconcile_eviction(provider_plan),
        Err(CloudFilesCoreError::InvalidContentStorageState {
            reason: "eviction plan removes an unowned provider cache",
        })
    );
}

#[test]
fn eviction_intent_and_record_cover_all_effects_ordering_and_late_replay() {
    let plan = ContentEvictionPlan::for_target(
        ContentStorageMode::Hybrid,
        ContentEvictionTarget::AllManagedContent,
    )
    .expect("plan fixture should be valid");
    let intent = ContentEvictionIntent::new(
        ContentEvictionOperationId::new("eviction").expect("operation fixture should be valid"),
        cache_key("file"),
        ContentEvictionTarget::AllManagedContent,
        SessionGeneration::new(7).expect("session fixture should be valid"),
    );
    assert_eq!(intent.operation_id().as_str(), "eviction");
    assert_eq!(intent.cache_key(), &cache_key("file"));
    assert_eq!(intent.target(), ContentEvictionTarget::AllManagedContent);
    assert_eq!(intent.session_generation().get(), 7);

    let mut record = ContentEvictionRecord::persist(intent.clone(), plan);
    assert_eq!(record.intent(), &intent);
    assert_eq!(record.plan(), plan);
    assert_eq!(record.state(), ContentEvictionState::IntentPersisted);
    for effect in [
        ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        ContentEvictionPhysicalEffect::PlatformMaterializationEvicted,
        ContentEvictionPhysicalEffect::PlatformContentInvalidated,
    ] {
        assert!(!record.has_observed_effect(effect));
    }
    assert_eq!(
        record.mark_metadata_reconciled(),
        Err(CloudFilesCoreError::InvalidContentEvictionTransition {
            reason: "metadata reconciliation requires all physical effects",
        })
    );
    assert_eq!(
        record.complete(),
        Err(CloudFilesCoreError::InvalidContentEvictionTransition {
            reason: "eviction completion requires metadata reconciliation",
        })
    );

    for effect in [
        ContentEvictionPhysicalEffect::PlatformContentInvalidated,
        ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        ContentEvictionPhysicalEffect::PlatformMaterializationEvicted,
    ] {
        assert_eq!(
            record.record_physical_effect(effect),
            Ok(ContentEvictionRecordTransition::Applied)
        );
        assert!(record.has_observed_effect(effect));
    }
    assert_eq!(record.state(), ContentEvictionState::PhysicalEffectsApplied);
    assert_eq!(
        record.mark_metadata_reconciled(),
        Ok(ContentEvictionRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_metadata_reconciled(),
        Ok(ContentEvictionRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentEvictionRecordTransition::Applied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentEvictionRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_physical_effect(ContentEvictionPhysicalEffect::ProviderCacheRemoved),
        Ok(ContentEvictionRecordTransition::AlreadyApplied)
    );
}

#[test]
fn eviction_record_rejects_effect_not_required_by_plan() {
    let plan = ContentEvictionPlan::for_target(
        ContentStorageMode::PlatformManaged,
        ContentEvictionTarget::PlatformMaterialization,
    )
    .expect("plan fixture should be valid");
    let intent = ContentEvictionIntent::new(
        ContentEvictionOperationId::new("eviction").expect("operation fixture should be valid"),
        cache_key("file"),
        ContentEvictionTarget::PlatformMaterialization,
        SessionGeneration::new(1).expect("session fixture should be valid"),
    );
    let mut record = ContentEvictionRecord::persist(intent, plan);
    assert_eq!(
        record.record_physical_effect(ContentEvictionPhysicalEffect::ProviderCacheRemoved),
        Err(CloudFilesCoreError::InvalidContentEvictionTransition {
            reason: "physical effect is not required by the eviction plan",
        })
    );
    assert_eq!(
        record.record_physical_effect(ContentEvictionPhysicalEffect::PlatformContentInvalidated),
        Err(CloudFilesCoreError::InvalidContentEvictionTransition {
            reason: "physical effect is not required by the eviction plan",
        })
    );
}

#[test]
fn cache_write_intent_and_record_cover_boundaries_order_and_late_replay() {
    let operation =
        ContentCacheWriteOperationId::new("write").expect("operation fixture should be valid");
    let cache = cache_key("file");
    assert_eq!(
        ContentCacheWriteIntent::new(
            operation.clone(),
            cache.clone(),
            8,
            ContentCacheWriteExtent::Range(range(7, 2)),
            SessionGeneration::new(1).expect("session fixture should be valid"),
        ),
        Err(CloudFilesCoreError::InvalidContentCacheWriteTransition {
            reason: "cache write range exceeds the expected content size",
        })
    );
    let intent = ContentCacheWriteIntent::new(
        operation,
        cache.clone(),
        8,
        ContentCacheWriteExtent::Range(range(0, 8)),
        SessionGeneration::new(2).expect("session fixture should be valid"),
    )
    .expect("intent fixture should be valid");
    assert_eq!(intent.operation_id().as_str(), "write");
    assert_eq!(intent.cache_key(), &cache);
    assert_eq!(intent.expected_size(), 8);
    assert_eq!(intent.extent(), ContentCacheWriteExtent::Range(range(0, 8)));
    assert_eq!(intent.session_generation().get(), 2);

    let mut record = ContentCacheWriteRecord::persist(intent.clone());
    assert_eq!(record.intent(), &intent);
    assert_eq!(record.state(), ContentCacheWriteState::IntentPersisted);
    assert_eq!(
        record.mark_coverage_committed(),
        Err(CloudFilesCoreError::InvalidContentCacheWriteTransition {
            reason: "coverage commit requires observable physical bytes",
        })
    );
    assert_eq!(
        record.complete(),
        Err(CloudFilesCoreError::InvalidContentCacheWriteTransition {
            reason: "cache write completion requires committed coverage",
        })
    );
    assert_eq!(
        record.mark_physical_bytes_committed(),
        Ok(ContentCacheWriteRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_physical_bytes_committed(),
        Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.mark_coverage_committed(),
        Ok(ContentCacheWriteRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_coverage_committed(),
        Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentCacheWriteRecordTransition::Applied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.mark_physical_bytes_committed(),
        Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.mark_coverage_committed(),
        Ok(ContentCacheWriteRecordTransition::AlreadyApplied)
    );
}

#[test]
fn whole_cache_write_extent_supports_nonempty_and_empty_files() {
    for size in [0, 8] {
        let intent = ContentCacheWriteIntent::new(
            ContentCacheWriteOperationId::new(format!("write-{size}"))
                .expect("operation fixture should be valid"),
            cache_key("file"),
            size,
            ContentCacheWriteExtent::Whole,
            SessionGeneration::new(1).expect("session fixture should be valid"),
        )
        .expect("whole intent should be valid");
        assert_eq!(intent.extent(), ContentCacheWriteExtent::Whole);
        assert_eq!(intent.expected_size(), size);
    }
}
