//! Versioned persistent identifiers for Apple File Provider items and system containers.

use std::fmt;

use aster_forge_cloud_files_core::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::{MacosBridgeError, Result};

/// Apple root-container identifier used by File Provider extensions.
pub const FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER: &str =
    "NSFileProviderRootContainerItemIdentifier";
/// Apple working-set pseudo-container identifier.
pub const FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER: &str =
    "NSFileProviderWorkingSetContainerItemIdentifier";
/// Apple trash pseudo-container identifier.
pub const FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER: &str =
    "NSFileProviderTrashContainerItemIdentifier";

const PREFIX: &str = "afcf1.";
const FIELD_COUNT: usize = 3;

/// File Provider system container that does not represent a product item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosFileProviderSystemContainer {
    /// Root container for the active domain.
    Root,
    /// Working-set pseudo-container maintained by the extension/system contract.
    WorkingSet,
    /// Trash pseudo-container.
    Trash,
}

/// Owned, validated persistent File Provider identifier.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MacosFileProviderIdentifier {
    /// One exact scoped product item.
    Item { encoded: String, key: CloudItemKey },
    /// A File Provider system container.
    System(MacosFileProviderSystemContainer),
}

impl MacosFileProviderIdentifier {
    /// Encodes one stable path-independent item key as a File Provider identifier.
    pub fn encode(key: &CloudItemKey) -> Self {
        let encoded = [
            key.scope().namespace_id().as_str(),
            key.scope().root_id().as_str(),
            key.item_id().as_str(),
        ]
        .map(|value| URL_SAFE_NO_PAD.encode(value.as_bytes()))
        .join(".");
        Self::Item {
            encoded: format!("{PREFIX}{encoded}"),
            key: key.clone(),
        }
    }

    /// Parses an identifier received from Swift/File Provider.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        match value.as_str() {
            FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER => {
                return Ok(Self::System(MacosFileProviderSystemContainer::Root));
            }
            FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER => {
                return Ok(Self::System(MacosFileProviderSystemContainer::WorkingSet));
            }
            FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER => {
                return Ok(Self::System(MacosFileProviderSystemContainer::Trash));
            }
            _ => {}
        }
        let payload = value
            .strip_prefix(PREFIX)
            .ok_or_else(|| invalid("identifier prefix or version is unsupported"))?;
        let fields = payload.split('.').collect::<Vec<_>>();
        if fields.len() != FIELD_COUNT {
            return Err(invalid(
                "identifier must contain exactly three encoded fields",
            ));
        }
        let mut decoded = Vec::with_capacity(FIELD_COUNT);
        for field in fields {
            if field.is_empty() {
                return Err(invalid("identifier field must not be empty"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(field)
                .map_err(|_| invalid("identifier field is not canonical base64url"))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| invalid("identifier field is not valid UTF-8"))?;
            if URL_SAFE_NO_PAD.encode(text.as_bytes()) != field {
                return Err(invalid("identifier field is not canonical base64url"));
            }
            decoded.push(text);
        }
        let mut decoded = decoded.into_iter();
        let namespace = decoded
            .next()
            .ok_or_else(|| invalid("identifier namespace is missing"))?;
        let root = decoded
            .next()
            .ok_or_else(|| invalid("identifier root is missing"))?;
        let item = decoded
            .next()
            .ok_or_else(|| invalid("identifier item is missing"))?;
        let namespace = CloudNamespaceId::new(namespace)
            .map_err(|_| invalid("identifier namespace must not be empty"))?;
        let root =
            CloudRootId::new(root).map_err(|_| invalid("identifier root must not be empty"))?;
        let item =
            CloudItemId::new(item).map_err(|_| invalid("identifier item must not be empty"))?;
        let key = CloudItemKey::new(CloudScope::new(namespace, root), item);
        Ok(Self::Item {
            encoded: value,
            key,
        })
    }

    /// Returns the exact identifier string supplied to File Provider.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Item { encoded, .. } => encoded,
            Self::System(MacosFileProviderSystemContainer::Root) => {
                FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER
            }
            Self::System(MacosFileProviderSystemContainer::WorkingSet) => {
                FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER
            }
            Self::System(MacosFileProviderSystemContainer::Trash) => {
                FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER
            }
        }
    }

    /// Returns the decoded product item key, or a classified system-container error.
    pub fn item_key(&self) -> Result<&CloudItemKey> {
        match self {
            Self::Item { key, .. } => Ok(key),
            Self::System(_) => Err(MacosBridgeError::SystemContainerIsNotItem),
        }
    }

    /// Returns the system-container class, when this is not a product item.
    pub const fn system_container(&self) -> Option<MacosFileProviderSystemContainer> {
        match self {
            Self::Item { .. } => None,
            Self::System(container) => Some(*container),
        }
    }

    /// Consumes the identifier into its exact string representation.
    pub fn into_string(self) -> String {
        match self {
            Self::Item { encoded, .. } => encoded,
            Self::System(container) => match container {
                MacosFileProviderSystemContainer::Root => {
                    FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER.to_owned()
                }
                MacosFileProviderSystemContainer::WorkingSet => {
                    FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER.to_owned()
                }
                MacosFileProviderSystemContainer::Trash => {
                    FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER.to_owned()
                }
            },
        }
    }
}

impl fmt::Debug for MacosFileProviderIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item { encoded, .. } => formatter
                .debug_struct("MacosFileProviderIdentifier")
                .field("kind", &"item")
                .field("byte_len", &encoded.len())
                .finish(),
            Self::System(container) => formatter
                .debug_tuple("MacosFileProviderIdentifier")
                .field(container)
                .finish(),
        }
    }
}

const fn invalid(reason: &'static str) -> MacosBridgeError {
    MacosBridgeError::InvalidIdentifier { reason }
}
