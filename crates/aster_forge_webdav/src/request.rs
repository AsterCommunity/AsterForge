//! Transport-neutral WebDAV request head parsing.

use http::{HeaderMap, Method, Uri};

use crate::DavPath;
use crate::event::DavOperation;
use crate::protocol::{
    DavProtocolError, Depth, Destination, IfHeader, destination_relative_path, parse_copy_depth,
    parse_delete_depth, parse_if_header, parse_lock_depth, parse_move_depth, parse_overwrite,
    parse_propfind_depth, strip_mount_prefix,
};

/// WebDAV method recognized by the protocol layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMethod {
    Options,
    Propfind,
    Proppatch,
    Get,
    Head,
    Put,
    Mkcol,
    Delete,
    Copy,
    Move,
    Lock,
    Unlock,
    Report,
    VersionControl,
}

/// How the transport adapter must handle a request body before product code runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavBodyPolicy {
    /// Reject the first non-empty body chunk.
    Empty,
    /// Collect the body up to the product-supplied XML limit.
    BoundedXml,
    /// Leave the body as a stream for the product storage adapter.
    Stream,
    /// Preserve the existing method behavior without consuming the body.
    Unused,
}

impl DavMethod {
    /// Methods in the canonical `Allow` rendering order.
    pub const ALL: [Self; 14] = [
        Self::Options,
        Self::Get,
        Self::Head,
        Self::Put,
        Self::Delete,
        Self::Mkcol,
        Self::Copy,
        Self::Move,
        Self::Propfind,
        Self::Proppatch,
        Self::Lock,
        Self::Unlock,
        Self::Report,
        Self::VersionControl,
    ];

    #[must_use]
    pub const fn index(self) -> u32 {
        match self {
            Self::Options => 0,
            Self::Get => 1,
            Self::Head => 2,
            Self::Put => 3,
            Self::Delete => 4,
            Self::Mkcol => 5,
            Self::Copy => 6,
            Self::Move => 7,
            Self::Propfind => 8,
            Self::Proppatch => 9,
            Self::Lock => 10,
            Self::Unlock => 11,
            Self::Report => 12,
            Self::VersionControl => 13,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Options => "OPTIONS",
            Self::Propfind => "PROPFIND",
            Self::Proppatch => "PROPPATCH",
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Put => "PUT",
            Self::Mkcol => "MKCOL",
            Self::Delete => "DELETE",
            Self::Copy => "COPY",
            Self::Move => "MOVE",
            Self::Lock => "LOCK",
            Self::Unlock => "UNLOCK",
            Self::Report => "REPORT",
            Self::VersionControl => "VERSION-CONTROL",
        }
    }

    /// Parses a supported HTTP/WebDAV method.
    #[must_use]
    pub fn from_method(method: &Method) -> Option<Self> {
        Self::from_name(method.as_str())
    }

    /// Parses a supported HTTP/WebDAV method name across transport implementations.
    #[must_use]
    pub fn from_name(method: &str) -> Option<Self> {
        match method {
            "OPTIONS" => Some(Self::Options),
            "PROPFIND" => Some(Self::Propfind),
            "PROPPATCH" => Some(Self::Proppatch),
            "GET" => Some(Self::Get),
            "HEAD" => Some(Self::Head),
            "PUT" => Some(Self::Put),
            "MKCOL" => Some(Self::Mkcol),
            "DELETE" => Some(Self::Delete),
            "COPY" => Some(Self::Copy),
            "MOVE" => Some(Self::Move),
            "LOCK" => Some(Self::Lock),
            "UNLOCK" => Some(Self::Unlock),
            "REPORT" => Some(Self::Report),
            "VERSION-CONTROL" => Some(Self::VersionControl),
            _ => None,
        }
    }

    /// Returns the corresponding observable operation.
    #[must_use]
    pub const fn operation(self) -> DavOperation {
        match self {
            Self::Options => DavOperation::Options,
            Self::Propfind => DavOperation::Propfind,
            Self::Proppatch => DavOperation::Proppatch,
            Self::Get => DavOperation::Get,
            Self::Head => DavOperation::Head,
            Self::Put => DavOperation::Put,
            Self::Mkcol => DavOperation::Mkcol,
            Self::Delete => DavOperation::Delete,
            Self::Copy => DavOperation::Copy,
            Self::Move => DavOperation::Move,
            Self::Lock => DavOperation::Lock,
            Self::Unlock => DavOperation::Unlock,
            Self::Report => DavOperation::Report,
            Self::VersionControl => DavOperation::VersionControl,
        }
    }

    /// Returns the body handling contract for this method.
    #[must_use]
    pub const fn body_policy(self) -> DavBodyPolicy {
        match self {
            Self::Options | Self::Mkcol | Self::Delete | Self::Copy | Self::Move | Self::Unlock => {
                DavBodyPolicy::Empty
            }
            Self::Propfind | Self::Proppatch | Self::Lock | Self::Report | Self::VersionControl => {
                DavBodyPolicy::BoundedXml
            }
            Self::Put => DavBodyPolicy::Stream,
            Self::Get | Self::Head => DavBodyPolicy::Unused,
        }
    }
}

/// Request origin needed for same-origin tagged URI and destination validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavRequestOrigin {
    pub scheme: String,
    pub host: String,
}

/// Parsed request target shared by known and unknown method handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavRequestTarget {
    pub target: DavPath,
    pub origin: DavRequestOrigin,
}

/// Parsed, body-independent WebDAV request data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavRequestHead {
    pub method: DavMethod,
    pub target: DavPath,
    pub origin: DavRequestOrigin,
    pub depth: Option<Depth>,
    pub overwrite: Option<bool>,
    pub destination: Option<Destination>,
    pub if_header: Option<IfHeader>,
}

impl DavRequestHead {
    /// Parses a mount-relative target before method dispatch or product code runs.
    pub fn parse_target(
        uri: &Uri,
        mount_path: &str,
        origin: &DavRequestOrigin,
    ) -> Result<DavRequestTarget, DavProtocolError> {
        let relative = strip_mount_prefix(uri.path(), mount_path).ok_or_else(|| {
            DavProtocolError::bad_request("Request target must stay under WebDAV prefix")
        })?;
        let target = DavPath::new(relative)
            .map_err(|_| DavProtocolError::bad_request("Invalid request path"))?;
        Ok(DavRequestTarget {
            target,
            origin: origin.clone(),
        })
    }

    /// Parses method-specific protocol headers after the target has been resolved.
    pub fn parse_known_method(
        method: DavMethod,
        request_target: &DavRequestTarget,
        headers: &HeaderMap,
        mount_path: &str,
    ) -> Result<Self, DavProtocolError> {
        let depth = match method {
            DavMethod::Propfind => Some(parse_propfind_depth(headers)?),
            DavMethod::Copy => Some(parse_copy_depth(headers)?),
            DavMethod::Move => Some(parse_move_depth(headers)?),
            DavMethod::Delete => Some(parse_delete_depth(headers)?),
            DavMethod::Lock => Some(parse_lock_depth(headers)?),
            _ => None,
        };
        let (overwrite, destination) = match method {
            DavMethod::Copy | DavMethod::Move => (
                Some(parse_overwrite(headers)?),
                Some(destination_relative_path(
                    headers,
                    mount_path,
                    &request_target.origin.scheme,
                    &request_target.origin.host,
                )?),
            ),
            _ => (None, None),
        };

        Ok(Self {
            method,
            target: request_target.target.clone(),
            origin: request_target.origin.clone(),
            depth,
            overwrite,
            destination,
            if_header: parse_if_header(headers)?,
        })
    }
}
