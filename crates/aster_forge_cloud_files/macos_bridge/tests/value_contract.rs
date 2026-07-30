use aster_forge_cloud_files_core::{ContentRevision, MetadataRevision};
use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES, MacosBridgeError, MacosFileProviderItemVersion,
};

#[test]
fn item_version_preserves_separate_exact_components_at_the_128_byte_boundary() {
    let metadata =
        MetadataRevision::new(vec![b'm'; FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES])
            .expect("metadata revision should be valid");
    let content = ContentRevision::new(vec![b'c'; FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES])
        .expect("content revision should be valid");
    let version = MacosFileProviderItemVersion::new(metadata.clone(), content.clone())
        .expect("boundary-sized version should be valid");
    assert_eq!(version.metadata(), &metadata);
    assert_eq!(version.content(), &content);
    assert_eq!(version.into_parts(), (metadata, content));
}

#[test]
fn item_version_accepts_minimum_and_just_below_limit_components() {
    for length in [1, FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES - 1] {
        let metadata =
            MetadataRevision::new(vec![b'm'; length]).expect("metadata revision should be valid");
        let content =
            ContentRevision::new(vec![b'c'; length]).expect("content revision should be valid");
        let version = MacosFileProviderItemVersion::new(metadata.clone(), content.clone())
            .expect("in-limit version should be valid");
        assert_eq!(version.metadata().as_bytes().len(), length);
        assert_eq!(version.content().as_bytes().len(), length);
        assert_eq!(version.into_parts(), (metadata, content));
    }
}

#[test]
fn item_version_rejects_each_component_one_byte_over_the_platform_limit() {
    let normal_metadata =
        MetadataRevision::from_slice(b"metadata").expect("metadata revision should be valid");
    let normal_content =
        ContentRevision::from_slice(b"content").expect("content revision should be valid");
    let too_large_metadata = MetadataRevision::new(vec![
        b'm';
        FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES
            + 1
    ])
    .expect("core revision should preserve opaque bytes");
    let too_large_content = ContentRevision::new(vec![
        b'c';
        FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES
            + 1
    ])
    .expect("core revision should preserve opaque bytes");
    assert!(matches!(
        MacosFileProviderItemVersion::new(too_large_metadata, normal_content.clone()),
        Err(MacosBridgeError::InvalidItemVersion {
            reason: "item version component exceeds the File Provider 128-byte limit"
        })
    ));
    assert!(matches!(
        MacosFileProviderItemVersion::new(normal_metadata, too_large_content),
        Err(MacosBridgeError::InvalidItemVersion {
            reason: "item version component exceeds the File Provider 128-byte limit"
        })
    ));
}

#[test]
fn directory_version_has_a_stable_nonempty_content_component() {
    let first = MacosFileProviderItemVersion::directory(
        MetadataRevision::from_slice(b"directory-m1").expect("revision should be valid"),
    )
    .expect("directory version should be valid");
    let second = MacosFileProviderItemVersion::directory(
        MetadataRevision::from_slice(b"directory-m2").expect("revision should be valid"),
    )
    .expect("directory version should be valid");
    assert_eq!(first.content(), second.content());
    assert_ne!(first.metadata(), second.metadata());
}
