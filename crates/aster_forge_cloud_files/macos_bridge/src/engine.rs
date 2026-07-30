//! Runtime-neutral read-only File Provider engine used by Swift bridge implementations.

use std::{num::NonZeroU64, sync::Arc};

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesBackend, CloudItemKey, CloudItemKind, ContentReadRequest, ContentRevision,
};
use bytes::Bytes;

use crate::{
    MacosBridgeError, MacosEnumerationPage, MacosEnumerationRequest, MacosFileProviderIdentifier,
    MacosFileProviderItem, Result,
};

/// Validated metadata required to stream one exact file revision into native staging storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosContentFetchPlan {
    key: CloudItemKey,
    revision: ContentRevision,
    size: u64,
}

impl MacosContentFetchPlan {
    /// Returns the exact scoped item being fetched.
    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the exact revision being fetched.
    #[must_use]
    pub const fn revision(&self) -> &ContentRevision {
        &self.revision
    }

    /// Returns the complete logical file size.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

/// One validated bounded chunk from a [`MacosContentFetchPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosFetchedContentChunk {
    offset: u64,
    bytes: Bytes,
}

impl MacosFetchedContentChunk {
    /// Returns the chunk's logical file offset.
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the bounded chunk bytes.
    pub const fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Consumes the chunk into its offset and bytes.
    pub fn into_parts(self) -> (u64, Bytes) {
        (self.offset, self.bytes)
    }
}

/// Product-neutral read-only engine for a single File Provider domain/root.
pub struct MacosReadOnlyEngine<B> {
    backend: Arc<B>,
    root_key: CloudItemKey,
}

impl<B> Clone for MacosReadOnlyEngine<B> {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            root_key: self.root_key.clone(),
        }
    }
}

impl<B> MacosReadOnlyEngine<B>
where
    B: CloudFilesBackend + 'static,
{
    /// Creates an engine scoped to one exact product-neutral root item.
    pub const fn new(backend: Arc<B>, root_key: CloudItemKey) -> Self {
        Self { backend, root_key }
    }

    /// Returns the stable root item key mapped to Apple's root system container.
    #[must_use]
    pub const fn root_key(&self) -> &CloudItemKey {
        &self.root_key
    }

    /// Loads an item by persistent identifier and validates its identity/root role.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn item(
        &self,
        identifier: &MacosFileProviderIdentifier,
    ) -> Result<MacosFileProviderItem> {
        let key = match identifier.system_container() {
            Some(crate::MacosFileProviderSystemContainer::Root) => &self.root_key,
            Some(
                crate::MacosFileProviderSystemContainer::WorkingSet
                | crate::MacosFileProviderSystemContainer::Trash,
            ) => return Err(MacosBridgeError::UnsupportedSystemContainer),
            None => identifier.item_key()?,
        };
        let item = self.backend.get_item(key).await?;
        if item.key() != key {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "get_item returned a different scoped stable identity",
            });
        }
        if item.is_root() != (key == &self.root_key) {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "get_item root shape did not match the File Provider root role",
            });
        }
        MacosFileProviderItem::from_cloud_item(&item, &self.root_key)
    }

    /// Loads and validates one backend page for a File Provider enumerator.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn enumerate(
        &self,
        request: &MacosEnumerationRequest,
    ) -> Result<MacosEnumerationPage> {
        let parent = request.backend_parent(&self.root_key)?;
        let parent_item = self.backend.get_item(parent).await?;
        if parent_item.key() != parent {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration parent lookup returned a different stable identity",
            });
        }
        if parent_item.kind() != CloudItemKind::Directory {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration parent is not a directory",
            });
        }
        if parent_item.is_root() != (parent == &self.root_key) {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration parent root shape did not match the requested container",
            });
        }
        let page = self.backend.list_children(parent, request.page()).await?;
        MacosEnumerationPage::from_backend(&self.root_key, parent, page)
    }

    /// Validates metadata and revision before a caller creates native staging storage.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn prepare_content_fetch(
        &self,
        identifier: &MacosFileProviderIdentifier,
        requested_revision: &ContentRevision,
    ) -> Result<MacosContentFetchPlan> {
        let key = identifier.item_key()?;
        let item = self.backend.get_item(key).await?;
        if item.key() != key {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "content metadata lookup returned a different stable identity",
            });
        }
        if item.is_root() || item.kind() != CloudItemKind::File {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "content fetch target is not a regular file",
            });
        }
        let content = item
            .content()
            .ok_or(MacosBridgeError::InvalidBackendResponse {
                reason: "regular file omitted content metadata",
            })?;
        if content.revision() != requested_revision {
            return Err(MacosBridgeError::Backend(
                aster_forge_cloud_files_core::CloudBackendError::new(
                    aster_forge_cloud_files_core::CloudBackendErrorKind::PreconditionFailed,
                ),
            ));
        }
        Ok(MacosContentFetchPlan {
            key: key.clone(),
            revision: requested_revision.clone(),
            size: content.size(),
        })
    }

    /// Reads one bounded chunk for a validated fetch plan.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn read_content_chunk(
        &self,
        plan: &MacosContentFetchPlan,
        offset: u64,
        maximum_length: NonZeroU64,
    ) -> Result<MacosFetchedContentChunk> {
        if offset >= plan.size {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "content chunk offset is outside the planned file",
            });
        }
        let length = maximum_length.get().min(plan.size - offset);
        let range = ByteRange::new(offset, length).map_err(|_| {
            MacosBridgeError::InvalidBackendResponse {
                reason: "content chunk range exceeded the planned file",
            }
        })?;
        let request =
            ContentReadRequest::range(plan.key.clone(), plan.revision.clone(), plan.size, range);
        let response = self.backend.read_content(&request).await?;
        request.validate_response(&response).map_err(|_| {
            MacosBridgeError::InvalidBackendResponse {
                reason: "content response violated the requested revision or chunk extent",
            }
        })?;
        let (_, response_offset, bytes, _) = response.into_parts();
        Ok(MacosFetchedContentChunk {
            offset: response_offset,
            bytes,
        })
    }
}
