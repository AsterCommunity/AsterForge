//! Transport-neutral WebDAV response model.

use bytes::Bytes;
use http::{HeaderMap, StatusCode};

use crate::DavContentStream;

/// WebDAV response body before transport adaptation.
pub enum DavResponseBody {
    Empty,
    Bytes(Bytes),
    Stream(DavContentStream),
}

/// Status, headers, and body produced by the protocol layer.
pub struct DavResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: DavResponseBody,
}

impl DavResponse {
    /// Creates an empty response.
    #[must_use]
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Empty,
        }
    }

    /// Creates a byte response.
    #[must_use]
    pub fn bytes(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Bytes(body.into()),
        }
    }
}
