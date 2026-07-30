use std::{sync::Arc, time::Duration};

use aster_forge_cloud_files_core::{
    BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudFilesBackend, CloudFilesCapabilities,
    CloudItem, CloudItemId, CloudItemKey, CloudItemPage, CloudMetadataBackend, CloudNamespaceId,
    CloudRootId, CloudScope, ContentReadRequest, ContentReadResponse, ContentRevision,
    MetadataRevision, PageCursor,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCloudFilesError, LinuxInode, LinuxInodeGeneration,
    LinuxInodeRecord, LinuxInodeTable, LinuxReadOnlyEngine,
};
use async_trait::async_trait;
use bytes::Bytes;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    RepeatedCursor,
    ChildOutsideParent,
    WrongGetIdentity,
    RootHasParent,
    NonRootIsRoot,
    WrongContentRevision,
}

struct FaultBackend {
    fault: Fault,
    root: CloudItem,
    file: CloudItem,
    other: CloudItem,
}

impl FaultBackend {
    fn new(fault: Fault) -> Self {
        let scope = CloudScope::new(
            CloudNamespaceId::new("fault-backend").expect("namespace should be valid"),
            CloudRootId::new("root").expect("root should be valid"),
        );
        let root_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("root-item").expect("root item should be valid"),
        );
        let file_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("file-item").expect("file item should be valid"),
        );
        let other_key = CloudItemKey::new(
            scope,
            CloudItemId::new("other-item").expect("other item should be valid"),
        );
        let revision =
            ContentRevision::from_slice(b"content-v1").expect("revision should be valid");
        let metadata = MetadataRevision::from_slice(b"metadata-v1")
            .expect("metadata revision should be valid");
        let root = CloudItem::directory(root_key.clone(), None, "root", metadata.clone())
            .expect("root should be valid");
        let file = CloudItem::file(
            file_key,
            root_key.item_id().clone(),
            "file.txt",
            metadata.clone(),
            CloudContentMetadata::new(revision, None, 4),
        )
        .expect("file should be valid");
        let other = CloudItem::directory(other_key, None, "other", metadata)
            .expect("other item should be valid");
        Self {
            fault,
            root,
            file,
            other,
        }
    }

    fn engine(self) -> LinuxReadOnlyEngine<Self> {
        let root_record = LinuxInodeRecord::new(
            self.root.key().clone(),
            LINUX_ROOT_INODE,
            LinuxInodeGeneration::new(1).expect("generation should be valid"),
        );
        let file_record = LinuxInodeRecord::new(
            self.file.key().clone(),
            LinuxInode::new(2).expect("inode should be valid"),
            LinuxInodeGeneration::new(1).expect("generation should be valid"),
        );
        let other_record = LinuxInodeRecord::new(
            self.other.key().clone(),
            LinuxInode::new(3).expect("inode should be valid"),
            LinuxInodeGeneration::new(1).expect("generation should be valid"),
        );
        LinuxReadOnlyEngine::new(
            Arc::new(self),
            Arc::new(
                LinuxInodeTable::new(root_record, [file_record, other_record])
                    .expect("fixture table should be valid"),
            ),
            LinuxAttributePolicy::new(1, 1, 0o444, 0o555, Duration::ZERO)
                .expect("policy should be valid"),
        )
    }
}

#[async_trait]
impl CloudMetadataBackend for FaultBackend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        if self.fault == Fault::WrongGetIdentity && key == self.root.key() {
            return Ok(self.file.clone());
        }
        if self.fault == Fault::RootHasParent && key == self.root.key() {
            return CloudItem::directory(
                self.root.key().clone(),
                Some(self.other.key().item_id().clone()),
                self.root.name(),
                self.root.metadata_revision().clone(),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse));
        }
        if key == self.root.key() {
            Ok(self.root.clone())
        } else if key == self.file.key() {
            Ok(self.file.clone())
        } else if key == self.other.key() {
            Ok(self.other.clone())
        } else {
            Err(CloudBackendError::new(CloudBackendErrorKind::NotFound))
        }
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        _cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        if parent != self.root.key() {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        }
        match self.fault {
            Fault::RepeatedCursor => Ok(CloudItemPage::new(
                Vec::new(),
                Some(PageCursor::from_slice(b"repeat").expect("cursor should be valid")),
            )),
            Fault::ChildOutsideParent => {
                let child = CloudItem::file(
                    self.file.key().clone(),
                    self.other.key().item_id().clone(),
                    "file.txt",
                    self.file.metadata_revision().clone(),
                    self.file
                        .content()
                        .cloned()
                        .expect("fixture file should have content"),
                )
                .expect("malformed backend child can still satisfy core item shape");
                Ok(CloudItemPage::new(vec![child], None))
            }
            Fault::WrongGetIdentity
            | Fault::RootHasParent
            | Fault::NonRootIsRoot
            | Fault::WrongContentRevision => Ok(CloudItemPage::new(vec![self.file.clone()], None)),
        }
    }

    async fn changes_since(
        &self,
        _scope: &CloudScope,
        _cursor: Option<&ChangeCursor>,
    ) -> BackendResult<ChangePage> {
        Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported))
    }
}

#[async_trait]
impl CloudContentBackend for FaultBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        if request.key() != self.file.key() {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        }
        let revision = match self.fault {
            Fault::WrongContentRevision => {
                ContentRevision::from_slice(b"content-v2").expect("revision should be valid")
            }
            _ => self
                .file
                .content()
                .expect("fixture file should have content")
                .revision()
                .clone(),
        };
        ContentReadResponse::new(revision, 0, Bytes::from_static(b"data"), 4)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

impl CloudFilesBackend for FaultBackend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        CloudFilesCapabilities::default()
    }
}

#[tokio::test]
async fn repeated_backend_page_cursor_is_rejected_before_directory_handle_creation() {
    let engine = FaultBackend::new(Fault::RepeatedCursor).engine();
    assert!(matches!(
        engine.open_directory(LINUX_ROOT_INODE).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "directory pagination repeated a continuation cursor"
        })
    ));
}

#[tokio::test]
async fn backend_child_outside_requested_parent_is_rejected() {
    let engine = FaultBackend::new(Fault::ChildOutsideParent).engine();
    assert!(matches!(
        engine.lookup(LINUX_ROOT_INODE, "file.txt").await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "directory child escaped the requested parent scope"
        })
    ));
}

#[tokio::test]
async fn get_item_identity_substitution_is_rejected() {
    let engine = FaultBackend::new(Fault::WrongGetIdentity).engine();
    assert!(matches!(
        engine.getattr(LINUX_ROOT_INODE).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "get_item returned a different scoped stable identity"
        })
    ));
}

#[tokio::test]
async fn restored_root_inode_requires_a_root_shaped_backend_item() {
    let engine = FaultBackend::new(Fault::RootHasParent).engine();
    assert!(matches!(
        engine.getattr(LINUX_ROOT_INODE).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "get_item root shape did not match the restored inode role"
        })
    ));
}

#[tokio::test]
async fn restored_non_root_inode_rejects_a_root_shaped_backend_item() {
    let engine = FaultBackend::new(Fault::NonRootIsRoot).engine();
    assert!(matches!(
        engine
            .getattr(LinuxInode::new(3).expect("inode should be valid"))
            .await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "get_item root shape did not match the restored inode role"
        })
    ));
}

#[tokio::test]
async fn content_revision_substitution_is_rejected_after_open_fence() {
    let engine = FaultBackend::new(Fault::WrongContentRevision).engine();
    let handle = engine
        .open_file(LinuxInode::new(2).expect("inode should be valid"))
        .await
        .expect("file should open with the original metadata revision");
    assert!(matches!(
        engine
            .read_file(
                LinuxInode::new(2).expect("inode should be valid"),
                handle,
                0,
                4
            )
            .await,
        Err(LinuxCloudFilesError::InvalidBackendResponse {
            reason: "content response violated the requested revision or range"
        })
    ));
}
