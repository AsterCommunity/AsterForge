//! Windows-only CFAPI calls.

use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, path::Path};

use windows::{
    Win32::{
        Foundation::HANDLE,
        Storage::{
            CloudFilters::{
                CF_CREATE_FLAG_NONE, CF_DEHYDRATE_FLAG_BACKGROUND, CF_DEHYDRATE_FLAG_NONE,
                CF_FS_METADATA, CF_HARDLINK_POLICY_ALLOWED, CF_HARDLINK_POLICY_NONE,
                CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_ALWAYS_FULL, CF_HYDRATION_POLICY_FULL,
                CF_HYDRATION_POLICY_MODIFIER_ALLOW_FULL_RESTART_HYDRATION,
                CF_HYDRATION_POLICY_MODIFIER_AUTO_DEHYDRATION_ALLOWED,
                CF_HYDRATION_POLICY_MODIFIER_NONE, CF_HYDRATION_POLICY_MODIFIER_STREAMING_ALLOWED,
                CF_HYDRATION_POLICY_MODIFIER_VALIDATION_REQUIRED, CF_HYDRATION_POLICY_PROGRESSIVE,
                CF_IN_SYNC_STATE_IN_SYNC, CF_IN_SYNC_STATE_NOT_IN_SYNC, CF_INSYNC_POLICY,
                CF_OPEN_FILE_FLAG_EXCLUSIVE, CF_OPEN_FILE_FLAG_WRITE_ACCESS, CF_PIN_STATE_EXCLUDED,
                CF_PIN_STATE_INHERIT, CF_PIN_STATE_PINNED, CF_PIN_STATE_UNPINNED,
                CF_PIN_STATE_UNSPECIFIED, CF_PLACEHOLDER_CREATE_FLAG_ALWAYS_FULL,
                CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION,
                CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC, CF_PLACEHOLDER_CREATE_FLAG_NONE,
                CF_PLACEHOLDER_CREATE_FLAG_SUPERSEDE, CF_PLACEHOLDER_CREATE_FLAGS,
                CF_PLACEHOLDER_CREATE_INFO, CF_PLACEHOLDER_MANAGEMENT_POLICY,
                CF_PLACEHOLDER_MANAGEMENT_POLICY_CONVERT_TO_UNRESTRICTED,
                CF_PLACEHOLDER_MANAGEMENT_POLICY_CREATE_UNRESTRICTED,
                CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
                CF_PLACEHOLDER_MANAGEMENT_POLICY_UPDATE_UNRESTRICTED, CF_POPULATION_POLICY,
                CF_POPULATION_POLICY_ALWAYS_FULL, CF_POPULATION_POLICY_FULL,
                CF_POPULATION_POLICY_MODIFIER_NONE,
                CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
                CF_REGISTER_FLAG_MARK_IN_SYNC_ON_ROOT, CF_REGISTER_FLAG_NONE,
                CF_REGISTER_FLAG_UPDATE, CF_REGISTER_FLAGS, CF_SET_IN_SYNC_FLAG_NONE,
                CF_SET_PIN_FLAG_NONE, CF_SET_PIN_FLAG_RECURSE, CF_SET_PIN_FLAG_RECURSE_ONLY,
                CF_SET_PIN_FLAG_RECURSE_STOP_ON_ERROR, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION,
                CfCloseHandle, CfCreatePlaceholders, CfDehydratePlaceholder, CfGetPlatformInfo,
                CfOpenFileWithOplock, CfRegisterSyncRoot, CfSetInSyncState, CfSetPinState,
                CfUnregisterSyncRoot,
            },
            FileSystem::FILE_BASIC_INFO,
        },
    },
    core::{GUID, HRESULT, PCWSTR},
};

use crate::{
    Result, WindowsCloudFilesError, WindowsHardLinkPolicy, WindowsHydrationPolicy,
    WindowsHydrationPolicyPrimary, WindowsPlaceholder, WindowsPlaceholderManagementPolicy,
    WindowsPlaceholderOptions, WindowsPlatformVersion, WindowsPopulationPolicy,
    WindowsSyncRootPolicies, WindowsSyncRootRegistration, WindowsSyncRootRegistrationOptions,
};

/// Per-entry result written by CFAPI after placeholder creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsPlaceholderCreateOutcome {
    /// Native HRESULT value written to the entry.
    pub result_code: i32,
    /// USN assigned to the created placeholder.
    pub create_usn: i64,
}

/// Native CFAPI pin state exposed without leaking windows-rs types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPinState {
    Unspecified,
    Pinned,
    Unpinned,
    Excluded,
    Inherit,
}

/// Recursive behavior for a pin-state operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowsPinRecursion {
    #[default]
    None,
    Descendants,
    DescendantsOnly,
    DescendantsStopOnError,
}

struct WindowsCloudFileHandle(HANDLE);

impl WindowsCloudFileHandle {
    fn open_for_update(path: &Path) -> Result<Self> {
        validate_path(path)?;
        let path = wide_null(path.as_os_str())?;
        // SAFETY: the UTF-16 path is NUL-terminated and remains alive for the complete synchronous
        // open call. CFAPI returns one owned protected handle on success.
        let handle = unsafe {
            CfOpenFileWithOplock(
                PCWSTR(path.as_ptr()),
                CF_OPEN_FILE_FLAG_EXCLUSIVE | CF_OPEN_FILE_FLAG_WRITE_ACCESS,
            )
        }?;
        Ok(Self(handle))
    }
}

impl Drop for WindowsCloudFileHandle {
    fn drop(&mut self) {
        // SAFETY: this value owns exactly one successful `CfOpenFileWithOplock` result and Drop
        // executes once. `CfCloseHandle` consumes no Rust pointer and has no failure result.
        unsafe { CfCloseHandle(self.0) };
    }
}

/// Removes one persistent CFAPI sync-root registration.
pub fn unregister_sync_root(sync_root_path: &Path) -> Result<()> {
    validate_path(sync_root_path)?;
    let path = wide_null(sync_root_path.as_os_str())?;
    // SAFETY: the UTF-16 path is NUL-terminated and remains alive for the synchronous call.
    unsafe { CfUnregisterSyncRoot(PCWSTR(path.as_ptr())) }.map_err(Into::into)
}

/// Marks one placeholder in-sync or not-in-sync.
pub fn set_placeholder_in_sync(path: &Path, in_sync: bool) -> Result<()> {
    let handle = WindowsCloudFileHandle::open_for_update(path)?;
    let state = if in_sync {
        CF_IN_SYNC_STATE_IN_SYNC
    } else {
        CF_IN_SYNC_STATE_NOT_IN_SYNC
    };
    // SAFETY: the protected handle is live for this synchronous call; the optional USN pointer is
    // omitted and no native pointer escapes.
    unsafe { CfSetInSyncState(handle.0, state, CF_SET_IN_SYNC_FLAG_NONE, None) }.map_err(Into::into)
}

/// Updates one placeholder pin state using explicit recursion semantics.
pub fn set_placeholder_pin_state(
    path: &Path,
    state: WindowsPinState,
    recursion: WindowsPinRecursion,
) -> Result<()> {
    let handle = WindowsCloudFileHandle::open_for_update(path)?;
    let state = match state {
        WindowsPinState::Unspecified => CF_PIN_STATE_UNSPECIFIED,
        WindowsPinState::Pinned => CF_PIN_STATE_PINNED,
        WindowsPinState::Unpinned => CF_PIN_STATE_UNPINNED,
        WindowsPinState::Excluded => CF_PIN_STATE_EXCLUDED,
        WindowsPinState::Inherit => CF_PIN_STATE_INHERIT,
    };
    let flags = match recursion {
        WindowsPinRecursion::None => CF_SET_PIN_FLAG_NONE,
        WindowsPinRecursion::Descendants => CF_SET_PIN_FLAG_RECURSE,
        WindowsPinRecursion::DescendantsOnly => CF_SET_PIN_FLAG_RECURSE_ONLY,
        WindowsPinRecursion::DescendantsStopOnError => CF_SET_PIN_FLAG_RECURSE_STOP_ON_ERROR,
    };
    // SAFETY: the protected handle is live for this synchronous call; overlapped I/O is disabled,
    // so CFAPI retains no Rust reference after returning.
    unsafe { CfSetPinState(handle.0, state, flags, None) }.map_err(Into::into)
}

/// Dehydrates a full placeholder or one exact signed CFAPI range.
pub fn dehydrate_placeholder(
    path: &Path,
    range: Option<aster_forge_cloud_files_core::ByteRange>,
    background: bool,
) -> Result<()> {
    let handle = WindowsCloudFileHandle::open_for_update(path)?;
    let (offset, length) = match range {
        Some(range) => {
            if !range
                .offset()
                .is_multiple_of(crate::CFAPI_TRANSFER_ALIGNMENT_BYTES)
                || !range
                    .length()
                    .is_multiple_of(crate::CFAPI_TRANSFER_ALIGNMENT_BYTES)
            {
                return Err(WindowsCloudFilesError::InvalidFetchTransfer {
                    reason: "partial dehydrate range must be aligned to 4096 bytes",
                });
            }
            (
                i64::try_from(range.offset()).map_err(|_| {
                    WindowsCloudFilesError::InvalidFetchTransfer {
                        reason: "dehydrate offset exceeds signed CFAPI boundary",
                    }
                })?,
                i64::try_from(range.length()).map_err(|_| {
                    WindowsCloudFilesError::InvalidFetchTransfer {
                        reason: "dehydrate length exceeds signed CFAPI boundary",
                    }
                })?,
            )
        }
        None => (0, -1),
    };
    let flags = if background {
        CF_DEHYDRATE_FLAG_BACKGROUND
    } else {
        CF_DEHYDRATE_FLAG_NONE
    };
    // SAFETY: the protected handle and scalar range remain valid for this synchronous call;
    // overlapped I/O is disabled and no pointer escapes.
    unsafe { CfDehydratePlaceholder(handle.0, offset, length, flags, None) }.map_err(Into::into)
}

/// Batch-level CFAPI placeholder creation outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsPlaceholderBatchOutcome {
    /// Number of entries CFAPI reports as processed.
    pub entries_processed: u32,
    /// One native result for every submitted entry.
    pub entries: Vec<WindowsPlaceholderCreateOutcome>,
}

/// Creates a batch of placeholders below one registered sync-root directory.
pub fn create_placeholders(
    base_directory: &Path,
    placeholders: &[WindowsPlaceholder],
) -> Result<WindowsPlaceholderBatchOutcome> {
    let _count = u32::try_from(placeholders.len())
        .map_err(|_| WindowsCloudFilesError::PlaceholderBatchTooLarge)?;
    let base_directory = wide_null(base_directory.as_os_str())?;
    let relative_names = placeholders
        .iter()
        .map(|placeholder| wide_null(OsStr::new(placeholder.relative_name())))
        .collect::<Result<Vec<_>>>()?;
    let mut native_entries = placeholders
        .iter()
        .zip(&relative_names)
        .map(|(placeholder, relative_name)| native_entry(placeholder, relative_name))
        .collect::<Result<Vec<_>>>()?;
    let mut entries_processed = 0u32;

    // SAFETY: every PCWSTR and FileIdentity pointer refers to owned buffers that remain alive and
    // immovable for the full synchronous call. The mutable native entry slice is uniquely borrowed,
    // and CFAPI receives its exact length. No pointer escapes this function.
    unsafe {
        CfCreatePlaceholders(
            PCWSTR(base_directory.as_ptr()),
            &mut native_entries,
            CF_CREATE_FLAG_NONE,
            Some(&mut entries_processed),
        )?;
    }

    Ok(WindowsPlaceholderBatchOutcome {
        entries_processed,
        entries: native_entries
            .into_iter()
            .map(|entry| WindowsPlaceholderCreateOutcome {
                result_code: entry.Result.0,
                create_usn: entry.CreateUsn,
            })
            .collect(),
    })
}

/// Persistently registers or updates one Windows Cloud Files sync root.
pub fn register_sync_root(
    sync_root_path: &Path,
    registration: &WindowsSyncRootRegistration,
) -> Result<WindowsPlatformVersion> {
    if sync_root_path.as_os_str().is_empty() {
        return Err(WindowsCloudFilesError::InvalidSyncRootPath {
            reason: "path is empty",
        });
    }
    let sync_root_path = wide_null(sync_root_path.as_os_str())?;
    let provider_name = wide_null(OsStr::new(registration.provider_name()))?;
    let provider_version = wide_null(OsStr::new(registration.provider_version()))?;

    // SAFETY: this call has no pointer inputs and initializes a returned value owned by windows-rs.
    let platform_info = unsafe { CfGetPlatformInfo()? };
    let platform = WindowsPlatformVersion {
        build: platform_info.BuildNumber,
        revision: platform_info.RevisionNumber,
        integration: platform_info.IntegrationNumber,
    };
    registration.validate_for_platform(platform)?;

    let sync_root_identity = registration.sync_root_identity().as_bytes();
    let sync_root_identity_len = u32::try_from(sync_root_identity.len()).map_err(|_| {
        WindowsCloudFilesError::SyncRootIdentityTooLarge {
            actual: sync_root_identity.len(),
            maximum: crate::CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
        }
    })?;
    let (root_file_identity, root_file_identity_len) =
        if let Some(identity) = registration.root_file_identity() {
            let length = u32::try_from(identity.len()).map_err(|_| {
                WindowsCloudFilesError::FileIdentityTooLarge {
                    actual: identity.len(),
                    maximum: crate::CFAPI_FILE_IDENTITY_MAX_BYTES,
                }
            })?;
            (identity.as_bytes().as_ptr().cast(), length)
        } else {
            (std::ptr::null(), 0)
        };

    let native_registration = CF_SYNC_REGISTRATION {
        StructSize: native_struct_size::<CF_SYNC_REGISTRATION>("CF_SYNC_REGISTRATION")?,
        ProviderName: PCWSTR(provider_name.as_ptr()),
        ProviderVersion: PCWSTR(provider_version.as_ptr()),
        SyncRootIdentity: sync_root_identity.as_ptr().cast(),
        SyncRootIdentityLength: sync_root_identity_len,
        FileIdentity: root_file_identity,
        FileIdentityLength: root_file_identity_len,
        ProviderId: GUID::from_u128(registration.provider_id().as_u128()),
    };
    let native_policies = native_sync_root_policies(registration.policies())?;

    // SAFETY: all PCWSTR and identity pointers refer to owned buffers or registration bytes that
    // remain alive and immovable for the full synchronous call. Both native structures contain
    // accurate StructSize values, and no pointer escapes this function.
    unsafe {
        CfRegisterSyncRoot(
            PCWSTR(sync_root_path.as_ptr()),
            &native_registration,
            &native_policies,
            native_registration_options(registration.options()),
        )?;
    }
    Ok(platform)
}

fn native_entry(
    placeholder: &WindowsPlaceholder,
    relative_name: &[u16],
) -> Result<CF_PLACEHOLDER_CREATE_INFO> {
    let metadata = placeholder.metadata();
    let times = metadata.times();
    let identity_len = u32::try_from(placeholder.identity().len()).map_err(|_| {
        WindowsCloudFilesError::FileIdentityTooLarge {
            actual: placeholder.identity().len(),
            maximum: crate::CFAPI_FILE_IDENTITY_MAX_BYTES,
        }
    })?;
    Ok(CF_PLACEHOLDER_CREATE_INFO {
        RelativeFileName: PCWSTR(relative_name.as_ptr()),
        FsMetadata: CF_FS_METADATA {
            BasicInfo: FILE_BASIC_INFO {
                CreationTime: times.creation,
                LastAccessTime: times.last_access,
                LastWriteTime: times.last_write,
                ChangeTime: times.change,
                FileAttributes: metadata.file_attributes(),
            },
            FileSize: metadata.file_size(),
        },
        FileIdentity: placeholder.identity().as_bytes().as_ptr().cast(),
        FileIdentityLength: identity_len,
        Flags: native_options(placeholder.options()),
        Result: HRESULT(0),
        CreateUsn: 0,
    })
}

fn native_options(options: WindowsPlaceholderOptions) -> CF_PLACEHOLDER_CREATE_FLAGS {
    let mut flags = CF_PLACEHOLDER_CREATE_FLAG_NONE;
    if options.mark_in_sync {
        flags |= CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC;
    }
    if options.disable_on_demand_population {
        flags |= CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION;
    }
    if options.always_full {
        flags |= CF_PLACEHOLDER_CREATE_FLAG_ALWAYS_FULL;
    }
    if options.supersede {
        flags |= CF_PLACEHOLDER_CREATE_FLAG_SUPERSEDE;
    }
    flags
}

fn native_sync_root_policies(policies: WindowsSyncRootPolicies) -> Result<CF_SYNC_POLICIES> {
    Ok(CF_SYNC_POLICIES {
        StructSize: native_struct_size::<CF_SYNC_POLICIES>("CF_SYNC_POLICIES")?,
        Hydration: native_hydration_policy(policies.hydration),
        Population: CF_POPULATION_POLICY {
            Primary: match policies.population {
                WindowsPopulationPolicy::Full => CF_POPULATION_POLICY_FULL,
                WindowsPopulationPolicy::AlwaysFull => CF_POPULATION_POLICY_ALWAYS_FULL,
            },
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY(policies.in_sync.bits()),
        HardLink: match policies.hard_links {
            WindowsHardLinkPolicy::Forbidden => CF_HARDLINK_POLICY_NONE,
            WindowsHardLinkPolicy::Allowed => CF_HARDLINK_POLICY_ALLOWED,
        },
        PlaceholderManagement: native_placeholder_management(policies.placeholder_management),
    })
}

fn native_hydration_policy(policy: WindowsHydrationPolicy) -> CF_HYDRATION_POLICY {
    let mut modifier = CF_HYDRATION_POLICY_MODIFIER_NONE;
    if policy.validation_required {
        modifier |= CF_HYDRATION_POLICY_MODIFIER_VALIDATION_REQUIRED;
    }
    if policy.streaming_allowed {
        modifier |= CF_HYDRATION_POLICY_MODIFIER_STREAMING_ALLOWED;
    }
    if policy.auto_dehydration_allowed {
        modifier |= CF_HYDRATION_POLICY_MODIFIER_AUTO_DEHYDRATION_ALLOWED;
    }
    if policy.allow_full_restart_hydration {
        modifier |= CF_HYDRATION_POLICY_MODIFIER_ALLOW_FULL_RESTART_HYDRATION;
    }
    CF_HYDRATION_POLICY {
        Primary: match policy.primary {
            WindowsHydrationPolicyPrimary::Progressive => CF_HYDRATION_POLICY_PROGRESSIVE,
            WindowsHydrationPolicyPrimary::Full => CF_HYDRATION_POLICY_FULL,
            WindowsHydrationPolicyPrimary::AlwaysFull => CF_HYDRATION_POLICY_ALWAYS_FULL,
        },
        Modifier: modifier,
    }
}

fn native_placeholder_management(
    policy: WindowsPlaceholderManagementPolicy,
) -> CF_PLACEHOLDER_MANAGEMENT_POLICY {
    let mut bits = CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT.0;
    if policy.create_unrestricted {
        bits |= CF_PLACEHOLDER_MANAGEMENT_POLICY_CREATE_UNRESTRICTED.0;
    }
    if policy.convert_unrestricted {
        bits |= CF_PLACEHOLDER_MANAGEMENT_POLICY_CONVERT_TO_UNRESTRICTED.0;
    }
    if policy.update_unrestricted {
        bits |= CF_PLACEHOLDER_MANAGEMENT_POLICY_UPDATE_UNRESTRICTED.0;
    }
    CF_PLACEHOLDER_MANAGEMENT_POLICY(bits)
}

fn native_registration_options(options: WindowsSyncRootRegistrationOptions) -> CF_REGISTER_FLAGS {
    let mut native = CF_REGISTER_FLAG_NONE;
    if options.update_existing {
        native |= CF_REGISTER_FLAG_UPDATE;
    }
    if options.disable_on_demand_population_on_root {
        native |= CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT;
    }
    if options.mark_in_sync_on_root {
        native |= CF_REGISTER_FLAG_MARK_IN_SYNC_ON_ROOT;
    }
    native
}

fn native_struct_size<T>(structure: &'static str) -> Result<u32> {
    u32::try_from(size_of::<T>())
        .map_err(|_| WindowsCloudFilesError::NativeStructureSizeOverflow { structure })
}

fn wide_null(value: &OsStr) -> Result<Vec<u16>> {
    let mut encoded = value.encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(WindowsCloudFilesError::EmbeddedNul);
    }
    encoded.push(0);
    Ok(encoded)
}

fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(WindowsCloudFilesError::InvalidSyncRootPath {
            reason: "path must be non-empty",
        });
    }
    Ok(())
}
