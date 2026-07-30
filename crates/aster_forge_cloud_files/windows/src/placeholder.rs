//! Owned placeholder creation inputs independent from temporary native pointers.

use aster_forge_cloud_files_core::{CloudItem, CloudItemKind};

use crate::{Result, WindowsCloudFilesError, WindowsFileIdentity};

/// Windows directory file-attribute bit.
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// Windows normal-file attribute bit.
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// Windows file times expressed as signed 100-nanosecond intervals since the Windows epoch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsFileTimes {
    /// Creation time.
    pub creation: i64,
    /// Last access time.
    pub last_access: i64,
    /// Last write time.
    pub last_write: i64,
    /// Metadata change time.
    pub change: i64,
}

/// Owned CFAPI filesystem metadata for one placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsPlaceholderMetadata {
    file_size: i64,
    file_attributes: u32,
    times: WindowsFileTimes,
}

impl WindowsPlaceholderMetadata {
    /// Maps one core item into minimal Windows filesystem metadata.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn from_item(item: &CloudItem, times: WindowsFileTimes) -> Result<Self> {
        let (file_size, file_attributes) = match item.kind() {
            CloudItemKind::File => {
                let content =
                    item.content()
                        .ok_or(WindowsCloudFilesError::InvalidPlaceholderMetadata {
                            reason: "file item is missing content metadata",
                        })?;
                let file_size = i64::try_from(content.size()).map_err(|_| {
                    WindowsCloudFilesError::FileSizeTooLarge {
                        size: content.size(),
                    }
                })?;
                (file_size, FILE_ATTRIBUTE_NORMAL)
            }
            CloudItemKind::Directory => (0, FILE_ATTRIBUTE_DIRECTORY),
        };
        Ok(Self {
            file_size,
            file_attributes,
            times,
        })
    }

    /// Returns the signed CFAPI file size.
    #[must_use]
    pub const fn file_size(&self) -> i64 {
        self.file_size
    }

    /// Returns Windows file-attribute bits.
    #[must_use]
    pub const fn file_attributes(&self) -> u32 {
        self.file_attributes
    }

    /// Returns all Windows file times.
    #[must_use]
    pub const fn times(&self) -> WindowsFileTimes {
        self.times
    }
}

/// CFAPI placeholder creation options kept independent from windows-rs types.
#[expect(
    clippy::struct_excessive_bools,
    reason = "these fields map independent CFAPI placeholder creation flags"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsPlaceholderOptions {
    /// Mark the placeholder in-sync during creation.
    pub mark_in_sync: bool,
    /// Disable on-demand directory population for this placeholder.
    pub disable_on_demand_population: bool,
    /// Require the placeholder to remain fully hydrated.
    pub always_full: bool,
    /// Replace an existing placeholder with the same relative name.
    pub supersede: bool,
}

/// Owned input for one `CfCreatePlaceholders` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsPlaceholder {
    relative_name: String,
    metadata: WindowsPlaceholderMetadata,
    identity: WindowsFileIdentity,
    options: WindowsPlaceholderOptions,
}

impl WindowsPlaceholder {
    /// Creates an owned placeholder request and verifies identity/item binding.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn from_item(
        item: &CloudItem,
        identity: WindowsFileIdentity,
        times: WindowsFileTimes,
        options: WindowsPlaceholderOptions,
    ) -> Result<Self> {
        validate_relative_name(item.name())?;
        if identity.decode()? != *item.key() {
            return Err(WindowsCloudFilesError::IdentityItemMismatch);
        }
        Ok(Self {
            relative_name: item.name().to_owned(),
            metadata: WindowsPlaceholderMetadata::from_item(item, times)?,
            identity,
            options,
        })
    }

    /// Returns the validated single-component Windows relative name.
    #[must_use]
    pub fn relative_name(&self) -> &str {
        &self.relative_name
    }

    /// Returns the owned filesystem metadata.
    #[must_use]
    pub const fn metadata(&self) -> WindowsPlaceholderMetadata {
        self.metadata
    }

    /// Returns the stable native identity bytes.
    #[must_use]
    pub const fn identity(&self) -> &WindowsFileIdentity {
        &self.identity
    }

    /// Returns creation options.
    #[must_use]
    pub const fn options(&self) -> WindowsPlaceholderOptions {
        self.options
    }

    /// Consumes the request and returns all owned parts.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        String,
        WindowsPlaceholderMetadata,
        WindowsFileIdentity,
        WindowsPlaceholderOptions,
    ) {
        (
            self.relative_name,
            self.metadata,
            self.identity,
            self.options,
        )
    }
}

fn validate_relative_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(invalid_name("name is empty"));
    }
    if name == "." || name == ".." {
        return Err(invalid_name("dot path components are reserved"));
    }
    if name.ends_with([' ', '.']) {
        return Err(invalid_name("name ends with a space or period"));
    }
    if name.chars().any(|character| {
        character <= '\u{1f}'
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    }) {
        return Err(invalid_name("name contains a Windows-reserved character"));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved_device_name(stem) {
        return Err(invalid_name("name uses a reserved Windows device name"));
    }
    Ok(())
}

fn is_reserved_device_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}

const fn invalid_name(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidPlaceholderName { reason }
}
