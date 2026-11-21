//! Package-local read-only backend for the Linux engine contract.
use std::collections::HashMap;

use aster_forge_cloud_files_core::{
    Alignment, BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudFilesBackend, CloudFilesCapabilities,
    CloudItem, CloudItemId, CloudItemKey, CloudItemKind, CloudItemPage, CloudMetadataBackend,
    CloudNamespaceId, CloudRootId, CloudScope, ContentReadRange, ContentReadRequest,
    ContentReadResponse, ContentRevision, EnumerationCapabilities, HydrationCapabilities,
    MetadataRevision, PageCursor, RangeHydrationCapabilities,
};
use async_trait::async_trait;
use bytes::Bytes;

pub struct SyntheticBackend {
    root: CloudItemKey,
    items: Vec<CloudItem>,
    contents: HashMap<CloudItemKey, Bytes>,
}

impl SyntheticBackend {
    pub fn read_only() -> Self {
        let scope = CloudScope::new(
            CloudNamespaceId::new("linux-engine-test").unwrap(),
            CloudRootId::new("root-a").unwrap(),
        );
        let key = |id| CloudItemKey::new(scope.clone(), CloudItemId::new(id).unwrap());
        let root = key("root-item");
        let docs = key("folder-docs");
        let metadata = || MetadataRevision::from_slice(b"metadata-v1").unwrap();
        let mut items = vec![
            CloudItem::directory(root.clone(), None, "root", metadata()).unwrap(),
            CloudItem::directory(
                docs.clone(),
                Some(root.item_id().clone()),
                "docs",
                metadata(),
            )
            .unwrap(),
        ];
        let mut contents = HashMap::new();
        for (id, parent, name, bytes) in [
            (
                "file-alpha",
                &docs,
                "alpha-renamed.txt",
                b"alpha version two".as_slice(),
            ),
            ("file-charlie", &root, "charlie.txt", b"charlie".as_slice()),
            ("file-delta", &root, "delta.txt", b"delta".as_slice()),
        ] {
            let file = key(id);
            items.push(
                CloudItem::file(
                    file.clone(),
                    parent.item_id().clone(),
                    name,
                    metadata(),
                    CloudContentMetadata::new(
                        ContentRevision::from_slice(id.as_bytes()).unwrap(),
                        None,
                        u64::try_from(bytes.len()).unwrap(),
                    ),
                )
                .unwrap(),
            );
            contents.insert(file, Bytes::copy_from_slice(bytes));
        }
        Self {
            root,
            items,
            contents,
        }
    }

    pub fn root_key(&self) -> &CloudItemKey {
        &self.root
    }
}

#[async_trait]
impl CloudMetadataBackend for SyntheticBackend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        self.items
            .iter()
            .find(|item| item.key() == key)
            .cloned()
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        if self.get_item(parent).await?.kind() != CloudItemKind::Directory {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let prefix = format!("{}:", parent.item_id().as_str());
        let offset = cursor.map_or(Ok(0), |cursor| {
            std::str::from_utf8(cursor.as_bytes())
                .ok()
                .and_then(|value| value.strip_prefix(&prefix))
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))
        })?;
        let children: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.parent_id() == Some(parent.item_id()))
            .cloned()
            .collect();
        if offset > children.len() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let end = offset.saturating_add(2).min(children.len());
        let next = (end < children.len())
            .then(|| PageCursor::new(format!("{prefix}{end}").into_bytes()).unwrap());
        Ok(CloudItemPage::new(children[offset..end].to_vec(), next))
    }

    async fn changes_since(
        &self,
        _: &CloudScope,
        _: Option<&ChangeCursor>,
    ) -> BackendResult<ChangePage> {
        Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported))
    }
}

#[async_trait]
impl CloudContentBackend for SyntheticBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        let item = self.get_item(request.key()).await?;
        let revision = item.content().unwrap().revision();
        if revision != request.revision() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let bytes = &self.contents[request.key()];
        let total = u64::try_from(bytes.len()).unwrap();
        let (offset, content) = match request.read_range() {
            ContentReadRange::Whole => (0, bytes.clone()),
            ContentReadRange::Range(range) if range.offset() <= total => (
                range.offset(),
                bytes.slice(
                    usize::try_from(range.offset()).unwrap()
                        ..usize::try_from(range.end_exclusive().min(total)).unwrap(),
                ),
            ),
            ContentReadRange::Range(_) => {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidRequest,
                ));
            }
        };
        let response = ContentReadResponse::new(revision.clone(), offset, content, total).unwrap();
        request
            .validate_response(&response)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        Ok(response)
    }
}

impl CloudFilesBackend for SyntheticBackend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        CloudFilesCapabilities {
            enumeration: EnumerationCapabilities {
                paged: true,
                stable_snapshot: true,
            },
            hydration: HydrationCapabilities {
                whole_file: true,
                range: Some(RangeHydrationCapabilities {
                    alignment: Alignment::ONE,
                    partial_cancel: false,
                }),
                ..HydrationCapabilities::default()
            },
            ..CloudFilesCapabilities::default()
        }
    }
}
