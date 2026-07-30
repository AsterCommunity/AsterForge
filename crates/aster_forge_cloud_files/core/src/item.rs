//! Product-neutral cloud item metadata and directory enumeration pages.

use crate::{
    CloudFilesCoreError, CloudItemId, CloudItemKey, ContentDigest, ContentRevision,
    MetadataRevision, PageCursor, Result,
};

/// Item kind shared by the initial platform-independent model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudItemKind {
    /// Regular file with content metadata.
    File,
    /// Directory whose children are enumerated separately.
    Directory,
}

/// Content-specific metadata attached to a regular file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudContentMetadata {
    revision: ContentRevision,
    digest: Option<ContentDigest>,
    size: u64,
}

impl CloudContentMetadata {
    /// Creates content metadata for one exact content revision.
    #[must_use]
    pub const fn new(revision: ContentRevision, digest: Option<ContentDigest>, size: u64) -> Self {
        Self {
            revision,
            digest,
            size,
        }
    }

    /// Returns the opaque content revision.
    #[must_use]
    pub const fn revision(&self) -> &ContentRevision {
        &self.revision
    }

    /// Returns the optional algorithm-tagged content digest.
    #[must_use]
    pub const fn digest(&self) -> Option<&ContentDigest> {
        self.digest.as_ref()
    }

    /// Returns the logical content size in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Consumes the metadata and returns its revision, digest, and size.
    #[must_use]
    pub fn into_parts(self) -> (ContentRevision, Option<ContentDigest>, u64) {
        (self.revision, self.digest, self.size)
    }
}

/// Stable item identity plus its current parent, name, kind, and revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudItem {
    key: CloudItemKey,
    parent_id: Option<CloudItemId>,
    name: String,
    kind: CloudItemKind,
    metadata_revision: MetadataRevision,
    content: Option<CloudContentMetadata>,
}

impl CloudItem {
    /// Creates a directory. `parent_id = None` identifies the root item for the scope.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn directory(
        key: CloudItemKey,
        parent_id: Option<CloudItemId>,
        name: impl Into<String>,
        metadata_revision: MetadataRevision,
    ) -> Result<Self> {
        let name = name.into();
        validate_item_location(&key, parent_id.as_ref(), &name)?;
        Ok(Self {
            key,
            parent_id,
            name,
            kind: CloudItemKind::Directory,
            metadata_revision,
            content: None,
        })
    }

    /// Creates a regular file. Files always require a parent within the same scope.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn file(
        key: CloudItemKey,
        parent_id: CloudItemId,
        name: impl Into<String>,
        metadata_revision: MetadataRevision,
        content: CloudContentMetadata,
    ) -> Result<Self> {
        let name = name.into();
        validate_item_location(&key, Some(&parent_id), &name)?;
        Ok(Self {
            key,
            parent_id: Some(parent_id),
            name,
            kind: CloudItemKind::File,
            metadata_revision,
            content: Some(content),
        })
    }

    /// Returns the fully scoped stable identity.
    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the parent item identity, or `None` for the root item.
    #[must_use]
    pub const fn parent_id(&self) -> Option<&CloudItemId> {
        self.parent_id.as_ref()
    }

    /// Returns the current item name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the current item kind.
    #[must_use]
    pub const fn kind(&self) -> CloudItemKind {
        self.kind
    }

    /// Returns the opaque metadata revision.
    #[must_use]
    pub const fn metadata_revision(&self) -> &MetadataRevision {
        &self.metadata_revision
    }

    /// Returns file content metadata, or `None` for a directory.
    #[must_use]
    pub const fn content(&self) -> Option<&CloudContentMetadata> {
        self.content.as_ref()
    }

    /// Returns whether this item is the root directory of its scope.
    #[must_use]
    pub const fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }

    /// Applies a same-root rename or move while preserving the stable item key and content state.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn moved(
        mut self,
        parent_id: CloudItemId,
        name: impl Into<String>,
        metadata_revision: MetadataRevision,
    ) -> Result<Self> {
        if self.is_root() {
            return Err(CloudFilesCoreError::invalid_item(
                "the root item must not be moved",
            ));
        }
        let name = name.into();
        if name.is_empty() {
            return Err(CloudFilesCoreError::empty("cloud item name"));
        }
        if &parent_id == self.key.item_id() {
            return Err(CloudFilesCoreError::invalid_item(
                "an item must not be its own parent",
            ));
        }
        self.parent_id = Some(parent_id);
        self.name = name;
        self.metadata_revision = metadata_revision;
        Ok(self)
    }
}

fn validate_item_location(
    key: &CloudItemKey,
    parent_id: Option<&CloudItemId>,
    name: &str,
) -> Result<()> {
    if name.is_empty() {
        return Err(CloudFilesCoreError::empty("cloud item name"));
    }
    if parent_id == Some(key.item_id()) {
        return Err(CloudFilesCoreError::invalid_item(
            "an item must not be its own parent",
        ));
    }
    Ok(())
}

/// One page of directory items returned by a backend adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudItemPage {
    items: Vec<CloudItem>,
    next_cursor: Option<PageCursor>,
}

impl CloudItemPage {
    /// Creates a directory page and its optional continuation cursor.
    #[must_use]
    pub const fn new(items: Vec<CloudItem>, next_cursor: Option<PageCursor>) -> Self {
        Self { items, next_cursor }
    }

    /// Returns the items in backend-defined page order.
    #[must_use]
    pub fn items(&self) -> &[CloudItem] {
        &self.items
    }

    /// Returns the next page cursor, or `None` when enumeration is complete.
    #[must_use]
    pub const fn next_cursor(&self) -> Option<&PageCursor> {
        self.next_cursor.as_ref()
    }

    /// Consumes the page and returns its items and continuation cursor.
    #[must_use]
    pub fn into_parts(self) -> (Vec<CloudItem>, Option<PageCursor>) {
        (self.items, self.next_cursor)
    }
}
