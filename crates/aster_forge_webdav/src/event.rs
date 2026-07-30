//! Observable WebDAV operation events.

use std::time::Duration;
use std::{panic::AssertUnwindSafe, panic::catch_unwind};

use crate::{DavBackendErrorKind, DavPath, DavRequestHead};

/// Protocol operations exposed to event observers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavOperation {
    Options,
    Propfind,
    Proppatch,
    Get,
    Head,
    Post,
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

/// Product-neutral class for a protocol-side failure observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavProtocolFailureClass {
    Request,
    Precondition,
    Capability,
    Backend,
    Response,
    Transport,
}

/// Outcome of a response stream after the operation has been planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavStreamOutcome {
    Completed,
    Cancelled { response_started: bool },
    Failed { response_started: bool },
}

/// Low-frequency scalar observations attached to a completed operation.
///
/// `None` means that a product did not collect that fact; zero is a collected zero. The type
/// intentionally contains no request body, credential, lock token, object key, or label string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavOperationObservations {
    pub bytes_received: Option<u64>,
    pub bytes_sent: Option<u64>,
    pub requested_ranges: Option<u64>,
    pub served_ranges: Option<u64>,
    pub resources: Option<u64>,
    pub backend_open_count: Option<u64>,
    pub backend_call_count: Option<u64>,
    pub protocol_failure: Option<DavProtocolFailureClass>,
    pub stream: Option<DavStreamOutcome>,
}

/// Failure reported by a non-authoritative observation sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("WebDAV observation sink failed")]
pub struct DavObservationError;

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
    pub observations: DavOperationObservations,
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
        Self::completed_with_observations(
            request_head,
            status,
            elapsed,
            backend_error,
            DavOperationObservations::default(),
        )
    }

    /// Builds a completed event with optional low-frequency observations.
    #[must_use]
    pub fn completed_with_observations(
        request_head: &DavRequestHead,
        status: u16,
        elapsed: Duration,
        backend_error: Option<DavBackendErrorKind>,
        observations: DavOperationObservations,
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
            observations,
        }
    }
}

/// Non-authoritative observer for audit adapters, metrics, tracing, and notifications.
///
/// Required mutations, quota updates, lock persistence, and cache correctness must complete in
/// the synchronous backend operation before this observer is called. `publish` must return
/// promptly without blocking on I/O; products that need asynchronous work must use a bounded,
/// non-blocking enqueue into a product-owned worker.
pub trait DavEventSink: Send + Sync {
    fn publish(&self, event: &DavEvent) -> Result<(), DavObservationError>;
}

/// Publishes a non-authoritative event without allowing observer failure to affect the operation.
pub fn publish_non_authoritative(sink: Option<&dyn DavEventSink>, event: &DavEvent) {
    if let Some(sink) = sink {
        let _ = catch_unwind(AssertUnwindSafe(|| sink.publish(event)));
    }
}

/// Event sink used when a product does not need protocol-level observation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDavEventSink;

impl DavEventSink for NoopDavEventSink {
    fn publish(&self, _event: &DavEvent) -> Result<(), DavObservationError> {
        Ok(())
    }
}
