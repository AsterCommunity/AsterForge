//! Owned sync-root registration model and policy validation.

use std::{fmt, mem::size_of, ops::BitOr};

use aster_forge_cloud_files_core::{CloudNamespaceId, CloudRootId, CloudScope};

use crate::{Result, WindowsCloudFilesError, WindowsFileIdentity};

/// Maximum `SyncRootIdentity` size accepted by `CfRegisterSyncRoot`.
pub const CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES: usize = 64 * 1024;

const MAGIC: &[u8; 4] = b"AFSR";
const FORMAT_VERSION: u8 = 1;
const HEADER_LEN: usize = MAGIC.len() + 1 + 2 * size_of::<u32>();
const MAX_PROVIDER_TEXT_UTF16_UNITS: usize = 255;
const PLACEHOLDER_MANAGEMENT_MIN_INTEGRATION: u32 = 0x310;
const FULL_RESTART_HYDRATION_MIN_INTEGRATION: u32 = 0x500;

/// Stable provider telemetry identity shared by every version and sync root of one provider.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsProviderId(u128);

impl WindowsProviderId {
    /// Creates a stable non-zero GUID value from its canonical `u128` representation.
    pub fn new(value: u128) -> Result<Self> {
        if value == 0 {
            return Err(WindowsCloudFilesError::EmptyProviderId);
        }
        Ok(Self(value))
    }

    /// Returns the canonical GUID value.
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Debug for WindowsProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WindowsProviderId({:032x})", self.0)
    }
}

/// Owned, versioned CFAPI sync-root identity for one exact Forge namespace/root scope.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WindowsSyncRootIdentity(Vec<u8>);

impl WindowsSyncRootIdentity {
    /// Encodes one product-neutral scope into the current Windows sync-root envelope.
    pub fn encode(scope: &CloudScope) -> Result<Self> {
        let fields = [
            scope.namespace_id().as_str().as_bytes(),
            scope.root_id().as_str().as_bytes(),
        ];
        let payload_len = fields
            .iter()
            .try_fold(0usize, |total, field| total.checked_add(field.len()))
            .ok_or(WindowsCloudFilesError::SyncRootIdentityTooLarge {
                actual: usize::MAX,
                maximum: CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
            })?;
        let total_len = HEADER_LEN.checked_add(payload_len).ok_or(
            WindowsCloudFilesError::SyncRootIdentityTooLarge {
                actual: usize::MAX,
                maximum: CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
            },
        )?;
        validate_sync_root_identity_size(total_len)?;

        let mut encoded = Vec::with_capacity(total_len);
        encoded.extend_from_slice(MAGIC);
        encoded.push(FORMAT_VERSION);
        for field in fields {
            let length = u32::try_from(field.len()).map_err(|_| {
                WindowsCloudFilesError::SyncRootIdentityTooLarge {
                    actual: field.len(),
                    maximum: CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
                }
            })?;
            encoded.extend_from_slice(&length.to_le_bytes());
        }
        for field in fields {
            encoded.extend_from_slice(field);
        }
        Ok(Self(encoded))
    }

    /// Imports and validates a canonical sync-root identity envelope.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        validate_sync_root_identity_size(bytes.len())?;
        decode_sync_root_identity(&bytes)?;
        Ok(Self(bytes))
    }

    /// Decodes the product-neutral namespace/root scope.
    pub fn decode(&self) -> Result<CloudScope> {
        decode_sync_root_identity(&self.0)
    }

    /// Returns the exact bytes persisted by CFAPI.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the encoded byte length.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the envelope is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consumes the identity and returns its exact bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for WindowsSyncRootIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsSyncRootIdentity")
            .field("format_version", &FORMAT_VERSION)
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Hydration behavior requested from CFAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsHydrationPolicyPrimary {
    /// Complete user I/O after the requested range is available and continue in the background.
    Progressive,
    /// Hydrate the complete file before completing user I/O.
    Full,
    /// Keep every placeholder fully hydrated.
    AlwaysFull,
}

/// Primary hydration behavior plus independently negotiated CFAPI modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsHydrationPolicy {
    /// Primary hydration behavior.
    pub primary: WindowsHydrationPolicyPrimary,
    /// Persist and validate provider bytes before completing user I/O.
    pub validation_required: bool,
    /// Permit the platform to serve provider bytes without storing them locally.
    pub streaming_allowed: bool,
    /// Permit native auto-dehydration of in-sync placeholders.
    pub auto_dehydration_allowed: bool,
    /// Permit full restart hydration used when file size changes during fetch.
    pub allow_full_restart_hydration: bool,
}

impl WindowsHydrationPolicy {
    /// Validates conflicts and platform-version gates.
    pub fn validate(self, platform: WindowsPlatformVersion) -> Result<()> {
        if self.validation_required && self.streaming_allowed {
            return Err(invalid_policy(
                "validation-required and streaming-allowed are mutually exclusive",
            ));
        }
        if self.allow_full_restart_hydration
            && platform.integration < FULL_RESTART_HYDRATION_MIN_INTEGRATION
        {
            return Err(WindowsCloudFilesError::UnsupportedPlatformIntegration {
                feature: "allow-full-restart-hydration",
                required: FULL_RESTART_HYDRATION_MIN_INTEGRATION,
                actual: platform.integration,
            });
        }
        Ok(())
    }
}

/// Supported namespace population behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPopulationPolicy {
    /// Populate every child when an unpopulated directory is accessed.
    Full,
    /// Assume the complete namespace is already present locally.
    AlwaysFull,
}

/// Supported CFAPI in-sync tracking bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct WindowsInSyncPolicy(u32);

impl WindowsInSyncPolicy {
    /// Track no optional metadata changes.
    pub const NONE: Self = Self(0);
    /// Track file creation time.
    pub const FILE_CREATION_TIME: Self = Self(0x0000_0001);
    /// Track file read-only attribute.
    pub const FILE_READ_ONLY: Self = Self(0x0000_0002);
    /// Track file hidden attribute.
    pub const FILE_HIDDEN: Self = Self(0x0000_0004);
    /// Track file system attribute.
    pub const FILE_SYSTEM: Self = Self(0x0000_0008);
    /// Track directory creation time.
    pub const DIRECTORY_CREATION_TIME: Self = Self(0x0000_0010);
    /// Track directory read-only attribute.
    pub const DIRECTORY_READ_ONLY: Self = Self(0x0000_0020);
    /// Track directory hidden attribute.
    pub const DIRECTORY_HIDDEN: Self = Self(0x0000_0040);
    /// Track directory system attribute.
    pub const DIRECTORY_SYSTEM: Self = Self(0x0000_0080);
    /// Track file last-write time.
    pub const FILE_LAST_WRITE_TIME: Self = Self(0x0000_0100);
    /// Track directory last-write time.
    pub const DIRECTORY_LAST_WRITE_TIME: Self = Self(0x0000_0200);
    /// Preserve in-sync state for sync-engine initiated modifications.
    pub const PRESERVE_FOR_SYNC_ENGINE: Self = Self(0x8000_0000);

    /// Returns the exact CFAPI bitset.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns whether all requested bits are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for WindowsInSyncPolicy {
    type Output = Self;

    fn bitor(self, other: Self) -> Self::Output {
        Self(self.0 | other.0)
    }
}

/// Whether placeholder hard links are permitted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowsHardLinkPolicy {
    /// Keep the CFAPI default and reject placeholder hard links.
    #[default]
    Forbidden,
    /// Permit supported hard-link scenarios.
    Allowed,
}

/// Permissions granted to non-provider processes while the sync root is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsPlaceholderManagementPolicy {
    /// Permit unrestricted placeholder creation.
    pub create_unrestricted: bool,
    /// Permit unrestricted conversion to placeholders.
    pub convert_unrestricted: bool,
    /// Permit unrestricted placeholder updates.
    pub update_unrestricted: bool,
}

impl WindowsPlaceholderManagementPolicy {
    /// Returns whether any integration-gated permission is requested.
    pub const fn is_unrestricted(self) -> bool {
        self.create_unrestricted || self.convert_unrestricted || self.update_unrestricted
    }
}

/// Complete CFAPI sync-root policy set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsSyncRootPolicies {
    /// Hydration behavior and modifiers.
    pub hydration: WindowsHydrationPolicy,
    /// Namespace population behavior.
    pub population: WindowsPopulationPolicy,
    /// Metadata changes that clear in-sync state.
    pub in_sync: WindowsInSyncPolicy,
    /// Placeholder hard-link behavior.
    pub hard_links: WindowsHardLinkPolicy,
    /// Non-provider placeholder-management permissions.
    pub placeholder_management: WindowsPlaceholderManagementPolicy,
}

impl WindowsSyncRootPolicies {
    /// Validates policy conflicts and platform integration gates.
    pub fn validate(self, platform: WindowsPlatformVersion) -> Result<()> {
        self.hydration.validate(platform)?;
        if self.placeholder_management.is_unrestricted()
            && platform.integration < PLACEHOLDER_MANAGEMENT_MIN_INTEGRATION
        {
            return Err(WindowsCloudFilesError::UnsupportedPlatformIntegration {
                feature: "unrestricted-placeholder-management",
                required: PLACEHOLDER_MANAGEMENT_MIN_INTEGRATION,
                actual: platform.integration,
            });
        }
        Ok(())
    }
}

/// Persistent registration flags independent from windows-rs values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsSyncRootRegistrationOptions {
    /// Update an existing registration instead of creating a new claim.
    pub update_existing: bool,
    /// Disable on-demand population for the root directory itself.
    pub disable_on_demand_population_on_root: bool,
    /// Mark the root directory in-sync during registration.
    pub mark_in_sync_on_root: bool,
}

/// Detected CFAPI platform version used for gated policy validation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsPlatformVersion {
    /// Windows platform build number.
    pub build: u32,
    /// Windows platform revision number.
    pub revision: u32,
    /// Cloud Files integration number.
    pub integration: u32,
}

/// Owned persistent sync-root registration request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsSyncRootRegistration {
    provider_name: String,
    provider_version: String,
    provider_id: WindowsProviderId,
    sync_root_identity: WindowsSyncRootIdentity,
    root_file_identity: WindowsFileIdentity,
    policies: WindowsSyncRootPolicies,
    options: WindowsSyncRootRegistrationOptions,
}

impl WindowsSyncRootRegistration {
    /// Creates a persistent registration request and validates all portable identity boundaries.
    pub fn new(
        provider_name: impl Into<String>,
        provider_version: impl Into<String>,
        provider_id: WindowsProviderId,
        sync_root_identity: WindowsSyncRootIdentity,
        root_file_identity: WindowsFileIdentity,
        policies: WindowsSyncRootPolicies,
        options: WindowsSyncRootRegistrationOptions,
    ) -> Result<Self> {
        let provider_name = provider_name.into();
        let provider_version = provider_version.into();
        validate_registration_text("provider name", &provider_name)?;
        validate_registration_text("provider version", &provider_version)?;
        if root_file_identity.decode()?.scope() != &sync_root_identity.decode()? {
            return Err(WindowsCloudFilesError::RootFileIdentityScopeMismatch);
        }
        Ok(Self {
            provider_name,
            provider_version,
            provider_id,
            sync_root_identity,
            root_file_identity,
            policies,
            options,
        })
    }

    /// Validates policies against one detected Windows platform version.
    pub fn validate_for_platform(&self, platform: WindowsPlatformVersion) -> Result<()> {
        self.policies.validate(platform)
    }

    /// Returns the user-facing provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Returns the user-facing provider version.
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// Returns the stable provider GUID.
    pub const fn provider_id(&self) -> WindowsProviderId {
        self.provider_id
    }

    /// Returns the persistent sync-root identity.
    pub const fn sync_root_identity(&self) -> &WindowsSyncRootIdentity {
        &self.sync_root_identity
    }

    /// Returns the required file identity used when callbacks target the root itself.
    pub const fn root_file_identity(&self) -> &WindowsFileIdentity {
        &self.root_file_identity
    }

    /// Returns the requested sync-root policies.
    pub const fn policies(&self) -> WindowsSyncRootPolicies {
        self.policies
    }

    /// Returns persistent registration flags.
    pub const fn options(&self) -> WindowsSyncRootRegistrationOptions {
        self.options
    }

    /// Consumes the registration and returns every owned boundary value.
    pub fn into_parts(
        self,
    ) -> (
        String,
        String,
        WindowsProviderId,
        WindowsSyncRootIdentity,
        WindowsFileIdentity,
        WindowsSyncRootPolicies,
        WindowsSyncRootRegistrationOptions,
    ) {
        (
            self.provider_name,
            self.provider_version,
            self.provider_id,
            self.sync_root_identity,
            self.root_file_identity,
            self.policies,
            self.options,
        )
    }
}

fn validate_sync_root_identity_size(actual: usize) -> Result<()> {
    if actual > CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES {
        return Err(WindowsCloudFilesError::SyncRootIdentityTooLarge {
            actual,
            maximum: CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES,
        });
    }
    Ok(())
}

fn decode_sync_root_identity(bytes: &[u8]) -> Result<CloudScope> {
    if bytes.len() < HEADER_LEN {
        return Err(invalid_sync_root_identity("identity envelope is truncated"));
    }
    if bytes.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        return Err(invalid_sync_root_identity(
            "identity envelope magic does not match",
        ));
    }
    if bytes[MAGIC.len()] != FORMAT_VERSION {
        return Err(invalid_sync_root_identity(
            "identity envelope version is unsupported",
        ));
    }

    let lengths_start = MAGIC.len() + 1;
    let mut lengths = [0usize; 2];
    for (index, length) in lengths.iter_mut().enumerate() {
        let start = lengths_start + index * size_of::<u32>();
        let end = start + size_of::<u32>();
        let raw: [u8; 4] = bytes[start..end]
            .try_into()
            .map_err(|_| invalid_sync_root_identity("identity length table is truncated"))?;
        *length = usize::try_from(u32::from_le_bytes(raw)).map_err(|_| {
            invalid_sync_root_identity("identity field length cannot be represented")
        })?;
    }
    let expected_len = lengths.iter().try_fold(HEADER_LEN, |total, length| {
        total
            .checked_add(*length)
            .ok_or_else(|| invalid_sync_root_identity("identity field lengths overflow"))
    })?;
    if expected_len != bytes.len() {
        return Err(invalid_sync_root_identity(
            "identity field lengths do not match the envelope",
        ));
    }

    let namespace_end = HEADER_LEN + lengths[0];
    let namespace = std::str::from_utf8(&bytes[HEADER_LEN..namespace_end])
        .map_err(|_| invalid_sync_root_identity("identity field is not UTF-8"))?;
    let root = std::str::from_utf8(&bytes[namespace_end..])
        .map_err(|_| invalid_sync_root_identity("identity field is not UTF-8"))?;
    Ok(CloudScope::new(
        CloudNamespaceId::new(namespace)?,
        CloudRootId::new(root)?,
    ))
}

fn validate_registration_text(field: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(invalid_registration_text(field, "value is empty"));
    }
    if value.encode_utf16().any(|unit| unit == 0) {
        return Err(invalid_registration_text(field, "value contains a NUL"));
    }
    if value.encode_utf16().count() > MAX_PROVIDER_TEXT_UTF16_UNITS {
        return Err(invalid_registration_text(
            field,
            "value exceeds 255 UTF-16 code units",
        ));
    }
    Ok(())
}

const fn invalid_sync_root_identity(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidSyncRootIdentity { reason }
}

const fn invalid_registration_text(
    field: &'static str,
    reason: &'static str,
) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidRegistrationString { field, reason }
}

const fn invalid_policy(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidSyncRootPolicy { reason }
}
