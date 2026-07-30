use std::time::Duration;

use aster_forge_cloud_files_core::{
    ByteRange, ChangeBatch, ChangeCursor, ChangePage, ChangeResetReason, CloudBackendError,
    CloudBackendErrorKind, CloudChange, CloudContentMetadata, CloudFilesCoreError, CloudItem,
    CloudItemId, CloudItemKey, CloudItemPage, CloudNamespaceId, CloudRootId, CloudScope,
    ContentDigest, ContentDigestAlgorithm, ContentReadRange, ContentReadRequest,
    ContentReadResponse, ContentRevision, DirectoryCookie, LocalContentGeneration,
    LocalContentReference, LocalContentSnapshot, MetadataRevision, PageCursor, RetryAdvice,
};
use bytes::Bytes;

fn scope() -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(" namespace/保留 ").expect("namespace fixture should be valid"),
        CloudRootId::new(" root/保留 ").expect("root fixture should be valid"),
    )
}

fn key(item_id: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope(),
        CloudItemId::new(item_id).expect("item id fixture should be valid"),
    )
}

fn metadata_revision(value: &[u8]) -> MetadataRevision {
    MetadataRevision::from_slice(value).expect("metadata revision fixture should be valid")
}

fn content_revision(value: &[u8]) -> ContentRevision {
    ContentRevision::from_slice(value).expect("content revision fixture should be valid")
}

fn content_metadata(size: u64) -> CloudContentMetadata {
    CloudContentMetadata::new(content_revision(b"content-v1"), None, size)
}

#[test]
fn string_identities_preserve_exact_values_and_support_all_access_paths() {
    let namespace = CloudNamespaceId::new(" namespace/保留 ")
        .expect("namespace fixture should preserve whitespace");
    let root = CloudRootId::new(" root/保留 ").expect("root fixture should preserve whitespace");
    let item = CloudItemId::new(" item/保留 ").expect("item fixture should preserve whitespace");

    assert_eq!(namespace.as_str(), " namespace/保留 ");
    assert_eq!(root.as_str(), " root/保留 ");
    assert_eq!(item.as_str(), " item/保留 ");
    assert_eq!(namespace.to_string(), " namespace/保留 ");
    assert_eq!(root.to_string(), " root/保留 ");
    assert_eq!(item.to_string(), " item/保留 ");
    assert_eq!(
        format!("{namespace:?}"),
        "CloudNamespaceId(\" namespace/保留 \")"
    );
    assert_eq!(format!("{root:?}"), "CloudRootId(\" root/保留 \")");
    assert_eq!(format!("{item:?}"), "CloudItemId(\" item/保留 \")");
    assert_eq!(namespace.into_string(), " namespace/保留 ");
    assert_eq!(root.into_string(), " root/保留 ");
    assert_eq!(item.into_string(), " item/保留 ");
}

#[test]
fn each_string_identity_reports_its_own_empty_field() {
    assert_eq!(
        CloudNamespaceId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud namespace id",
        })
    );
    assert_eq!(
        CloudRootId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud root id",
        })
    );
    assert_eq!(
        CloudItemId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud item id",
        })
    );
}

#[test]
fn scoped_identity_parts_round_trip_without_normalization() {
    let original_scope = scope();
    let original_key = CloudItemKey::new(
        original_scope.clone(),
        CloudItemId::new(" item ").expect("item fixture should be valid"),
    );

    assert_eq!(original_key.scope(), &original_scope);
    assert_eq!(original_key.item_id().as_str(), " item ");

    let (scope_part, item_part) = original_key.into_parts();
    let (namespace_part, root_part) = scope_part.into_parts();
    assert_eq!(namespace_part.into_string(), " namespace/保留 ");
    assert_eq!(root_part.into_string(), " root/保留 ");
    assert_eq!(item_part.into_string(), " item ");
}

#[test]
fn revisions_and_digest_support_slice_owned_debug_and_consume_paths() {
    let metadata = MetadataRevision::from_slice(&[0, 0xff, 1])
        .expect("metadata revision fixture should be valid");
    let content =
        ContentRevision::new(vec![0, 0xff, 2]).expect("content revision fixture should be valid");
    let algorithm = ContentDigestAlgorithm::new(" SHA-256 ")
        .expect("algorithm fixture should preserve exact spelling");
    let digest = ContentDigest::new(algorithm.clone(), vec![0, 1, 2, 3])
        .expect("digest fixture should be valid");

    assert_eq!(metadata.as_bytes(), &[0, 0xff, 1]);
    assert_eq!(content.as_bytes(), &[0, 0xff, 2]);
    assert_eq!(format!("{metadata:?}"), "MetadataRevision { byte_len: 3 }");
    assert_eq!(format!("{content:?}"), "ContentRevision { byte_len: 3 }");
    assert_eq!(algorithm.as_str(), " SHA-256 ");
    assert_eq!(digest.algorithm(), &algorithm);
    assert_eq!(digest.value(), &[0, 1, 2, 3]);
    assert_eq!(
        format!("{digest:?}"),
        "ContentDigest { algorithm: ContentDigestAlgorithm(\" SHA-256 \"), byte_len: 4 }"
    );

    assert_eq!(metadata.into_bytes(), vec![0, 0xff, 1]);
    assert_eq!(content.into_bytes(), vec![0, 0xff, 2]);
    let (algorithm, value) = digest.into_parts();
    assert_eq!(algorithm.into_string(), " SHA-256 ");
    assert_eq!(value, vec![0, 1, 2, 3]);
}

#[test]
fn revisions_and_digest_report_precise_empty_fields() {
    assert_eq!(
        MetadataRevision::from_slice(&[]),
        Err(CloudFilesCoreError::EmptyValue {
            field: "metadata revision",
        })
    );
    assert_eq!(
        ContentRevision::from_slice(&[]),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content revision",
        })
    );
    assert_eq!(
        ContentDigestAlgorithm::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content digest algorithm",
        })
    );
    assert_eq!(
        ContentDigest::new(
            ContentDigestAlgorithm::new("sha-256").expect("algorithm fixture should be valid"),
            Vec::new(),
        ),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content digest value",
        })
    );
}

#[test]
fn every_cursor_preserves_bytes_and_supports_all_access_paths() {
    let raw = [0, 0xff, b'/', 0];
    let page = PageCursor::from_slice(&raw).expect("page cursor fixture should be valid");
    let change = ChangeCursor::from_slice(&raw).expect("change cursor fixture should be valid");
    let directory =
        DirectoryCookie::from_slice(&raw).expect("directory cookie fixture should be valid");

    assert_eq!(page.as_bytes(), raw);
    assert_eq!(change.as_bytes(), raw);
    assert_eq!(directory.as_bytes(), raw);
    assert_eq!(format!("{page:?}"), "PageCursor { byte_len: 4 }");
    assert_eq!(format!("{change:?}"), "ChangeCursor { byte_len: 4 }");
    assert_eq!(format!("{directory:?}"), "DirectoryCookie { byte_len: 4 }");
    assert_eq!(page.into_bytes(), raw);
    assert_eq!(change.into_bytes(), raw);
    assert_eq!(directory.into_bytes(), raw);
}

#[test]
fn every_cursor_reports_its_own_empty_field() {
    assert_eq!(
        PageCursor::new(Vec::new()),
        Err(CloudFilesCoreError::EmptyValue {
            field: "page cursor",
        })
    );
    assert_eq!(
        ChangeCursor::new(Vec::new()),
        Err(CloudFilesCoreError::EmptyValue {
            field: "change cursor",
        })
    );
    assert_eq!(
        DirectoryCookie::new(Vec::new()),
        Err(CloudFilesCoreError::EmptyValue {
            field: "directory cookie",
        })
    );
}

#[test]
fn content_metadata_item_page_and_change_batch_round_trip_all_parts() {
    let digest = ContentDigest::new(
        ContentDigestAlgorithm::new("sha-256").expect("algorithm fixture should be valid"),
        vec![7; 32],
    )
    .expect("digest fixture should be valid");
    let content = CloudContentMetadata::new(content_revision(b"content-v2"), Some(digest), 9);
    assert_eq!(content.size(), 9);
    assert_eq!(
        content.digest().map(ContentDigest::value),
        Some(&[7; 32][..])
    );
    let (revision, digest, size) = content.into_parts();
    assert_eq!(revision.into_bytes(), b"content-v2");
    assert_eq!(digest.expect("digest should be present").value(), &[7; 32]);
    assert_eq!(size, 9);

    let item = CloudItem::file(
        key("file"),
        CloudItemId::new("parent").expect("parent fixture should be valid"),
        "file.bin",
        metadata_revision(b"metadata-v1"),
        content_metadata(9),
    )
    .expect("file fixture should be valid");
    let page_cursor = PageCursor::from_slice(b"next-page").expect("cursor fixture should be valid");
    let page = CloudItemPage::new(vec![item.clone()], Some(page_cursor));
    assert_eq!(page.items(), std::slice::from_ref(&item));
    assert_eq!(
        page.next_cursor().map(PageCursor::as_bytes),
        Some(&b"next-page"[..])
    );
    let (items, cursor) = page.into_parts();
    assert_eq!(items, vec![item.clone()]);
    assert_eq!(
        cursor.expect("cursor should be present").into_bytes(),
        b"next-page"
    );

    let upsert = CloudChange::Upsert { item: item.clone() };
    let delete = CloudChange::Delete {
        key: key("deleted"),
        previous_parent_id: Some(
            CloudItemId::new("old-parent").expect("parent fixture should be valid"),
        ),
        metadata_revision: Some(metadata_revision(b"delete-v1")),
    };
    assert_eq!(upsert.key(), item.key());
    assert_eq!(delete.key().item_id().as_str(), "deleted");

    let batch = ChangeBatch::new(
        vec![upsert.clone(), delete.clone()],
        ChangeCursor::from_slice(b"next-change").expect("cursor fixture should be valid"),
        true,
    );
    assert_eq!(batch.changes(), &[upsert.clone(), delete.clone()]);
    assert_eq!(batch.next_cursor().as_bytes(), b"next-change");
    assert!(batch.has_more());
    let (changes, cursor, has_more) = batch.into_parts();
    assert_eq!(changes, vec![upsert, delete]);
    assert_eq!(cursor.into_bytes(), b"next-change");
    assert!(has_more);

    assert_eq!(
        ChangePage::ResetRequired {
            reason: ChangeResetReason::BackendStateRebuilt,
        },
        ChangePage::ResetRequired {
            reason: ChangeResetReason::BackendStateRebuilt,
        }
    );
}

#[test]
fn move_rejects_empty_name_and_self_parent_for_files_and_directories() {
    let file = CloudItem::file(
        key("file"),
        CloudItemId::new("parent").expect("parent fixture should be valid"),
        "file.bin",
        metadata_revision(b"metadata-v1"),
        content_metadata(1),
    )
    .expect("file fixture should be valid");
    assert_eq!(
        file.clone().moved(
            CloudItemId::new("parent").expect("parent fixture should be valid"),
            "",
            metadata_revision(b"metadata-v2"),
        ),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud item name",
        })
    );
    assert_eq!(
        file.moved(
            CloudItemId::new("file").expect("item fixture should be valid"),
            "renamed.bin",
            metadata_revision(b"metadata-v2"),
        ),
        Err(CloudFilesCoreError::InvalidItemShape {
            reason: "an item must not be its own parent",
        })
    );

    let directory = CloudItem::directory(
        key("directory"),
        Some(CloudItemId::new("parent").expect("parent fixture should be valid")),
        "directory",
        metadata_revision(b"metadata-v1"),
    )
    .expect("directory fixture should be valid");
    assert_eq!(
        directory.moved(
            CloudItemId::new("directory").expect("item fixture should be valid"),
            "renamed",
            metadata_revision(b"metadata-v2"),
        ),
        Err(CloudFilesCoreError::InvalidItemShape {
            reason: "an item must not be its own parent",
        })
    );
}

#[test]
fn byte_range_rejects_zero_and_overflow_and_reports_exact_boundaries() {
    assert_eq!(
        ByteRange::new(0, 0),
        Err(CloudFilesCoreError::InvalidByteRange {
            reason: "range length must be greater than zero",
        })
    );
    assert_eq!(
        ByteRange::new(u64::MAX, 1),
        Err(CloudFilesCoreError::InvalidByteRange {
            reason: "range end exceeds u64",
        })
    );
    let last_byte = ByteRange::new(u64::MAX - 1, 1).expect("last-byte range should be valid");
    assert_eq!(last_byte.offset(), u64::MAX - 1);
    assert_eq!(last_byte.length(), 1);
    assert_eq!(last_byte.end_exclusive(), u64::MAX);
}

#[test]
fn content_read_request_accessors_and_response_parts_preserve_exact_values() {
    let item_key = key("file");
    let revision = content_revision(b"content-v1");
    let range = ByteRange::new(2, 3).expect("range fixture should be valid");
    let request = ContentReadRequest::range(item_key.clone(), revision.clone(), 5, range);
    assert_eq!(request.key(), &item_key);
    assert_eq!(request.revision(), &revision);
    assert_eq!(request.expected_size(), 5);
    assert_eq!(request.read_range(), ContentReadRange::Range(range));

    let response = ContentReadResponse::new(revision.clone(), 2, Bytes::from_static(b"cde"), 5)
        .expect("response fixture should be valid");
    assert_eq!(response.revision(), &revision);
    assert_eq!(response.offset(), 2);
    assert_eq!(response.bytes().as_ref(), b"cde");
    assert_eq!(response.byte_len(), 3);
    assert_eq!(response.total_size(), 5);
    assert!(!response.is_complete_file());
    request
        .validate_response(&response)
        .expect("exact range response should validate");

    let (part_revision, offset, bytes, total_size) = response.into_parts();
    assert_eq!(part_revision, revision);
    assert_eq!(offset, 2);
    assert_eq!(bytes.as_ref(), b"cde");
    assert_eq!(total_size, 5);
}

#[test]
fn content_response_rejects_offsets_and_ranges_outside_logical_size() {
    let revision = content_revision(b"content-v1");
    assert_eq!(
        ContentReadResponse::new(revision.clone(), 6, Bytes::new(), 5),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response bytes exceed the logical file size",
        })
    );
    assert_eq!(
        ContentReadResponse::new(revision.clone(), 4, Bytes::from_static(b"xx"), 5),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response bytes exceed the logical file size",
        })
    );
    assert_eq!(
        ContentReadResponse::new(revision, u64::MAX, Bytes::from_static(b"x"), u64::MAX),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response byte range exceeds u64",
        })
    );
}

#[test]
fn whole_content_response_validation_covers_empty_offset_short_and_revision_edges() {
    let item_key = key("file");
    let revision = content_revision(b"content-v1");
    let request = ContentReadRequest::whole(item_key, revision.clone(), 4);

    let wrong_revision = ContentReadResponse::new(
        content_revision(b"content-v2"),
        0,
        Bytes::from_static(b"data"),
        4,
    )
    .expect("response fixture should be valid");
    assert_eq!(
        request.validate_response(&wrong_revision),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response revision does not match the requested revision",
        })
    );

    let nonzero_offset =
        ContentReadResponse::new(revision.clone(), 1, Bytes::from_static(b"ata"), 4)
            .expect("response fixture should be valid");
    assert_eq!(
        request.validate_response(&nonzero_offset),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "whole-file response does not contain the complete file",
        })
    );

    let short = ContentReadResponse::new(revision.clone(), 0, Bytes::from_static(b"dat"), 4)
        .expect("response fixture should be valid");
    assert_eq!(
        request.validate_response(&short),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "whole-file response does not contain the complete file",
        })
    );

    let empty_request = ContentReadRequest::whole(key("empty"), revision.clone(), 0);
    let empty = ContentReadResponse::new(revision, 0, Bytes::new(), 0)
        .expect("empty response fixture should be valid");
    assert!(empty.is_complete_file());
    empty_request
        .validate_response(&empty)
        .expect("empty whole-file response should validate");
}

#[test]
fn range_response_validation_covers_wrong_offset_lengths_eof_and_beyond_eof() {
    let revision = content_revision(b"content-v1");
    let request = ContentReadRequest::range(
        key("file"),
        revision.clone(),
        5,
        ByteRange::new(3, 4).expect("range fixture should be valid"),
    );

    let wrong_offset = ContentReadResponse::new(revision.clone(), 2, Bytes::from_static(b"cde"), 5)
        .expect("response fixture should be valid");
    assert_eq!(
        request.validate_response(&wrong_offset),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "range response starts at a different offset",
        })
    );

    let too_short = ContentReadResponse::new(revision.clone(), 3, Bytes::from_static(b"d"), 5)
        .expect("response fixture should be valid");
    assert_eq!(
        request.validate_response(&too_short),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "range response length does not match the requested extent",
        })
    );

    let truncated_at_eof =
        ContentReadResponse::new(revision.clone(), 3, Bytes::from_static(b"de"), 5)
            .expect("response fixture should be valid");
    request
        .validate_response(&truncated_at_eof)
        .expect("EOF truncation should validate");

    let exact_eof_request = ContentReadRequest::range(
        key("file"),
        revision.clone(),
        5,
        ByteRange::new(5, 3).expect("range fixture should be valid"),
    );
    let exact_eof = ContentReadResponse::new(revision.clone(), 5, Bytes::new(), 5)
        .expect("response fixture should be valid");
    exact_eof_request
        .validate_response(&exact_eof)
        .expect("range starting exactly at EOF should validate as empty");

    let beyond_eof_request = ContentReadRequest::range(
        key("file"),
        revision.clone(),
        5,
        ByteRange::new(6, 3).expect("range fixture should be valid"),
    );
    let beyond_eof = ContentReadResponse::new(revision, 6, Bytes::new(), 6)
        .expect("internally valid response fixture should be valid");
    assert_eq!(
        beyond_eof_request.validate_response(&beyond_eof),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response size does not match metadata for the requested revision",
        })
    );
}

#[test]
fn every_backend_error_kind_preserves_each_retry_classification() {
    let kinds = [
        CloudBackendErrorKind::NotFound,
        CloudBackendErrorKind::AuthenticationRequired,
        CloudBackendErrorKind::PermissionDenied,
        CloudBackendErrorKind::Conflict,
        CloudBackendErrorKind::PreconditionFailed,
        CloudBackendErrorKind::InvalidRequest,
        CloudBackendErrorKind::RateLimited,
        CloudBackendErrorKind::TemporarilyUnavailable,
        CloudBackendErrorKind::Unsupported,
        CloudBackendErrorKind::InvalidResponse,
        CloudBackendErrorKind::Internal,
    ];

    for kind in kinds {
        let never = CloudBackendError::new(kind);
        assert_eq!(never.kind(), kind);
        assert_eq!(never.retry_advice(), RetryAdvice::Never);
        assert!(!never.is_retryable());
        assert_eq!(
            never.to_string(),
            format!("cloud-files backend operation failed: {kind:?}")
        );

        let retry = CloudBackendError::retryable(kind);
        assert_eq!(retry.kind(), kind);
        assert_eq!(retry.retry_advice(), RetryAdvice::Retry);
        assert!(retry.is_retryable());

        let after = CloudBackendError::retry_after(kind, Duration::from_millis(250));
        assert_eq!(after.kind(), kind);
        assert_eq!(
            after.retry_advice(),
            RetryAdvice::RetryAfter(Duration::from_millis(250))
        );
        assert!(after.is_retryable());
    }
}

#[test]
fn local_content_values_preserve_opaque_reference_generation_and_digest() {
    assert_eq!(
        LocalContentReference::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "local content reference",
        })
    );
    let reference = LocalContentReference::new(" local://opaque/保留 ")
        .expect("reference fixture should preserve exact value");
    assert_eq!(reference.as_str(), " local://opaque/保留 ");
    assert_eq!(
        format!("{reference:?}"),
        "LocalContentReference { byte_len: 23 }"
    );
    assert_eq!(reference.clone().into_string(), " local://opaque/保留 ");

    let generation = LocalContentGeneration::new(1).expect("generation one should be valid");
    assert_eq!(generation.get(), 1);
    let max_generation =
        LocalContentGeneration::new(u64::MAX).expect("maximum generation should be valid");
    assert_eq!(max_generation.get(), u64::MAX);

    let digest = ContentDigest::new(
        ContentDigestAlgorithm::new("sha-256").expect("algorithm fixture should be valid"),
        vec![9; 32],
    )
    .expect("digest fixture should be valid");
    let item_key = key("dirty-file");
    let snapshot = LocalContentSnapshot::new(
        item_key.clone(),
        generation,
        reference,
        123,
        Some(digest.clone()),
    );
    assert_eq!(snapshot.item_key(), &item_key);
    assert_eq!(snapshot.generation(), generation);
    assert_eq!(snapshot.reference().as_str(), " local://opaque/保留 ");
    assert_eq!(snapshot.size(), 123);
    assert_eq!(snapshot.digest(), Some(&digest));
}
