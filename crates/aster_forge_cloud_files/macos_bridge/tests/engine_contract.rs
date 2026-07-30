use std::{num::NonZeroU64, sync::Arc};

use aster_forge_cloud_files_core::{
    BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudFilesBackend, CloudFilesCapabilities,
    CloudItem, CloudItemId, CloudItemKey, CloudItemPage, CloudMetadataBackend, CloudNamespaceId,
    CloudRootId, CloudScope, ContentReadRequest, ContentReadResponse, ContentRevision,
    MetadataRevision, PageCursor,
};
use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER, MacosBridgeError, MacosEnumerationPage,
    MacosEnumerationRequest, MacosEnumerationState, MacosFileProviderIdentifier,
    MacosFileProviderItemKind, MacosReadOnlyEngine,
};
use async_trait::async_trait;
use bytes::Bytes;

struct Backend {
    root: CloudItem,
    docs: CloudItem,
    file: CloudItem,
    contents: Bytes,
    fault: Fault,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    WrongIdentity,
    WrongNonRootIdentity,
    WrongNonRootRootRole,
    WrongParent,
    DuplicateName,
    WrongRevision,
    WrongOffset,
    ShortContent,
    WrongTotalSize,
    ReadError,
}

impl Backend {
    fn fixture(fault: Fault) -> Self {
        let scope = CloudScope::new(
            CloudNamespaceId::new("macos-engine").expect("namespace should be valid"),
            CloudRootId::new("default").expect("root should be valid"),
        );
        let root_key = key(&scope, "root");
        let docs_key = key(&scope, "docs");
        let file_key = key(&scope, "hello");
        let metadata = revision(b"metadata-v1");
        let content = content_revision(b"content-v1");
        let bytes = Bytes::from_static(b"hello from macOS File Provider\n");
        let root = CloudItem::directory(root_key.clone(), None, "root", metadata.clone())
            .expect("root should be valid");
        let docs = CloudItem::directory(
            docs_key,
            Some(root_key.item_id().clone()),
            "docs",
            metadata.clone(),
        )
        .expect("directory should be valid");
        let file = CloudItem::file(
            file_key,
            root_key.item_id().clone(),
            "hello.txt",
            metadata,
            CloudContentMetadata::new(
                content,
                None,
                u64::try_from(bytes.len()).expect("fixture size should fit u64"),
            ),
        )
        .expect("file should be valid");
        Self {
            root,
            docs,
            file,
            contents: bytes,
            fault,
        }
    }

    fn engine(self) -> MacosReadOnlyEngine<Self> {
        let root_key = self.root.key().clone();
        MacosReadOnlyEngine::new(Arc::new(self), root_key)
    }

    fn empty_file_fixture() -> Self {
        let mut backend = Self::fixture(Fault::None);
        backend.file = CloudItem::file(
            backend.file.key().clone(),
            backend.root.key().item_id().clone(),
            backend.file.name(),
            backend.file.metadata_revision().clone(),
            CloudContentMetadata::new(content_revision(b"empty-v1"), None, 0),
        )
        .expect("empty file should be valid");
        backend.contents = Bytes::new();
        backend
    }
}

#[async_trait]
impl CloudMetadataBackend for Backend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        if self.fault == Fault::WrongIdentity && key == self.root.key() {
            return Ok(self.file.clone());
        }
        if self.fault == Fault::WrongNonRootIdentity && key == self.file.key() {
            return Ok(self.docs.clone());
        }
        if self.fault == Fault::WrongNonRootRootRole && key == self.docs.key() {
            return CloudItem::directory(
                self.docs.key().clone(),
                None,
                self.docs.name(),
                self.docs.metadata_revision().clone(),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse));
        }
        if key == self.root.key() {
            Ok(self.root.clone())
        } else if key == self.docs.key() {
            Ok(self.docs.clone())
        } else if key == self.file.key() {
            Ok(self.file.clone())
        } else {
            Err(CloudBackendError::new(CloudBackendErrorKind::NotFound))
        }
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        if parent != self.root.key() {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        }
        if cursor.is_none() {
            let item = if self.fault == Fault::WrongParent {
                CloudItem::file(
                    self.file.key().clone(),
                    self.docs.key().item_id().clone(),
                    self.file.name(),
                    self.file.metadata_revision().clone(),
                    self.file
                        .content()
                        .cloned()
                        .expect("file should have content"),
                )
                .expect("malformed parent remains a valid core item")
            } else {
                self.file.clone()
            };
            return Ok(CloudItemPage::new(
                vec![item],
                Some(PageCursor::from_slice(b"page-2").expect("cursor should be valid")),
            ));
        }
        let second = if self.fault == Fault::DuplicateName {
            CloudItem::directory(
                self.docs.key().clone(),
                Some(self.root.key().item_id().clone()),
                self.file.name(),
                self.docs.metadata_revision().clone(),
            )
            .expect("duplicate name remains a valid core item")
        } else {
            self.docs.clone()
        };
        Ok(CloudItemPage::new(vec![second], None))
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
impl CloudContentBackend for Backend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        if self.fault == Fault::ReadError {
            return Err(CloudBackendError::retryable(
                CloudBackendErrorKind::TemporarilyUnavailable,
            ));
        }
        let revision = if self.fault == Fault::WrongRevision {
            content_revision(b"content-v2")
        } else {
            request.revision().clone()
        };
        let requested_range = match request.read_range() {
            aster_forge_cloud_files_core::ContentReadRange::Whole => None,
            aster_forge_cloud_files_core::ContentReadRange::Range(range) => Some(range),
        };
        let normal_offset = requested_range.map_or(0, |range| range.offset());
        let normal_bytes = requested_range.map_or_else(
            || self.contents.clone(),
            |range| {
                let start = usize::try_from(range.offset()).expect("fixture offset should fit");
                let length = usize::try_from(range.length()).expect("fixture length should fit");
                let end = start.saturating_add(length).min(self.contents.len());
                self.contents.slice(start..end)
            },
        );
        let (offset, bytes, total_size) = match self.fault {
            Fault::WrongOffset => (
                1,
                self.contents.slice(1..),
                u64::try_from(self.contents.len()).expect("fixture size should fit u64"),
            ),
            Fault::ShortContent => (
                0,
                self.contents.slice(..self.contents.len() - 1),
                u64::try_from(self.contents.len()).expect("fixture size should fit u64"),
            ),
            Fault::WrongTotalSize => (
                0,
                self.contents.clone(),
                u64::try_from(self.contents.len() + 1).expect("fixture size should fit u64"),
            ),
            _ => (normal_offset, normal_bytes, request.expected_size()),
        };
        ContentReadResponse::new(revision, offset, bytes, total_size)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

impl CloudFilesBackend for Backend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        CloudFilesCapabilities::default()
    }
}

#[tokio::test]
async fn root_item_paged_enumeration_and_revision_bound_fetch_work() {
    let engine = Backend::fixture(Fault::None).engine();
    let root = MacosFileProviderIdentifier::parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER)
        .expect("root identifier should parse");
    let root_item = engine.item(&root).await.expect("root item should load");
    assert_eq!(root_item.kind(), MacosFileProviderItemKind::Directory);
    assert_eq!(root_item.parent_identifier().as_str(), root.as_str());

    let mut enumeration = MacosEnumerationState::new(root.clone());
    let first = engine
        .enumerate(&enumeration.request().expect("first request should exist"))
        .await
        .expect("first page should load");
    let first = enumeration
        .accept_page(first)
        .expect("first page should advance state");
    assert_eq!(first.items()[0].filename(), "hello.txt");
    assert_eq!(first.items()[0].parent_identifier().as_str(), root.as_str());
    let next = first
        .next_page()
        .cloned()
        .expect("first page should continue");
    assert_eq!(
        enumeration
            .request()
            .expect("second request should exist")
            .page(),
        Some(&next)
    );
    let second = engine
        .enumerate(&enumeration.request().expect("second request should exist"))
        .await
        .expect("second page should load");
    let second = enumeration
        .accept_page(second)
        .expect("second page should finish state");
    assert_eq!(second.items()[0].filename(), "docs");
    assert!(second.next_page().is_none());
    assert!(enumeration.is_finished());
    assert!(matches!(
        enumeration.request(),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration requested another page after terminal completion"
        })
    ));

    let file = first.items()[0].identifier().clone();
    let requested = first.items()[0].version().content().clone();
    let plan = engine
        .prepare_content_fetch(&file, &requested)
        .await
        .expect("content plan should load");
    assert_eq!(plan.revision(), &requested);
    let chunk = engine
        .read_content_chunk(
            &plan,
            0,
            NonZeroU64::new(8).expect("chunk size should be non-zero"),
        )
        .await
        .expect("first chunk should load");
    assert_eq!(chunk.offset(), 0);
    assert_eq!(chunk.bytes(), &Bytes::from_static(b"hello fr"));
    let (offset, bytes) = chunk.into_parts();
    assert_eq!(offset, 0);
    assert_eq!(bytes, Bytes::from_static(b"hello fr"));
    let chunk = engine
        .read_content_chunk(
            &plan,
            8,
            NonZeroU64::new(64).expect("chunk size should be non-zero"),
        )
        .await
        .expect("tail chunk should load");
    assert_eq!(
        chunk.bytes(),
        &Bytes::from_static(b"om macOS File Provider\n")
    );
}

#[tokio::test]
async fn engine_accessors_clone_and_non_root_item_mapping_preserve_scope() {
    let backend = Backend::fixture(Fault::None);
    let root_key = backend.root.key().clone();
    let file_key = backend.file.key().clone();
    let engine = backend.engine();
    assert_eq!(engine.root_key(), &root_key);
    assert_eq!(engine.clone().root_key(), &root_key);

    let identifier =
        MacosFileProviderIdentifier::encode(&file_key).expect("identifier fixture should fit");
    let item = engine
        .item(&identifier)
        .await
        .expect("non-root item should load");
    assert_eq!(item.identifier().item_key().expect("item key"), &file_key);
    assert_eq!(
        item.parent_identifier().as_str(),
        FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER
    );
}

#[tokio::test]
async fn unsupported_system_containers_are_rejected_for_item_enumeration_and_content() {
    let engine = Backend::fixture(Fault::None).engine();
    for raw in [
        aster_forge_cloud_files_macos_bridge::FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER,
        aster_forge_cloud_files_macos_bridge::FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER,
    ] {
        let identifier =
            MacosFileProviderIdentifier::parse(raw).expect("system identifier should parse");
        assert!(matches!(
            engine.item(&identifier).await,
            Err(MacosBridgeError::UnsupportedSystemContainer)
        ));
        assert!(matches!(
            engine
                .enumerate(&MacosEnumerationRequest::new(identifier.clone(), None))
                .await,
            Err(MacosBridgeError::UnsupportedSystemContainer)
        ));
        assert!(matches!(
            engine
                .prepare_content_fetch(&identifier, &content_revision(b"content-v1"))
                .await,
            Err(MacosBridgeError::SystemContainerIsNotItem)
        ));
    }
}

#[tokio::test]
async fn enumeration_validates_parent_identity_kind_and_root_role() {
    let root = MacosFileProviderIdentifier::parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER)
        .expect("root identifier should parse");
    assert!(matches!(
        Backend::fixture(Fault::WrongIdentity)
            .engine()
            .enumerate(&MacosEnumerationRequest::new(root, None))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration parent lookup returned a different stable identity"
        })
    ));

    let backend = Backend::fixture(Fault::None);
    let file = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend
            .engine()
            .enumerate(&MacosEnumerationRequest::new(file, None))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration parent is not a directory"
        })
    ));

    let backend = Backend::fixture(Fault::WrongNonRootRootRole);
    let docs = MacosFileProviderIdentifier::encode(backend.docs.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend
            .engine()
            .enumerate(&MacosEnumerationRequest::new(docs, None))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration parent root shape did not match the requested container"
        })
    ));
}

#[tokio::test]
async fn item_rejects_non_root_backend_identity_and_root_role_drift() {
    let backend = Backend::fixture(Fault::WrongNonRootIdentity);
    let file = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend.engine().item(&file).await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "get_item returned a different scoped stable identity"
        })
    ));

    let backend = Backend::fixture(Fault::WrongNonRootRootRole);
    let docs = MacosFileProviderIdentifier::encode(backend.docs.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend.engine().item(&docs).await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "get_item root shape did not match the File Provider root role"
        })
    ));
}

#[tokio::test]
async fn content_fetch_rejects_directory_stale_metadata_and_identity_drift() {
    let backend = Backend::fixture(Fault::None);
    let docs = MacosFileProviderIdentifier::encode(backend.docs.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend
            .engine()
            .prepare_content_fetch(&docs, &content_revision(b"content-v1"))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "content fetch target is not a regular file"
        })
    ));

    let backend = Backend::fixture(Fault::None);
    let file = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    let result = backend
        .engine()
        .prepare_content_fetch(&file, &content_revision(b"stale-content"))
        .await;
    assert!(matches!(
        result,
        Err(MacosBridgeError::Backend(error))
            if error.kind() == CloudBackendErrorKind::PreconditionFailed
    ));

    let backend = Backend::fixture(Fault::WrongNonRootIdentity);
    let file = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    assert!(matches!(
        backend
            .engine()
            .prepare_content_fetch(&file, &content_revision(b"content-v1"))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "content metadata lookup returned a different stable identity"
        })
    ));
}

#[tokio::test]
async fn content_fetch_rejects_every_invalid_chunk_extent_and_propagates_backend_errors() {
    for fault in [
        Fault::WrongOffset,
        Fault::ShortContent,
        Fault::WrongTotalSize,
    ] {
        let backend = Backend::fixture(fault);
        let identifier = MacosFileProviderIdentifier::encode(backend.file.key())
            .expect("identifier fixture should fit");
        let revision = backend
            .file
            .content()
            .expect("file should have content")
            .revision()
            .clone();
        let engine = backend.engine();
        let plan = engine
            .prepare_content_fetch(&identifier, &revision)
            .await
            .expect("fault applies only to content bytes");
        assert!(matches!(
            engine
                .read_content_chunk(
                    &plan,
                    0,
                    NonZeroU64::new(64).expect("chunk size should be non-zero"),
                )
                .await,
            Err(MacosBridgeError::InvalidBackendResponse {
                reason: "content response violated the requested revision or chunk extent"
            })
        ));
    }

    let backend = Backend::fixture(Fault::ReadError);
    let identifier = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    let revision = backend
        .file
        .content()
        .expect("file should have content")
        .revision()
        .clone();
    let engine = backend.engine();
    let plan = engine
        .prepare_content_fetch(&identifier, &revision)
        .await
        .expect("metadata should load before read failure");
    let result = engine
        .read_content_chunk(
            &plan,
            0,
            NonZeroU64::new(64).expect("chunk size should be non-zero"),
        )
        .await;
    assert!(matches!(
        result,
        Err(MacosBridgeError::Backend(error))
            if error.kind() == CloudBackendErrorKind::TemporarilyUnavailable
                && error.is_retryable()
    ));
}

#[tokio::test]
async fn empty_file_fetch_is_a_zero_length_plan_without_a_read() {
    let backend = Backend::empty_file_fixture();
    let identifier = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    let revision = backend
        .file
        .content()
        .expect("file should have content")
        .revision()
        .clone();
    let engine = backend.engine();
    let fetched = engine
        .prepare_content_fetch(&identifier, &revision)
        .await
        .expect("empty file should fetch");
    assert_eq!(fetched.revision(), &revision);
    assert_eq!(fetched.size(), 0);
    assert!(matches!(
        engine
            .read_content_chunk(
                &fetched,
                0,
                NonZeroU64::new(1).expect("chunk size should be non-zero"),
            )
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "content chunk offset is outside the planned file"
        })
    ));
}

#[tokio::test]
async fn backend_not_found_classification_is_preserved() {
    let backend = Backend::fixture(Fault::None);
    let unknown = MacosFileProviderIdentifier::encode(&key(backend.root.key().scope(), "missing"))
        .expect("identifier fixture should fit");
    let result = backend.engine().item(&unknown).await;
    assert!(matches!(
        result,
        Err(MacosBridgeError::Backend(error))
            if error.kind() == CloudBackendErrorKind::NotFound
    ));
}

#[tokio::test]
async fn wrong_identity_parent_and_content_revision_are_rejected() {
    let root = MacosFileProviderIdentifier::parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER)
        .expect("root identifier should parse");
    assert!(matches!(
        Backend::fixture(Fault::WrongIdentity)
            .engine()
            .item(&root)
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "get_item returned a different scoped stable identity"
        })
    ));
    assert!(matches!(
        Backend::fixture(Fault::WrongParent)
            .engine()
            .enumerate(&MacosEnumerationRequest::new(root, None))
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumerated item escaped the requested parent scope"
        })
    ));

    let backend = Backend::fixture(Fault::WrongRevision);
    let identifier = MacosFileProviderIdentifier::encode(backend.file.key())
        .expect("identifier fixture should fit");
    let revision = backend
        .file
        .content()
        .expect("file should have content")
        .revision()
        .clone();
    let engine = backend.engine();
    let plan = engine
        .prepare_content_fetch(&identifier, &revision)
        .await
        .expect("metadata should load before revision drift");
    assert!(matches!(
        engine
            .read_content_chunk(
                &plan,
                0,
                NonZeroU64::new(64).expect("chunk size should be non-zero"),
            )
            .await,
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "content response violated the requested revision or chunk extent"
        })
    ));
}

#[test]
fn enumeration_page_rejects_duplicate_names_within_one_backend_page() {
    let backend = Backend::fixture(Fault::DuplicateName);
    let parent = backend.root.key().clone();
    let duplicate = CloudItem::directory(
        backend.docs.key().clone(),
        Some(parent.item_id().clone()),
        backend.file.name(),
        backend.docs.metadata_revision().clone(),
    )
    .expect("duplicate name remains a valid core item");
    assert!(matches!(
        MacosEnumerationPage::from_backend(
            &parent,
            &parent,
            CloudItemPage::new(vec![backend.file.clone(), duplicate], None)
        ),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration returned duplicate filenames"
        })
    ));
}

#[test]
fn enumeration_state_rejects_cross_page_duplicate_items_names_and_cursors() {
    let backend = Backend::fixture(Fault::None);
    let root = MacosFileProviderIdentifier::parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER)
        .expect("root identifier should parse");
    let first = MacosEnumerationPage::from_backend(
        backend.root.key(),
        backend.root.key(),
        CloudItemPage::new(
            vec![backend.file.clone()],
            Some(PageCursor::from_slice(b"repeat").expect("cursor should be valid")),
        ),
    )
    .expect("first page should be valid");
    let mut state = MacosEnumerationState::new(root.clone());
    state.accept_page(first).expect("first page should advance");

    let duplicate_item = MacosEnumerationPage::from_backend(
        backend.root.key(),
        backend.root.key(),
        CloudItemPage::new(vec![backend.file.clone()], None),
    )
    .expect("page-local item should be valid");
    assert!(matches!(
        state.accept_page(duplicate_item),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration repeated a persistent identifier across pages"
        })
    ));

    let duplicate_name_item = CloudItem::directory(
        backend.docs.key().clone(),
        Some(backend.root.key().item_id().clone()),
        backend.file.name(),
        backend.docs.metadata_revision().clone(),
    )
    .expect("duplicate name remains a valid core item");
    let duplicate_name = MacosEnumerationPage::from_backend(
        backend.root.key(),
        backend.root.key(),
        CloudItemPage::new(vec![duplicate_name_item], None),
    )
    .expect("page-local item should be valid");
    assert!(matches!(
        state.accept_page(duplicate_name),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration repeated a filename across pages"
        })
    ));

    let mut cursor_state = MacosEnumerationState::new(root);
    let first_cursor = MacosEnumerationPage::from_backend(
        backend.root.key(),
        backend.root.key(),
        CloudItemPage::new(
            Vec::new(),
            Some(PageCursor::from_slice(b"repeat").expect("cursor should be valid")),
        ),
    )
    .expect("first cursor page should be valid");
    cursor_state
        .accept_page(first_cursor)
        .expect("first cursor should advance");
    let repeated_cursor = MacosEnumerationPage::from_backend(
        backend.root.key(),
        backend.root.key(),
        CloudItemPage::new(
            Vec::new(),
            Some(PageCursor::from_slice(b"repeat").expect("cursor should be valid")),
        ),
    )
    .expect("second cursor page should be locally valid");
    assert!(matches!(
        cursor_state.accept_page(repeated_cursor),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration repeated a continuation cursor"
        })
    ));
}

fn key(scope: &CloudScope, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope.clone(),
        CloudItemId::new(item).expect("item should be valid"),
    )
}

fn revision(value: &[u8]) -> MetadataRevision {
    MetadataRevision::from_slice(value).expect("metadata revision should be valid")
}

fn content_revision(value: &[u8]) -> ContentRevision {
    ContentRevision::from_slice(value).expect("content revision should be valid")
}
