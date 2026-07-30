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
    Patch,
    Mkcol,
    Delete,
    Copy,
    Move,
    Lock,
    Unlock,
    Acl,
    Report,
    VersionControl,
    Checkout,
    Checkin,
    Uncheckout,
    Mkworkspace,
    Update,
    Label,
    Merge,
    BaselineControl,
    Mkactivity,
    Search,
    Orderpatch,
    Mkredirectref,
    Updateredirectref,
    Bind,
    Unbind,
    Rebind,
    Post,
}

/// How the transport adapter must handle a request body before product code runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavBodyPolicy {
    /// Reject the first non-empty body chunk.
    Empty,
    /// Collect an XML body up to the supplied byte limit.
    BoundedXml { maximum: usize },
    /// Collect an opaque body up to the patch format's byte limit.
    Bounded { maximum: usize },
    /// Leave the body as a stream for the product storage adapter.
    Stream,
    /// Preserve the existing method behavior without consuming the body.
    Unused,
}

impl DavMethod {
    /// Methods in the canonical `Allow` rendering order.
    pub const ALL: [Self; 33] = [
        Self::Options,
        Self::Get,
        Self::Head,
        Self::Post,
        Self::Put,
        Self::Patch,
        Self::Delete,
        Self::Mkcol,
        Self::Copy,
        Self::Move,
        Self::Propfind,
        Self::Proppatch,
        Self::Lock,
        Self::Unlock,
        Self::Acl,
        Self::Report,
        Self::VersionControl,
        Self::Checkout,
        Self::Checkin,
        Self::Uncheckout,
        Self::Mkworkspace,
        Self::Update,
        Self::Label,
        Self::Merge,
        Self::BaselineControl,
        Self::Mkactivity,
        Self::Search,
        Self::Orderpatch,
        Self::Mkredirectref,
        Self::Updateredirectref,
        Self::Bind,
        Self::Unbind,
        Self::Rebind,
    ];

    #[must_use]
    pub const fn index(self) -> u32 {
        match self {
            Self::Options => 0,
            Self::Get => 1,
            Self::Head => 2,
            Self::Post => 3,
            Self::Put => 4,
            Self::Patch => 5,
            Self::Delete => 6,
            Self::Mkcol => 7,
            Self::Copy => 8,
            Self::Move => 9,
            Self::Propfind => 10,
            Self::Proppatch => 11,
            Self::Lock => 12,
            Self::Unlock => 13,
            Self::Acl => 14,
            Self::Report => 15,
            Self::VersionControl => 16,
            Self::Checkout => 17,
            Self::Checkin => 18,
            Self::Uncheckout => 19,
            Self::Mkworkspace => 20,
            Self::Update => 21,
            Self::Label => 22,
            Self::Merge => 23,
            Self::BaselineControl => 24,
            Self::Mkactivity => 25,
            Self::Search => 26,
            Self::Orderpatch => 27,
            Self::Mkredirectref => 28,
            Self::Updateredirectref => 29,
            Self::Bind => 30,
            Self::Unbind => 31,
            Self::Rebind => 32,
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
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Mkcol => "MKCOL",
            Self::Delete => "DELETE",
            Self::Copy => "COPY",
            Self::Move => "MOVE",
            Self::Lock => "LOCK",
            Self::Unlock => "UNLOCK",
            Self::Acl => "ACL",
            Self::Report => "REPORT",
            Self::VersionControl => "VERSION-CONTROL",
            Self::Checkout => "CHECKOUT",
            Self::Checkin => "CHECKIN",
            Self::Uncheckout => "UNCHECKOUT",
            Self::Mkworkspace => "MKWORKSPACE",
            Self::Update => "UPDATE",
            Self::Label => "LABEL",
            Self::Merge => "MERGE",
            Self::BaselineControl => "BASELINE-CONTROL",
            Self::Mkactivity => "MKACTIVITY",
            Self::Search => "SEARCH",
            Self::Orderpatch => "ORDERPATCH",
            Self::Mkredirectref => "MKREDIRECTREF",
            Self::Updateredirectref => "UPDATEREDIRECTREF",
            Self::Bind => "BIND",
            Self::Unbind => "UNBIND",
            Self::Rebind => "REBIND",
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
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "MKCOL" => Some(Self::Mkcol),
            "DELETE" => Some(Self::Delete),
            "COPY" => Some(Self::Copy),
            "MOVE" => Some(Self::Move),
            "LOCK" => Some(Self::Lock),
            "UNLOCK" => Some(Self::Unlock),
            "ACL" => Some(Self::Acl),
            "REPORT" => Some(Self::Report),
            "VERSION-CONTROL" => Some(Self::VersionControl),
            "CHECKOUT" => Some(Self::Checkout),
            "CHECKIN" => Some(Self::Checkin),
            "UNCHECKOUT" => Some(Self::Uncheckout),
            "MKWORKSPACE" => Some(Self::Mkworkspace),
            "UPDATE" => Some(Self::Update),
            "LABEL" => Some(Self::Label),
            "MERGE" => Some(Self::Merge),
            "BASELINE-CONTROL" => Some(Self::BaselineControl),
            "MKACTIVITY" => Some(Self::Mkactivity),
            "SEARCH" => Some(Self::Search),
            "ORDERPATCH" => Some(Self::Orderpatch),
            "MKREDIRECTREF" => Some(Self::Mkredirectref),
            "UPDATEREDIRECTREF" => Some(Self::Updateredirectref),
            "BIND" => Some(Self::Bind),
            "UNBIND" => Some(Self::Unbind),
            "REBIND" => Some(Self::Rebind),
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
            Self::Post => DavOperation::Post,
            Self::Put => DavOperation::Put,
            Self::Patch => DavOperation::Patch,
            Self::Mkcol => DavOperation::Mkcol,
            Self::Delete => DavOperation::Delete,
            Self::Copy => DavOperation::Copy,
            Self::Move => DavOperation::Move,
            Self::Lock => DavOperation::Lock,
            Self::Unlock => DavOperation::Unlock,
            Self::Acl => DavOperation::Acl,
            Self::Report => DavOperation::Report,
            Self::VersionControl => DavOperation::VersionControl,
            Self::Checkout => DavOperation::Checkout,
            Self::Checkin => DavOperation::Checkin,
            Self::Uncheckout => DavOperation::Uncheckout,
            Self::Mkworkspace => DavOperation::Mkworkspace,
            Self::Update => DavOperation::Update,
            Self::Label => DavOperation::Label,
            Self::Merge => DavOperation::Merge,
            Self::BaselineControl => DavOperation::BaselineControl,
            Self::Mkactivity => DavOperation::Mkactivity,
            Self::Search => DavOperation::Search,
            Self::Orderpatch => DavOperation::Orderpatch,
            Self::Mkredirectref => DavOperation::Mkredirectref,
            Self::Updateredirectref => DavOperation::Updateredirectref,
            Self::Bind => DavOperation::Bind,
            Self::Unbind => DavOperation::Unbind,
            Self::Rebind => DavOperation::Rebind,
        }
    }
}

/// Compact, duplicate-free set of methods in canonical protocol order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavMethodSet(u64);

impl DavMethodSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_methods(methods: &[DavMethod]) -> Self {
        let mut set = Self::empty();
        let mut index = 0;
        while index < methods.len() {
            set = set.with(methods[index]);
            index += 1;
        }
        set
    }

    #[must_use]
    pub const fn with(self, method: DavMethod) -> Self {
        Self(self.0 | (1u64 << method.index()))
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, method: DavMethod) -> bool {
        self.0 & (1u64 << method.index()) != 0
    }

    #[must_use]
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn iter(self) -> DavMethodSetIter {
        DavMethodSetIter {
            set: self,
            index: 0,
        }
    }

    #[must_use]
    pub fn render(self) -> String {
        let mut rendered = String::new();
        for method in self.iter() {
            if !rendered.is_empty() {
                rendered.push_str(", ");
            }
            rendered.push_str(method.as_str());
        }
        rendered
    }
}

/// Iterator over methods in canonical protocol order.
#[derive(Debug, Clone, Copy)]
pub struct DavMethodSetIter {
    set: DavMethodSet,
    index: usize,
}

impl Iterator for DavMethodSetIter {
    type Item = DavMethod;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < DavMethod::ALL.len() {
            let method = DavMethod::ALL[self.index];
            self.index += 1;
            if self.set.contains(method) {
                return Some(method);
            }
        }
        None
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
pub struct DavRequestTarget<'a> {
    pub target: DavPath,
    pub origin: DavRequestOrigin,
    pub mount_path: &'a str,
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
    pub fn parse_target<'a>(
        uri: &Uri,
        mount_path: &'a str,
        origin: &DavRequestOrigin,
    ) -> Result<DavRequestTarget<'a>, DavProtocolError> {
        let relative = strip_mount_prefix(uri.path(), mount_path).ok_or_else(|| {
            DavProtocolError::bad_request("Request target must stay under WebDAV prefix")
        })?;
        let target = DavPath::new(relative)
            .map_err(|_| DavProtocolError::bad_request("Invalid request path"))?;
        Ok(DavRequestTarget {
            target,
            origin: origin.clone(),
            mount_path,
        })
    }

    /// Parses method-specific protocol headers after the target has been resolved.
    pub fn parse_known_method(
        method: DavMethod,
        request_target: &DavRequestTarget<'_>,
        headers: &HeaderMap,
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
                    request_target.mount_path,
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
