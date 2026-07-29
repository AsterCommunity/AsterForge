//! Owned callback snapshots and active CFAPI connection lifecycle fencing.

use std::{
    ffi::OsString,
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};

use aster_forge_cloud_files_core::HydrationCoordinator;
use aster_forge_cloud_files_core::{
    Alignment, ByteRange, CloudBackendErrorKind, ContentReadRange, ContentReadResponse,
    ContentRevision, HydrationError, HydrationRequest, SessionGeneration, SessionState,
};
use bytes::Bytes;

use crate::{
    Result, WindowsCloudFilesError, WindowsFetchDataWaiterRegistry, WindowsFileIdentity,
    WindowsSyncRootIdentity,
};

/// Maximum priority value documented by CFAPI.
pub const CFAPI_MAX_PRIORITY_HINT: u8 = 15;
/// Physical range alignment required by CFAPI transfer operations.
pub const CFAPI_TRANSFER_ALIGNMENT_BYTES: u64 = 4096;

/// Additional callback metadata and implicit-hydration behavior requested at connect time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsSyncRootConnectOptions {
    /// Ask CFAPI to populate owned process metadata in callback snapshots.
    pub require_process_info: bool,
    /// Ask CFAPI to provide a full normalized placeholder path.
    pub require_full_file_path: bool,
    /// Block provider-originated implicit hydration before a callback is generated.
    pub block_self_implicit_hydration: bool,
}

/// Opaque native connection key returned by `CfConnectSyncRoot`.
///
/// This value is deliberately distinct from [`SessionGeneration`]. The key addresses one native
/// communication channel; the generation is the durable monotonic fence used by Forge work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsConnectionKey(i64);

impl WindowsConnectionKey {
    /// Wraps the exact opaque scalar returned by CFAPI.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the exact native scalar.
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Opaque transfer key used by CFAPI to correlate hydration operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsTransferKey(i64);

impl WindowsTransferKey {
    /// Wraps the exact callback scalar.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the exact callback scalar.
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Opaque request key used by newer CFAPI operation correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsRequestKey(i64);

impl WindowsRequestKey {
    /// Wraps the exact callback scalar.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the exact callback scalar.
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// CFAPI callback range length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowsCallbackRangeLength {
    /// One exact non-zero byte count.
    Exact(u64),
    /// Native `CF_EOF` (`-1`), meaning from the offset through end of file.
    ToEnd,
}

/// Owned CFAPI callback range preserving the native `CF_EOF` distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsCallbackRange {
    offset: u64,
    length: WindowsCallbackRangeLength,
}

impl WindowsCallbackRange {
    /// Creates one exact non-empty range and rejects end overflow.
    pub fn exact(offset: u64, length: u64) -> Result<Self> {
        if offset > i64::MAX as u64 {
            return Err(invalid_callback(
                "callback range offset exceeds signed CFAPI boundary",
            ));
        }
        if length == 0 {
            return Err(invalid_callback("callback range length must be non-zero"));
        }
        if length > i64::MAX as u64 {
            return Err(invalid_callback(
                "callback range length exceeds signed CFAPI boundary",
            ));
        }
        if offset
            .checked_add(length)
            .is_none_or(|end| end > i64::MAX as u64)
        {
            return Err(invalid_callback(
                "callback range end exceeds signed CFAPI boundary",
            ));
        }
        Ok(Self {
            offset,
            length: WindowsCallbackRangeLength::Exact(length),
        })
    }

    /// Creates a range extending from `offset` through the current end of file.
    pub fn to_end(offset: u64) -> Result<Self> {
        if offset > i64::MAX as u64 {
            return Err(invalid_callback(
                "callback range offset exceeds signed CFAPI boundary",
            ));
        }
        Ok(Self {
            offset,
            length: WindowsCallbackRangeLength::ToEnd,
        })
    }

    /// Converts the signed CFAPI offset/length pair into an owned range.
    pub fn from_cfapi(offset: i64, length: i64) -> Result<Self> {
        let offset = u64::try_from(offset)
            .map_err(|_| invalid_callback("callback range offset must be non-negative"))?;
        match length {
            -1 => Self::to_end(offset),
            1.. => Self::exact(
                offset,
                u64::try_from(length)
                    .map_err(|_| invalid_callback("callback range length exceeds u64"))?,
            ),
            _ => Err(invalid_callback(
                "callback range length must be positive or CF_EOF",
            )),
        }
    }

    /// Returns the first requested byte offset.
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Returns the exact native length semantics.
    pub const fn length(self) -> WindowsCallbackRangeLength {
        self.length
    }

    /// Returns the exclusive end for an exact range, or `None` for `ToEnd`.
    pub const fn end_exclusive(self) -> Option<u64> {
        match self.length {
            WindowsCallbackRangeLength::Exact(length) => Some(self.offset + length),
            WindowsCallbackRangeLength::ToEnd => None,
        }
    }
}

/// Owned process information requested through `CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsProcessInfoSnapshot {
    /// Native process identifier.
    pub process_id: u32,
    /// Executable image path, preserving Windows wide-string data.
    pub image_path: Option<PathBuf>,
    /// Optional package name.
    pub package_name: Option<OsString>,
    /// Optional application identifier.
    pub application_id: Option<OsString>,
    /// Optional command line.
    pub command_line: Option<OsString>,
    /// Native logon session identifier.
    pub session_id: u32,
}

/// Owned common data copied from `CF_CALLBACK_INFO` before the callback returns.
#[derive(Clone, PartialEq, Eq)]
pub struct WindowsCallbackInfoSnapshot {
    generation: SessionGeneration,
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    volume_guid_name: Option<OsString>,
    volume_dos_name: Option<OsString>,
    volume_serial_number: u32,
    sync_root_file_id: i64,
    sync_root_identity: WindowsSyncRootIdentity,
    file_id: i64,
    file_size: u64,
    file_identity: WindowsFileIdentity,
    normalized_path: Option<PathBuf>,
    priority_hint: u8,
    process: Option<WindowsProcessInfoSnapshot>,
}

impl WindowsCallbackInfoSnapshot {
    /// Creates and validates a fully owned callback-info snapshot.
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor mirrors the flat, versioned CF_CALLBACK_INFO ownership boundary"
    )]
    pub fn new(
        generation: SessionGeneration,
        connection_key: WindowsConnectionKey,
        transfer_key: WindowsTransferKey,
        request_key: WindowsRequestKey,
        volume_guid_name: Option<OsString>,
        volume_dos_name: Option<OsString>,
        volume_serial_number: u32,
        sync_root_file_id: i64,
        sync_root_identity: WindowsSyncRootIdentity,
        file_id: i64,
        file_size: u64,
        file_identity: WindowsFileIdentity,
        normalized_path: Option<PathBuf>,
        priority_hint: u8,
        process: Option<WindowsProcessInfoSnapshot>,
    ) -> Result<Self> {
        if priority_hint > CFAPI_MAX_PRIORITY_HINT {
            return Err(invalid_callback("priority hint exceeds CFAPI maximum"));
        }
        if file_size > i64::MAX as u64 {
            return Err(invalid_callback(
                "callback file size exceeds signed CFAPI boundary",
            ));
        }
        let scope = sync_root_identity.decode()?;
        let item_key = file_identity.decode()?;
        if item_key.scope() != &scope {
            return Err(invalid_callback(
                "file identity belongs to another sync-root scope",
            ));
        }
        Ok(Self {
            generation,
            connection_key,
            transfer_key,
            request_key,
            volume_guid_name,
            volume_dos_name,
            volume_serial_number,
            sync_root_file_id,
            sync_root_identity,
            file_id,
            file_size,
            file_identity,
            normalized_path,
            priority_hint,
            process,
        })
    }

    /// Returns the durable platform session fence captured at callback ingress.
    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }

    /// Returns the native connection key used by later CFAPI operations.
    pub const fn connection_key(&self) -> WindowsConnectionKey {
        self.connection_key
    }

    /// Returns the native transfer key.
    pub const fn transfer_key(&self) -> WindowsTransferKey {
        self.transfer_key
    }

    /// Returns the native request key.
    pub const fn request_key(&self) -> WindowsRequestKey {
        self.request_key
    }

    /// Returns the optional volume GUID name.
    pub fn volume_guid_name(&self) -> Option<&std::ffi::OsStr> {
        self.volume_guid_name.as_deref()
    }

    /// Returns the optional DOS volume name.
    pub fn volume_dos_name(&self) -> Option<&std::ffi::OsStr> {
        self.volume_dos_name.as_deref()
    }

    /// Returns the native volume serial number.
    pub const fn volume_serial_number(&self) -> u32 {
        self.volume_serial_number
    }

    /// Returns the sync-root file identifier.
    pub const fn sync_root_file_id(&self) -> i64 {
        self.sync_root_file_id
    }

    /// Returns the owned sync-root identity bytes.
    pub const fn sync_root_identity(&self) -> &WindowsSyncRootIdentity {
        &self.sync_root_identity
    }

    /// Returns the native placeholder file identifier.
    pub const fn file_id(&self) -> i64 {
        self.file_id
    }

    /// Returns the non-negative file size copied from CFAPI.
    pub const fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Returns the owned stable item identity.
    pub const fn file_identity(&self) -> &WindowsFileIdentity {
        &self.file_identity
    }

    /// Returns the optional owned normalized path.
    pub fn normalized_path(&self) -> Option<&std::path::Path> {
        self.normalized_path.as_deref()
    }

    /// Returns the native scheduling priority hint.
    pub const fn priority_hint(&self) -> u8 {
        self.priority_hint
    }

    /// Returns optional owned process metadata.
    pub const fn process(&self) -> Option<&WindowsProcessInfoSnapshot> {
        self.process.as_ref()
    }
}

impl fmt::Debug for WindowsCallbackInfoSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsCallbackInfoSnapshot")
            .field("generation", &self.generation)
            .field("connection_key", &self.connection_key)
            .field("transfer_key", &self.transfer_key)
            .field("request_key", &self.request_key)
            .field("volume_serial_number", &self.volume_serial_number)
            .field("sync_root_file_id", &self.sync_root_file_id)
            .field("sync_root_identity", &self.sync_root_identity)
            .field("file_id", &self.file_id)
            .field("file_size", &self.file_size)
            .field("file_identity", &self.file_identity)
            .field("priority_hint", &self.priority_hint)
            .field("has_process_info", &self.process.is_some())
            .finish_non_exhaustive()
    }
}

/// Bitset copied from `CF_CALLBACK_FETCH_DATA_FLAGS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct WindowsFetchDataFlags(u32);

impl WindowsFetchDataFlags {
    /// Recovery of a hydration interrupted by an unclean provider or system shutdown.
    pub const RECOVERY: Self = Self(0x1);
    /// Hydration was explicitly initiated rather than caused by ordinary file I/O.
    pub const EXPLICIT_HYDRATION: Self = Self(0x2);

    /// Preserves the exact native bitset, including future bits.
    pub const fn from_bits_retain(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the exact native bitset.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns whether all bits in `other` are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Owned `FETCH_DATA` callback snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsFetchDataSnapshot {
    info: WindowsCallbackInfoSnapshot,
    flags: WindowsFetchDataFlags,
    required_range: WindowsCallbackRange,
    optional_range: Option<WindowsCallbackRange>,
    last_dehydration_time: i64,
    last_dehydration_reason: u32,
}

impl WindowsFetchDataSnapshot {
    /// Creates an owned fetch snapshot after native pointers have been copied.
    pub const fn new(
        info: WindowsCallbackInfoSnapshot,
        flags: WindowsFetchDataFlags,
        required_range: WindowsCallbackRange,
        optional_range: Option<WindowsCallbackRange>,
        last_dehydration_time: i64,
        last_dehydration_reason: u32,
    ) -> Self {
        Self {
            info,
            flags,
            required_range,
            optional_range,
            last_dehydration_time,
            last_dehydration_reason,
        }
    }

    /// Returns common owned callback information.
    pub const fn info(&self) -> &WindowsCallbackInfoSnapshot {
        &self.info
    }

    /// Returns native fetch flags.
    pub const fn flags(&self) -> WindowsFetchDataFlags {
        self.flags
    }

    /// Returns whether CFAPI is recovering hydration interrupted by an unclean provider or system
    /// shutdown. Products should reconcile their durable content/cache state before selecting the
    /// revision passed to `hydrate` or `restart_hydration`.
    pub const fn is_recovery(&self) -> bool {
        self.flags.contains(WindowsFetchDataFlags::RECOVERY)
    }

    /// Returns the range required to satisfy outstanding I/O.
    pub const fn required_range(&self) -> WindowsCallbackRange {
        self.required_range
    }

    /// Returns the optional broader hydration hint.
    pub const fn optional_range(&self) -> Option<WindowsCallbackRange> {
        self.optional_range
    }

    /// Returns the native last-dehydration timestamp.
    pub const fn last_dehydration_time(&self) -> i64 {
        self.last_dehydration_time
    }

    /// Returns the exact native last-dehydration reason bits.
    pub const fn last_dehydration_reason(&self) -> u32 {
        self.last_dehydration_reason
    }
}

/// Bitset copied from `CF_CALLBACK_CANCEL_FLAGS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct WindowsCancelFetchDataFlags(u32);

impl WindowsCancelFetchDataFlags {
    /// The platform's fixed callback timeout expired.
    pub const IO_TIMEOUT: Self = Self(0x1);
    /// The requesting application or user aborted hydration.
    pub const IO_ABORTED: Self = Self(0x2);

    /// Preserves the exact native bitset, including future bits.
    pub const fn from_bits_retain(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the exact native bitset.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns whether all bits in `other` are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Owned `CANCEL_FETCH_DATA` callback snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsCancelFetchDataSnapshot {
    info: WindowsCallbackInfoSnapshot,
    flags: WindowsCancelFetchDataFlags,
    range: WindowsCallbackRange,
}

impl WindowsCancelFetchDataSnapshot {
    /// Creates an owned cancellation snapshot.
    pub const fn new(
        info: WindowsCallbackInfoSnapshot,
        flags: WindowsCancelFetchDataFlags,
        range: WindowsCallbackRange,
    ) -> Self {
        Self { info, flags, range }
    }

    /// Returns common owned callback information.
    pub const fn info(&self) -> &WindowsCallbackInfoSnapshot {
        &self.info
    }

    /// Returns the native cancellation reason flags.
    pub const fn flags(&self) -> WindowsCancelFetchDataFlags {
        self.flags
    }

    /// Returns the original hydration subrange that is no longer needed.
    pub const fn range(&self) -> WindowsCallbackRange {
        self.range
    }
}

/// Accepted hydration request plus its non-cloneable active-session lease.
#[derive(Debug)]
pub struct WindowsFetchDataRequest {
    snapshot: WindowsFetchDataSnapshot,
    lease: WindowsCallbackLease,
    terminal: FetchTerminalGate,
}

/// Product-neutral metadata replacement for one CFAPI `RESTART_HYDRATION` operation.
///
/// The optional identity must still decode to the same stable `CloudItemKey` as the callback.
/// Revision resolution remains in the product backend adapter; this value only carries the native
/// placeholder state that CFAPI may update while restarting pending I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsRestartHydration {
    metadata: Option<crate::WindowsPlaceholderMetadata>,
    identity: Option<WindowsFileIdentity>,
    mark_in_sync: bool,
}

impl WindowsRestartHydration {
    /// Starts with no metadata or identity replacement.
    pub const fn new() -> Self {
        Self {
            metadata: None,
            identity: None,
            mark_in_sync: false,
        }
    }

    /// Replaces the native filesystem metadata, including the logical file size.
    pub const fn with_metadata(mut self, metadata: crate::WindowsPlaceholderMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Replaces the persisted native identity after same-item validation.
    pub fn with_identity(mut self, identity: WindowsFileIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Marks the restarted placeholder in-sync after successful native completion.
    pub const fn mark_in_sync(mut self) -> Self {
        self.mark_in_sync = true;
        self
    }

    /// Returns the optional replacement metadata.
    pub const fn metadata(&self) -> Option<crate::WindowsPlaceholderMetadata> {
        self.metadata
    }

    /// Returns the optional replacement identity.
    pub const fn identity(&self) -> Option<&WindowsFileIdentity> {
        self.identity.as_ref()
    }

    /// Returns whether the native restart should set CFAPI in-sync state.
    pub const fn should_mark_in_sync(&self) -> bool {
        self.mark_in_sync
    }

    #[cfg(windows)]
    fn validate_for(&self, info: &WindowsCallbackInfoSnapshot) -> Result<()> {
        if let Some(identity) = &self.identity
            && identity.decode()? != info.file_identity().decode()?
        {
            return Err(WindowsCloudFilesError::IdentityItemMismatch);
        }
        Ok(())
    }
}

/// Cloneable correlation handle for reporting progress while the request itself is awaiting
/// hydration. It owns no terminal completion authority.
#[derive(Debug, Clone)]
pub struct WindowsFetchDataProgressReporter {
    correlation: WindowsFetchDataCorrelation,
}

impl WindowsFetchDataProgressReporter {
    /// Returns the owned callback correlation used for progress reporting.
    pub const fn correlation(&self) -> WindowsFetchDataCorrelation {
        self.correlation
    }

    /// Reports one monotonic progress sample and refreshes the matching local watchdog.
    #[cfg(windows)]
    pub fn report(
        &self,
        registry: &WindowsFetchDataWaiterRegistry,
        progress: crate::WindowsFetchDataProgress,
        now: std::time::Instant,
    ) -> Result<usize> {
        let updated = registry.report_progress_correlation(self.correlation, progress, now)?;
        if updated != 0 {
            crate::native_connection::report_fetch_progress(self.correlation, progress)?;
        }
        Ok(updated)
    }
}

/// Compact scalar correlation needed by waiter lookup and `CfReportProviderProgress`.
///
/// Unlike [`WindowsCallbackInfoSnapshot`], this value owns no paths, identities, or process text,
/// so cloning a progress reporter does not duplicate callback payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsFetchDataCorrelation {
    generation: SessionGeneration,
    connection_key: WindowsConnectionKey,
    transfer_key: WindowsTransferKey,
    request_key: WindowsRequestKey,
    file_id: i64,
}

impl WindowsFetchDataCorrelation {
    pub(crate) fn from_info(info: &WindowsCallbackInfoSnapshot) -> Self {
        Self {
            generation: info.generation(),
            connection_key: info.connection_key(),
            transfer_key: info.transfer_key(),
            request_key: info.request_key(),
            file_id: info.file_id(),
        }
    }

    /// Returns the accepting session generation.
    pub const fn generation(self) -> SessionGeneration {
        self.generation
    }

    /// Returns the native connection key.
    pub const fn connection_key(self) -> WindowsConnectionKey {
        self.connection_key
    }

    /// Returns the native transfer key.
    pub const fn transfer_key(self) -> WindowsTransferKey {
        self.transfer_key
    }

    /// Returns the native request key.
    pub const fn request_key(self) -> WindowsRequestKey {
        self.request_key
    }

    /// Returns the native placeholder file identifier.
    pub const fn file_id(self) -> i64 {
        self.file_id
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FetchTerminalGate {
    state: Arc<AtomicU8>,
}

impl Default for FetchTerminalGate {
    fn default() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(FetchTerminalState::Pending as u8)),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FetchTerminalState {
    #[default]
    Pending,
    NativeAttempted,
    PlatformCancelled,
    WatchdogTimedOut,
}

impl FetchTerminalGate {
    fn state(&self) -> FetchTerminalState {
        match self.state.load(Ordering::Acquire) {
            value if value == FetchTerminalState::NativeAttempted as u8 => {
                FetchTerminalState::NativeAttempted
            }
            value if value == FetchTerminalState::PlatformCancelled as u8 => {
                FetchTerminalState::PlatformCancelled
            }
            value if value == FetchTerminalState::WatchdogTimedOut as u8 => {
                FetchTerminalState::WatchdogTimedOut
            }
            _ => FetchTerminalState::Pending,
        }
    }

    fn attempted(&self) -> bool {
        self.state() == FetchTerminalState::NativeAttempted
    }

    pub(crate) fn platform_cancelled(&self) -> bool {
        self.state() == FetchTerminalState::PlatformCancelled
    }

    pub(crate) fn watchdog_timed_out(&self) -> bool {
        self.state() == FetchTerminalState::WatchdogTimedOut
    }

    #[cfg(any(windows, test))]
    pub(crate) fn begin_native(&self) -> bool {
        self.transition(
            FetchTerminalState::Pending,
            FetchTerminalState::NativeAttempted,
        ) || self.transition(
            FetchTerminalState::WatchdogTimedOut,
            FetchTerminalState::NativeAttempted,
        )
    }

    pub(crate) fn cancel_by_platform(&self) -> bool {
        self.transition(
            FetchTerminalState::Pending,
            FetchTerminalState::PlatformCancelled,
        ) || self.transition(
            FetchTerminalState::WatchdogTimedOut,
            FetchTerminalState::PlatformCancelled,
        )
    }

    pub(crate) fn cancel_by_watchdog(&self) -> bool {
        self.transition(
            FetchTerminalState::Pending,
            FetchTerminalState::WatchdogTimedOut,
        )
    }

    fn transition(&self, from: FetchTerminalState, to: FetchTerminalState) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

impl WindowsFetchDataRequest {
    /// Returns the immutable callback snapshot.
    pub const fn snapshot(&self) -> &WindowsFetchDataSnapshot {
        &self.snapshot
    }

    /// Returns the accepted generation lease.
    pub const fn lease(&self) -> &WindowsCallbackLease {
        &self.lease
    }

    /// Returns whether this request already made its one terminal native completion attempt.
    pub fn terminal_attempted(&self) -> bool {
        self.terminal.attempted()
    }

    /// Creates an owned progress reporter that can be moved to another worker while this request
    /// awaits hydration.
    pub fn progress_reporter(&self) -> WindowsFetchDataProgressReporter {
        WindowsFetchDataProgressReporter {
            correlation: WindowsFetchDataCorrelation::from_info(self.snapshot.info()),
        }
    }

    /// Restarts pending CFAPI hydration after the backend has established that the original
    /// placeholder bytes or revision are no longer valid. This consumes the terminal native
    /// attempt for the old callback; CFAPI will issue a subsequent callback for the restart.
    #[cfg(windows)]
    pub fn restart_hydration(self, replacement: WindowsRestartHydration) -> Result<()> {
        replacement.validate_for(self.snapshot.info())?;
        if !self.terminal.begin_native() {
            return Ok(());
        }
        crate::native_connection::restart_fetch_hydration(&self.snapshot, &replacement)
    }

    /// Builds the exact revision-bound core hydration request for this callback.
    ///
    /// The caller resolves `revision` from product-owned metadata using the stable item identity
    /// in the snapshot. Windows callback paths and filenames are never used as revision sources.
    pub fn hydration_request(
        &self,
        revision: ContentRevision,
        alignment: Alignment,
    ) -> Result<HydrationRequest> {
        let info = self.snapshot.info();
        let key = info.file_identity().decode()?;
        let range = hydration_byte_range(self.snapshot.required_range(), info.file_size())?;
        let platform_alignment = Alignment::new(CFAPI_TRANSFER_ALIGNMENT_BYTES)
            .ok_or_else(|| invalid_transfer("CFAPI transfer alignment must be non-zero"))?;
        let alignment = platform_alignment.intersection(alignment)?;
        Ok(HydrationRequest::range(
            key,
            revision,
            info.file_size(),
            range,
            alignment,
            info.generation(),
        ))
    }

    /// Waits for product-neutral hydration work and returns an owned, callback-bound transfer.
    ///
    /// This method performs no native call and does not consume terminal completion ownership.
    /// Platform workers may use it for explicit orchestration; ordinary Windows workers should
    /// prefer `WindowsFetchDataRequest::hydrate` so every error is terminally reported to CFAPI.
    pub async fn prepare_transfer(
        &self,
        coordinator: &HydrationCoordinator,
        revision: ContentRevision,
        alignment: Alignment,
    ) -> Result<WindowsFetchDataTransfer> {
        let hydration = self.hydration_request(revision.clone(), alignment)?;
        let waiter = coordinator.request(hydration)?;
        let response = waiter.wait().await?;
        WindowsFetchDataTransfer::from_response(&self.snapshot, &revision, response)
    }

    /// Registers this callback's core waiter for native cancellation while hydration is pending.
    ///
    /// A platform cancellation that covers the complete waiter range returns `Cancelled` without
    /// manufacturing a core/backend failure. Partial cancellation retains the waiter because bytes
    /// outside that subrange remain required by CFAPI.
    pub async fn prepare_registered_transfer(
        &mut self,
        coordinator: &HydrationCoordinator,
        revision: ContentRevision,
        alignment: Alignment,
        registry: &WindowsFetchDataWaiterRegistry,
    ) -> Result<WindowsFetchDataPreparation> {
        if self.terminal.platform_cancelled() {
            return Ok(WindowsFetchDataPreparation::Cancelled);
        }
        if self.terminal.watchdog_timed_out() {
            return Ok(WindowsFetchDataPreparation::TimedOut);
        }
        let hydration = self.hydration_request(revision.clone(), alignment)?;
        let range = match hydration.read().read_range() {
            ContentReadRange::Range(range) => range,
            ContentReadRange::Whole => {
                return Err(invalid_transfer(
                    "CFAPI fetch hydration must use one logical range",
                ));
            }
        };
        let waiter = coordinator.request(hydration)?;
        let registration = registry.register(
            &self.snapshot,
            range,
            waiter.cancellation_handle(),
            self.terminal.clone(),
        )?;
        let response = waiter.wait().await;
        let platform_cancelled = registration.platform_cancelled();
        let watchdog_timed_out = registration.watchdog_timed_out();
        drop(registration);
        if platform_cancelled {
            return Ok(WindowsFetchDataPreparation::Cancelled);
        }
        if watchdog_timed_out {
            return Ok(WindowsFetchDataPreparation::TimedOut);
        }
        match response {
            Ok(response) => {
                WindowsFetchDataTransfer::from_response(&self.snapshot, &revision, response)
                    .map(WindowsFetchDataPreparation::Transfer)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Resolves this request through the product-neutral hydration coordinator and transfers the
    /// exact logical bytes to CFAPI.
    ///
    /// Backend lookup, authentication, retries, and revision selection remain in the backend and
    /// product adapter. This method owns the terminal success/failure mapping once work starts.
    #[cfg(windows)]
    pub async fn hydrate(
        mut self,
        coordinator: &HydrationCoordinator,
        revision: ContentRevision,
        alignment: Alignment,
        registry: &WindowsFetchDataWaiterRegistry,
    ) -> Result<()> {
        match self
            .prepare_registered_transfer(coordinator, revision, alignment, registry)
            .await
        {
            Ok(WindowsFetchDataPreparation::Transfer(transfer)) => {
                self.complete_transfer_inner(&transfer)
            }
            Ok(WindowsFetchDataPreparation::Cancelled) => Ok(()),
            Ok(WindowsFetchDataPreparation::TimedOut) => {
                self.finish_request_error(WindowsCloudFilesError::FetchDataWatchdogTimeout)
            }
            Err(error) => self.finish_request_error(error),
        }
    }

    /// Completes this request with one already resolved core content response.
    #[cfg(windows)]
    pub fn complete(
        mut self,
        revision: &ContentRevision,
        response: ContentReadResponse,
    ) -> Result<()> {
        self.complete_inner(revision, response)
    }

    /// Completes this request with a structured CFAPI failure at most once.
    #[cfg(windows)]
    pub fn fail(mut self, failure: WindowsFetchDataFailure) -> Result<()> {
        self.fail_inner(failure)
    }

    #[cfg(windows)]
    pub(crate) fn fail_inner(&mut self, failure: WindowsFetchDataFailure) -> Result<()> {
        if !self.terminal.begin_native() {
            return Ok(());
        }
        crate::native_connection::complete_fetch_failure(&self.snapshot, failure)
    }

    #[cfg(windows)]
    fn complete_inner(
        &mut self,
        revision: &ContentRevision,
        response: ContentReadResponse,
    ) -> Result<()> {
        if self.terminal.attempted() {
            return Ok(());
        }
        let transfer =
            match WindowsFetchDataTransfer::from_response(&self.snapshot, revision, response) {
                Ok(transfer) => transfer,
                Err(error) => return self.finish_request_error(error),
            };
        self.complete_transfer_inner(&transfer)
    }

    #[cfg(windows)]
    fn complete_transfer_inner(&mut self, transfer: &WindowsFetchDataTransfer) -> Result<()> {
        if !self.terminal.begin_native() {
            return Ok(());
        }
        crate::native_connection::complete_fetch_success(&self.snapshot, transfer)
    }

    #[cfg(windows)]
    fn finish_request_error(&mut self, error: WindowsCloudFilesError) -> Result<()> {
        let failure = match &error {
            WindowsCloudFilesError::Hydration(error) => {
                WindowsFetchDataFailure::from_hydration_error(error)
            }
            _ => WindowsFetchDataFailure::InvalidRequest,
        };
        match self.fail_inner(failure) {
            Ok(()) => Err(error),
            Err(completion_error) => Err(completion_error),
        }
    }
}

/// Portable outcome of waiting for one cancellation-registered CFAPI hydration request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsFetchDataPreparation {
    /// Owned logical bytes are ready for a synchronous native transfer.
    Transfer(WindowsFetchDataTransfer),
    /// CFAPI cancelled the complete waiter range before hydration completed.
    Cancelled,
    /// The host watchdog cancelled the waiter before the platform callback deadline.
    TimedOut,
}

/// Owned logical bytes ready for one synchronous CFAPI transfer call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsFetchDataTransfer {
    offset: i64,
    length: i64,
    bytes: Bytes,
}

impl WindowsFetchDataTransfer {
    /// Validates that a core response exactly satisfies the native required range.
    pub fn from_response(
        snapshot: &WindowsFetchDataSnapshot,
        revision: &ContentRevision,
        response: ContentReadResponse,
    ) -> Result<Self> {
        let info = snapshot.info();
        let expected = hydration_byte_range(snapshot.required_range(), info.file_size())?;
        let request = aster_forge_cloud_files_core::ContentReadRequest::range(
            info.file_identity().decode()?,
            revision.clone(),
            info.file_size(),
            expected,
        );
        request.validate_response(&response)?;
        let (_, offset, bytes, total_size) = response.into_parts();
        if total_size != info.file_size() {
            return Err(invalid_transfer(
                "hydrated response size does not match callback file size",
            ));
        }
        let offset = i64::try_from(offset)
            .map_err(|_| invalid_transfer("transfer offset exceeds signed CFAPI boundary"))?;
        if bytes.is_empty() {
            return Err(invalid_transfer("successful transfer must contain bytes"));
        }
        let length = i64::try_from(bytes.len())
            .map_err(|_| invalid_transfer("transfer length exceeds signed CFAPI boundary"))?;
        Ok(Self {
            offset,
            length,
            bytes,
        })
    }

    /// Returns the first logical byte offset.
    pub const fn offset(&self) -> i64 {
        self.offset
    }

    /// Returns the owned bytes kept alive for the synchronous native call.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the signed CFAPI transfer length.
    pub const fn length(&self) -> i64 {
        self.length
    }
}

impl Drop for WindowsFetchDataRequest {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let _ = self.fail_inner(WindowsFetchDataFailure::ProviderTerminated);
        }
    }
}

/// Structured failure classifications accepted by the minimal fetch terminal path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsFetchDataFailure {
    /// The callback snapshot or request contract was malformed.
    InvalidRequest,
    /// The bounded callback queue or provider resources were exhausted.
    InsufficientResources,
    /// The provider connection is closing or its worker is gone.
    ProviderTerminated,
    /// Product credentials are missing, expired, or rejected.
    AuthenticationFailed,
    /// The requesting principal cannot read this item.
    AccessDenied,
    /// The callback metadata no longer matches current backend content state.
    NotInSync,
    /// A transient backend or transport outage prevented hydration.
    NetworkUnavailable,
    /// The backend does not support the requested content operation.
    NotSupported,
    /// The platform or user cancelled the hydration request.
    Cancelled,
    /// The provider failed without a narrower portable classification.
    Unsuccessful,
}

/// Owned notification that CFAPI has already completed a local filesystem effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsObservedNotification {
    /// CFAPI completed a dehydration operation.
    DehydrateCompleted {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native completion flag bits, including future values.
        flags: u32,
        /// Native dehydration reason bits, including future values.
        reason: u32,
    },
    /// CFAPI completed a deletion operation.
    DeleteCompleted {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native completion flag bits, including future values.
        flags: u32,
    },
    /// CFAPI completed a rename or move operation.
    RenameCompleted {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native completion flag bits, including future values.
        flags: u32,
        /// Source path copied before the native callback returned.
        source_path: Option<PathBuf>,
    },
}

impl WindowsObservedNotification {
    /// Returns the common callback information.
    pub const fn info(&self) -> &WindowsCallbackInfoSnapshot {
        match self {
            Self::DehydrateCompleted { info, .. }
            | Self::DeleteCompleted { info, .. }
            | Self::RenameCompleted { info, .. } => info,
        }
    }
}

/// Owned CFAPI operation that blocks a local filesystem effect until the provider acknowledges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsPreflightSnapshot {
    /// Previously transferred data must be validated before CFAPI may serve it.
    ValidateData {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native validation flag bits, including future values.
        flags: u32,
        /// Exact logical range awaiting validation.
        range: WindowsCallbackRange,
    },
    /// A placeholder is about to dehydrate.
    Dehydrate {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native dehydration flag bits, including future values.
        flags: u32,
        /// Native dehydration reason bits, including future values.
        reason: u32,
    },
    /// A placeholder is about to be deleted.
    Delete {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native delete flag bits, including future values.
        flags: u32,
    },
    /// A placeholder is about to be renamed or moved.
    Rename {
        /// Common owned callback correlation and stable identity.
        info: WindowsCallbackInfoSnapshot,
        /// Native rename flag bits, including future values.
        flags: u32,
        /// Target path copied before the native callback returned.
        target_path: Option<PathBuf>,
    },
}

impl WindowsPreflightSnapshot {
    /// Returns common callback information.
    pub const fn info(&self) -> &WindowsCallbackInfoSnapshot {
        match self {
            Self::ValidateData { info, .. }
            | Self::Dehydrate { info, .. }
            | Self::Delete { info, .. }
            | Self::Rename { info, .. } => info,
        }
    }
}

/// Accepted preflight request with exactly one native acknowledgement responsibility.
#[derive(Debug)]
pub struct WindowsPreflightRequest {
    snapshot: WindowsPreflightSnapshot,
    lease: WindowsCallbackLease,
    acknowledged: bool,
}

impl WindowsPreflightRequest {
    /// Returns the immutable preflight snapshot.
    pub const fn snapshot(&self) -> &WindowsPreflightSnapshot {
        &self.snapshot
    }

    /// Returns the accepted generation lease.
    pub const fn lease(&self) -> &WindowsCallbackLease {
        &self.lease
    }

    /// Returns whether this request has already attempted its native acknowledgement.
    pub const fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    /// Acknowledges that the product has durably accepted this local platform effect.
    #[cfg(windows)]
    pub fn approve(mut self) -> Result<()> {
        self.acknowledge_inner(WindowsFetchDataFailure::Unsuccessful, true)
    }

    /// Denies the local platform effect with a structured CFAPI failure classification.
    #[cfg(windows)]
    pub fn deny(mut self, failure: WindowsFetchDataFailure) -> Result<()> {
        self.acknowledge_inner(failure, false)
    }

    #[cfg(windows)]
    pub(crate) fn acknowledge_inner(
        &mut self,
        failure: WindowsFetchDataFailure,
        approved: bool,
    ) -> Result<()> {
        if self.acknowledged {
            return Ok(());
        }
        self.acknowledged = true;
        crate::native_connection::complete_preflight(&self.snapshot, failure, approved)
    }
}

impl Drop for WindowsPreflightRequest {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let _ = self.acknowledge_inner(WindowsFetchDataFailure::ProviderTerminated, false);
        }
    }
}

/// Bounded callback-ingress counters for one native connection generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsCallbackQueueMetrics {
    accepted_callbacks: u64,
    queued_fetch_data: u64,
    queued_cancel_observations: u64,
    queued_preflights: u64,
    queued_observations: u64,
    fetch_queue_full: u64,
    cancel_observation_queue_full: u64,
    preflight_queue_full: u64,
    observation_queue_full: u64,
    receiver_disconnected: u64,
    closing_rejections: u64,
    invalid_snapshot_rejections: u64,
    panic_failures: u64,
}

impl WindowsCallbackQueueMetrics {
    pub const fn accepted_callbacks(self) -> u64 {
        self.accepted_callbacks
    }
    pub const fn queued_fetch_data(self) -> u64 {
        self.queued_fetch_data
    }
    pub const fn queued_cancel_observations(self) -> u64 {
        self.queued_cancel_observations
    }
    /// Returns blocking native preflight requests queued for durable product decisions.
    pub const fn queued_preflights(self) -> u64 {
        self.queued_preflights
    }
    /// Returns completed native filesystem observations queued for reconciliation.
    pub const fn queued_observations(self) -> u64 {
        self.queued_observations
    }
    pub const fn fetch_queue_full(self) -> u64 {
        self.fetch_queue_full
    }
    pub const fn cancel_observation_queue_full(self) -> u64 {
        self.cancel_observation_queue_full
    }
    /// Returns blocking preflight requests failed because the queue was full.
    pub const fn preflight_queue_full(self) -> u64 {
        self.preflight_queue_full
    }
    /// Returns completed observations dropped because the queue was full.
    pub const fn observation_queue_full(self) -> u64 {
        self.observation_queue_full
    }
    pub const fn receiver_disconnected(self) -> u64 {
        self.receiver_disconnected
    }
    pub const fn closing_rejections(self) -> u64 {
        self.closing_rejections
    }
    pub const fn invalid_snapshot_rejections(self) -> u64 {
        self.invalid_snapshot_rejections
    }
    pub const fn panic_failures(self) -> u64 {
        self.panic_failures
    }
}

impl WindowsFetchDataFailure {
    /// Maps a product-neutral hydration failure to the closest CFAPI terminal classification.
    pub fn from_hydration_error(error: &HydrationError) -> Self {
        match error {
            HydrationError::Cancelled => Self::Cancelled,
            HydrationError::Contract(_) => Self::InvalidRequest,
            HydrationError::InFlightLimitExceeded { .. } => Self::InsufficientResources,
            HydrationError::Backend(error) => match error.kind() {
                CloudBackendErrorKind::AuthenticationRequired => Self::AuthenticationFailed,
                CloudBackendErrorKind::PermissionDenied => Self::AccessDenied,
                CloudBackendErrorKind::NotFound
                | CloudBackendErrorKind::Conflict
                | CloudBackendErrorKind::PreconditionFailed => Self::NotInSync,
                CloudBackendErrorKind::InvalidRequest => Self::InvalidRequest,
                CloudBackendErrorKind::RateLimited
                | CloudBackendErrorKind::TemporarilyUnavailable => Self::NetworkUnavailable,
                CloudBackendErrorKind::Unsupported => Self::NotSupported,
                CloudBackendErrorKind::InvalidResponse | CloudBackendErrorKind::Internal => {
                    Self::Unsuccessful
                }
            },
        }
    }
}

/// Accepted cancellation notification plus its non-cloneable active-session lease.
#[derive(Debug)]
pub struct WindowsCancelFetchDataRequest {
    snapshot: WindowsCancelFetchDataSnapshot,
    lease: WindowsCallbackLease,
}

impl WindowsCancelFetchDataRequest {
    /// Returns the immutable callback snapshot.
    pub const fn snapshot(&self) -> &WindowsCancelFetchDataSnapshot {
        &self.snapshot
    }

    /// Returns the accepted generation lease.
    pub const fn lease(&self) -> &WindowsCallbackLease {
        &self.lease
    }
}

/// One immutable callback snapshot accepted by a specific connection generation.
#[derive(Debug)]
pub enum WindowsCallbackRequest {
    /// A hydration request that requires a later terminal `CfExecute` operation.
    FetchData(WindowsFetchDataRequest),
    /// A cancellation notification for an existing hydration request or subrange.
    CancelFetchData(WindowsCancelFetchDataRequest),
    /// A blocking native filesystem operation awaiting one explicit acknowledgement.
    Preflight(WindowsPreflightRequest),
    /// A completed native filesystem effect delivered for durable observation/reconciliation.
    Observation {
        /// Owned completion snapshot.
        notification: WindowsObservedNotification,
        /// Lease fencing this observation to the accepting session generation.
        lease: WindowsCallbackLease,
    },
}

impl WindowsCallbackRequest {
    /// Returns common owned callback information.
    pub const fn info(&self) -> &WindowsCallbackInfoSnapshot {
        match self {
            Self::FetchData(request) => request.snapshot.info(),
            Self::CancelFetchData(request) => request.snapshot.info(),
            Self::Preflight(request) => request.snapshot.info(),
            Self::Observation { notification, .. } => notification.info(),
        }
    }

    /// Returns the generation that accepted this request.
    pub fn generation(&self) -> SessionGeneration {
        self.info().generation()
    }

    /// Binds a fetch snapshot to the exact generation lease that accepted it.
    pub fn fetch_data(
        snapshot: WindowsFetchDataSnapshot,
        lease: WindowsCallbackLease,
    ) -> Result<Self> {
        validate_request_generation(snapshot.info().generation(), lease.generation())?;
        Ok(Self::FetchData(WindowsFetchDataRequest {
            snapshot,
            lease,
            terminal: FetchTerminalGate::default(),
        }))
    }

    /// Binds a cancellation snapshot to the exact generation lease that accepted it.
    pub fn cancel_fetch_data(
        snapshot: WindowsCancelFetchDataSnapshot,
        lease: WindowsCallbackLease,
    ) -> Result<Self> {
        validate_request_generation(snapshot.info().generation(), lease.generation())?;
        Ok(Self::CancelFetchData(WindowsCancelFetchDataRequest {
            snapshot,
            lease,
        }))
    }

    /// Binds a blocking preflight snapshot to the generation that accepted it.
    pub fn preflight(
        snapshot: WindowsPreflightSnapshot,
        lease: WindowsCallbackLease,
    ) -> Result<Self> {
        validate_request_generation(snapshot.info().generation(), lease.generation())?;
        Ok(Self::Preflight(WindowsPreflightRequest {
            snapshot,
            lease,
            acknowledged: false,
        }))
    }

    /// Binds a completed notification to the generation that accepted it.
    pub fn observation(
        notification: WindowsObservedNotification,
        lease: WindowsCallbackLease,
    ) -> Result<Self> {
        validate_request_generation(notification.info().generation(), lease.generation())?;
        Ok(Self::Observation {
            notification,
            lease,
        })
    }
}

#[derive(Debug)]
struct ConnectionLifecycle {
    state: SessionState,
    active_callbacks: usize,
}

#[derive(Debug, Default)]
struct AtomicWindowsCallbackQueueMetrics {
    accepted_callbacks: AtomicU64,
    queued_fetch_data: AtomicU64,
    queued_cancel_observations: AtomicU64,
    queued_preflights: AtomicU64,
    queued_observations: AtomicU64,
    fetch_queue_full: AtomicU64,
    cancel_observation_queue_full: AtomicU64,
    preflight_queue_full: AtomicU64,
    observation_queue_full: AtomicU64,
    receiver_disconnected: AtomicU64,
    closing_rejections: AtomicU64,
    invalid_snapshot_rejections: AtomicU64,
    panic_failures: AtomicU64,
}

impl AtomicWindowsCallbackQueueMetrics {
    fn snapshot(&self) -> WindowsCallbackQueueMetrics {
        WindowsCallbackQueueMetrics {
            accepted_callbacks: self.accepted_callbacks.load(Ordering::Relaxed),
            queued_fetch_data: self.queued_fetch_data.load(Ordering::Relaxed),
            queued_cancel_observations: self.queued_cancel_observations.load(Ordering::Relaxed),
            queued_preflights: self.queued_preflights.load(Ordering::Relaxed),
            queued_observations: self.queued_observations.load(Ordering::Relaxed),
            fetch_queue_full: self.fetch_queue_full.load(Ordering::Relaxed),
            cancel_observation_queue_full: self
                .cancel_observation_queue_full
                .load(Ordering::Relaxed),
            preflight_queue_full: self.preflight_queue_full.load(Ordering::Relaxed),
            observation_queue_full: self.observation_queue_full.load(Ordering::Relaxed),
            receiver_disconnected: self.receiver_disconnected.load(Ordering::Relaxed),
            closing_rejections: self.closing_rejections.load(Ordering::Relaxed),
            invalid_snapshot_rejections: self.invalid_snapshot_rejections.load(Ordering::Relaxed),
            panic_failures: self.panic_failures.load(Ordering::Relaxed),
        }
    }

    #[cfg(any(windows, test))]
    fn increment(&self, event: QueueMetricEvent) {
        let counter = match event {
            QueueMetricEvent::Accepted => &self.accepted_callbacks,
            QueueMetricEvent::QueuedFetch => &self.queued_fetch_data,
            QueueMetricEvent::QueuedCancel => &self.queued_cancel_observations,
            QueueMetricEvent::QueuedPreflight => &self.queued_preflights,
            QueueMetricEvent::QueuedObservation => &self.queued_observations,
            QueueMetricEvent::FetchFull => &self.fetch_queue_full,
            QueueMetricEvent::CancelFull => &self.cancel_observation_queue_full,
            QueueMetricEvent::PreflightFull => &self.preflight_queue_full,
            QueueMetricEvent::ObservationFull => &self.observation_queue_full,
            QueueMetricEvent::Disconnected => &self.receiver_disconnected,
            QueueMetricEvent::Closing => &self.closing_rejections,
            QueueMetricEvent::Invalid => &self.invalid_snapshot_rejections,
            QueueMetricEvent::Panic => &self.panic_failures,
        };
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        });
    }
}

#[derive(Debug)]
struct ConnectionSessionInner {
    generation: SessionGeneration,
    lifecycle: Mutex<ConnectionLifecycle>,
    queue_metrics: AtomicWindowsCallbackQueueMetrics,
}

/// Shareable lifecycle fence for one active native connection generation.
#[derive(Debug, Clone)]
pub struct WindowsConnectionSession {
    inner: Arc<ConnectionSessionInner>,
}

impl WindowsConnectionSession {
    /// Starts one accepting connection generation.
    pub fn new(generation: SessionGeneration) -> Self {
        Self {
            inner: Arc::new(ConnectionSessionInner {
                generation,
                lifecycle: Mutex::new(ConnectionLifecycle {
                    state: SessionState::Accepting,
                    active_callbacks: 0,
                }),
                queue_metrics: AtomicWindowsCallbackQueueMetrics::default(),
            }),
        }
    }

    /// Returns the durable generation fence owned by this connection.
    pub fn generation(&self) -> SessionGeneration {
        self.inner.generation
    }

    /// Returns the current lifecycle state.
    pub fn state(&self) -> SessionState {
        lock_lifecycle(&self.inner).state
    }

    /// Returns the number of accepted callback requests still owned by downstream work.
    pub fn active_callbacks(&self) -> usize {
        lock_lifecycle(&self.inner).active_callbacks
    }

    /// Returns a point-in-time bounded callback ingress snapshot.
    pub fn queue_metrics(&self) -> WindowsCallbackQueueMetrics {
        self.inner.queue_metrics.snapshot()
    }

    #[cfg(any(windows, test))]
    fn record_queue_event(&self, event: QueueMetricEvent) {
        self.inner.queue_metrics.increment(event);
    }

    #[cfg(any(windows, test))]
    pub(crate) fn record_accepted_callback(&self) {
        self.record_queue_event(QueueMetricEvent::Accepted);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_queued_fetch(&self) {
        self.record_queue_event(QueueMetricEvent::QueuedFetch);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_queued_cancel(&self) {
        self.record_queue_event(QueueMetricEvent::QueuedCancel);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_fetch_queue_full(&self) {
        self.record_queue_event(QueueMetricEvent::FetchFull);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_cancel_queue_full(&self) {
        self.record_queue_event(QueueMetricEvent::CancelFull);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_queued_preflight(&self) {
        self.record_queue_event(QueueMetricEvent::QueuedPreflight);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_queued_observation(&self) {
        self.record_queue_event(QueueMetricEvent::QueuedObservation);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_preflight_queue_full(&self) {
        self.record_queue_event(QueueMetricEvent::PreflightFull);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_observation_queue_full(&self) {
        self.record_queue_event(QueueMetricEvent::ObservationFull);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_receiver_disconnected(&self) {
        self.record_queue_event(QueueMetricEvent::Disconnected);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_closing_rejection(&self) {
        self.record_queue_event(QueueMetricEvent::Closing);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_invalid_snapshot(&self) {
        self.record_queue_event(QueueMetricEvent::Invalid);
    }
    #[cfg(any(windows, test))]
    pub(crate) fn record_panic_failure(&self) {
        self.record_queue_event(QueueMetricEvent::Panic);
    }

    /// Accepts one callback only while this exact generation is accepting new work.
    pub fn begin_callback(
        &self,
        callback_generation: SessionGeneration,
    ) -> Result<WindowsCallbackLease> {
        if callback_generation != self.inner.generation {
            return Err(WindowsCloudFilesError::StaleConnectionGeneration {
                expected: self.inner.generation.get(),
                actual: callback_generation.get(),
            });
        }
        let mut lifecycle = lock_lifecycle(&self.inner);
        if lifecycle.state != SessionState::Accepting {
            return Err(WindowsCloudFilesError::ConnectionNotAccepting {
                state: lifecycle.state,
            });
        }
        lifecycle.active_callbacks = lifecycle
            .active_callbacks
            .checked_add(1)
            .ok_or(WindowsCloudFilesError::ActiveCallbackCountOverflow)?;
        drop(lifecycle);
        Ok(WindowsCallbackLease {
            session: self.inner.clone(),
            released: false,
        })
    }

    /// Moves `Accepting -> Closing`, rejecting all later callback ingress.
    ///
    /// Returns `true` only for the first close request. Repeated calls are idempotent.
    pub fn begin_closing(&self) -> bool {
        let mut lifecycle = lock_lifecycle(&self.inner);
        if lifecycle.state == SessionState::Accepting {
            lifecycle.state = SessionState::Closing;
            true
        } else {
            false
        }
    }

    /// Records successful native disconnect and begins draining accepted work.
    ///
    /// The session closes immediately when no callback leases remain; otherwise the final lease
    /// release performs `Draining -> Closed`.
    pub fn mark_disconnected(&self) -> Result<()> {
        let mut lifecycle = lock_lifecycle(&self.inner);
        match lifecycle.state {
            SessionState::Closing => {
                lifecycle.state = if lifecycle.active_callbacks == 0 {
                    SessionState::Closed
                } else {
                    SessionState::Draining
                };
                Ok(())
            }
            SessionState::Draining | SessionState::Closed => Ok(()),
            SessionState::Accepting => Err(WindowsCloudFilesError::InvalidConnectionTransition {
                from: SessionState::Accepting,
                to: SessionState::Draining,
            }),
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy)]
enum QueueMetricEvent {
    Accepted,
    QueuedFetch,
    QueuedCancel,
    QueuedPreflight,
    QueuedObservation,
    FetchFull,
    CancelFull,
    PreflightFull,
    ObservationFull,
    Disconnected,
    Closing,
    Invalid,
    Panic,
}

/// Non-cloneable ownership proof for one callback accepted before the closing fence.
pub struct WindowsCallbackLease {
    session: Arc<ConnectionSessionInner>,
    released: bool,
}

impl WindowsCallbackLease {
    /// Returns the generation that accepted the callback.
    pub fn generation(&self) -> SessionGeneration {
        self.session.generation
    }

    /// Releases the accepted callback count explicitly.
    pub fn release(mut self) {
        self.release_inner();
    }

    /// Checks whether this exact accepted callback may complete against `session`.
    pub fn accepts_completion(&self, session: &WindowsConnectionSession) -> bool {
        !self.released
            && Arc::ptr_eq(&self.session, &session.inner)
            && lock_lifecycle(&self.session).state != SessionState::Closed
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        let mut lifecycle = lock_lifecycle(&self.session);
        lifecycle.active_callbacks = lifecycle.active_callbacks.saturating_sub(1);
        if lifecycle.state == SessionState::Draining && lifecycle.active_callbacks == 0 {
            lifecycle.state = SessionState::Closed;
        }
        self.released = true;
    }
}

impl fmt::Debug for WindowsCallbackLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsCallbackLease")
            .field("generation", &self.session.generation)
            .field("released", &self.released)
            .finish()
    }
}

impl Drop for WindowsCallbackLease {
    fn drop(&mut self) {
        self.release_inner();
    }
}

fn lock_lifecycle(inner: &ConnectionSessionInner) -> MutexGuard<'_, ConnectionLifecycle> {
    match inner.lifecycle.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn invalid_callback(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidCallbackSnapshot { reason }
}

fn invalid_transfer(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidFetchTransfer { reason }
}

fn hydration_byte_range(range: WindowsCallbackRange, file_size: u64) -> Result<ByteRange> {
    if !range
        .offset()
        .is_multiple_of(CFAPI_TRANSFER_ALIGNMENT_BYTES)
    {
        return Err(invalid_transfer(
            "CFAPI transfer offset must be aligned to 4096 bytes",
        ));
    }
    let length = match range.length() {
        WindowsCallbackRangeLength::Exact(length) => length,
        WindowsCallbackRangeLength::ToEnd => file_size
            .checked_sub(range.offset())
            .ok_or_else(|| invalid_transfer("CF_EOF transfer offset exceeds callback file size"))?,
    };
    if length == 0 {
        return Err(invalid_transfer(
            "CFAPI fetch range resolves to an empty successful transfer",
        ));
    }
    let Some(end) = range.offset().checked_add(length) else {
        return Err(invalid_transfer("CFAPI fetch range end exceeds u64"));
    };
    if end > file_size {
        return Err(invalid_transfer(
            "CFAPI fetch range exceeds callback file size",
        ));
    }
    if end != file_size && !length.is_multiple_of(CFAPI_TRANSFER_ALIGNMENT_BYTES) {
        return Err(invalid_transfer(
            "CFAPI transfer length must be aligned to 4096 bytes unless it reaches end-of-file",
        ));
    }
    ByteRange::new(range.offset(), length).map_err(Into::into)
}

fn validate_request_generation(
    snapshot: SessionGeneration,
    lease: SessionGeneration,
) -> Result<()> {
    if snapshot == lease {
        return Ok(());
    }
    Err(WindowsCloudFilesError::StaleConnectionGeneration {
        expected: lease.get(),
        actual: snapshot.get(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{FetchTerminalGate, SessionGeneration, WindowsConnectionSession};

    #[test]
    fn fetch_terminal_gate_allows_exactly_one_attempt() {
        let gate = FetchTerminalGate::default();
        assert!(!gate.attempted());
        assert!(gate.begin_native());
        assert!(gate.attempted());
        assert!(!gate.begin_native());
        assert!(gate.attempted());
        assert!(!gate.cancel_by_platform());
    }

    #[test]
    fn platform_cancellation_suppresses_every_native_attempt() {
        let gate = FetchTerminalGate::default();
        assert!(gate.cancel_by_platform());
        assert!(!gate.attempted());
        assert!(!gate.cancel_by_platform());
        assert!(!gate.begin_native());
        assert!(!gate.attempted());
    }

    #[test]
    fn watchdog_timeout_preserves_one_native_failure_attempt() {
        let gate = FetchTerminalGate::default();
        assert!(gate.cancel_by_watchdog());
        assert!(gate.watchdog_timed_out());
        assert!(!gate.attempted());
        assert!(gate.begin_native());
        assert!(gate.attempted());
        assert!(!gate.begin_native());
    }

    #[test]
    fn callback_queue_metrics_count_each_bounded_ingress_outcome() {
        let generation = SessionGeneration::new(1).expect("generation fixture should be valid");
        let session = WindowsConnectionSession::new(generation);
        session.record_accepted_callback();
        session.record_queued_fetch();
        session.record_queued_cancel();
        session.record_queued_preflight();
        session.record_queued_observation();
        session.record_fetch_queue_full();
        session.record_cancel_queue_full();
        session.record_preflight_queue_full();
        session.record_observation_queue_full();
        session.record_receiver_disconnected();
        session.record_closing_rejection();
        session.record_invalid_snapshot();
        session.record_panic_failure();
        let metrics = session.queue_metrics();
        assert_eq!(metrics.accepted_callbacks(), 1);
        assert_eq!(metrics.queued_fetch_data(), 1);
        assert_eq!(metrics.queued_cancel_observations(), 1);
        assert_eq!(metrics.queued_preflights(), 1);
        assert_eq!(metrics.queued_observations(), 1);
        assert_eq!(metrics.fetch_queue_full(), 1);
        assert_eq!(metrics.cancel_observation_queue_full(), 1);
        assert_eq!(metrics.preflight_queue_full(), 1);
        assert_eq!(metrics.observation_queue_full(), 1);
        assert_eq!(metrics.receiver_disconnected(), 1);
        assert_eq!(metrics.closing_rejections(), 1);
        assert_eq!(metrics.invalid_snapshot_rejections(), 1);
        assert_eq!(metrics.panic_failures(), 1);

        session
            .inner
            .queue_metrics
            .accepted_callbacks
            .store(u64::MAX, Ordering::Relaxed);
        session.record_accepted_callback();
        assert_eq!(session.queue_metrics().accepted_callbacks(), u64::MAX);
    }

    #[test]
    fn callback_queue_metrics_do_not_lose_concurrent_updates() {
        let generation = SessionGeneration::new(1).expect("generation fixture should be valid");
        let session = WindowsConnectionSession::new(generation);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let session = session.clone();
                scope.spawn(move || {
                    for _ in 0..1_000 {
                        session.record_accepted_callback();
                    }
                });
            }
        });
        assert_eq!(session.queue_metrics().accepted_callbacks(), 8_000);
    }
}
