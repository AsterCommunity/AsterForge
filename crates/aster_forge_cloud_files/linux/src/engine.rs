//! Runtime-neutral read-only Linux engine used by the native FUSE adapter.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime},
};

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesBackend, CloudItem, CloudItemKey, CloudItemKind, ContentReadRequest,
    ContentRevision, PageCursor, SessionGeneration,
};
use bytes::Bytes;

use crate::{
    LINUX_ROOT_INODE, LinuxCloudFilesError, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord,
    LinuxInodeTable, Result,
};

const FUSE_BLOCK_SIZE: u64 = 512;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Validates a string-backed cloud item name for one Linux directory component.
pub fn validate_linux_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(LinuxCloudFilesError::InvalidName {
            reason: "name must not be empty",
        });
    }
    if name == "." || name == ".." {
        return Err(LinuxCloudFilesError::InvalidName {
            reason: "dot components are reserved",
        });
    }
    if name.contains('/') {
        return Err(LinuxCloudFilesError::InvalidName {
            reason: "name must not contain a path separator",
        });
    }
    if name.contains('\0') {
        return Err(LinuxCloudFilesError::InvalidName {
            reason: "name must not contain NUL",
        });
    }
    Ok(())
}

/// Portable Linux node kind mapped to `fuser::FileType` at the native boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxNodeKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
}

/// Attribute defaults for metadata not present in the product-neutral core item model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxAttributePolicy {
    uid: u32,
    gid: u32,
    file_permissions: u16,
    directory_permissions: u16,
    block_size: u32,
    cache_ttl: Duration,
    fallback_time: SystemTime,
}

impl LinuxAttributePolicy {
    /// Creates an explicit Linux attribute policy.
    pub fn new(
        uid: u32,
        gid: u32,
        file_permissions: u16,
        directory_permissions: u16,
        cache_ttl: Duration,
    ) -> Result<Self> {
        if file_permissions & !0o7777 != 0 || directory_permissions & !0o7777 != 0 {
            return Err(LinuxCloudFilesError::InvalidConfiguration {
                reason: "permissions must contain only Unix permission and special-mode bits",
            });
        }
        Ok(Self {
            uid,
            gid,
            file_permissions,
            directory_permissions,
            block_size: 4096,
            cache_ttl,
            fallback_time: SystemTime::UNIX_EPOCH,
        })
    }

    /// Returns the mount-owner user ID.
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    /// Returns the mount-owner group ID.
    pub const fn gid(&self) -> u32 {
        self.gid
    }

    /// Returns regular-file permissions.
    pub const fn file_permissions(&self) -> u16 {
        self.file_permissions
    }

    /// Returns directory permissions.
    pub const fn directory_permissions(&self) -> u16 {
        self.directory_permissions
    }

    /// Returns the reported preferred I/O block size.
    pub const fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Returns the kernel entry and attribute cache TTL.
    pub const fn cache_ttl(&self) -> Duration {
        self.cache_ttl
    }

    /// Returns the timestamp used until a platform adapter supplies native times.
    pub const fn fallback_time(&self) -> SystemTime {
        self.fallback_time
    }
}

impl Default for LinuxAttributePolicy {
    fn default() -> Self {
        Self {
            uid: 0,
            gid: 0,
            file_permissions: 0o444,
            directory_permissions: 0o555,
            block_size: 4096,
            cache_ttl: Duration::from_secs(1),
            fallback_time: SystemTime::UNIX_EPOCH,
        }
    }
}

/// Complete portable attributes for one restored inode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxFileAttributes {
    inode: LinuxInode,
    size: u64,
    blocks: u64,
    kind: LinuxNodeKind,
    permissions: u16,
    links: u32,
    uid: u32,
    gid: u32,
    block_size: u32,
    time: SystemTime,
}

impl LinuxFileAttributes {
    /// Returns the restored inode.
    pub const fn inode(&self) -> LinuxInode {
        self.inode
    }

    /// Returns the logical content size.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns allocated 512-byte block count reported to the kernel.
    pub const fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Returns the portable node kind.
    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }

    /// Returns Unix permission bits.
    pub const fn permissions(&self) -> u16 {
        self.permissions
    }

    /// Returns the reported hard-link count.
    pub const fn links(&self) -> u32 {
        self.links
    }

    /// Returns the owner user ID.
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    /// Returns the owner group ID.
    pub const fn gid(&self) -> u32 {
        self.gid
    }

    /// Returns the preferred I/O block size.
    pub const fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Returns the fallback timestamp used for all native time fields.
    pub const fn time(&self) -> SystemTime {
        self.time
    }
}

/// Metadata reply for one inode lookup or getattr request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxNode {
    key: CloudItemKey,
    generation: LinuxInodeGeneration,
    attributes: LinuxFileAttributes,
}

impl LinuxNode {
    /// Returns the stable scoped cloud identity.
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the inode generation fence.
    pub const fn generation(&self) -> LinuxInodeGeneration {
        self.generation
    }

    /// Returns portable Linux attributes.
    pub const fn attributes(&self) -> &LinuxFileAttributes {
        &self.attributes
    }

    pub(crate) fn with_size(mut self, size: u64) -> Self {
        self.attributes.size = size;
        self.attributes.blocks = size.div_ceil(FUSE_BLOCK_SIZE);
        self
    }
}

/// Non-zero open-file handle scoped to one mount process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinuxFileHandle(u64);

impl LinuxFileHandle {
    /// Restores a native file handle supplied by FUSE.
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(Self(value))
    }

    /// Returns the native handle value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Non-zero open-directory handle scoped to one mount process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinuxDirectoryHandle(u64);

impl LinuxDirectoryHandle {
    /// Restores a native directory handle supplied by FUSE.
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(Self(value))
    }

    /// Returns the native handle value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One child captured in an open-directory snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxDirectoryEntry {
    inode: LinuxInode,
    generation: LinuxInodeGeneration,
    name: String,
    kind: LinuxNodeKind,
}

impl LinuxDirectoryEntry {
    /// Returns the restored child inode.
    pub const fn inode(&self) -> LinuxInode {
        self.inode
    }

    /// Returns the child generation fence.
    pub const fn generation(&self) -> LinuxInodeGeneration {
        self.generation
    }

    /// Returns the exact UTF-8 backend name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the portable child kind.
    pub const fn kind(&self) -> LinuxNodeKind {
        self.kind
    }

    pub(crate) fn from_node(name: String, node: &LinuxNode) -> Self {
        Self {
            inode: node.attributes.inode,
            generation: node.generation,
            name,
            kind: node.attributes.kind,
        }
    }
}

/// Immutable directory stream captured at `opendir` time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxDirectorySnapshot {
    directory: LinuxInode,
    parent: LinuxInode,
    entries: Arc<[LinuxDirectoryEntry]>,
}

impl LinuxDirectorySnapshot {
    /// Returns the opened directory inode.
    pub const fn directory(&self) -> LinuxInode {
        self.directory
    }

    /// Returns the parent inode used by the `..` entry.
    pub const fn parent(&self) -> LinuxInode {
        self.parent
    }

    /// Returns children in the backend enumeration order captured by this handle.
    pub fn entries(&self) -> &[LinuxDirectoryEntry] {
        &self.entries
    }

    pub(crate) fn with_entries(
        directory: LinuxInode,
        parent: LinuxInode,
        entries: Vec<LinuxDirectoryEntry>,
    ) -> Self {
        Self {
            directory,
            parent,
            entries: entries.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct OpenFile {
    inode: LinuxInode,
    key: CloudItemKey,
    revision: ContentRevision,
    size: u64,
}

#[derive(Default)]
struct HandleState {
    next: u64,
    files: HashMap<LinuxFileHandle, OpenFile>,
    directories: HashMap<LinuxDirectoryHandle, LinuxDirectorySnapshot>,
}

impl HandleState {
    fn allocate(&mut self) -> Result<u64> {
        let current = if self.next == 0 { 1 } else { self.next };
        let Some(next) = current.checked_add(1) else {
            return Err(LinuxCloudFilesError::HandleExhausted);
        };
        self.next = next;
        Ok(current)
    }
}

struct EngineInner<B> {
    backend: Arc<B>,
    inodes: Arc<LinuxInodeTable>,
    attributes: LinuxAttributePolicy,
    handles: Mutex<HandleState>,
}

/// Product-neutral read-only engine shared by native FUSE callbacks and deterministic tests.
pub struct LinuxReadOnlyEngine<B> {
    inner: Arc<EngineInner<B>>,
}

impl<B> Clone for LinuxReadOnlyEngine<B> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<B> LinuxReadOnlyEngine<B>
where
    B: CloudFilesBackend + 'static,
{
    /// Creates a read-only engine from a backend, restored inode table, and attribute policy.
    pub fn new(
        backend: Arc<B>,
        inodes: Arc<LinuxInodeTable>,
        attributes: LinuxAttributePolicy,
    ) -> Self {
        Self {
            inner: Arc::new(EngineInner {
                backend,
                inodes,
                attributes,
                handles: Mutex::new(HandleState::default()),
            }),
        }
    }

    /// Returns the active immutable inode table.
    pub fn inode_table(&self) -> &LinuxInodeTable {
        &self.inner.inodes
    }

    /// Returns the active native attribute policy.
    pub fn attribute_policy(&self) -> &LinuxAttributePolicy {
        &self.inner.attributes
    }

    /// Resolves one name by enumerating all backend pages for the parent snapshot.
    pub async fn lookup(&self, parent: LinuxInode, name: &str) -> Result<LinuxNode> {
        validate_linux_name(name)?;
        let parent_item = self.load_item_for_overlay(parent).await?;
        if parent_item.kind() != CloudItemKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let children = self.load_children(parent_item.key()).await?;
        let Some(item) = children.into_iter().find(|item| item.name() == name) else {
            return Err(LinuxCloudFilesError::Backend(
                aster_forge_cloud_files_core::CloudBackendError::new(
                    aster_forge_cloud_files_core::CloudBackendErrorKind::NotFound,
                ),
            ));
        };
        self.node_from_item(&item)
    }

    /// Loads attributes for one restored inode.
    pub async fn getattr(&self, inode: LinuxInode) -> Result<LinuxNode> {
        let item = self.load_item_for_overlay(inode).await?;
        self.node_from_item(&item)
    }

    /// Opens a regular file and captures its exact content revision for subsequent range reads.
    pub async fn open_file(&self, inode: LinuxInode) -> Result<LinuxFileHandle> {
        let item = self.load_item_for_overlay(inode).await?;
        if item.kind() != CloudItemKind::File {
            return Err(LinuxCloudFilesError::NotFile);
        }
        let Some(content) = item.content() else {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "regular file omitted content metadata",
            });
        };
        let mut handles = lock(&self.inner.handles);
        let handle = LinuxFileHandle(handles.allocate()?);
        handles.files.insert(
            handle,
            OpenFile {
                inode,
                key: item.key().clone(),
                revision: content.revision().clone(),
                size: content.size(),
            },
        );
        Ok(handle)
    }

    pub(crate) async fn hydrate_for_write(
        &self,
        inode: LinuxInode,
        session_generation: SessionGeneration,
    ) -> Result<(crate::LinuxWriteOpenRequest, Bytes)> {
        let item = self.load_item_for_overlay(inode).await?;
        self.hydrate_item_for_write(&item, session_generation).await
    }

    pub(crate) async fn hydrate_item_for_write(
        &self,
        item: &CloudItem,
        session_generation: SessionGeneration,
    ) -> Result<(crate::LinuxWriteOpenRequest, Bytes)> {
        if item.kind() != CloudItemKind::File {
            return Err(LinuxCloudFilesError::NotFile);
        }
        let content = item
            .content()
            .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "regular file omitted content metadata",
            })?;
        let request = ContentReadRequest::whole(
            item.key().clone(),
            content.revision().clone(),
            content.size(),
        );
        let response = self.inner.backend.read_content(&request).await?;
        request.validate_response(&response).map_err(|_| {
            LinuxCloudFilesError::InvalidBackendResponse {
                reason: "write hydration violated the requested revision or complete-file extent",
            }
        })?;
        let (_, _, bytes, _) = response.into_parts();
        Ok((
            crate::LinuxWriteOpenRequest::new(
                item.key().clone(),
                content.revision().clone(),
                content.size(),
                session_generation,
            ),
            bytes,
        ))
    }

    /// Reads an exact range using the revision captured by `open_file`.
    pub async fn read_file(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
        offset: u64,
        size: u32,
    ) -> Result<Bytes> {
        let opened = {
            let handles = lock(&self.inner.handles);
            let Some(opened) = handles.files.get(&handle) else {
                return Err(LinuxCloudFilesError::StaleHandle);
            };
            if opened.inode != inode {
                return Err(LinuxCloudFilesError::StaleHandle);
            }
            opened.clone()
        };
        if size == 0 || offset >= opened.size {
            return Ok(Bytes::new());
        }
        let length = u64::from(size).min(opened.size - offset);
        let range = ByteRange::new(offset, length).map_err(|_| {
            LinuxCloudFilesError::InvalidBackendResponse {
                reason: "validated FUSE range could not be represented by the core model",
            }
        })?;
        let request = ContentReadRequest::range(opened.key, opened.revision, opened.size, range);
        let response = self.inner.backend.read_content(&request).await?;
        request.validate_response(&response).map_err(|_| {
            LinuxCloudFilesError::InvalidBackendResponse {
                reason: "content response violated the requested revision or range",
            }
        })?;
        Ok(response.into_parts().2)
    }

    /// Releases one file handle. Releasing the same handle twice reports a stale handle.
    pub fn release_file(&self, handle: LinuxFileHandle) -> Result<()> {
        if lock(&self.inner.handles).files.remove(&handle).is_none() {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(())
    }

    /// Opens a directory and freezes one complete paged snapshot for stable FUSE cookies.
    pub async fn open_directory(&self, inode: LinuxInode) -> Result<LinuxDirectoryHandle> {
        let item = self.load_item_for_overlay(inode).await?;
        if item.kind() != CloudItemKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let children = self.load_children(item.key()).await?;
        let entries = children
            .iter()
            .map(|child| {
                let node = self.node_from_item(child)?;
                Ok(LinuxDirectoryEntry {
                    inode: node.attributes.inode,
                    generation: node.generation,
                    name: child.name().to_owned(),
                    kind: node.attributes.kind,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let parent = match item.parent_id() {
            Some(parent_id) => {
                let parent_key = CloudItemKey::new(item.key().scope().clone(), parent_id.clone());
                self.inner
                    .inodes
                    .by_key(&parent_key)
                    .ok_or(LinuxCloudFilesError::MissingInodeRecord)?
                    .inode()
            }
            None => inode,
        };
        let snapshot = LinuxDirectorySnapshot {
            directory: inode,
            parent,
            entries: entries.into(),
        };
        let mut handles = lock(&self.inner.handles);
        let handle = LinuxDirectoryHandle(handles.allocate()?);
        handles.directories.insert(handle, snapshot);
        Ok(handle)
    }

    /// Returns the immutable snapshot associated with an open directory handle.
    pub fn directory_snapshot(
        &self,
        inode: LinuxInode,
        handle: LinuxDirectoryHandle,
    ) -> Result<LinuxDirectorySnapshot> {
        let handles = lock(&self.inner.handles);
        let Some(snapshot) = handles.directories.get(&handle) else {
            return Err(LinuxCloudFilesError::StaleHandle);
        };
        if snapshot.directory != inode {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(snapshot.clone())
    }

    /// Releases one directory snapshot handle.
    pub fn release_directory(&self, handle: LinuxDirectoryHandle) -> Result<()> {
        if lock(&self.inner.handles)
            .directories
            .remove(&handle)
            .is_none()
        {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(())
    }

    pub(crate) async fn load_item_for_overlay(&self, inode: LinuxInode) -> Result<CloudItem> {
        let record = self
            .inner
            .inodes
            .by_inode(inode)
            .ok_or(LinuxCloudFilesError::UnknownInode { inode: inode.get() })?;
        let item = self.inner.backend.get_item(record.key()).await?;
        if item.key() != record.key() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "get_item returned a different scoped stable identity",
            });
        }
        if item.is_root() != (inode == LINUX_ROOT_INODE) {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "get_item root shape did not match the restored inode role",
            });
        }
        Ok(item)
    }

    pub(crate) async fn load_children_for_overlay(
        &self,
        parent: &CloudItemKey,
    ) -> Result<Vec<CloudItem>> {
        let mut cursor: Option<PageCursor> = None;
        let mut seen_cursors = HashSet::new();
        let mut seen_names = HashSet::new();
        let mut children = Vec::new();
        loop {
            let page = self
                .inner
                .backend
                .list_children(parent, cursor.as_ref())
                .await?;
            let (items, next_cursor) = page.into_parts();
            for item in items {
                if item.key().scope() != parent.scope()
                    || item.parent_id() != Some(parent.item_id())
                {
                    return Err(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "directory child escaped the requested parent scope",
                    });
                }
                validate_linux_name(item.name())?;
                if !seen_names.insert(item.name().to_owned()) {
                    return Err(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "directory enumeration returned duplicate child names",
                    });
                }
                children.push(item);
            }
            let Some(next_cursor) = next_cursor else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "directory pagination repeated a continuation cursor",
                });
            }
            cursor = Some(next_cursor);
        }
        Ok(children)
    }

    async fn load_children(&self, parent: &CloudItemKey) -> Result<Vec<CloudItem>> {
        let children = self.load_children_for_overlay(parent).await?;
        if children
            .iter()
            .any(|item| self.inner.inodes.by_key(item.key()).is_none())
        {
            return Err(LinuxCloudFilesError::MissingInodeRecord);
        }
        Ok(children)
    }

    fn node_from_item(&self, item: &CloudItem) -> Result<LinuxNode> {
        if !item.is_root() {
            validate_linux_name(item.name())?;
        }
        let record = self
            .inner
            .inodes
            .by_key(item.key())
            .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
        self.node_from_item_and_record(item, record)
    }

    pub(crate) fn node_from_item_and_record(
        &self,
        item: &CloudItem,
        record: &LinuxInodeRecord,
    ) -> Result<LinuxNode> {
        if !item.is_root() {
            validate_linux_name(item.name())?;
        }
        if item.key() != record.key() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "linux inode record did not match the item stable identity",
            });
        }
        let (kind, size, permissions, links) = match item.kind() {
            CloudItemKind::File => {
                let Some(content) = item.content() else {
                    return Err(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "regular file omitted content metadata",
                    });
                };
                (
                    LinuxNodeKind::File,
                    content.size(),
                    self.inner.attributes.file_permissions,
                    1,
                )
            }
            CloudItemKind::Directory => (
                LinuxNodeKind::Directory,
                0,
                self.inner.attributes.directory_permissions,
                2,
            ),
        };
        Ok(LinuxNode {
            key: item.key().clone(),
            generation: record.generation(),
            attributes: LinuxFileAttributes {
                inode: record.inode(),
                size,
                blocks: size.div_ceil(FUSE_BLOCK_SIZE),
                kind,
                permissions,
                links,
                uid: self.inner.attributes.uid,
                gid: self.inner.attributes.gid,
                block_size: self.inner.attributes.block_size,
                time: self.inner.attributes.fallback_time,
            },
        })
    }
}
