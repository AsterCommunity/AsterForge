//! Stable scoped identities independent from local paths and native platform handles.

use std::fmt;

use crate::{CloudFilesCoreError, Result};

macro_rules! string_identity {
    ($name:ident, $field:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Creates an identity while preserving the caller-provided opaque value exactly.
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() {
                    return Err(CloudFilesCoreError::empty($field));
                }
                Ok(Self(value))
            }

            /// Returns the opaque identity value.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identity and returns its opaque value.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_identity!(
    CloudNamespaceId,
    "cloud namespace id",
    "Opaque identity namespace used to isolate unrelated backend identity spaces."
);
string_identity!(
    CloudRootId,
    "cloud root id",
    "Stable root identity within one cloud namespace."
);
string_identity!(
    CloudItemId,
    "cloud item id",
    "Stable path-independent item identity within one cloud root."
);

/// Namespace and root pair that scopes item identities.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudScope {
    namespace_id: CloudNamespaceId,
    root_id: CloudRootId,
}

impl CloudScope {
    /// Creates a scoped root identity.
    pub const fn new(namespace_id: CloudNamespaceId, root_id: CloudRootId) -> Self {
        Self {
            namespace_id,
            root_id,
        }
    }

    /// Returns the backend identity namespace.
    pub const fn namespace_id(&self) -> &CloudNamespaceId {
        &self.namespace_id
    }

    /// Returns the stable root identity.
    pub const fn root_id(&self) -> &CloudRootId {
        &self.root_id
    }

    /// Consumes the scope and returns its namespace and root identities.
    pub fn into_parts(self) -> (CloudNamespaceId, CloudRootId) {
        (self.namespace_id, self.root_id)
    }
}

/// Fully scoped identity for one cloud item.
///
/// Paths, names, local inodes, CFAPI identity blobs, and File Provider identifiers are adapter
/// state and are deliberately absent from this key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudItemKey {
    scope: CloudScope,
    item_id: CloudItemId,
}

impl CloudItemKey {
    /// Creates a fully scoped item key.
    pub const fn new(scope: CloudScope, item_id: CloudItemId) -> Self {
        Self { scope, item_id }
    }

    /// Returns the namespace/root scope.
    pub const fn scope(&self) -> &CloudScope {
        &self.scope
    }

    /// Returns the stable path-independent item identity.
    pub const fn item_id(&self) -> &CloudItemId {
        &self.item_id
    }

    /// Consumes the key and returns its scope and item identity.
    pub fn into_parts(self) -> (CloudScope, CloudItemId) {
        (self.scope, self.item_id)
    }
}
