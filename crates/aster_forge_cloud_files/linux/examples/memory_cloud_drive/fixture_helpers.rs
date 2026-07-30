fn key(scope: &CloudScope, item: &str) -> aster_forge_cloud_files_core::Result<CloudItemKey> {
    Ok(CloudItemKey::new(scope.clone(), CloudItemId::new(item)?))
}

fn metadata_revision(value: &str) -> aster_forge_cloud_files_core::Result<MetadataRevision> {
    MetadataRevision::from_slice(value.as_bytes())
}

fn content_revision(value: &str) -> aster_forge_cloud_files_core::Result<ContentRevision> {
    ContentRevision::from_slice(value.as_bytes())
}

fn file(
    key: CloudItemKey,
    parent: &CloudItemKey,
    name: &str,
    revision: &str,
    bytes: &[u8],
) -> aster_forge_cloud_files_core::Result<CloudItem> {
    CloudItem::file(
        key,
        parent.item_id().clone(),
        name,
        metadata_revision(&format!("{revision}-metadata"))?,
        CloudContentMetadata::new(
            content_revision(revision)?,
            None,
            u64::try_from(bytes.len()).map_err(|_| {
                aster_forge_cloud_files_core::CloudFilesCoreError::InvalidContentResponse {
                    reason: "example content length does not fit u64",
                }
            })?,
        ),
    )
}
