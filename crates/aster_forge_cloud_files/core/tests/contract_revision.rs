use std::any::TypeId;

use aster_forge_cloud_files_core::{
    ContentDigest, ContentDigestAlgorithm, ContentRevision, MetadataRevision,
};

#[test]
fn metadata_and_content_revisions_are_distinct_opaque_types() {
    assert_ne!(
        TypeId::of::<MetadataRevision>(),
        TypeId::of::<ContentRevision>()
    );

    let metadata = MetadataRevision::new(b"revision-1".to_vec())
        .expect("metadata revision fixture should be valid");
    let content = ContentRevision::new(b"revision-1".to_vec())
        .expect("content revision fixture should be valid");

    assert_eq!(metadata.as_bytes(), content.as_bytes());
}

#[test]
fn revisions_are_equality_tokens_without_ordering_or_digest_semantics() {
    let etag = ContentRevision::new(b"strong-etag".to_vec())
        .expect("content revision fixture should be valid");
    let same_etag = ContentRevision::from_slice(b"strong-etag")
        .expect("content revision fixture should be valid");

    assert_eq!(etag, same_etag);

    let digest = ContentDigest::new(
        ContentDigestAlgorithm::new("sha-256").expect("algorithm fixture should be valid"),
        vec![0x42; 32],
    )
    .expect("digest fixture should be valid");

    assert_eq!(digest.algorithm().as_str(), "sha-256");
    assert_eq!(digest.value(), &[0x42; 32]);
    assert_ne!(etag.as_bytes(), digest.value());
}

#[test]
fn empty_revision_and_digest_values_are_rejected() {
    assert!(MetadataRevision::new(Vec::new()).is_err());
    assert!(ContentRevision::new(Vec::new()).is_err());
    assert!(ContentDigestAlgorithm::new("").is_err());

    let algorithm =
        ContentDigestAlgorithm::new("sha-256").expect("algorithm fixture should be valid");
    assert!(ContentDigest::new(algorithm, Vec::new()).is_err());
}
