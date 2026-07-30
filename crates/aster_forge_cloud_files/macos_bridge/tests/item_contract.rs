use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudItem, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
    CloudScope, ContentRevision, MetadataRevision,
};
use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER, MacosBridgeError, MacosFileProviderIdentifier,
    MacosFileProviderItem, MacosFileProviderItemKind,
};

#[test]
fn root_directory_and_root_children_use_the_apple_root_identifier() {
    let scope = scope("items", "default");
    let root_key = key(&scope, "root");
    let metadata = metadata(b"metadata-v1");
    let root = CloudItem::directory(root_key.clone(), None, "root", metadata.clone())
        .expect("root should be valid");
    let child = CloudItem::directory(
        key(&scope, "docs"),
        Some(root_key.item_id().clone()),
        "docs",
        metadata.clone(),
    )
    .expect("child directory should be valid");

    let root_item =
        MacosFileProviderItem::from_cloud_item(&root, &root_key).expect("root should convert");
    assert_eq!(
        root_item.identifier().as_str(),
        FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER
    );
    assert_eq!(
        root_item.parent_identifier().as_str(),
        FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER
    );
    assert_eq!(root_item.filename(), "root");
    assert_eq!(root_item.kind(), MacosFileProviderItemKind::Directory);
    assert_eq!(root_item.size(), 0);
    assert_eq!(root_item.version().metadata(), &metadata);
    assert!(!root_item.version().content().as_bytes().is_empty());

    let child_item = MacosFileProviderItem::from_cloud_item(&child, &root_key)
        .expect("root child should convert");
    assert_eq!(
        child_item.parent_identifier().as_str(),
        FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER
    );
    assert_eq!(
        child_item.identifier().item_key().expect("child key"),
        child.key()
    );
}

#[test]
fn nested_file_preserves_parent_size_and_separate_versions() {
    let scope = scope("items", "default");
    let root_key = key(&scope, "root");
    let docs_key = key(&scope, "docs");
    let metadata = metadata(b"metadata-file");
    let content = content(b"content-file");
    let file = CloudItem::file(
        key(&scope, "report"),
        docs_key.item_id().clone(),
        "report.txt",
        metadata.clone(),
        CloudContentMetadata::new(content.clone(), None, 42),
    )
    .expect("file should be valid");

    let converted =
        MacosFileProviderItem::from_cloud_item(&file, &root_key).expect("file should convert");
    assert_eq!(converted.kind(), MacosFileProviderItemKind::File);
    assert_eq!(converted.size(), 42);
    assert_eq!(converted.version().metadata(), &metadata);
    assert_eq!(converted.version().content(), &content);
    assert_eq!(
        converted
            .parent_identifier()
            .item_key()
            .expect("nested parent should be an item"),
        &docs_key
    );
}

#[test]
fn conversion_rejects_scope_root_role_filename_and_version_violations() {
    let item_scope = scope("items", "default");
    let other_scope = scope("items", "other");
    let root_key = key(&item_scope, "root");
    let metadata = metadata(b"metadata");

    let escaped = CloudItem::directory(
        key(&other_scope, "escaped"),
        Some(CloudItemId::new("parent").expect("parent should be valid")),
        "escaped",
        metadata.clone(),
    )
    .expect("cross-scope fixture should be a valid standalone item");
    assert!(matches!(
        MacosFileProviderItem::from_cloud_item(&escaped, &root_key),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "item escaped the File Provider root scope"
        })
    ));

    let wrong_root = CloudItem::directory(
        key(&item_scope, "not-root"),
        None,
        "root-shaped",
        metadata.clone(),
    )
    .expect("root-shaped fixture should be valid");
    assert!(matches!(
        MacosFileProviderItem::from_cloud_item(&wrong_root, &root_key),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "item root shape did not match the File Provider root role"
        })
    ));

    for invalid_name in ["bad/name", "bad\0name"] {
        let item = CloudItem::directory(
            key(&item_scope, invalid_name),
            Some(root_key.item_id().clone()),
            invalid_name,
            metadata.clone(),
        )
        .expect("invalid File Provider name remains a valid core item");
        assert!(matches!(
            MacosFileProviderItem::from_cloud_item(&item, &root_key),
            Err(MacosBridgeError::InvalidBackendResponse {
                reason: "item filename is not one File Provider path component"
            })
        ));
    }

    let oversized_metadata =
        MetadataRevision::new(vec![b'm'; 129]).expect("core revision should preserve opaque bytes");
    let oversized = CloudItem::directory(
        key(&item_scope, "oversized"),
        Some(root_key.item_id().clone()),
        "oversized",
        oversized_metadata,
    )
    .expect("oversized revision remains a valid core item");
    assert!(matches!(
        MacosFileProviderItem::from_cloud_item(&oversized, &root_key),
        Err(MacosBridgeError::InvalidItemVersion { .. })
    ));

    let oversized_content = CloudItem::file(
        key(&item_scope, "oversized-file"),
        root_key.item_id().clone(),
        "oversized-file",
        metadata,
        CloudContentMetadata::new(
            ContentRevision::new(vec![b'c'; 129])
                .expect("core revision should preserve opaque bytes"),
            None,
            0,
        ),
    )
    .expect("oversized content revision remains a valid core item");
    assert!(matches!(
        MacosFileProviderItem::from_cloud_item(&oversized_content, &root_key),
        Err(MacosBridgeError::InvalidItemVersion { .. })
    ));
}

#[test]
fn identifier_debug_output_redacts_item_identity_but_classifies_system_values() {
    let scope = scope("secret-namespace", "secret-root");
    let item = MacosFileProviderIdentifier::encode(&key(&scope, "secret-item"))
        .expect("identifier fixture should fit");
    let debug = format!("{item:?}");
    assert!(debug.contains("kind"));
    assert!(debug.contains("byte_len"));
    assert!(!debug.contains("secret"));

    let root = MacosFileProviderIdentifier::parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER)
        .expect("root identifier should parse");
    assert!(format!("{root:?}").contains("Root"));
}

fn scope(namespace: &str, root: &str) -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(namespace).expect("namespace should be valid"),
        CloudRootId::new(root).expect("root should be valid"),
    )
}

fn key(scope: &CloudScope, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope.clone(),
        CloudItemId::new(item).expect("item should be valid"),
    )
}

fn metadata(value: &[u8]) -> MetadataRevision {
    MetadataRevision::from_slice(value).expect("metadata should be valid")
}

fn content(value: &[u8]) -> ContentRevision {
    ContentRevision::from_slice(value).expect("content should be valid")
}
