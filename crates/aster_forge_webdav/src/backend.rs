//! Product adapter ports consumed by a WebDAV protocol engine.

use std::pin::Pin;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::{DavPath, DavXmlElement, Depth};

/// Stream used for product-independent WebDAV content transfer.
pub type DavContentStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DavBackendError>> + Send + 'static>>;

/// Stable backend failure categories that the protocol layer can map to WebDAV responses.
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
    /// Stable failure category. Product details stay in product logs and errors.
    pub kind: DavBackendErrorKind,
}

impl DavBackendError {
    /// Creates a classified backend error.
    #[must_use]
    pub const fn new(kind: DavBackendErrorKind) -> Self {
        Self { kind }
    }
}

/// WebDAV resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavResourceKind {
    File,
    Collection,
}

/// Protocol-visible resource metadata supplied by the product adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavResourceMetadata {
    pub kind: DavResourceKind,
    pub content_length: u64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub created_at: Option<SystemTime>,
    pub modified_at: Option<SystemTime>,
}

/// Protocol-visible state used to evaluate one resource referenced by a WebDAV `If` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavIfResourceState {
    pub etag: Option<String>,
    pub lock_tokens: Vec<String>,
}

/// Product adapter used by the protocol layer while evaluating WebDAV `If` conditions.
#[async_trait]
pub trait DavIfStateResolver: Send + Sync {
    async fn resolve_if_state(&self, path: &DavPath)
    -> Result<DavIfResourceState, DavBackendError>;
}

/// One child returned by a collection listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavDirectoryEntry {
    pub path: DavPath,
    pub metadata: DavResourceMetadata,
}

/// Result of opening resource content for a WebDAV response.
pub struct DavReadOutcome {
    pub metadata: DavResourceMetadata,
    pub content: DavContentStream,
}

/// Parameters for a WebDAV `PUT` operation.
pub struct DavWriteRequest {
    pub path: DavPath,
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    pub checksum: Option<String>,
    pub overwrite: bool,
    pub content: DavContentStream,
}

/// Result of a successful `PUT` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavWriteOutcome {
    pub created: bool,
    pub metadata: DavResourceMetadata,
}

/// Resource operations that remain authoritative in the product adapter.
#[async_trait]
pub trait DavResourceBackend: Send + Sync {
    async fn metadata(&self, path: &DavPath) -> Result<DavResourceMetadata, DavBackendError>;
    async fn list(
        &self,
        path: &DavPath,
        depth: Depth,
    ) -> Result<Vec<DavDirectoryEntry>, DavBackendError>;
    async fn read(&self, path: &DavPath) -> Result<DavReadOutcome, DavBackendError>;
    async fn write(&self, request: DavWriteRequest) -> Result<DavWriteOutcome, DavBackendError>;
    async fn create_collection(&self, path: &DavPath) -> Result<(), DavBackendError>;
    async fn delete(&self, path: &DavPath, depth: Depth) -> Result<(), DavBackendError>;
    async fn copy(
        &self,
        source: &DavPath,
        destination: &DavPath,
        depth: Depth,
        overwrite: bool,
    ) -> Result<(), DavBackendError>;
    async fn move_resource(
        &self,
        source: &DavPath,
        destination: &DavPath,
        overwrite: bool,
    ) -> Result<(), DavBackendError>;
    async fn quota(&self) -> Result<(u64, Option<u64>), DavBackendError>;
}

/// Expanded DAV property name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DavPropertyName {
    pub namespace: Option<String>,
    pub local_name: String,
}

/// Stored dead-property value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavProperty {
    pub name: DavPropertyName,
    pub xml: Option<DavXmlElement>,
}

/// One property set/remove mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPropertyPatch {
    pub remove: bool,
    pub property: DavProperty,
}

/// Result of one property mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPropertyPatchOutcome {
    pub name: DavPropertyName,
    pub status: http::StatusCode,
}

/// Dead-property persistence supplied by the product adapter.
#[async_trait]
pub trait DavPropertyBackend: Send + Sync {
    async fn properties(
        &self,
        path: &DavPath,
        include_values: bool,
    ) -> Result<Vec<DavProperty>, DavBackendError>;
    async fn patch_properties(
        &self,
        path: &DavPath,
        patches: Vec<DavPropertyPatch>,
    ) -> Result<Vec<DavPropertyPatchOutcome>, DavBackendError>;
}

/// Parameters for acquiring a WebDAV lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavLockRequest {
    pub path: DavPath,
    pub owner_xml: Option<DavXmlElement>,
    pub timeout: Option<Duration>,
    pub shared: bool,
    pub deep: bool,
}

/// Protocol-visible lock state supplied by the product adapter.
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

/// Lock persistence supplied by the product adapter.
#[async_trait]
pub trait DavLockBackend: Send + Sync {
    async fn acquire(&self, request: DavLockRequest) -> Result<DavLockInfo, DavBackendError>;
    async fn refresh(
        &self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> Result<DavLockInfo, DavBackendError>;
    async fn release(&self, path: &DavPath, token: &str) -> Result<(), DavBackendError>;
    async fn discover(&self, path: &DavPath) -> Result<Vec<DavLockInfo>, DavBackendError>;
    async fn check_write(
        &self,
        path: &DavPath,
        deep: bool,
        submitted_tokens: &[String],
    ) -> Result<(), DavBackendError>;
}

/// One protocol-visible resource version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionInfo {
    pub version_id: String,
    pub href: String,
    pub created_at: Option<SystemTime>,
    pub etag: Option<String>,
}

/// Optional DeltaV capability supplied by the product adapter.
#[async_trait]
pub trait DavVersionBackend: Send + Sync {
    async fn versions(&self, path: &DavPath) -> Result<Vec<DavVersionInfo>, DavBackendError>;
    async fn enable_version_control(&self, path: &DavPath) -> Result<(), DavBackendError>;
}

/// Aggregate capability boundary required by a complete WebDAV protocol engine.
pub trait DavBackend: DavResourceBackend + DavPropertyBackend + DavLockBackend {}

impl<T> DavBackend for T where T: DavResourceBackend + DavPropertyBackend + DavLockBackend {}
