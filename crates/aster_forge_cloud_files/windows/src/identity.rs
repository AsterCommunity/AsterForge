//! Versioned mapping between `CloudItemKey` and CFAPI `FileIdentity` bytes.

use std::{fmt, mem::size_of};

use aster_forge_cloud_files_core::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};

use crate::{Result, WindowsCloudFilesError};

/// Maximum `FileIdentity` size accepted by Windows Cloud Files.
pub const CFAPI_FILE_IDENTITY_MAX_BYTES: usize = 4096;

const MAGIC: &[u8; 4] = b"AFCF";
const FORMAT_VERSION: u8 = 1;
const HEADER_LEN: usize = MAGIC.len() + 1 + 3 * size_of::<u32>();

/// Owned, validated CFAPI file identity for one exact Forge item key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WindowsFileIdentity(Vec<u8>);

impl WindowsFileIdentity {
    /// Encodes a scoped, path-independent Forge item key using the current versioned envelope.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn encode(key: &CloudItemKey) -> Result<Self> {
        let fields = [
            key.scope().namespace_id().as_str().as_bytes(),
            key.scope().root_id().as_str().as_bytes(),
            key.item_id().as_str().as_bytes(),
        ];
        let payload_len = fields
            .iter()
            .try_fold(0usize, |total, field| total.checked_add(field.len()))
            .ok_or(WindowsCloudFilesError::FileIdentityTooLarge {
                actual: usize::MAX,
                maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
            })?;
        let total_len = HEADER_LEN.checked_add(payload_len).ok_or(
            WindowsCloudFilesError::FileIdentityTooLarge {
                actual: usize::MAX,
                maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
            },
        )?;
        validate_size(total_len)?;

        let mut encoded = Vec::with_capacity(total_len);
        encoded.extend_from_slice(MAGIC);
        encoded.push(FORMAT_VERSION);
        for field in fields {
            let len = u32::try_from(field.len()).map_err(|_| {
                WindowsCloudFilesError::FileIdentityTooLarge {
                    actual: field.len(),
                    maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
                }
            })?;
            encoded.extend_from_slice(&len.to_le_bytes());
        }
        for field in fields {
            encoded.extend_from_slice(field);
        }
        Ok(Self(encoded))
    }

    /// Validates owned bytes and preserves the canonical identity envelope.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        validate_size(bytes.len())?;
        decode_bytes(&bytes)?;
        Ok(Self(bytes))
    }

    /// Decodes the stable scoped Forge item key.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn decode(&self) -> Result<CloudItemKey> {
        decode_bytes(&self.0)
    }

    /// Returns the exact bytes supplied to CFAPI.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the encoded byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the encoded identity is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consumes the identity and returns its exact bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for WindowsFileIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsFileIdentity")
            .field("format_version", &FORMAT_VERSION)
            .field("byte_len", &self.0.len())
            .finish()
    }
}

fn validate_size(actual: usize) -> Result<()> {
    if actual > CFAPI_FILE_IDENTITY_MAX_BYTES {
        return Err(WindowsCloudFilesError::FileIdentityTooLarge {
            actual,
            maximum: CFAPI_FILE_IDENTITY_MAX_BYTES,
        });
    }
    Ok(())
}

fn decode_bytes(bytes: &[u8]) -> Result<CloudItemKey> {
    if bytes.len() < HEADER_LEN {
        return Err(invalid("identity envelope is truncated"));
    }
    if bytes.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        return Err(invalid("identity envelope magic does not match"));
    }
    if bytes[MAGIC.len()] != FORMAT_VERSION {
        return Err(invalid("identity envelope version is unsupported"));
    }

    let lengths_start = MAGIC.len() + 1;
    let mut lengths = [0usize; 3];
    for (index, length) in lengths.iter_mut().enumerate() {
        let start = lengths_start + index * size_of::<u32>();
        let end = start + size_of::<u32>();
        let raw: [u8; 4] = bytes[start..end]
            .try_into()
            .map_err(|_| invalid("identity length table is truncated"))?;
        *length = usize::try_from(u32::from_le_bytes(raw))
            .map_err(|_| invalid("identity field length cannot be represented"))?;
    }

    let expected_len = lengths.iter().try_fold(HEADER_LEN, |total, length| {
        total
            .checked_add(*length)
            .ok_or_else(|| invalid("identity field lengths overflow"))
    })?;
    if expected_len != bytes.len() {
        return Err(invalid("identity field lengths do not match the envelope"));
    }

    let mut offset = HEADER_LEN;
    let mut fields = Vec::with_capacity(lengths.len());
    for length in lengths {
        let end = offset + length;
        let value = std::str::from_utf8(&bytes[offset..end])
            .map_err(|_| invalid("identity field is not UTF-8"))?;
        fields.push(value.to_owned());
        offset = end;
    }

    let mut fields = fields.into_iter();
    let namespace = fields
        .next()
        .ok_or_else(|| invalid("identity namespace is missing"))?;
    let root = fields
        .next()
        .ok_or_else(|| invalid("identity root is missing"))?;
    let item = fields
        .next()
        .ok_or_else(|| invalid("identity item is missing"))?;
    Ok(CloudItemKey::new(
        CloudScope::new(CloudNamespaceId::new(namespace)?, CloudRootId::new(root)?),
        CloudItemId::new(item)?,
    ))
}

const fn invalid(reason: &'static str) -> WindowsCloudFilesError {
    WindowsCloudFilesError::InvalidFileIdentity { reason }
}
