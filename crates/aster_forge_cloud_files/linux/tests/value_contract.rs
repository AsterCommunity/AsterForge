use std::time::Duration;

use aster_forge_cloud_files_core::{
    CloudBackendErrorKind, CloudFilesStoreErrorKind, CloudItemId, CloudItemKey, CloudNamespaceId,
    CloudRootId, CloudScope, ContentRevision, LocalContentGeneration, LocalContentReference,
    LocalContentSnapshot, SessionGeneration,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCloudFilesError, LinuxDispatchRejection,
    LinuxErrorCode, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord, LinuxInodeTable,
    LinuxMountConfig, LinuxNamespaceMutationStoreErrorKind, LinuxRequestDispatcher,
    LinuxWriteCommit, LinuxWriteOpenRequest, LinuxWriteSession, LinuxWriteSessionId,
    backend_error_code, namespace_store_error_code, writeback_store_error_code,
};

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

fn record(scope: &CloudScope, item: &str, inode: u64, generation: u64) -> LinuxInodeRecord {
    LinuxInodeRecord::new(
        key(scope, item),
        LinuxInode::new(inode).expect("inode should be valid"),
        LinuxInodeGeneration::new(generation).expect("generation should be valid"),
    )
}

#[test]
fn inode_and_generation_cover_zero_one_and_max_boundaries() {
    assert!(matches!(
        LinuxInode::new(0),
        Err(LinuxCloudFilesError::ZeroInode)
    ));
    assert_eq!(
        LinuxInode::new(1).expect("one should be valid"),
        LINUX_ROOT_INODE
    );
    assert_eq!(
        LinuxInode::new(u64::MAX)
            .expect("maximum inode should be valid")
            .get(),
        u64::MAX
    );
    assert!(matches!(
        LinuxInodeGeneration::new(0),
        Err(LinuxCloudFilesError::ZeroGeneration)
    ));
    assert_eq!(
        LinuxInodeGeneration::new(u64::MAX)
            .expect("maximum generation should be valid")
            .get(),
        u64::MAX
    );
}

#[test]
fn inode_table_rejects_root_scope_inode_and_item_collisions() {
    let active = scope("provider", "root-a");
    let other = scope("provider", "root-b");
    let generation = LinuxInodeGeneration::new(1).expect("generation should be valid");
    let wrong_root = LinuxInodeRecord::new(
        key(&active, "root"),
        LinuxInode::new(2).expect("inode should be valid"),
        generation,
    );
    assert!(matches!(
        LinuxInodeTable::new(wrong_root, []),
        Err(LinuxCloudFilesError::RootInodeMismatch)
    ));

    let root = LinuxInodeRecord::new(key(&active, "root"), LINUX_ROOT_INODE, generation);
    assert!(matches!(
        LinuxInodeTable::new(root.clone(), [record(&other, "foreign", 2, 1)]),
        Err(LinuxCloudFilesError::ScopeMismatch)
    ));
    assert!(matches!(
        LinuxInodeTable::new(root.clone(), [record(&active, "child", 1, 1)]),
        Err(LinuxCloudFilesError::RootInodeMismatch)
    ));
    assert!(matches!(
        LinuxInodeTable::new(
            root.clone(),
            [
                record(&active, "left", 2, 1),
                record(&active, "right", 2, 2)
            ]
        ),
        Err(LinuxCloudFilesError::DuplicateInode { inode: 2 })
    ));
    assert!(matches!(
        LinuxInodeTable::new(
            root,
            [record(&active, "same", 2, 1), record(&active, "same", 3, 2)]
        ),
        Err(LinuxCloudFilesError::DuplicateItem)
    ));
}

#[test]
fn inode_record_accessors_and_table_lookups_preserve_exact_values() {
    let active = scope("provider", "root-a");
    let root = LinuxInodeRecord::new(
        key(&active, "root"),
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(9).expect("generation should be valid"),
    );
    let child = record(&active, "child", 42, 17);
    let table = LinuxInodeTable::new(root.clone(), [child.clone()]).expect("table should be valid");
    assert_eq!(table.scope(), &active);
    assert_eq!(table.by_key(root.key()), Some(&root));
    assert_eq!(table.by_inode(LINUX_ROOT_INODE), Some(&root));
    assert_eq!(table.by_key(child.key()), Some(&child));
    assert_eq!(table.by_inode(child.inode()), Some(&child));
    assert_eq!(table.records().count(), 1);
    assert!(
        table
            .by_inode(LinuxInode::new(99).expect("inode should be valid"))
            .is_none()
    );

    let expected_key = child.key().clone();
    let (actual_key, inode, generation) = child.into_parts();
    assert_eq!(actual_key, expected_key);
    assert_eq!(inode.get(), 42);
    assert_eq!(generation.get(), 17);
}

#[test]
fn every_backend_error_kind_has_a_stable_linux_classification() {
    let cases = [
        (CloudBackendErrorKind::NotFound, LinuxErrorCode::NotFound),
        (
            CloudBackendErrorKind::AuthenticationRequired,
            LinuxErrorCode::AccessDenied,
        ),
        (
            CloudBackendErrorKind::PermissionDenied,
            LinuxErrorCode::AccessDenied,
        ),
        (CloudBackendErrorKind::Conflict, LinuxErrorCode::Stale),
        (
            CloudBackendErrorKind::PreconditionFailed,
            LinuxErrorCode::Stale,
        ),
        (
            CloudBackendErrorKind::InvalidRequest,
            LinuxErrorCode::InvalidArgument,
        ),
        (CloudBackendErrorKind::RateLimited, LinuxErrorCode::TryAgain),
        (
            CloudBackendErrorKind::TemporarilyUnavailable,
            LinuxErrorCode::TryAgain,
        ),
        (
            CloudBackendErrorKind::Unsupported,
            LinuxErrorCode::NotSupported,
        ),
        (CloudBackendErrorKind::InvalidResponse, LinuxErrorCode::Io),
        (CloudBackendErrorKind::Internal, LinuxErrorCode::Io),
    ];
    for (kind, expected) in cases {
        assert_eq!(backend_error_code(kind), expected, "{kind:?}");
    }
    let store_cases = [
        (CloudFilesStoreErrorKind::NotFound, LinuxErrorCode::NotFound),
        (CloudFilesStoreErrorKind::Conflict, LinuxErrorCode::Stale),
        (
            CloudFilesStoreErrorKind::InvalidTransition,
            LinuxErrorCode::Io,
        ),
        (
            CloudFilesStoreErrorKind::PersistenceFailure,
            LinuxErrorCode::Io,
        ),
    ];
    for (kind, expected) in store_cases {
        assert_eq!(writeback_store_error_code(kind), expected, "{kind:?}");
    }
    let namespace_cases = [
        (
            LinuxNamespaceMutationStoreErrorKind::NotFound,
            LinuxErrorCode::NotFound,
        ),
        (
            LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
            LinuxErrorCode::AlreadyExists,
        ),
        (
            LinuxNamespaceMutationStoreErrorKind::Fenced,
            LinuxErrorCode::Stale,
        ),
        (
            LinuxNamespaceMutationStoreErrorKind::Conflict,
            LinuxErrorCode::Io,
        ),
        (
            LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
            LinuxErrorCode::Io,
        ),
    ];
    for (kind, expected) in namespace_cases {
        assert_eq!(namespace_store_error_code(kind), expected, "{kind:?}");
    }
    assert_eq!(
        LinuxCloudFilesError::MissingInodeRecord.error_code(),
        LinuxErrorCode::Io
    );
    let local_cases = [
        (
            LinuxCloudFilesError::UnknownInode { inode: 99 },
            LinuxErrorCode::NotFound,
        ),
        (LinuxCloudFilesError::StaleHandle, LinuxErrorCode::Stale),
        (
            LinuxCloudFilesError::InvalidName { reason: "fixture" },
            LinuxErrorCode::InvalidArgument,
        ),
        (LinuxCloudFilesError::NotDirectory, LinuxErrorCode::NotFound),
        (LinuxCloudFilesError::NotFile, LinuxErrorCode::NotFound),
        (LinuxCloudFilesError::ZeroInode, LinuxErrorCode::Io),
    ];
    for (error, expected) in local_cases {
        assert_eq!(error.error_code(), expected);
    }
    assert_eq!(
        LinuxDispatchRejection::Saturated.error_code(),
        LinuxErrorCode::TryAgain
    );
    assert_eq!(
        LinuxDispatchRejection::Closing.error_code(),
        LinuxErrorCode::Io
    );
}

#[test]
fn writeback_values_preserve_exact_opaque_and_revision_bound_fields() {
    assert!(LinuxWriteSessionId::new("").is_err());
    let session_id =
        LinuxWriteSessionId::new("session-secret").expect("session id should be valid");
    assert_eq!(session_id.as_str(), "session-secret");
    assert!(!format!("{session_id:?}").contains("session-secret"));

    let active = scope("provider", "root-a");
    let item_key = key(&active, "item-a");
    let revision =
        ContentRevision::from_slice(b"content-v1").expect("content revision should be valid");
    let mount_generation = SessionGeneration::new(11).expect("generation should be valid");
    let request =
        LinuxWriteOpenRequest::new(item_key.clone(), revision.clone(), 17, mount_generation);
    assert_eq!(request.key(), &item_key);
    assert_eq!(request.base_revision(), &revision);
    assert_eq!(request.base_size(), 17);
    assert_eq!(request.session_generation(), mount_generation);

    let session = LinuxWriteSession::new(session_id, item_key.clone(), 19, mount_generation);
    assert_eq!(session.id().as_str(), "session-secret");
    assert_eq!(session.key(), &item_key);
    assert_eq!(session.size(), 19);
    assert_eq!(session.session_generation(), mount_generation);
    assert!(session.snapshot().is_none());

    let snapshot = LocalContentSnapshot::new(
        item_key.clone(),
        LocalContentGeneration::new(3).expect("generation should be valid"),
        LocalContentReference::new("local-reference").expect("reference should be valid"),
        19,
        None,
    );
    let recovered = LinuxWriteSession::from_recovered(
        LinuxWriteSessionId::new("recovered-secret").expect("session id should be valid"),
        snapshot.clone(),
        mount_generation,
    );
    assert_eq!(recovered.key(), &item_key);
    assert_eq!(recovered.size(), 19);
    assert_eq!(recovered.session_generation(), mount_generation);
    assert_eq!(recovered.snapshot(), Some(&snapshot));
    let commit = LinuxWriteCommit::new(snapshot.clone());
    assert_eq!(commit.snapshot(), &snapshot);
    assert_eq!(commit.into_snapshot(), snapshot);
}

#[test]
fn attribute_and_mount_configuration_validate_all_public_boundaries() {
    let policy = LinuxAttributePolicy::new(10, 11, 0o4644, 0o2755, Duration::from_secs(3))
        .expect("valid Unix modes should be accepted");
    assert_eq!(policy.uid(), 10);
    assert_eq!(policy.gid(), 11);
    assert_eq!(policy.file_permissions(), 0o4644);
    assert_eq!(policy.directory_permissions(), 0o2755);
    assert_eq!(policy.block_size(), 4096);
    assert_eq!(policy.cache_ttl(), Duration::from_secs(3));
    assert_eq!(policy.fallback_time(), std::time::SystemTime::UNIX_EPOCH);
    assert!(LinuxAttributePolicy::new(0, 0, 0o10000, 0, Duration::ZERO).is_err());
    assert!(LinuxAttributePolicy::new(0, 0, 0, 0o10000, Duration::ZERO).is_err());

    for invalid in ["", "bad,name", "bad\0name"] {
        assert!(LinuxMountConfig::new(invalid).is_err(), "{invalid:?}");
    }
    let config = LinuxMountConfig::new("forge-cloud")
        .expect("filesystem name should be valid")
        .with_subtype("cloud-files")
        .expect("subtype should be valid")
        .with_kernel_threads(4, true)
        .expect("thread count should be valid");
    assert_eq!(config.filesystem_name(), "forge-cloud");
    assert_eq!(config.subtype(), Some("cloud-files"));
    assert_eq!(config.kernel_threads(), Some(4));
    assert!(config.clone_fd());
    assert!(
        LinuxMountConfig::new("forge")
            .expect("filesystem name should be valid")
            .with_subtype("bad,subtype")
            .is_err()
    );
    assert!(
        LinuxMountConfig::new("forge")
            .expect("filesystem name should be valid")
            .with_kernel_threads(0, false)
            .is_err()
    );
}

#[tokio::test]
async fn dispatcher_validates_capacity_and_counts_panics_as_terminal_tasks() {
    assert!(LinuxRequestDispatcher::new(tokio::runtime::Handle::current(), 0).is_err());
    let dispatcher = LinuxRequestDispatcher::new(tokio::runtime::Handle::current(), 1)
        .expect("dispatcher should be valid");
    dispatcher
        .reserve()
        .expect("reservation should succeed")
        .spawn(async { panic!("synthetic task panic") });
    for _ in 0..16 {
        if dispatcher.metrics().completed == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let metrics = dispatcher.metrics();
    assert_eq!(metrics.accepted, 1);
    assert_eq!(metrics.completed, 1);
    assert_eq!(metrics.panicked, 1);
    assert_eq!(metrics.saturated, 0);
    assert_eq!(metrics.closing, 0);

    dispatcher.close();
    assert!(matches!(
        dispatcher.reserve(),
        Err(LinuxDispatchRejection::Closing)
    ));

    dispatcher.spawn_cleanup(async {});
    for _ in 0..16 {
        if dispatcher.metrics().completed == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let metrics = dispatcher.metrics();
    assert_eq!(metrics.accepted, 2);
    assert_eq!(metrics.completed, 2);
}
