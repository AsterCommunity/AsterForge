//! Structured Windows adapter failures.

use thiserror::Error;

/// Result type returned by the Windows cloud-files adapter.
pub type Result<T> = std::result::Result<T, WindowsCloudFilesError>;

/// Product-neutral classifications for Windows-specific mapping and native-call failures.
#[derive(Debug, Error)]
pub enum WindowsCloudFilesError {
    /// The encoded CFAPI file identity exceeded the documented 4 KiB boundary.
    #[error("CFAPI file identity is {actual} bytes, exceeding the {maximum}-byte limit")]
    FileIdentityTooLarge {
        /// Encoded byte length.
        actual: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// The encoded sync-root identity exceeded the documented 64 KiB boundary.
    #[error("CFAPI sync-root identity is {actual} bytes, exceeding the {maximum}-byte limit")]
    SyncRootIdentityTooLarge {
        /// Encoded byte length.
        actual: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// An encoded identity envelope was malformed or used an unsupported format version.
    #[error("invalid CFAPI file identity: {reason}")]
    InvalidFileIdentity {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// An encoded sync-root identity envelope was malformed or unsupported.
    #[error("invalid CFAPI sync-root identity: {reason}")]
    InvalidSyncRootIdentity {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// A product-neutral Forge cloud-files value or hydration contract was invalid.
    #[error(transparent)]
    Core(#[from] aster_forge_cloud_files_core::CloudFilesCoreError),
    /// The supplied identity described another item.
    #[error("CFAPI file identity does not match the placeholder item")]
    IdentityItemMismatch,
    /// The optional root file identity belonged to another namespace/root scope.
    #[error("CFAPI root file identity does not match the sync-root scope")]
    RootFileIdentityScopeMismatch,
    /// A provider-facing registration string violated the CFAPI contract.
    #[error("invalid CFAPI {field}: {reason}")]
    InvalidRegistrationString {
        /// Registration field name.
        field: &'static str,
        /// Stable reason suitable for logs and tests.
        reason: &'static str,
    },
    /// A zero provider GUID would delegate identity generation to display text.
    #[error("CFAPI provider id must be a stable non-zero GUID")]
    EmptyProviderId,
    /// Hydration or registration policies contradicted each other.
    #[error("invalid CFAPI sync-root policy: {reason}")]
    InvalidSyncRootPolicy {
        /// Stable policy conflict reason.
        reason: &'static str,
    },
    /// A requested policy needs a newer CFAPI platform integration level.
    #[error(
        "CFAPI feature {feature} requires integration 0x{required:x}, current integration is 0x{actual:x}"
    )]
    UnsupportedPlatformIntegration {
        /// Feature or policy requiring the newer integration.
        feature: &'static str,
        /// Minimum integration number.
        required: u32,
        /// Detected integration number.
        actual: u32,
    },
    /// The sync-root path was empty or otherwise unsuitable for registration.
    #[error("invalid CFAPI sync-root path: {reason}")]
    InvalidSyncRootPath {
        /// Stable path validation reason.
        reason: &'static str,
    },
    /// A windows-rs native structure size could not fit the CFAPI `u32` field.
    #[error("native CFAPI structure size for {structure} exceeds u32")]
    NativeStructureSizeOverflow {
        /// Native structure name.
        structure: &'static str,
    },
    /// A Windows placeholder name was not a valid single path component.
    #[error("invalid Windows placeholder name: {reason}")]
    InvalidPlaceholderName {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// Core item metadata could not be represented as a Windows placeholder.
    #[error("invalid Windows placeholder metadata: {reason}")]
    InvalidPlaceholderMetadata {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// A file size could not be represented by the signed CFAPI metadata field.
    #[error("placeholder size {size} exceeds the CFAPI signed 64-bit file-size boundary")]
    FileSizeTooLarge {
        /// Unsigned core file size.
        size: u64,
    },
    /// A Windows path contained an embedded NUL and therefore could not become a wide C string.
    #[error("Windows path contains an embedded NUL")]
    EmbeddedNul,
    /// A placeholder batch exceeded the count representable by CFAPI.
    #[error("placeholder batch contains more entries than CFAPI can represent")]
    PlaceholderBatchTooLarge,
    /// A callback snapshot contained malformed scalar, range, identity, or owned-string data.
    #[error("invalid CFAPI callback snapshot: {reason}")]
    InvalidCallbackSnapshot {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// Hydrated bytes could not be represented as one valid CFAPI transfer operation.
    #[error("invalid CFAPI fetch-data transfer: {reason}")]
    InvalidFetchTransfer {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// Product-neutral hydration coordination or backend work failed.
    #[error(transparent)]
    Hydration(#[from] aster_forge_cloud_files_core::HydrationError),
    /// A callback or completion belonged to another platform session generation.
    #[error("stale CFAPI connection generation: expected {expected}, received {actual}")]
    StaleConnectionGeneration {
        /// Generation owned by the active connection.
        expected: u64,
        /// Generation captured by the callback or completion.
        actual: u64,
    },
    /// The active connection lifecycle rejected a new callback.
    #[error("CFAPI connection is not accepting callbacks in state {state:?}")]
    ConnectionNotAccepting {
        /// Current product-neutral session state.
        state: aster_forge_cloud_files_core::SessionState,
    },
    /// The requested connection lifecycle transition skipped a required state.
    #[error("invalid CFAPI connection transition from {from:?} to {to:?}")]
    InvalidConnectionTransition {
        /// Current product-neutral session state.
        from: aster_forge_cloud_files_core::SessionState,
        /// Requested product-neutral session state.
        to: aster_forge_cloud_files_core::SessionState,
    },
    /// The active callback counter exceeded the host address space.
    #[error("CFAPI active callback count overflow")]
    ActiveCallbackCountOverflow,
    /// The process-local active hydration waiter identity space was exhausted.
    #[error("CFAPI active fetch waiter identity exhausted")]
    ActiveFetchWaiterIdOverflow,
    /// The active hydration waiter counter exceeded the host address space.
    #[error("CFAPI active fetch waiter count overflow")]
    ActiveFetchWaiterCountOverflow,
    /// A CFAPI progress value was outside the representable or monotonic range.
    #[error("invalid CFAPI provider progress: {reason}")]
    InvalidProviderProgress {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// The CFAPI callback watchdog configuration was not a usable positive duration.
    #[error("invalid CFAPI callback watchdog timeout: {reason}")]
    InvalidWatchdogTimeout {
        /// Stable adapter-level reason suitable for logs and tests.
        reason: &'static str,
    },
    /// A pending fetch exceeded the local watchdog deadline.
    #[error("CFAPI fetch-data callback watchdog timed out")]
    FetchDataWatchdogTimeout,
    /// A native CFAPI operation failed.
    #[cfg(windows)]
    #[error("CFAPI operation failed: {0}")]
    Native(#[from] windows::core::Error),
}
