mod support;

use std::collections::HashMap;

use aster_forge_cloud_files_core::{
    ChangePage, ChangeResetReason, CloudChange, CloudItem, CloudItemKey, CloudMetadataBackend,
};

use support::SyntheticBackend;

fn apply(snapshot: &mut HashMap<CloudItemKey, CloudItem>, change: &CloudChange) {
    match change {
        CloudChange::Upsert { item } => {
            snapshot.insert(item.key().clone(), item.clone());
        }
        CloudChange::Delete { key, .. } => {
            snapshot.remove(key);
        }
    }
}

#[tokio::test]
async fn anchored_changes_preserve_move_identity_and_emit_tombstones() {
    let backend = SyntheticBackend::full();
    let first = backend
        .changes_since(backend.scope(), None)
        .await
        .expect("first change page should succeed");
    let ChangePage::Batch(first) = first else {
        panic!("expected a change batch");
    };
    assert!(first.has_more());
    assert_eq!(first.changes().len(), 1);

    let CloudChange::Upsert { item } = &first.changes()[0] else {
        panic!("first change should be an upsert");
    };
    assert_eq!(item.key(), backend.moved_file_before().key());
    assert_ne!(item.parent_id(), backend.moved_file_before().parent_id());
    assert_ne!(item.name(), backend.moved_file_before().name());

    let second = backend
        .changes_since(backend.scope(), Some(first.next_cursor()))
        .await
        .expect("second change page should succeed");
    let ChangePage::Batch(second) = second else {
        panic!("expected a change batch");
    };
    assert!(!second.has_more());
    assert!(matches!(second.changes(), [CloudChange::Delete { .. }]));
}

#[tokio::test]
async fn replaying_the_same_batches_is_idempotent_for_snapshot_state() {
    let backend = SyntheticBackend::full();
    let first = backend
        .changes_since(backend.scope(), None)
        .await
        .expect("first change page should succeed");
    let ChangePage::Batch(first) = first else {
        panic!("expected a change batch");
    };
    let second = backend
        .changes_since(backend.scope(), Some(first.next_cursor()))
        .await
        .expect("second change page should succeed");
    let ChangePage::Batch(second) = second else {
        panic!("expected a change batch");
    };

    let mut once = backend.initial_snapshot();
    for change in first.changes().iter().chain(second.changes()) {
        apply(&mut once, change);
    }
    let mut twice = once.clone();
    for change in first.changes().iter().chain(second.changes()) {
        apply(&mut twice, change);
    }

    assert_eq!(once, twice);
    assert_eq!(
        once.get(backend.moved_file().key()),
        Some(backend.moved_file())
    );
    assert!(!once.contains_key(backend.deleted_file().key()));
}

#[tokio::test]
async fn expired_cursor_requires_full_reconciliation() {
    let backend = SyntheticBackend::full();
    let result = backend
        .changes_since(backend.scope(), Some(&SyntheticBackend::expired_cursor()))
        .await
        .expect("expired cursor is an explicit reset result");

    assert_eq!(
        result,
        ChangePage::ResetRequired {
            reason: ChangeResetReason::CursorExpired,
        }
    );
}

#[tokio::test]
async fn invalidated_identity_mapping_is_distinct_from_cursor_expiry() {
    let backend = SyntheticBackend::full();
    let result = backend
        .changes_since(
            backend.scope(),
            Some(&SyntheticBackend::identity_invalid_cursor()),
        )
        .await
        .expect("identity invalidation is an explicit reset result");

    assert_eq!(
        result,
        ChangePage::ResetRequired {
            reason: ChangeResetReason::IdentityMappingInvalidated,
        }
    );
}
