mod support;

use std::collections::HashSet;

use aster_forge_cloud_files_core::{CloudBackendErrorKind, CloudMetadataBackend, PageCursor};

use support::SyntheticBackend;

#[tokio::test]
async fn full_backend_enumerates_multiple_pages_without_duplicates_or_omissions() {
    let backend = SyntheticBackend::full();
    let first = backend
        .list_children(backend.root_key(), None)
        .await
        .expect("first page should succeed");
    let second = backend
        .list_children(backend.root_key(), first.next_cursor())
        .await
        .expect("second page should succeed");

    assert_eq!(first.items().len(), 2);
    assert_eq!(second.items().len(), 1);
    assert!(second.next_cursor().is_none());

    let keys = first
        .items()
        .iter()
        .chain(second.items())
        .map(|item| item.key().clone())
        .collect::<HashSet<_>>();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn page_cursor_is_scoped_to_the_parent_enumeration() {
    let backend = SyntheticBackend::full();
    let first = backend
        .list_children(backend.root_key(), None)
        .await
        .expect("first page should succeed");
    let cursor = first
        .next_cursor()
        .expect("root fixture should have another page");
    let moved_file = backend.moved_file();
    let folder_key = aster_forge_cloud_files_core::CloudItemKey::new(
        backend.scope().clone(),
        moved_file
            .parent_id()
            .expect("moved file should have a parent")
            .clone(),
    );

    let error = backend
        .list_children(&folder_key, Some(cursor))
        .await
        .expect_err("cursor from another parent should fail");
    assert_eq!(error.kind(), CloudBackendErrorKind::InvalidRequest);
}

#[tokio::test]
async fn malformed_page_cursor_is_rejected_as_invalid_request() {
    let backend = SyntheticBackend::full();
    let cursor = PageCursor::from_slice(b"not-a-synthetic-page-cursor")
        .expect("cursor fixture should be valid");

    let error = backend
        .list_children(backend.root_key(), Some(&cursor))
        .await
        .expect_err("malformed cursor should fail");
    assert_eq!(error.kind(), CloudBackendErrorKind::InvalidRequest);
}

#[tokio::test]
async fn page_cursor_rejects_non_utf8_non_numeric_and_out_of_bounds_offsets() {
    let backend = SyntheticBackend::full();
    let prefix = format!(
        "page|{}|{}|{}|",
        backend.scope().namespace_id(),
        backend.scope().root_id(),
        backend.root_key().item_id()
    );
    let cursors = [
        PageCursor::new(vec![0xff]).expect("cursor fixture should be valid"),
        PageCursor::new(format!("{prefix}not-a-number").into_bytes())
            .expect("cursor fixture should be valid"),
        PageCursor::new(format!("{prefix}999").into_bytes())
            .expect("cursor fixture should be valid"),
    ];

    for cursor in cursors {
        assert_eq!(
            backend
                .list_children(backend.root_key(), Some(&cursor))
                .await
                .expect_err("invalid page position should fail")
                .kind(),
            CloudBackendErrorKind::InvalidRequest
        );
    }
}
