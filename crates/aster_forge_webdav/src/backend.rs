//! Product adapter ports consumed by the `WebDAV` protocol layer.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use aster_forge_utils::http_range::HttpByteRange;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use http::StatusCode;

use crate::{DavPath, DavXmlElement};

/// Stream used for product-independent `WebDAV` content transfer.
pub type DavContentStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DavBackendError>> + Send + 'static>>;

/// Opened representation stream and the exact number of bytes it is expected to yield.
pub struct DavOpenedDownload {
    /// Byte stream producing the representation payload in order.
    pub stream: DavContentStream,
    /// Exact payload length the open must yield; the protocol layer validates it.
    pub expected_length: u64,
}

impl DavOpenedDownload {
    /// Creates an opened download from a stream and its planned payload length.
    #[must_use]
    pub const fn new(stream: DavContentStream, expected_length: u64) -> Self {
        Self {
            stream,
            expected_length,
        }
    }
}

/// Failure while opening a planned representation stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavDownloadOpenError {
    /// Backend failure reported while opening the planned source.
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    /// Opened source disagrees with the length planned from current metadata.
    #[error("download source length does not match the protocol plan")]
    LengthMismatch {
        /// Representation size planned from protocol-visible metadata.
        planned: u64,
        /// Length actually reported by the opened source.
        opened: u64,
    },
}

/// Stable backend failure categories mapped by the protocol layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavBackendErrorKind {
    /// Target resource or a required parent collection does not exist.
    NotFound,
    /// Adapter policy or permission denied the operation.
    Forbidden,
    /// Current resource state conflicts with the requested mutation.
    Conflict,
    /// Target resource name already exists.
    AlreadyExists,
    /// Storage quota or space is exhausted.
    InsufficientStorage,
    /// Payload exceeds an adapter or storage size limit.
    PayloadTooLarge,
    /// Target is locked and the required tokens were not submitted.
    Locked,
    /// Request content is invalid for the adapter.
    InvalidInput,
    /// Operation or option is not supported by the adapter.
    Unsupported,
    /// Unclassified adapter-side failure.
    Internal,
}

/// Product-neutral failure returned by a product adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("WebDAV backend operation failed: {kind:?}")]
pub struct DavBackendError {
    /// Coarse failure category the protocol layer maps to a response status.
    pub kind: DavBackendErrorKind,
}

impl DavBackendError {
    /// Creates an error from a stable failure category.
    #[must_use]
    pub const fn new(kind: DavBackendErrorKind) -> Self {
        Self { kind }
    }
}

/// Low-level file-system failure exposed by the product adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    /// Target path does not exist.
    #[error("not found")]
    NotFound,
    /// Adapter policy or permission denied the file-system operation.
    #[error("forbidden")]
    Forbidden,
    /// Failure that fits no more specific file-system category.
    #[error("general failure")]
    GeneralFailure,
    /// Target name already exists.
    #[error("already exists")]
    Exists,
    /// Storage quota or space is exhausted.
    #[error("insufficient storage")]
    InsufficientStorage,
    /// Payload exceeds the storage size limit.
    #[error("too large")]
    TooLarge,
    /// Request arguments are invalid for the file-system port.
    #[error("bad request")]
    BadRequest,
}

impl From<FsError> for DavBackendError {
    fn from(error: FsError) -> Self {
        let kind = match error {
            FsError::NotFound => DavBackendErrorKind::NotFound,
            FsError::Forbidden => DavBackendErrorKind::Forbidden,
            FsError::GeneralFailure => DavBackendErrorKind::Internal,
            FsError::Exists => DavBackendErrorKind::AlreadyExists,
            FsError::InsufficientStorage => DavBackendErrorKind::InsufficientStorage,
            FsError::TooLarge => DavBackendErrorKind::PayloadTooLarge,
            FsError::BadRequest => DavBackendErrorKind::InvalidInput,
        };
        Self::new(kind)
    }
}

/// Result type used by the canonical [`DavFileSystem`] port.
pub type FsResult<T> = Result<T, FsError>;
/// Pinned future type used by the canonical [`DavFileSystem`] port.
pub type FsFuture<'a, T> = Pin<Box<dyn Future<Output = FsResult<T>> + Send + 'a>>;

/// `WebDAV` resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DavResourceKind {
    /// Non-collection resource with a representation.
    File,
    /// `WebDAV` collection per RFC 4918.
    Collection,
}

/// Protocol-visible state used to evaluate one resource referenced by an `If` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavIfResourceState {
    /// Current entity tag compared against `If` list members.
    pub etag: Option<String>,
    /// Lock tokens currently held on the resource.
    pub lock_tokens: Vec<String>,
}

/// Product adapter used while evaluating `WebDAV` `If` conditions.
#[async_trait]
pub trait DavIfStateResolver: Send + Sync {
    /// Resolves the protocol-visible `If` state for one resource path.
    ///
    /// # Errors
    ///
    /// Returns a backend error when the adapter cannot read the resource state.
    async fn resolve_if_state(&self, path: &DavPath)
    -> Result<DavIfResourceState, DavBackendError>;
}

/// Sequential write contract selected by the protocol planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavMutationCredentials {
    /// Positively submitted lock tokens whose URI scope applies to this mutation.
    pub submitted_lock_tokens: Vec<String>,
}

impl DavMutationCredentials {
    /// Merges another validated credential set while preserving a canonical unique token list.
    pub fn merge(&mut self, other: Self) {
        self.submitted_lock_tokens
            .extend(other.submitted_lock_tokens);
        self.submitted_lock_tokens.sort();
        self.submitted_lock_tokens.dedup();
    }
}

/// Sequential write contract selected by the protocol planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavWriteOptions {
    /// Truncates the existing representation before writing.
    pub truncate: bool,
    /// Creates the target resource when it does not exist.
    pub create: bool,
    /// Atomically fail if the target already exists; product adapters must enforce this at open.
    pub create_new: bool,
    /// Planned final length, when the protocol layer can determine it up front.
    pub expected_length: Option<u64>,
    /// Integrity checksum the adapter should verify across the committed write, when provided.
    pub checksum: Option<String>,
    /// Validated mutation credentials such as submitted lock tokens.
    pub credentials: DavMutationCredentials,
}

/// Protocol-visible resource metadata supplied by the product adapter.
pub trait DavMetaData: Send + Sync {
    /// Representation size in bytes; collections report zero.
    fn len(&self) -> u64;
    ///
    /// # Errors
    ///
    /// Returns a backend error when the product adapter cannot read the metadata value.
    fn modified(&self) -> FsResult<SystemTime>;
    /// Reports whether the resource is a `WebDAV` collection.
    fn is_dir(&self) -> bool;
    /// Current entity tag used for conditional requests, when the adapter provides one.
    fn etag(&self) -> Option<String>;
    /// Content type advertised for the representation, when the adapter provides one.
    fn content_type(&self) -> Option<&str> {
        None
    }
    ///
    /// # Errors
    ///
    /// Returns a backend error when the product adapter cannot read the metadata value.
    fn created(&self) -> FsResult<SystemTime>;
    /// Reports whether the representation carries no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Reports whether the resource is a non-collection resource.
    fn is_file(&self) -> bool {
        !self.is_dir()
    }
}

/// One entry returned by a bounded directory page.
///
/// Metadata is part of the page so product adapters can batch it with enumeration instead of
/// creating an N+1 lookup contract. `stable_key` must be strictly ordered within and across pages.
pub trait DavDirectoryEntry: Send {
    /// Metadata type batched into the page by the adapter.
    type Metadata: DavMetaData;

    /// Entry base name in the adapter's native byte representation.
    fn name(&self) -> &[u8];
    /// Metadata captured with the page to avoid per-entry lookups.
    fn metadata(&self) -> &Self::Metadata;
    /// Page-ordering key; defaults to the name and must stay stable across pages.
    fn stable_key(&self) -> &[u8] {
        self.name()
    }
}

/// One bounded directory-page request with an opaque product-owned continuation cursor.
///
/// [`DavDirectoryEnumerator::read_directory_page`] receives a non-zero `maximum_entries` value.
/// The backend must return no more than that number of entries.
#[derive(Debug, Clone, Copy)]
pub struct DavDirectoryPageRequest<'a, C> {
    /// Collection whose children are enumerated.
    pub path: &'a DavPath,
    /// Continuation cursor from the previous page, if any.
    pub cursor: Option<&'a C>,
    /// Upper bound on returned entries; the protocol layer always sends a non-zero value.
    pub maximum_entries: usize,
}

/// One product-owned directory page.
///
/// Entries must be in strictly ascending [`DavDirectoryEntry::stable_key`] order and must not
/// exceed the request's `maximum_entries`. An empty `entries` collection must use `None` for
/// `next_cursor` rather than returning an empty continuation page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavDirectoryPage<E, C> {
    /// Entries in strictly ascending stable-key order within the request bounds.
    pub entries: Vec<E>,
    /// Opaque continuation cursor; `None` once the enumeration is complete.
    pub next_cursor: Option<C>,
}

/// Product adapter for stable, bounded directory enumeration.
pub trait DavDirectoryEnumerator: Send + Sync {
    /// Opaque, product-owned continuation cursor type.
    type Cursor: Eq + Hash + Send + Sync;
    /// Entry type yielded by pages of this enumerator.
    type Entry: DavDirectoryEntry;

    /// Returns one page satisfying the bounds and ordering contract on
    /// [`DavDirectoryPageRequest`] and [`DavDirectoryPage`].
    fn read_directory_page<'a>(
        &'a self,
        request: DavDirectoryPageRequest<'a, Self::Cursor>,
    ) -> impl Future<Output = Result<DavDirectoryPage<Self::Entry, Self::Cursor>, DavBackendError>>
    + Send
    + 'a;
}

/// Product adapter used by GET/HEAD after the protocol layer selects a representation.
pub trait DavDownloadSource: Send + Sync {
    /// Metadata type resolved by this source.
    type Metadata: DavMetaData;

    /// Resolves current protocol-visible metadata for one path.
    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> impl Future<Output = Result<Self::Metadata, DavBackendError>> + Send + 'a;
    /// Opens the complete representation stream selected by the protocol layer.
    fn open_full<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> impl Future<Output = Result<DavOpenedDownload, DavBackendError>> + Send + 'a;
    /// Opens one negotiated byte range of the representation as a fresh stream.
    fn open_range<'a>(
        &'a self,
        path: &'a DavPath,
        range: HttpByteRange,
    ) -> impl Future<Output = Result<DavOpenedDownload, DavBackendError>> + Send + 'a;
}

/// Sequential write handle. A successful `finish` is the product adapter's commit boundary.
///
/// Dropping a handle before `finish` or `abort` must roll back or clean up the uncommitted write.
/// It must never commit implicitly or leak staging resources. `abort` is the explicit abandonment
/// path and must perform the same cleanup before it returns.
pub trait DavWriteHandle: Send {
    /// Appends one chunk to the sequential write at the current offset.
    fn write_bytes(
        &mut self,
        buf: Bytes,
    ) -> impl Future<Output = Result<(), DavBackendError>> + Send + '_;
    /// Commits the write; the only success path that may make content visible.
    fn finish(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
    /// Abandons the write and releases staging resources.
    fn abort(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
}

/// Product adapter used for complete, sequential representation writes.
pub trait DavWriteSystem: Send + Sync {
    /// Sequential write handle type produced by this system.
    type Handle: DavWriteHandle;

    /// Opens a sequential write under the given options and returns its handle.
    fn open_write<'a>(
        &'a self,
        path: &'a DavPath,
        options: DavWriteOptions,
    ) -> impl Future<Output = Result<Self::Handle, DavBackendError>> + Send + 'a;
}

/// Explicit random-write handle used only by negotiated partial-write capabilities.
///
/// Dropping a handle before `finish` or `abort` must roll back or clean up the uncommitted write.
/// It must never commit implicitly or leak staging resources. `finish` remains the sole commit
/// boundary, while `abort` is the explicit abandonment and cleanup path.
pub trait DavRandomWriteHandle: Send {
    /// Writes one chunk at an explicit byte offset.
    fn write_at(
        &mut self,
        offset: u64,
        buf: Bytes,
    ) -> impl Future<Output = Result<(), DavBackendError>> + Send + '_;
    /// Commits the partial write; the only success path that may make content visible.
    fn finish(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
    /// Abandons the partial write and releases staging resources.
    fn abort(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
}

/// Optional product adapter for random writes. Ordinary writers do not implement this port.
pub trait DavRandomWriteSystem: Send + Sync {
    /// Random-write handle type produced by this system.
    type Handle: DavRandomWriteHandle;

    /// Opens an explicit-offset write under the given options and returns its handle.
    fn open_random_write<'a>(
        &'a self,
        path: &'a DavPath,
        options: DavWriteOptions,
    ) -> impl Future<Output = Result<Self::Handle, DavBackendError>> + Send + 'a;
}

/// Stored dead property exchanged with the product adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavProp {
    /// Dead-property local name.
    pub name: String,
    /// Namespace prefix used for serialization, when known.
    pub prefix: Option<String>,
    /// Namespace URI of the property, when declared.
    pub namespace: Option<String>,
    /// Raw XML value bytes; `None` for properties without content.
    pub xml: Option<Vec<u8>>,
}

/// Canonical resource and dead-property backend port.
pub trait DavFileSystem: Send + Sync {
    /// Reads protocol-visible metadata for one path.
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>>;
    /// Creates the collection at `path`, enforcing the submitted lock credentials.
    fn create_dir<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: DavMutationCredentials,
    ) -> FsFuture<'a, ()>;
    /// Removes the empty collection at `path`.
    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()>;
    /// Removes the file resource at `path`.
    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()>;
    /// Moves or renames the resource at `from` to `to`.
    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()>;
    /// Duplicates the resource at `from` to `to` as a deep copy.
    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()>;

    /// Returns used bytes and the quota limit, when the adapter enforces one.
    fn get_quota(&self) -> FsFuture<'_, (u64, Option<u64>)> {
        Box::pin(async { Ok((0, None)) })
    }

    /// Reports whether the adapter can serve dead properties.
    fn have_props<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { false })
    }

    /// Returns stored dead properties for one path.
    ///
    /// When `do_content` is set, returned values include their XML content.
    fn get_props<'a>(
        &'a self,
        _path: &'a DavPath,
        _do_content: bool,
    ) -> FsFuture<'a, Vec<DavProp>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Serial fallback that calls [`DavFileSystem::get_props`] once per path.
    ///
    /// Production adapters should override this method when the persistence layer supports a
    /// genuine batch query.
    fn get_props_many<'a>(
        &'a self,
        paths: &'a [DavPath],
        do_content: bool,
    ) -> FsFuture<'a, HashMap<DavPath, Vec<DavProp>>> {
        Box::pin(async move {
            let mut result = HashMap::with_capacity(paths.len());
            for path in paths {
                result.insert(path.clone(), self.get_props(path, do_content).await?);
            }
            Ok(result)
        })
    }

    /// Applies one dead-property patch batch and reports a status per property.
    fn patch_props<'a>(
        &'a self,
        _path: &'a DavPath,
        _patches: Vec<(bool, DavProp)>,
    ) -> FsFuture<'a, Vec<(StatusCode, DavProp)>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Protocol-visible lock state persisted by the product adapter.
#[derive(Debug, Clone)]
pub struct DavLock {
    /// Opaque lock token URI identifying this lock.
    pub token: String,
    /// Root path the lock directly covers.
    pub path: Box<DavPath>,
    /// Authenticated principal that created the lock, when known.
    pub principal: Option<String>,
    /// Client-supplied `owner` XML preserved verbatim.
    pub owner: Option<Box<DavXmlElement>>,
    /// Absolute expiry time, when the adapter tracks one.
    pub timeout_at: Option<SystemTime>,
    /// Remaining lifetime granted at creation or refresh.
    pub timeout: Option<Duration>,
    /// Whether the lock is shared instead of exclusive.
    pub shared: bool,
    /// Whether the lock covers the whole subtree below `path`.
    pub deep: bool,
}

/// Pinned future type used by the [`DavLockSystem`] port.
pub type LsFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Failure reported by lock preflight before persistence is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavLockPreflightError {
    /// Lock count or depth limit for the target scope is exceeded.
    LimitExceeded,
    /// Preflight failure without a more specific category.
    GeneralFailure,
}

/// Failure while acquiring, refreshing, validating, or removing locks.
#[derive(Debug, Clone)]
pub enum DavLockError {
    /// An existing lock conflicts with the request; carries the blocking lock state.
    Conflict(Box<DavLock>),
    /// Parent collection required by the lock request does not exist.
    ParentMissing,
    /// Submitted token does not match the lock being modified.
    TokenMismatch,
    /// Lock count or depth limit for the target scope is exceeded.
    LimitExceeded,
    /// Referenced lock or resource does not exist.
    NotFound,
    /// Adapter-side persistence failure without a protocol mapping.
    Backend,
}

/// Outcome of a successful lock acquisition.
#[derive(Debug, Clone)]
pub struct DavLockAcquireResult {
    /// Persisted lock state as created.
    pub lock: DavLock,
    /// Whether the locked resource already existed before the request.
    pub resource_existed: bool,
}

/// Product backend input for acquiring a new `WebDAV` lock.
#[derive(Debug)]
pub struct DavLockAcquireRequest<'a> {
    /// Resource path the lock root covers.
    pub path: &'a DavPath,
    /// Authenticated principal creating the lock, when known.
    pub principal: Option<&'a str>,
    /// Client-supplied `owner` XML to persist with the lock.
    pub owner: Option<&'a DavXmlElement>,
    /// Requested lifetime; the adapter may clamp it.
    pub timeout: Option<Duration>,
    /// Whether the lock is shared instead of exclusive.
    pub shared: bool,
    /// Whether the lock covers the subtree below `path`.
    pub deep: bool,
    /// Validated mutation credentials such as submitted lock tokens.
    pub credentials: DavMutationCredentials,
}

/// Canonical lock persistence and conflict backend port.
pub trait DavLockSystem: Send + Sync {
    /// Runs adapter-side preflight checks before a lock is created.
    ///
    /// # Errors
    ///
    /// Returns a preflight error when the target scope cannot accept another lock.
    fn prepare_lock(&self, _path: &DavPath) -> LsFuture<'_, Result<(), DavLockPreflightError>> {
        Box::pin(async { Ok(()) })
    }

    /// Acquires a new lock atomically after preflight succeeded.
    ///
    /// # Errors
    ///
    /// Returns a typed lock failure such as a conflicting lock or a missing parent.
    fn lock<'a>(
        &'a self,
        request: DavLockAcquireRequest<'a>,
    ) -> LsFuture<'a, Result<DavLockAcquireResult, DavLockError>>;

    /// Removes the lock identified by `token` covering `path`.
    fn unlock(&self, path: &DavPath, token: &str) -> LsFuture<'_, Result<(), DavLockError>>;
    /// Refreshes the lock lifetime, optionally negotiating a new timeout.
    fn refresh(
        &self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, DavLockError>>;
    /// Validates lock coverage for one mutation against the submitted tokens.
    fn check(
        &self,
        path: &DavPath,
        principal: Option<&str>,
        ignore_principal: bool,
        deep: bool,
        submitted_tokens: &[String],
    ) -> LsFuture<'_, Result<(), DavLockError>>;
    /// Returns all locks directly or deeply covering `path`.
    fn discover(&self, path: &DavPath) -> LsFuture<'_, Result<Vec<DavLock>, DavBackendError>>;
    /// Serial fallback that calls [`DavLockSystem::discover`] once per path.
    ///
    /// Production adapters should override this method when the persistence layer supports a
    /// genuine batch query.
    fn discover_many<'a>(
        &'a self,
        paths: &'a [DavPath],
    ) -> LsFuture<'a, Result<HashMap<DavPath, Vec<DavLock>>, DavBackendError>> {
        Box::pin(async move {
            let mut result = HashMap::with_capacity(paths.len());
            for path in paths {
                result.insert(path.clone(), self.discover(path).await?);
            }
            Ok(result)
        })
    }
    /// Returns locks that would conflict with a mutation of the requested depth.
    fn conflicting_locks(
        &self,
        path: &DavPath,
        deep: bool,
    ) -> LsFuture<'_, Result<Vec<DavLock>, DavBackendError>>;
    /// Removes all locks under `path`, matching DELETE rollback semantics.
    fn delete(&self, path: &DavPath) -> LsFuture<'_, Result<(), DavLockError>>;
}
