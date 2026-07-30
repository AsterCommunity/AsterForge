//! Distinct opaque continuation tokens for listing, change tracking, and native directory streams.

use std::fmt;

use crate::{CloudFilesCoreError, Result};

macro_rules! opaque_cursor {
    ($name:ident, $field:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(Vec<u8>);

        impl $name {
            /// Creates a non-empty opaque continuation token.
            pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() {
                    return Err(CloudFilesCoreError::empty($field));
                }
                Ok(Self(value))
            }

            /// Copies a non-empty continuation token from a byte slice.
            pub fn from_slice(value: &[u8]) -> Result<Self> {
                Self::new(value.to_vec())
            }

            /// Returns the opaque token bytes.
            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            /// Consumes the token and returns its opaque bytes.
            pub fn into_bytes(self) -> Vec<u8> {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("byte_len", &self.0.len())
                    .finish()
            }
        }
    };
}

opaque_cursor!(
    PageCursor,
    "page cursor",
    "Backend list-pagination position for one enumeration sequence."
);
opaque_cursor!(
    ChangeCursor,
    "change cursor",
    "Durable backend checkpoint for an incremental change stream."
);
opaque_cursor!(
    DirectoryCookie,
    "directory cookie",
    "Continuation token scoped to one native open-directory stream."
);
