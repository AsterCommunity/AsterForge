//! Product adapter ports consumed by the WebDAV protocol layer.

use std::collections::HashMap;
use std::future::Future;
use std::io::SeekFrom;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::{Buf, Bytes};
use futures::Stream;
use http::StatusCode;

use crate::{DavPath, DavXmlElement};

/// Stream used for product-independent WebDAV content transfer.
pub type DavContentStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DavBackendError>> + Send + 'static>>;

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

/// Opaque product-side identity used to batch dead-property reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DavPropertyTarget {
    pub kind: DavResourceKind,
    pub id: i64,
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

/// File open contract selected by the protocol planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub truncate: bool,
    pub create: bool,
    pub create_new: bool,
    pub size: Option<u64>,
    pub checksum: Option<String>,
}

impl OpenOptions {
    #[must_use]
    pub fn read() -> Self {
        Self {
            read: true,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn write() -> Self {
        Self {
            write: true,
            ..Self::default()
        }
    }
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
    fn property_target(&self) -> Option<DavPropertyTarget> {
        None
    }
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

/// Open file handle supplied by a product adapter.
pub trait DavFile: Send {
    fn metadata<'a>(&'a mut self) -> FsFuture<'a, Box<dyn DavMetaData>>;
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes>;
    fn write_bytes(&mut self, buf: Bytes) -> FsFuture<'_, ()>;
    fn write_buf(&mut self, buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()>;
    fn seek(&mut self, pos: SeekFrom) -> FsFuture<'_, u64>;
    fn flush(&mut self) -> FsFuture<'_, ()>;
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
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>>;
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

    fn get_props_many_for_targets<'a>(
        &'a self,
        targets: &'a [(DavPath, DavPropertyTarget)],
        do_content: bool,
    ) -> FsFuture<'a, HashMap<DavPath, Vec<DavProp>>> {
        Box::pin(async move {
            let paths = targets
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            self.get_props_many(&paths, do_content).await
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
    Conflict(DavLock),
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

    fn unlock(&self, path: &DavPath, token: &str) -> LsFuture<'_, Result<(), ()>>;
    fn refresh(
        &self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, ()>>;
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
    fn delete(&self, path: &DavPath) -> LsFuture<'_, Result<(), ()>>;
}

/// Lock value used by protocol response composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavLockInfo {
    pub token: String,
    pub path: DavPath,
    pub owner_xml: Option<DavXmlElement>,
    pub timeout_at: Option<SystemTime>,
    pub timeout: Option<Duration>,
    pub shared: bool,
    pub deep: bool,
}
