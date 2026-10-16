//! Validated identifiers and injectable refresh ID generation.

use crate::{AuthError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! identifier {
    ($name:ident, $doc:literal, $redact:expr) => {
        #[doc = $doc]
        #[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            /// Constructs a nonempty identifier of at most 256 bytes without control characters.
            ///
            /// # Errors
            /// Returns `InvalidPolicy` for invalid identifiers. Deserialization applies the same
            /// checks, so signed JSON cannot bypass the constructor's invariants.
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }
            /// Returns the storage/wire identity; never log refresh JTIs.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = AuthError;
            fn try_from(value: String) -> Result<Self> {
                Self::new(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&if $redact { "<redacted>" } else { &self.0 })
                    .finish()
            }
        }
    };
}

identifier!(
    Subject,
    "Stable product subject; integer IDs and UUIDs use their string representation.",
    false
);
identifier!(
    SessionId,
    "Server-side session identity, distinct from its single-use refresh JTI.",
    false
);
identifier!(
    RefreshJti,
    "Single-use refresh identity. Debug output is always redacted.",
    true
);

/// Supplies unpredictable refresh IDs in production and deterministic IDs in tests.
pub trait IdSource: Send + Sync {
    /// Generates the next refresh identifier.
    ///
    /// # Errors
    /// Returns a mechanism error when generation fails. Production sources must supply unique,
    /// unpredictable values; stores must also enforce JTI uniqueness.
    fn next_id(&self) -> Result<RefreshJti>;
}

/// Production ID source using the upstream UUID v4 implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomIdSource;
impl IdSource for RandomIdSource {
    fn next_id(&self) -> Result<RefreshJti> {
        RefreshJti::new(uuid::Uuid::new_v4().to_string())
    }
}

pub(crate) fn validate_identifier(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AuthError::InvalidPolicy);
    }
    Ok(())
}
