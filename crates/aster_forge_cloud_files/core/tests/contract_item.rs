use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudFilesCoreError, CloudItem, CloudItemId, CloudItemKey, CloudItemKind,
    CloudNamespaceId, CloudRootId, CloudScope, ContentRevision, MetadataRevision,
};

fn scope() -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
        CloudRootId::new("root").expect("root fixture should be valid"),
    )
}

fn key(item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope(),
        CloudItemId::new(item).expect("item fixture should be valid"),
    )
}

fn metadata(value: &str) -> MetadataRevision {
    MetadataRevision::from_slice(value.as_bytes()).expect("metadata fixture should be valid")
}

fn content() -> CloudContentMetadata {
    CloudContentMetadata::new(
        ContentRevision::from_slice(b"content-v1").expect("content fixture should be valid"),
        None,
        12,
    )
}

#[test]
fn root_is_a_directory_without_content_metadata() {
    let root = CloudItem::directory(key("root-item"), None, "root", metadata("root-m1"))
        .expect("root fixture should be valid");

    assert!(root.is_root());
    assert_eq!(root.kind(), CloudItemKind::Directory);
    assert!(root.content().is_none());
}

#[test]
fn file_requires_a_parent_and_content_metadata_by_constructor_shape() {
    let parent = CloudItemId::new("parent").expect("parent fixture should be valid");
    let file = CloudItem::file(
        key("file"),
        parent.clone(),
        "file.txt",
        metadata("file-m1"),
        content(),
    )
    .expect("file fixture should be valid");

    assert_eq!(file.parent_id(), Some(&parent));
    assert_eq!(file.kind(), CloudItemKind::File);
    assert_eq!(file.content().map(CloudContentMetadata::size), Some(12));
}

#[test]
fn item_rejects_empty_names_and_self_parenting() {
    assert_eq!(
        CloudItem::directory(key("directory"), None, "", metadata("directory-m1")),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud item name",
        })
    );

    let self_key = key("self-parent");
    assert_eq!(
        CloudItem::file(
            self_key.clone(),
            self_key.item_id().clone(),
            "bad.txt",
            metadata("bad-m1"),
            content(),
        ),
        Err(CloudFilesCoreError::InvalidItemShape {
            reason: "an item must not be its own parent",
        })
    );
}

#[test]
fn same_root_move_preserves_key_and_content_revision() {
    let original = CloudItem::file(
        key("stable-file"),
        CloudItemId::new("parent-a").expect("parent fixture should be valid"),
        "old.txt",
        metadata("metadata-v1"),
        content(),
    )
    .expect("file fixture should be valid");
    let original_key = original.key().clone();
    let original_content_revision = original
        .content()
        .expect("file should have content metadata")
        .revision()
        .clone();

    let moved = original
        .moved(
            CloudItemId::new("parent-b").expect("parent fixture should be valid"),
            "new.txt",
            metadata("metadata-v2"),
        )
        .expect("move should be valid");

    assert_eq!(moved.key(), &original_key);
    assert_eq!(moved.name(), "new.txt");
    assert_eq!(
        moved
            .content()
            .expect("file should have content metadata")
            .revision(),
        &original_content_revision
    );
}

#[test]
fn root_item_cannot_be_moved_below_another_parent() {
    let root = CloudItem::directory(key("root-item"), None, "root", metadata("root-m1"))
        .expect("root fixture should be valid");

    assert_eq!(
        root.moved(
            CloudItemId::new("other-parent").expect("parent fixture should be valid"),
            "moved-root",
            metadata("root-m2"),
        ),
        Err(CloudFilesCoreError::InvalidItemShape {
            reason: "the root item must not be moved",
        })
    );
}
