#[path = "../../core/tests/support/mod.rs"]
mod support;

use std::{collections::HashSet, sync::Arc, time::Duration};

use aster_forge_cloud_files_core::{
    CloudItem, CloudItemKey, CloudItemKind, CloudMetadataBackend, PageCursor,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCloudFilesError, LinuxDirectoryHandle,
    LinuxDispatchRejection, LinuxFileHandle, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord,
    LinuxInodeTable, LinuxNodeKind, LinuxReadOnlyEngine, LinuxRequestDispatcher,
    validate_linux_name,
};
use support::SyntheticBackend;

async fn all_items(backend: &SyntheticBackend) -> Vec<CloudItem> {
    let mut pending = vec![backend.root_key().clone()];
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    while let Some(key) = pending.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let item = backend
            .get_item(&key)
            .await
            .expect("synthetic item should exist");
        if item.kind() == CloudItemKind::Directory {
            let mut cursor: Option<PageCursor> = None;
            loop {
                let page = backend
                    .list_children(item.key(), cursor.as_ref())
                    .await
                    .expect("synthetic child page should be valid");
                let (children, next) = page.into_parts();
                pending.extend(children.into_iter().map(|child| child.key().clone()));
                let Some(next) = next else {
                    break;
                };
                cursor = Some(next);
            }
        }
        items.push(item);
    }
    items
}

async fn engine() -> LinuxReadOnlyEngine<SyntheticBackend> {
    let backend = Arc::new(SyntheticBackend::read_only());
    let root_key = backend.root_key().clone();
    let items = all_items(&backend).await;
    let root = items
        .iter()
        .find(|item| item.key() == &root_key)
        .expect("root should be enumerated");
    let root_record = LinuxInodeRecord::new(
        root.key().clone(),
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    let records = items
        .iter()
        .filter(|item| item.key() != root.key())
        .enumerate()
        .map(|(index, item)| {
            let inode = u64::try_from(index + 2).expect("fixture inode should fit");
            LinuxInodeRecord::new(
                item.key().clone(),
                LinuxInode::new(inode).expect("fixture inode should be valid"),
                LinuxInodeGeneration::new(inode).expect("fixture generation should be valid"),
            )
        })
        .collect::<Vec<_>>();
    let table = Arc::new(
        LinuxInodeTable::new(root_record, records).expect("fixture inode table should be valid"),
    );
    let attributes = LinuxAttributePolicy::new(1000, 1001, 0o444, 0o555, Duration::ZERO)
        .expect("fixture policy should be valid");
    LinuxReadOnlyEngine::new(backend, table, attributes)
}

#[test]
fn inode_records_are_scope_bound_unique_and_rooted_at_one() {
    let scope = aster_forge_cloud_files_core::CloudScope::new(
        aster_forge_cloud_files_core::CloudNamespaceId::new("linux-test")
            .expect("namespace should be valid"),
        aster_forge_cloud_files_core::CloudRootId::new("root").expect("root should be valid"),
    );
    let root = CloudItemKey::new(
        scope.clone(),
        aster_forge_cloud_files_core::CloudItemId::new("root-item").expect("item should be valid"),
    );
    let child = CloudItemKey::new(
        scope,
        aster_forge_cloud_files_core::CloudItemId::new("child-item").expect("item should be valid"),
    );
    let root_record = LinuxInodeRecord::new(
        root,
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(7).expect("generation should be valid"),
    );
    let child_record = LinuxInodeRecord::new(
        child,
        LinuxInode::new(2).expect("inode should be valid"),
        LinuxInodeGeneration::new(8).expect("generation should be valid"),
    );
    let table = LinuxInodeTable::new(root_record.clone(), [child_record.clone()])
        .expect("table should be valid");
    assert_eq!(table.root(), &root_record);
    assert_eq!(table.by_inode(child_record.inode()), Some(&child_record));

    let duplicate = LinuxInodeTable::new(root_record, [child_record.clone(), child_record]);
    assert!(matches!(
        duplicate,
        Err(LinuxCloudFilesError::DuplicateItem)
    ));
}

#[test]
fn linux_name_boundaries_reject_path_and_reserved_dot_components() {
    for name in ["", ".", "..", "child/name", "nul\0name"] {
        assert!(validate_linux_name(name).is_err(), "{name:?} should fail");
    }
    assert!(validate_linux_name("alpha.txt").is_ok());
    assert!(validate_linux_name("目录").is_ok());
}

#[tokio::test]
async fn paged_directory_snapshot_and_revision_bound_range_read_work() {
    let engine = engine().await;
    let cloned = engine.clone();
    assert_eq!(cloned.inode_table().scope(), engine.inode_table().scope());
    assert_eq!(engine.attribute_policy().uid(), 1000);
    let docs = engine
        .lookup(LINUX_ROOT_INODE, "docs")
        .await
        .expect("docs should resolve");
    assert_eq!(docs.attributes().permissions(), 0o555);
    assert_eq!(docs.attributes().kind(), LinuxNodeKind::Directory);
    assert_eq!(docs.attributes().size(), 0);
    assert_eq!(docs.attributes().blocks(), 0);
    assert_eq!(docs.attributes().links(), 2);
    assert_eq!(docs.attributes().uid(), 1000);
    assert_eq!(docs.attributes().gid(), 1001);
    assert_eq!(docs.attributes().block_size(), 4096);
    assert_eq!(docs.attributes().time(), std::time::SystemTime::UNIX_EPOCH);
    assert_eq!(docs.key().item_id().as_str(), "folder-docs");
    assert_ne!(docs.generation().get(), 0);

    let directory = engine
        .open_directory(docs.attributes().inode())
        .await
        .expect("directory should open");
    let snapshot = engine
        .directory_snapshot(docs.attributes().inode(), directory)
        .expect("directory snapshot should remain valid");
    assert_eq!(snapshot.entries().len(), 1);
    assert_eq!(snapshot.directory(), docs.attributes().inode());
    assert_eq!(snapshot.parent(), LINUX_ROOT_INODE);
    assert_eq!(snapshot.entries()[0].name(), "alpha-renamed.txt");
    assert_eq!(snapshot.entries()[0].kind(), LinuxNodeKind::File);
    assert_ne!(snapshot.entries()[0].inode().get(), 0);
    assert_ne!(snapshot.entries()[0].generation().get(), 0);
    assert_ne!(directory.get(), 0);

    let file = engine
        .lookup(docs.attributes().inode(), "alpha-renamed.txt")
        .await
        .expect("moved file should resolve through its current parent");
    let handle = engine
        .open_file(file.attributes().inode())
        .await
        .expect("file should open");
    assert_ne!(handle.get(), 0);
    assert_eq!(file.attributes().kind(), LinuxNodeKind::File);
    assert_eq!(file.attributes().links(), 1);
    assert_eq!(file.attributes().blocks(), 1);
    assert!(
        engine
            .read_file(docs.attributes().inode(), handle, 0, 1)
            .await
            .is_err()
    );
    assert!(
        engine
            .read_file(file.attributes().inode(), handle, 0, 0)
            .await
            .expect("zero-sized reads should succeed")
            .is_empty()
    );
    let bytes = engine
        .read_file(file.attributes().inode(), handle, 6, 7)
        .await
        .expect("range read should use the captured revision");
    assert_eq!(&bytes[..], b"version");
    assert!(
        engine
            .read_file(
                file.attributes().inode(),
                handle,
                file.attributes().size(),
                4
            )
            .await
            .expect("EOF read should be valid")
            .is_empty()
    );

    engine
        .release_file(handle)
        .expect("release should remove the open handle");
    assert!(matches!(
        engine.release_file(handle),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
    assert!(matches!(
        engine
            .read_file(file.attributes().inode(), handle, 0, 1)
            .await,
        Err(LinuxCloudFilesError::StaleHandle)
    ));
    engine
        .release_directory(directory)
        .expect("directory release should remove its snapshot");
    assert!(matches!(
        engine.directory_snapshot(docs.attributes().inode(), directory),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
    assert!(matches!(
        engine.open_file(docs.attributes().inode()).await,
        Err(LinuxCloudFilesError::NotFile)
    ));
    assert!(matches!(
        engine.open_directory(file.attributes().inode()).await,
        Err(LinuxCloudFilesError::NotDirectory)
    ));
    assert!(matches!(
        engine.lookup(file.attributes().inode(), "child").await,
        Err(LinuxCloudFilesError::NotDirectory)
    ));
    assert!(matches!(
        engine.lookup(LINUX_ROOT_INODE, "missing.txt").await,
        Err(LinuxCloudFilesError::Backend(_))
    ));
    assert!(matches!(
        engine
            .getattr(LinuxInode::new(999).expect("inode should be valid"))
            .await,
        Err(LinuxCloudFilesError::UnknownInode { inode: 999 })
    ));

    let root_directory = engine
        .open_directory(LINUX_ROOT_INODE)
        .await
        .expect("root directory should open");
    let root_snapshot = engine
        .directory_snapshot(LINUX_ROOT_INODE, root_directory)
        .expect("root snapshot should be valid");
    assert_eq!(root_snapshot.parent(), LINUX_ROOT_INODE);
    assert!(matches!(
        engine.directory_snapshot(docs.attributes().inode(), root_directory),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
    engine
        .release_directory(root_directory)
        .expect("root directory should release");
    assert!(matches!(
        engine.release_directory(root_directory),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
}

#[tokio::test]
async fn missing_restored_mapping_is_rejected_before_kernel_exposure() {
    let backend = Arc::new(SyntheticBackend::read_only());
    let root_record = LinuxInodeRecord::new(
        backend.root_key().clone(),
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    let engine = LinuxReadOnlyEngine::new(
        backend,
        Arc::new(LinuxInodeTable::new(root_record, []).expect("root table should be valid")),
        LinuxAttributePolicy::default(),
    );
    assert!(matches!(
        engine.lookup(LINUX_ROOT_INODE, "docs").await,
        Err(LinuxCloudFilesError::MissingInodeRecord)
    ));
}

#[tokio::test]
async fn dispatcher_is_bounded_and_rejects_closing_work_without_blocking() {
    let dispatcher = LinuxRequestDispatcher::new(tokio::runtime::Handle::current(), 1)
        .expect("dispatcher should be valid");
    let reservation = dispatcher.reserve().expect("first request should reserve");
    assert!(matches!(
        dispatcher.reserve(),
        Err(LinuxDispatchRejection::Saturated)
    ));
    reservation.spawn(async {});
    tokio::task::yield_now().await;
    assert_eq!(dispatcher.metrics().completed, 1);
    assert_eq!(dispatcher.metrics().accepted, 1);
    assert_eq!(dispatcher.metrics().saturated, 1);

    dispatcher.close();
    dispatcher.close();
    assert!(matches!(
        dispatcher.reserve(),
        Err(LinuxDispatchRejection::Closing)
    ));
    assert!(dispatcher.is_closing());
    assert_eq!(dispatcher.metrics().closing, 1);
}

#[test]
fn zero_native_handles_are_stale_not_valid_file_or_directory_handles() {
    assert!(matches!(
        LinuxFileHandle::new(0),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
    assert!(matches!(
        LinuxDirectoryHandle::new(0),
        Err(LinuxCloudFilesError::StaleHandle)
    ));
}
