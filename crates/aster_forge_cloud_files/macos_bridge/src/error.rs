//! File Provider bridge errors and portable native classifications.

use aster_forge_cloud_files_core::{CloudBackendError, CloudBackendErrorKind, SessionState};

/// Result returned by the macOS File Provider bridge.
pub type Result<T> = std::result::Result<T, MacosBridgeError>;

/// Product-neutral classification mapped by Swift to `NSFileProviderError` or `CocoaError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MacosErrorCode {
    /// Operation completed successfully.
    Success = 0,
    /// Item or container does not exist.
    NotFound = 1,
    /// User authentication is required before the operation can continue.
    NotAuthenticated = 2,
    /// The authenticated principal lacks permission for the operation.
    PermissionDenied = 3,
    /// Requested item version is stale.
    VersionOutOfDate = 4,
    /// Request should be retried after a transient condition.
    TryAgain = 5,
    /// Operation is unsupported by the active adapter/backend.
    NotSupported = 6,
    /// Native request or FFI input is invalid.
    InvalidArgument = 7,
    /// Enumeration state must be rebuilt from a clean anchor/page.
    SyncAnchorExpired = 8,
    /// Request was cancelled.
    Cancelled = 9,
    /// Extension session is closing or closed.
    ProviderNotFound = 10,
    /// Internal, contract, transport, or panic failure.
    Internal = 11,
}

/// Errors produced by identifier, enumeration, session, and FFI mechanics.
#[derive(Debug, thiserror::Error)]
pub enum MacosBridgeError {
    /// A persistent item identifier used an unsupported or malformed envelope.
    #[error("invalid macOS File Provider identifier: {reason}")]
    InvalidIdentifier {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A system container was used where a product item was required.
    #[error("File Provider system container cannot be decoded as a cloud item")]
    SystemContainerIsNotItem,
    /// An item version cannot represent one or both revision values.
    #[error("invalid macOS File Provider item version: {reason}")]
    InvalidItemVersion {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A backend item violated its stable identity, parent, name, or content contract.
    #[error("invalid cloud backend response: {reason}")]
    InvalidBackendResponse {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A page request targeted a system container unsupported by this batch.
    #[error("unsupported File Provider system container for enumeration")]
    UnsupportedSystemContainer,
    /// A callback generation did not match the active extension generation.
    #[error("stale extension generation: expected {expected}, got {actual}")]
    StaleSessionGeneration {
        /// Active generation.
        expected: u64,
        /// Callback generation.
        actual: u64,
    },
    /// The extension session rejected new work.
    #[error("extension session is not accepting requests: {state:?}")]
    SessionNotAccepting {
        /// Current lifecycle state.
        state: SessionState,
    },
    /// A lifecycle transition occurred out of order.
    #[error("invalid extension lifecycle transition from {from:?} to {to:?}")]
    InvalidSessionTransition {
        /// Current state.
        from: SessionState,
        /// Requested state.
        to: SessionState,
    },
    /// Accepted request count exceeded the host address space.
    #[error("active File Provider request count overflow")]
    ActiveRequestCountOverflow,
    /// An FFI pointer/length pair or UTF-8 input was invalid.
    #[error("invalid macOS bridge FFI input: {reason}")]
    InvalidFfiInput {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A panic was contained at the C ABI boundary.
    #[error("panic contained at macOS bridge FFI boundary")]
    FfiPanic,
    /// The product-owned backend reported a classified failure.
    #[error(transparent)]
    Backend(#[from] CloudBackendError),
}

impl MacosBridgeError {
    /// Returns the native-facing portable classification.
    #[must_use]
    pub const fn error_code(&self) -> MacosErrorCode {
        match self {
            Self::InvalidIdentifier { .. }
            | Self::SystemContainerIsNotItem
            | Self::InvalidItemVersion { .. }
            | Self::InvalidFfiInput { .. } => MacosErrorCode::InvalidArgument,
            Self::UnsupportedSystemContainer => MacosErrorCode::NotSupported,
            Self::StaleSessionGeneration { .. } => MacosErrorCode::ProviderNotFound,
            Self::SessionNotAccepting { .. } | Self::InvalidSessionTransition { .. } => {
                MacosErrorCode::ProviderNotFound
            }
            Self::ActiveRequestCountOverflow
            | Self::InvalidBackendResponse { .. }
            | Self::FfiPanic => MacosErrorCode::Internal,
            Self::Backend(error) => backend_error_code(error.kind()),
        }
    }
}

/// Maps product-neutral backend failures to File Provider-facing classifications.
#[must_use]
pub const fn backend_error_code(kind: CloudBackendErrorKind) -> MacosErrorCode {
    match kind {
        CloudBackendErrorKind::NotFound => MacosErrorCode::NotFound,
        CloudBackendErrorKind::AuthenticationRequired => MacosErrorCode::NotAuthenticated,
        CloudBackendErrorKind::PermissionDenied => MacosErrorCode::PermissionDenied,
        CloudBackendErrorKind::Conflict | CloudBackendErrorKind::PreconditionFailed => {
            MacosErrorCode::VersionOutOfDate
        }
        CloudBackendErrorKind::InvalidRequest => MacosErrorCode::InvalidArgument,
        CloudBackendErrorKind::RateLimited | CloudBackendErrorKind::TemporarilyUnavailable => {
            MacosErrorCode::TryAgain
        }
        CloudBackendErrorKind::Unsupported => MacosErrorCode::NotSupported,
        CloudBackendErrorKind::InvalidResponse | CloudBackendErrorKind::Internal => {
            MacosErrorCode::Internal
        }
    }
}
