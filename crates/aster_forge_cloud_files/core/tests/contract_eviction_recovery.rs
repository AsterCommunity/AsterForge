mod support;

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesStoreErrorKind, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
    CloudScope, ContentCacheKey, ContentEvictionBegin, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionPhysicalEffect, ContentEvictionPlan,
    ContentEvictionState, ContentEvictionTarget, ContentRevision, ContentStorageEntry,
    ContentStorageMode, ContentStorageStore, PlatformMaterializationState, SessionGeneration,
    StoreWriteStatus,
};

use support::{
    SyntheticBackend,
    content_storage::{MemoryContentStorageStore, SyntheticPhysicalContent},
    failure_injection::FailurePoint,
};

fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("range fixture should be valid")
}

fn revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes()).expect("revision fixture should be valid")
}

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn operation_id(value: &str) -> ContentEvictionOperationId {
    ContentEvictionOperationId::new(value).expect("eviction operation fixture should be valid")
}

fn cache_key(backend: &SyntheticBackend, revision: &str) -> ContentCacheKey {
    ContentCacheKey::new(backend.moved_file().key().clone(), self::revision(revision))
}

fn scoped_cache_key(namespace: &str, root: &str, item: &str, value: &str) -> ContentCacheKey {
    ContentCacheKey::new(
        CloudItemKey::new(
            CloudScope::new(
                CloudNamespaceId::new(namespace).expect("namespace fixture should be valid"),
                CloudRootId::new(root).expect("root fixture should be valid"),
            ),
            CloudItemId::new(item).expect("item fixture should be valid"),
        ),
        revision(value),
    )
}

fn intent(
    operation: &str,
    key: ContentCacheKey,
    target: ContentEvictionTarget,
) -> ContentEvictionIntent {
    ContentEvictionIntent::new(operation_id(operation), key, target, generation(1))
}

async fn create_entry(
    store: &MemoryContentStorageStore,
    key: ContentCacheKey,
    mode: ContentStorageMode,
) {
    store
        .create_content_entry(ContentStorageEntry::new(key, mode, 16))
        .await
        .expect("content entry creation should succeed");
}

#[test]
fn eviction_plans_reflect_physical_ownership_and_native_invalidation() {
    let provider = ContentEvictionPlan::for_target(
        ContentStorageMode::ProviderManaged,
        ContentEvictionTarget::ProviderCache,
    )
    .expect("provider target should be valid");
    assert!(provider.remove_provider_cache());
    assert!(!provider.evict_platform_materialization());
    assert!(provider.invalidate_platform_content());

    let platform = ContentEvictionPlan::for_target(
        ContentStorageMode::PlatformManaged,
        ContentEvictionTarget::AllManagedContent,
    )
    .expect("platform target should be valid");
    assert!(!platform.remove_provider_cache());
    assert!(platform.evict_platform_materialization());
    assert!(!platform.invalidate_platform_content());

    let hybrid = ContentEvictionPlan::for_target(
        ContentStorageMode::Hybrid,
        ContentEvictionTarget::AllManagedContent,
    )
    .expect("hybrid target should be valid");
    assert!(hybrid.remove_provider_cache());
    assert!(hybrid.evict_platform_materialization());
    assert!(hybrid.invalidate_platform_content());

    assert!(
        ContentEvictionPlan::for_target(
            ContentStorageMode::PlatformManaged,
            ContentEvictionTarget::ProviderCache,
        )
        .is_err()
    );
    assert!(
        ContentEvictionPlan::for_target(
            ContentStorageMode::ProviderManaged,
            ContentEvictionTarget::PlatformMaterialization,
        )
        .is_err()
    );
}

#[tokio::test]
async fn provider_eviction_requires_removal_and_native_invalidation_before_metadata_reconcile() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "provider-eviction-v1");
    create_entry(&store, key.clone(), ContentStorageMode::ProviderManaged).await;
    store
        .record_provider_cached_range(&key, range(0, 16))
        .await
        .expect("provider coverage should be recorded");
    let operation = operation_id("provider-eviction");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation.clone(),
            key.clone(),
            ContentEvictionTarget::ProviderCache,
            generation(1),
        ))
        .await
        .expect("eviction intent should persist");

    store
        .record_content_eviction_effect(
            &operation,
            ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        )
        .await
        .expect("provider removal marker should persist");
    let partial = store
        .load_content_eviction(&operation)
        .await
        .expect("eviction load should succeed")
        .expect("eviction should exist");
    assert_eq!(partial.state(), ContentEvictionState::IntentPersisted);
    let error = store
        .reconcile_content_eviction_metadata(&operation)
        .await
        .expect_err("metadata must wait for native invalidation");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);

    store
        .record_content_eviction_effect(
            &operation,
            ContentEvictionPhysicalEffect::PlatformContentInvalidated,
        )
        .await
        .expect("platform invalidation marker should persist");
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("eviction load should succeed")
            .expect("eviction should exist")
            .state(),
        ContentEvictionState::PhysicalEffectsApplied
    );
    store
        .reconcile_content_eviction_metadata(&operation)
        .await
        .expect("metadata reconciliation should succeed");
    let snapshot = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert!(
        snapshot
            .provider_ranges()
            .expect("provider coverage should exist")
            .ranges()
            .is_empty()
    );
    assert!(snapshot.is_eviction_reserved());
    store
        .complete_content_eviction(&operation)
        .await
        .expect("completion should succeed");
    assert!(
        !store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .is_eviction_reserved()
    );
}

#[tokio::test]
async fn platform_eviction_reconciles_to_dataless_without_provider_coverage() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "platform-eviction-v1");
    create_entry(&store, key.clone(), ContentStorageMode::PlatformManaged).await;
    store
        .observe_platform_materialization(&key, PlatformMaterializationState::Complete)
        .await
        .expect("platform state should be observed");
    let operation = operation_id("platform-eviction");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation.clone(),
            key.clone(),
            ContentEvictionTarget::PlatformMaterialization,
            generation(1),
        ))
        .await
        .expect("eviction intent should persist");
    store
        .record_content_eviction_effect(
            &operation,
            ContentEvictionPhysicalEffect::PlatformMaterializationEvicted,
        )
        .await
        .expect("platform eviction marker should persist");
    store
        .reconcile_content_eviction_metadata(&operation)
        .await
        .expect("metadata reconciliation should succeed");

    let snapshot = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert!(snapshot.provider_ranges().is_none());
    assert_eq!(
        snapshot.platform_materialization(),
        Some(PlatformMaterializationState::Dataless)
    );
}

#[tokio::test]
async fn hybrid_all_eviction_waits_for_all_three_effects_and_rejects_unplanned_effects() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "hybrid-eviction-v1");
    create_entry(&store, key.clone(), ContentStorageMode::Hybrid).await;
    store
        .record_provider_cached_range(&key, range(0, 16))
        .await
        .expect("provider coverage should be recorded");
    store
        .observe_platform_materialization(&key, PlatformMaterializationState::Complete)
        .await
        .expect("platform state should be observed");
    let operation = operation_id("hybrid-all");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation.clone(),
            key.clone(),
            ContentEvictionTarget::AllManagedContent,
            generation(1),
        ))
        .await
        .expect("hybrid eviction intent should persist");

    for effect in [
        ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        ContentEvictionPhysicalEffect::PlatformMaterializationEvicted,
    ] {
        store
            .record_content_eviction_effect(&operation, effect)
            .await
            .expect("required effect should persist");
    }
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("eviction load should succeed")
            .expect("eviction should exist")
            .state(),
        ContentEvictionState::IntentPersisted
    );
    store
        .record_content_eviction_effect(
            &operation,
            ContentEvictionPhysicalEffect::PlatformContentInvalidated,
        )
        .await
        .expect("final required effect should persist");
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("eviction load should succeed")
            .expect("eviction should exist")
            .state(),
        ContentEvictionState::PhysicalEffectsApplied
    );

    let second_key = cache_key(&backend, "hybrid-platform-only-v1");
    create_entry(&store, second_key.clone(), ContentStorageMode::Hybrid).await;
    let second_operation = operation_id("hybrid-platform-only");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            second_operation.clone(),
            second_key,
            ContentEvictionTarget::PlatformMaterialization,
            generation(1),
        ))
        .await
        .expect("platform-only intent should persist");
    let error = store
        .record_content_eviction_effect(
            &second_operation,
            ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        )
        .await
        .expect_err("effect outside the durable plan must be rejected");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
}

#[tokio::test]
async fn intent_and_each_durable_transition_are_idempotent_after_lost_returns() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "lost-returns-v1");
    create_entry(&store, key.clone(), ContentStorageMode::ProviderManaged).await;
    store
        .record_provider_cached_range(&key, range(0, 16))
        .await
        .expect("provider coverage should be recorded");
    let operation = operation_id("lost-returns");
    let eviction_intent = ContentEvictionIntent::new(
        operation.clone(),
        key,
        ContentEvictionTarget::ProviderCache,
        generation(1),
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentEvictionIntentPersistBeforeReturn);
    store
        .begin_content_eviction(eviction_intent.clone())
        .await
        .expect_err("intent persist return should be lost");
    assert!(matches!(
        store
            .begin_content_eviction(eviction_intent)
            .await
            .expect("repeated intent should find durable state"),
        ContentEvictionBegin::AlreadyPersisted { .. }
    ));

    for effect in [
        ContentEvictionPhysicalEffect::ProviderCacheRemoved,
        ContentEvictionPhysicalEffect::PlatformContentInvalidated,
    ] {
        store
            .failures()
            .arm(FailurePoint::AfterContentEvictionEffectPersistBeforeReturn);
        store
            .record_content_eviction_effect(&operation, effect)
            .await
            .expect_err("effect marker return should be lost");
        assert_eq!(
            store
                .record_content_eviction_effect(&operation, effect)
                .await
                .expect("repeated effect marker should be idempotent"),
            StoreWriteStatus::AlreadyApplied
        );
    }

    store
        .failures()
        .arm(FailurePoint::AfterContentEvictionMetadataPersistBeforeReturn);
    store
        .reconcile_content_eviction_metadata(&operation)
        .await
        .expect_err("metadata persist return should be lost");
    assert_eq!(
        store
            .reconcile_content_eviction_metadata(&operation)
            .await
            .expect("repeated metadata reconciliation should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentEvictionCompletionPersistBeforeReturn);
    store
        .complete_content_eviction(&operation)
        .await
        .expect_err("completion persist return should be lost");
    assert_eq!(
        store
            .complete_content_eviction(&operation)
            .await
            .expect("repeated completion should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .record_content_eviction_effect(
                &operation,
                ContentEvictionPhysicalEffect::ProviderCacheRemoved,
            )
            .await
            .expect("a durable effect marker remains idempotent after completion"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .reconcile_content_eviction_metadata(&operation)
            .await
            .expect("durable metadata reconciliation remains idempotent after completion"),
        StoreWriteStatus::AlreadyApplied
    );
    assert!(
        store
            .recoverable_content_evictions(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .is_empty()
    );
}

#[tokio::test]
async fn physical_effect_before_marker_crash_is_recovered_by_observation() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "physical-observation-v1");
    create_entry(&store, key.clone(), ContentStorageMode::ProviderManaged).await;
    store
        .record_provider_cached_range(&key, range(0, 16))
        .await
        .expect("provider coverage should be recorded");
    let operation = operation_id("physical-observation");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation.clone(),
            key,
            ContentEvictionTarget::ProviderCache,
            generation(1),
        ))
        .await
        .expect("intent should persist before physical work");

    let mut physical = SyntheticPhysicalContent::new(true, false);
    physical.remove_provider_cache();
    assert!(!physical.provider_cache_present());
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("eviction load should succeed")
            .expect("eviction should exist")
            .state(),
        ContentEvictionState::IntentPersisted,
        "physical removal happened but its marker was not yet durable"
    );

    if !physical.provider_cache_present() {
        store
            .record_content_eviction_effect(
                &operation,
                ContentEvictionPhysicalEffect::ProviderCacheRemoved,
            )
            .await
            .expect("startup observation should reconstruct removal marker");
    }
    physical.invalidate_platform_content();
    store
        .record_content_eviction_effect(
            &operation,
            ContentEvictionPhysicalEffect::PlatformContentInvalidated,
        )
        .await
        .expect("invalidation marker should persist");
    assert_eq!(physical.platform_invalidation_count(), 1);
    assert!(!physical.platform_materialized());
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("eviction load should succeed")
            .expect("eviction should exist")
            .state(),
        ContentEvictionState::PhysicalEffectsApplied
    );
}

#[tokio::test]
async fn recovery_scan_is_scope_filtered_sorted_and_excludes_completed_records() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key_a = cache_key(&backend, "recovery-a-v1");
    let key_b = cache_key(&backend, "recovery-b-v1");
    create_entry(&store, key_a.clone(), ContentStorageMode::PlatformManaged).await;
    create_entry(&store, key_b.clone(), ContentStorageMode::PlatformManaged).await;

    for (operation, key) in [("recover-z", key_a), ("recover-a", key_b)] {
        store
            .begin_content_eviction(intent(
                operation,
                key,
                ContentEvictionTarget::PlatformMaterialization,
            ))
            .await
            .expect("recovery fixture intent should persist");
    }
    let other_key = scoped_cache_key("other-namespace", "other-root", "other-file", "other-v1");
    create_entry(
        &store,
        other_key.clone(),
        ContentStorageMode::PlatformManaged,
    )
    .await;
    store
        .begin_content_eviction(intent(
            "recover-other",
            other_key.clone(),
            ContentEvictionTarget::PlatformMaterialization,
        ))
        .await
        .expect("other-scope recovery fixture should persist");
    let records = store
        .recoverable_content_evictions(backend.scope())
        .await
        .expect("recovery scan should succeed");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].intent().operation_id().as_str(), "recover-a");
    assert_eq!(records[1].intent().operation_id().as_str(), "recover-z");
    assert_eq!(
        store
            .recoverable_content_evictions(other_key.item_key().scope())
            .await
            .expect("other-scope recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["recover-other"]
    );

    let complete = operation_id("recover-a");
    store
        .record_content_eviction_effect(
            &complete,
            ContentEvictionPhysicalEffect::PlatformMaterializationEvicted,
        )
        .await
        .expect("physical marker should persist");
    store
        .reconcile_content_eviction_metadata(&complete)
        .await
        .expect("metadata reconciliation should succeed");
    store
        .complete_content_eviction(&complete)
        .await
        .expect("completion should succeed");
    let records = store
        .recoverable_content_evictions(backend.scope())
        .await
        .expect("recovery scan should succeed");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].intent().operation_id().as_str(), "recover-z");
}

#[tokio::test]
async fn operation_identity_conflicts_do_not_replace_existing_intent() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "operation-conflict-v1");
    create_entry(&store, key.clone(), ContentStorageMode::Hybrid).await;
    let operation = operation_id("same-operation");
    store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation.clone(),
            key.clone(),
            ContentEvictionTarget::ProviderCache,
            generation(1),
        ))
        .await
        .expect("first intent should persist");

    let error = store
        .begin_content_eviction(ContentEvictionIntent::new(
            operation,
            key,
            ContentEvictionTarget::AllManagedContent,
            generation(1),
        ))
        .await
        .expect_err("same operation id must not identify different intent content");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::Conflict);
}
