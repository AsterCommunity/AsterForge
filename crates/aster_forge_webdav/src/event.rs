//! Observable WebDAV operation events.

use std::time::Duration;

use crate::{DavBackendErrorKind, DavPath, DavRequestHead};

/// Protocol operations exposed to event observers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavOperation {
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

/// Protocol result exposed to observers without credentials, bodies, or lock tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavEventOutcome {
    Succeeded {
        /// HTTP/WebDAV response status without leaking a transport crate version.
        status: u16,
    },
    Failed {
        /// HTTP/WebDAV response status without leaking a transport crate version.
        status: u16,
        backend_error: Option<DavBackendErrorKind>,
    },
}

impl DavEventOutcome {
    /// Classifies a completed response. Informational, success, and redirection statuses are
    /// successful protocol outcomes; client and server errors are failures.
    #[must_use]
    pub const fn from_status(status: u16, backend_error: Option<DavBackendErrorKind>) -> Self {
        if status < 400 {
            Self::Succeeded { status }
        } else {
            Self::Failed {
                status,
                backend_error,
            }
        }
    }

    /// Returns the completed HTTP/WebDAV status.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Succeeded { status } | Self::Failed { status, .. } => status,
        }
    }
}

/// One completed WebDAV operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavEvent {
    pub request_id: Option<String>,
    pub operation: DavOperation,
    pub source: DavPath,
    pub destination: Option<DavPath>,
    pub outcome: DavEventOutcome,
    pub elapsed: Duration,
}

impl DavEvent {
    /// Builds the transport-neutral event for one completed request.
    ///
    /// Only protocol routing data is copied from the request head. Conditional headers,
    /// credentials, request bodies, and lock tokens are deliberately excluded.
    #[must_use]
    pub fn completed(
        request_head: &DavRequestHead,
        status: u16,
        elapsed: Duration,
        backend_error: Option<DavBackendErrorKind>,
    ) -> Self {
        Self {
            request_id: None,
            operation: request_head.method.operation(),
            source: request_head.target.clone(),
            destination: request_head
                .destination
                .as_ref()
                .map(|destination| destination.path.clone()),
            outcome: DavEventOutcome::from_status(status, backend_error),
            elapsed,
        }
    }
}

/// Non-authoritative observer for audit adapters, metrics, tracing, and notifications.
///
/// Required mutations, quota updates, lock persistence, and cache correctness must complete in
/// the synchronous backend operation before this observer is called.
pub trait DavEventSink: Send + Sync {
    fn publish(&self, event: &DavEvent);
}

/// Event sink used when a product does not need protocol-level observation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDavEventSink;

impl DavEventSink for NoopDavEventSink {
    fn publish(&self, _event: &DavEvent) {}
}
