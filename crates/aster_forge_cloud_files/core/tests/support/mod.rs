// Each integration-test crate imports the same fixture module but exercises a different subset.
#![allow(dead_code)]

pub mod content_storage;
pub mod failure_injection;
pub mod hydration;
pub mod store;

use std::collections::HashMap;

use aster_forge_cloud_files_core::{
    Alignment, AnchoredChangeCapabilities, BackendResult, ByteRange, ChangeBatch,
    ChangeCapabilities, ChangeCursor, ChangePage, ChangeResetReason, CloudBackendError,
    CloudBackendErrorKind, CloudContentBackend, CloudContentMetadata, CloudFilesBackend,
    CloudFilesCapabilities, CloudItem, CloudItemId, CloudItemKey, CloudItemPage,
    CloudMetadataBackend, CloudNamespaceId, CloudRootId, CloudScope, ContentReadRange,
    ContentReadRequest, ContentReadResponse, ContentRevision, EnumerationCapabilities,
    HydrationCapabilities, IdentityCapabilities, MetadataRevision, MutationCapabilities,
    PageCursor, RangeHydrationCapabilities, RevisionCapabilities,
};
use async_trait::async_trait;
use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntheticProfile {
    Full,
    Limited,
    ReadOnly,
}

pub struct SyntheticBackend {
    profile: SyntheticProfile,
    capabilities: CloudFilesCapabilities,
    scope: CloudScope,
    root_key: CloudItemKey,
    moved_file_before: CloudItem,
    moved_file: CloudItem,
    deleted_file: CloudItem,
    items: HashMap<CloudItemKey, CloudItem>,
    children: HashMap<CloudItemKey, Vec<CloudItemKey>>,
    contents: HashMap<CloudItemKey, (ContentRevision, Bytes)>,
    changes: Vec<aster_forge_cloud_files_core::CloudChange>,
    page_size: usize,
    change_batch_size: usize,
}

impl SyntheticBackend {
    pub fn full() -> Self {
        Self::fixture(SyntheticProfile::Full)
    }

    pub fn limited() -> Self {
        Self::fixture(SyntheticProfile::Limited)
    }

    pub fn read_only() -> Self {
        Self::fixture(SyntheticProfile::ReadOnly)
    }

    fn fixture(profile: SyntheticProfile) -> Self {
        let namespace = CloudNamespaceId::new("synthetic-provider")
            .expect("synthetic namespace should be valid");
        let root_id = CloudRootId::new("root-a").expect("synthetic root should be valid");
        let scope = CloudScope::new(namespace, root_id);

        let root_key = item_key(&scope, "root-item");
        let folder_key = item_key(&scope, "folder-docs");
        let moved_file_key = item_key(&scope, "file-alpha");
        let deleted_file_key = item_key(&scope, "file-deleted");
        let file_c_key = item_key(&scope, "file-charlie");
        let file_d_key = item_key(&scope, "file-delta");

        let root =
            CloudItem::directory(root_key.clone(), None, "root", metadata_revision("root-m1"))
                .expect("synthetic root should be valid");
        let folder = CloudItem::directory(
            folder_key.clone(),
            Some(root_key.item_id().clone()),
            "docs",
            metadata_revision("folder-m1"),
        )
        .expect("synthetic folder should be valid");
        let moved_file_before = CloudItem::file(
            moved_file_key.clone(),
            root_key.item_id().clone(),
            "alpha.txt",
            metadata_revision("alpha-m1"),
            content_metadata("alpha-c2", b"alpha version two"),
        )
        .expect("synthetic file should be valid");
        let moved_file = moved_file_before
            .clone()
            .moved(
                folder_key.item_id().clone(),
                "alpha-renamed.txt",
                metadata_revision("alpha-m2"),
            )
            .expect("synthetic move should be valid");
        let deleted_file = CloudItem::file(
            deleted_file_key.clone(),
            root_key.item_id().clone(),
            "deleted.txt",
            metadata_revision("deleted-m1"),
            content_metadata("deleted-c1", b"deleted content"),
        )
        .expect("synthetic deleted file should be valid");
        let file_c = CloudItem::file(
            file_c_key.clone(),
            root_key.item_id().clone(),
            "charlie.txt",
            metadata_revision("charlie-m1"),
            content_metadata("charlie-c1", b"charlie"),
        )
        .expect("synthetic file should be valid");
        let file_d = CloudItem::file(
            file_d_key.clone(),
            root_key.item_id().clone(),
            "delta.txt",
            metadata_revision("delta-m1"),
            content_metadata("delta-c1", b"delta"),
        )
        .expect("synthetic file should be valid");

        let items = HashMap::from([
            (root_key.clone(), root),
            (folder_key.clone(), folder),
            (moved_file_key.clone(), moved_file.clone()),
            (file_c_key.clone(), file_c.clone()),
            (file_d_key.clone(), file_d.clone()),
        ]);
        let children = HashMap::from([
            (
                root_key.clone(),
                vec![folder_key.clone(), file_c_key.clone(), file_d_key.clone()],
            ),
            (folder_key.clone(), vec![moved_file_key.clone()]),
        ]);
        let contents = HashMap::from([
            content_entry(&moved_file, b"alpha version two"),
            content_entry(&file_c, b"charlie"),
            content_entry(&file_d, b"delta"),
        ]);
        let changes = vec![
            aster_forge_cloud_files_core::CloudChange::Upsert {
                item: moved_file.clone(),
            },
            aster_forge_cloud_files_core::CloudChange::Delete {
                key: deleted_file_key,
                previous_parent_id: deleted_file.parent_id().cloned(),
                metadata_revision: Some(metadata_revision("deleted-m2")),
            },
        ];

        let mut capabilities = CloudFilesCapabilities::unconstrained();
        capabilities.identity = IdentityCapabilities {
            stable_item_ids: true,
            path_independent_item_ids: true,
            same_root_move_preserves_identity: true,
            cross_root_move_preserves_identity: false,
        };
        capabilities.enumeration = EnumerationCapabilities {
            paged: profile != SyntheticProfile::Limited,
            stable_snapshot: true,
        };
        capabilities.revisions = RevisionCapabilities {
            separate_metadata_and_content: profile != SyntheticProfile::Limited,
            conditional_metadata_mutation: profile == SyntheticProfile::Full,
            conditional_content_access: profile != SyntheticProfile::Limited,
        };
        capabilities.changes = match profile {
            SyntheticProfile::Full | SyntheticProfile::ReadOnly => ChangeCapabilities {
                anchored: Some(AnchoredChangeCapabilities {
                    cursor_reset: true,
                    tombstones: true,
                }),
                snapshot_diff: true,
                external_invalidation: true,
            },
            SyntheticProfile::Limited => ChangeCapabilities {
                anchored: None,
                snapshot_diff: true,
                external_invalidation: false,
            },
        };
        capabilities.hydration = match profile {
            SyntheticProfile::Limited => HydrationCapabilities {
                whole_file: true,
                ..HydrationCapabilities::default()
            },
            SyntheticProfile::Full | SyntheticProfile::ReadOnly => HydrationCapabilities {
                whole_file: true,
                range: Some(RangeHydrationCapabilities {
                    alignment: Alignment::ONE,
                    partial_cancel: false,
                }),
                ..HydrationCapabilities::default()
            },
        };
        capabilities.mutations = match profile {
            SyntheticProfile::Full => MutationCapabilities {
                create: true,
                modify_metadata: true,
                modify_content: true,
                delete: true,
                atomic_move: true,
                cross_root_move: false,
                resumable_upload: true,
            },
            SyntheticProfile::Limited => MutationCapabilities {
                create: true,
                modify_metadata: true,
                modify_content: true,
                delete: true,
                atomic_move: false,
                cross_root_move: false,
                resumable_upload: false,
            },
            SyntheticProfile::ReadOnly => MutationCapabilities::default(),
        };

        Self {
            profile,
            capabilities,
            scope,
            root_key,
            moved_file_before,
            moved_file,
            deleted_file,
            items,
            children,
            contents,
            changes,
            page_size: if profile == SyntheticProfile::Limited {
                usize::MAX
            } else {
                2
            },
            change_batch_size: 1,
        }
    }

    pub fn profile(&self) -> SyntheticProfile {
        self.profile
    }

    pub fn scope(&self) -> &CloudScope {
        &self.scope
    }

    pub fn root_key(&self) -> &CloudItemKey {
        &self.root_key
    }

    pub fn moved_file_before(&self) -> &CloudItem {
        &self.moved_file_before
    }

    pub fn moved_file(&self) -> &CloudItem {
        &self.moved_file
    }

    pub fn deleted_file(&self) -> &CloudItem {
        &self.deleted_file
    }

    pub fn initial_snapshot(&self) -> HashMap<CloudItemKey, CloudItem> {
        HashMap::from([
            (
                self.moved_file_before.key().clone(),
                self.moved_file_before.clone(),
            ),
            (self.deleted_file.key().clone(), self.deleted_file.clone()),
        ])
    }

    pub fn expired_cursor() -> ChangeCursor {
        ChangeCursor::from_slice(b"expired").expect("expired cursor fixture should be valid")
    }

    pub fn identity_invalid_cursor() -> ChangeCursor {
        ChangeCursor::from_slice(b"identity-invalid")
            .expect("identity-invalid cursor fixture should be valid")
    }

    fn encode_page_cursor(parent: &CloudItemKey, offset: usize) -> PageCursor {
        PageCursor::new(
            format!(
                "page|{}|{}|{}|{offset}",
                parent.scope().namespace_id(),
                parent.scope().root_id(),
                parent.item_id()
            )
            .into_bytes(),
        )
        .expect("generated page cursor should be valid")
    }

    fn decode_page_cursor(parent: &CloudItemKey, cursor: &PageCursor) -> BackendResult<usize> {
        let value = std::str::from_utf8(cursor.as_bytes())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let expected_prefix = format!(
            "page|{}|{}|{}|",
            parent.scope().namespace_id(),
            parent.scope().root_id(),
            parent.item_id()
        );
        let offset = value
            .strip_prefix(&expected_prefix)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        offset
            .parse::<usize>()
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))
    }

    fn encode_change_cursor(offset: usize) -> ChangeCursor {
        ChangeCursor::new(format!("change:{offset}").into_bytes())
            .expect("generated change cursor should be valid")
    }

    fn decode_change_cursor(cursor: Option<&ChangeCursor>) -> BackendResult<usize> {
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        let value = std::str::from_utf8(cursor.as_bytes())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let offset = value
            .strip_prefix("change:")
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        offset
            .parse::<usize>()
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))
    }
}

#[async_trait]
impl CloudMetadataBackend for SyntheticBackend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        self.items
            .get(key)
            .cloned()
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        let parent_item = self
            .items
            .get(parent)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
        if parent_item.kind() != aster_forge_cloud_files_core::CloudItemKind::Directory {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let child_keys = self.children.get(parent).map(Vec::as_slice).unwrap_or(&[]);
        let offset = match cursor {
            Some(cursor) => Self::decode_page_cursor(parent, cursor)?,
            None => 0,
        };
        if offset > child_keys.len() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let end = offset.saturating_add(self.page_size).min(child_keys.len());
        let items = child_keys[offset..end]
            .iter()
            .filter_map(|key| self.items.get(key).cloned())
            .collect();
        let next_cursor = (end < child_keys.len()).then(|| Self::encode_page_cursor(parent, end));
        Ok(CloudItemPage::new(items, next_cursor))
    }

    async fn changes_since(
        &self,
        scope: &CloudScope,
        cursor: Option<&ChangeCursor>,
    ) -> BackendResult<ChangePage> {
        if scope != &self.scope {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        }
        if self.capabilities.changes.anchored.is_none() {
            return Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported));
        }
        if cursor == Some(&Self::expired_cursor()) {
            return Ok(ChangePage::ResetRequired {
                reason: ChangeResetReason::CursorExpired,
            });
        }
        if cursor == Some(&Self::identity_invalid_cursor()) {
            return Ok(ChangePage::ResetRequired {
                reason: ChangeResetReason::IdentityMappingInvalidated,
            });
        }
        let offset = Self::decode_change_cursor(cursor)?;
        if offset > self.changes.len() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let end = offset
            .saturating_add(self.change_batch_size)
            .min(self.changes.len());
        Ok(ChangePage::Batch(ChangeBatch::new(
            self.changes[offset..end].to_vec(),
            Self::encode_change_cursor(end),
            end < self.changes.len(),
        )))
    }
}

#[async_trait]
impl CloudContentBackend for SyntheticBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        let Some((revision, content)) = self.contents.get(request.key()) else {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        };
        if revision != request.revision() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        if matches!(request.read_range(), ContentReadRange::Range(_))
            && self.capabilities.hydration.range.is_none()
        {
            return Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported));
        }

        let total_size = u64::try_from(content.len())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let (offset, bytes) = match request.read_range() {
            ContentReadRange::Whole => (0, content.clone()),
            ContentReadRange::Range(range) => {
                if range.offset() > total_size {
                    return Err(CloudBackendError::new(
                        CloudBackendErrorKind::InvalidRequest,
                    ));
                }
                let start = usize::try_from(range.offset())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                let end_u64 = range.end_exclusive().min(total_size);
                let end = usize::try_from(end_u64)
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                (range.offset(), content.slice(start..end))
            }
        };
        let response = ContentReadResponse::new(revision.clone(), offset, bytes, total_size)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        request
            .validate_response(&response)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        Ok(response)
    }
}

impl CloudFilesBackend for SyntheticBackend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        self.capabilities
    }
}

fn item_key(scope: &CloudScope, item_id: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope.clone(),
        CloudItemId::new(item_id).expect("synthetic item id should be valid"),
    )
}

fn metadata_revision(value: &str) -> MetadataRevision {
    MetadataRevision::from_slice(value.as_bytes())
        .expect("synthetic metadata revision should be valid")
}

fn content_revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes())
        .expect("synthetic content revision should be valid")
}

fn content_metadata(revision: &str, bytes: &[u8]) -> CloudContentMetadata {
    CloudContentMetadata::new(
        content_revision(revision),
        None,
        u64::try_from(bytes.len()).expect("synthetic content length should fit u64"),
    )
}

fn content_entry(item: &CloudItem, bytes: &[u8]) -> (CloudItemKey, (ContentRevision, Bytes)) {
    let revision = item
        .content()
        .expect("synthetic file should have content metadata")
        .revision()
        .clone();
    (
        item.key().clone(),
        (revision, Bytes::copy_from_slice(bytes)),
    )
}

pub fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("synthetic range should be valid")
}
