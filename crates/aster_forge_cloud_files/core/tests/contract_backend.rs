mod support;

use std::time::Duration;

use aster_forge_cloud_files_core::{
    ChangeCursor, CloudBackendError, CloudBackendErrorKind, CloudContentBackend, CloudFilesBackend,
    CloudFilesCoreError, CloudItemId, CloudItemKey, CloudMetadataBackend, CloudNamespaceId,
    CloudRootId, CloudScope, ContentReadRequest, ContentReadResponse, ContentRevision, RetryAdvice,
};
use bytes::Bytes;

use support::{SyntheticBackend, SyntheticProfile, range};

fn assert_backend_object_safe(_: &dyn CloudFilesBackend) {}

#[test]
fn provisional_backend_port_is_object_safe() {
    let backend = SyntheticBackend::full();
    assert_backend_object_safe(&backend);
}

#[tokio::test]
async fn revision_bound_whole_and_range_reads_return_exact_bytes() {
    let backend = SyntheticBackend::full();
    let file = backend.moved_file();
    let content = file
        .content()
        .expect("synthetic file should have content metadata");
    let revision = content.revision().clone();

    let whole_request =
        ContentReadRequest::whole(file.key().clone(), revision.clone(), content.size());
    let whole = backend
        .read_content(&whole_request)
        .await
        .expect("whole read should succeed");
    assert_eq!(whole.bytes().as_ref(), b"alpha version two");
    assert!(whole.is_complete_file());
    whole_request
        .validate_response(&whole)
        .expect("whole response should satisfy request");

    let range_request =
        ContentReadRequest::range(file.key().clone(), revision, content.size(), range(6, 7));
    let partial = backend
        .read_content(&range_request)
        .await
        .expect("range read should succeed");
    assert_eq!(partial.offset(), 6);
    assert_eq!(partial.bytes().as_ref(), b"version");
    assert!(!partial.is_complete_file());
    range_request
        .validate_response(&partial)
        .expect("range response should satisfy request");
}

#[tokio::test]
async fn stale_content_revision_is_a_precondition_failure() {
    let backend = SyntheticBackend::full();
    let request = ContentReadRequest::whole(
        backend.moved_file().key().clone(),
        aster_forge_cloud_files_core::ContentRevision::from_slice(b"stale")
            .expect("stale revision fixture should be valid"),
        backend
            .moved_file()
            .content()
            .expect("synthetic file should have content metadata")
            .size(),
    );

    let error = backend
        .read_content(&request)
        .await
        .expect_err("stale revision should fail");
    assert_eq!(error.kind(), CloudBackendErrorKind::PreconditionFailed);
    assert!(!error.is_retryable());
}

#[test]
fn response_size_must_match_metadata_for_the_requested_revision() {
    let backend = SyntheticBackend::full();
    let file = backend.moved_file();
    let content = file
        .content()
        .expect("synthetic file should have content metadata");
    let request = ContentReadRequest::whole(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
    );
    let response = ContentReadResponse::new(
        content.revision().clone(),
        0,
        Bytes::from_static(b"short"),
        5,
    )
    .expect("internally consistent response fixture should be valid");

    assert_eq!(
        request.validate_response(&response),
        Err(CloudFilesCoreError::InvalidContentResponse {
            reason: "response size does not match metadata for the requested revision",
        })
    );
}

#[tokio::test]
async fn limited_backend_reports_anchored_changes_and_range_reads_as_unsupported() {
    let backend = SyntheticBackend::limited();
    assert_eq!(backend.profile(), SyntheticProfile::Limited);
    assert!(backend.capabilities().changes.anchored.is_none());
    assert!(backend.capabilities().changes.snapshot_diff);
    assert!(backend.capabilities().hydration.range.is_none());

    let change_error = backend
        .changes_since(backend.scope(), None)
        .await
        .expect_err("limited backend should reject anchored changes");
    assert_eq!(change_error.kind(), CloudBackendErrorKind::Unsupported);

    let file = backend.moved_file();
    let content = file
        .content()
        .expect("synthetic file should have content metadata");
    let request = ContentReadRequest::range(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
        range(0, 1),
    );
    let read_error = backend
        .read_content(&request)
        .await
        .expect_err("limited backend should reject range reads");
    assert_eq!(read_error.kind(), CloudBackendErrorKind::Unsupported);
}

#[test]
fn read_only_backend_has_no_mutation_capabilities() {
    let backend = SyntheticBackend::read_only();
    assert_eq!(backend.profile(), SyntheticProfile::ReadOnly);
    assert_eq!(
        backend.capabilities().mutations,
        aster_forge_cloud_files_core::MutationCapabilities::default()
    );
    assert!(backend.capabilities().hydration.whole_file);
    assert!(backend.capabilities().hydration.range.is_some());
}

#[test]
fn backend_errors_keep_classification_separate_from_retry_advice() {
    let unavailable = CloudBackendError::retryable(CloudBackendErrorKind::TemporarilyUnavailable);
    assert_eq!(unavailable.retry_advice(), RetryAdvice::Retry);
    assert!(unavailable.is_retryable());

    let throttled =
        CloudBackendError::retry_after(CloudBackendErrorKind::RateLimited, Duration::from_secs(30));
    assert_eq!(
        throttled.retry_advice(),
        RetryAdvice::RetryAfter(Duration::from_secs(30))
    );

    let permission = CloudBackendError::new(CloudBackendErrorKind::PermissionDenied);
    assert_eq!(permission.retry_advice(), RetryAdvice::Never);
    assert!(!permission.is_retryable());
}

#[tokio::test]
async fn missing_items_parents_scopes_and_content_are_not_found() {
    let backend = SyntheticBackend::full();
    let missing_key = CloudItemKey::new(
        backend.scope().clone(),
        CloudItemId::new("missing").expect("item fixture should be valid"),
    );

    assert_eq!(
        backend
            .get_item(&missing_key)
            .await
            .expect_err("missing item should fail")
            .kind(),
        CloudBackendErrorKind::NotFound
    );
    assert_eq!(
        backend
            .list_children(&missing_key, None)
            .await
            .expect_err("missing parent should fail")
            .kind(),
        CloudBackendErrorKind::NotFound
    );
    let missing_scope = CloudScope::new(
        CloudNamespaceId::new("other-provider").expect("namespace fixture should be valid"),
        CloudRootId::new("other-root").expect("root fixture should be valid"),
    );
    assert_eq!(
        backend
            .changes_since(&missing_scope, None)
            .await
            .expect_err("unknown scope should fail")
            .kind(),
        CloudBackendErrorKind::NotFound
    );
    let missing_read = ContentReadRequest::whole(
        missing_key,
        ContentRevision::from_slice(b"missing-revision").expect("revision fixture should be valid"),
        0,
    );
    assert_eq!(
        backend
            .read_content(&missing_read)
            .await
            .expect_err("missing content should fail")
            .kind(),
        CloudBackendErrorKind::NotFound
    );
}

#[tokio::test]
async fn file_cannot_be_enumerated_as_a_directory() {
    let backend = SyntheticBackend::full();
    assert_eq!(
        backend
            .list_children(backend.moved_file().key(), None)
            .await
            .expect_err("regular file enumeration should fail")
            .kind(),
        CloudBackendErrorKind::InvalidRequest
    );
}

#[tokio::test]
async fn change_cursor_rejects_malformed_and_out_of_bounds_positions() {
    let backend = SyntheticBackend::full();
    for bytes in [
        &b"not-a-change-cursor"[..],
        &[0xff][..],
        &b"change:not-a-number"[..],
        &b"change:999"[..],
    ] {
        let cursor = ChangeCursor::from_slice(bytes).expect("cursor fixture should be valid");
        assert_eq!(
            backend
                .changes_since(backend.scope(), Some(&cursor))
                .await
                .expect_err("invalid cursor should fail")
                .kind(),
            CloudBackendErrorKind::InvalidRequest
        );
    }
}

#[tokio::test]
async fn range_reads_truncate_at_eof_but_reject_offsets_beyond_eof() {
    let backend = SyntheticBackend::full();
    let file = backend.moved_file();
    let content = file
        .content()
        .expect("synthetic file should have content metadata");

    let truncated_request = ContentReadRequest::range(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
        range(content.size() - 2, 10),
    );
    let truncated = backend
        .read_content(&truncated_request)
        .await
        .expect("range crossing EOF should truncate");
    assert_eq!(truncated.offset(), content.size() - 2);
    assert_eq!(truncated.byte_len(), 2);
    truncated_request
        .validate_response(&truncated)
        .expect("truncated response should validate");

    let eof_request = ContentReadRequest::range(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
        range(content.size(), 1),
    );
    let eof = backend
        .read_content(&eof_request)
        .await
        .expect("range at EOF should return empty bytes");
    assert_eq!(eof.offset(), content.size());
    assert_eq!(eof.byte_len(), 0);

    let beyond_request = ContentReadRequest::range(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
        range(content.size() + 1, 1),
    );
    assert_eq!(
        backend
            .read_content(&beyond_request)
            .await
            .expect_err("range beyond EOF should fail")
            .kind(),
        CloudBackendErrorKind::InvalidRequest
    );
}
