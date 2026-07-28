//! Product adapter ports consumed by the WebDAV protocol layer.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use aster_forge_utils::http_range::HttpByteRange;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use http::StatusCode;

use crate::{DavPath, DavXmlElement};

/// Stream used for product-independent WebDAV content transfer.
pub type DavContentStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DavBackendError>> + Send + 'static>>;

/// Opened representation stream and the exact number of bytes it is expected to yield.
pub struct DavOpenedDownload {
    pub stream: DavContentStream,
    pub expected_length: u64,
}

impl DavOpenedDownload {
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
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error("download source length does not match the protocol plan")]
    LengthMismatch { planned: u64, opened: u64 },
}

/// Stable backend failure categories mapped by the protocol layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavBackendErrorKind {
    NotFound,
    Forbidden,
    Conflict,
    AlreadyExists,
    InsufficientStorage,
    PayloadTooLarge,
    Locked,
    InvalidInput,
    Unsupported,
    Internal,
}

/// Product-neutral failure returned by a product adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("WebDAV backend operation failed: {kind:?}")]
pub struct DavBackendError {
    pub kind: DavBackendErrorKind,
}

impl DavBackendError {
    #[must_use]
    pub const fn new(kind: DavBackendErrorKind) -> Self {
        Self { kind }
    }
}

/// Low-level file-system failure exposed by the product adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("general failure")]
    GeneralFailure,
    #[error("already exists")]
    Exists,
    #[error("insufficient storage")]
    InsufficientStorage,
    #[error("too large")]
    TooLarge,
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

pub type FsResult<T> = Result<T, FsError>;
pub type FsFuture<'a, T> = Pin<Box<dyn Future<Output = FsResult<T>> + Send + 'a>>;
pub type FsStream<T> = Pin<Box<dyn Stream<Item = FsResult<T>> + Send>>;

/// WebDAV resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DavResourceKind {
    File,
    Collection,
}

/// Protocol-visible state used to evaluate one resource referenced by an `If` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavIfResourceState {
    pub etag: Option<String>,
    pub lock_tokens: Vec<String>,
}

/// Product adapter used while evaluating WebDAV `If` conditions.
#[async_trait]
pub trait DavIfStateResolver: Send + Sync {
    async fn resolve_if_state(&self, path: &DavPath)
    -> Result<DavIfResourceState, DavBackendError>;
}

/// Metadata loading mode for directory entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDirMeta {
    Data,
}

/// Sequential write contract selected by the protocol planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavWriteOptions {
    pub truncate: bool,
    pub create: bool,
    pub create_new: bool,
    pub expected_length: Option<u64>,
    pub checksum: Option<String>,
}

/// Protocol-visible resource metadata supplied by the product adapter.
pub trait DavMetaData: Send + Sync {
    fn len(&self) -> u64;
    fn modified(&self) -> FsResult<SystemTime>;
    fn is_dir(&self) -> bool;
    fn etag(&self) -> Option<String>;
    fn content_type(&self) -> Option<&str> {
        None
    }
    fn created(&self) -> FsResult<SystemTime>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn is_file(&self) -> bool {
        !self.is_dir()
    }
}

/// One directory entry returned by a product adapter.
pub trait DavDirEntry: Send {
    fn name(&self) -> Vec<u8>;
    fn metadata<'a>(&'a self) -> FsFuture<'a, Box<dyn DavMetaData>>;
}

/// Product adapter used by GET/HEAD after the protocol layer selects a representation.
pub trait DavDownloadSource: Send + Sync {
    type Metadata: DavMetaData;

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> impl Future<Output = Result<Self::Metadata, DavBackendError>> + Send + 'a;
    fn open_full<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> impl Future<Output = Result<DavOpenedDownload, DavBackendError>> + Send + 'a;
    fn open_range<'a>(
        &'a self,
        path: &'a DavPath,
        range: HttpByteRange,
    ) -> impl Future<Output = Result<DavOpenedDownload, DavBackendError>> + Send + 'a;
}

/// Sequential write handle. A successful `finish` is the product adapter's commit boundary.
pub trait DavWriteHandle: Send {
    fn write_bytes(
        &mut self,
        buf: Bytes,
    ) -> impl Future<Output = Result<(), DavBackendError>> + Send + '_;
    fn finish(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
    fn abort(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
}

/// Product adapter used for complete, sequential representation writes.
pub trait DavWriteSystem: Send + Sync {
    type Handle: DavWriteHandle;

    fn open_write<'a>(
        &'a self,
        path: &'a DavPath,
        options: DavWriteOptions,
    ) -> impl Future<Output = Result<Self::Handle, DavBackendError>> + Send + 'a;
}

/// Explicit random-write handle used only by negotiated partial-write capabilities.
pub trait DavRandomWriteHandle: Send {
    fn write_at(
        &mut self,
        offset: u64,
        buf: Bytes,
    ) -> impl Future<Output = Result<(), DavBackendError>> + Send + '_;
    fn finish(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
    fn abort(self) -> impl Future<Output = Result<(), DavBackendError>> + Send;
}

/// Optional product adapter for random writes. Ordinary writers do not implement this port.
pub trait DavRandomWriteSystem: Send + Sync {
    type Handle: DavRandomWriteHandle;

    fn open_random_write<'a>(
        &'a self,
        path: &'a DavPath,
        options: DavWriteOptions,
    ) -> impl Future<Output = Result<Self::Handle, DavBackendError>> + Send + 'a;
}

/// Stored dead property exchanged with the product adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavProp {
    pub name: String,
    pub prefix: Option<String>,
    pub namespace: Option<String>,
    pub xml: Option<Vec<u8>>,
}

/// Canonical resource and dead-property backend port.
pub trait DavFileSystem: Send + Sync {
    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>>;
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>>;
    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()>;
    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()>;
    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()>;
    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()>;
    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()>;

    fn get_quota(&self) -> FsFuture<'_, (u64, Option<u64>)> {
        Box::pin(async { Ok((0, None)) })
    }

    fn have_props<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { false })
    }

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
    pub token: String,
    pub path: Box<DavPath>,
    pub principal: Option<String>,
    pub owner: Option<Box<DavXmlElement>>,
    pub timeout_at: Option<SystemTime>,
    pub timeout: Option<Duration>,
    pub shared: bool,
    pub deep: bool,
}

pub type LsFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavLockPreflightError {
    LimitExceeded,
    GeneralFailure,
}

#[derive(Debug, Clone)]
pub enum DavLockError {
    Conflict(Box<DavLock>),
    TokenMismatch,
    LimitExceeded,
    Backend,
}

/// Canonical lock persistence and conflict backend port.
pub trait DavLockSystem: Send + Sync {
    fn prepare_lock(&self, _path: &DavPath) -> LsFuture<'_, Result<(), DavLockPreflightError>> {
        Box::pin(async { Ok(()) })
    }

    fn lock(
        &self,
        path: &DavPath,
        principal: Option<&str>,
        owner: Option<&DavXmlElement>,
        timeout: Option<Duration>,
        shared: bool,
        deep: bool,
    ) -> LsFuture<'_, Result<DavLock, DavLockError>>;

    fn unlock(&self, path: &DavPath, token: &str) -> LsFuture<'_, Result<(), DavLockError>>;
    fn refresh(
        &self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, DavLockError>>;
    fn check(
        &self,
        path: &DavPath,
        principal: Option<&str>,
        ignore_principal: bool,
        deep: bool,
        submitted_tokens: &[String],
    ) -> LsFuture<'_, Result<(), DavLock>>;
    fn discover(&self, path: &DavPath) -> LsFuture<'_, Vec<DavLock>>;
    fn discover_many<'a>(
        &'a self,
        paths: &'a [DavPath],
    ) -> LsFuture<'a, HashMap<DavPath, Vec<DavLock>>> {
        Box::pin(async move {
            let mut result = HashMap::with_capacity(paths.len());
            for path in paths {
                result.insert(path.clone(), self.discover(path).await);
            }
            result
        })
    }
    fn conflicting_locks(&self, path: &DavPath, deep: bool) -> LsFuture<'_, Vec<DavLock>>;
    fn delete(&self, path: &DavPath) -> LsFuture<'_, Result<(), DavLockError>>;
}
