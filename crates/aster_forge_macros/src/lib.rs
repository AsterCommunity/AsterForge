//! Product-neutral declarative macros for lossless string protocol enums.
//!
//! Consumers own variant names, wire values, and all execution policy.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

/// Declare a string-valued protocol enum with lossless unknown-value handling.
///
/// Generates `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, Serde serialization,
/// deserialization, and owned `String` conversions. Consumers must directly depend
/// on `serde` with its `derive` feature. Known values are exact and case-sensitive;
/// unknown strings are retained, including empty strings. Only string inputs deserialize.
/// `Unknown` is reserved, at least one known variant is required, and wire values must
/// be unique. This macro does not decide whether a value is executable or successful.
///
/// ```
/// use aster_forge_macros::wire_enum;
/// wire_enum! {
///     /// Product-defined wire state.
///     pub enum Status {
///         /// Ready to run.
///         Ready => "ready",
///     }
/// }
/// assert_eq!(Status::from(String::from("ready")), Status::Ready);
/// assert_eq!(Status::from(String::from("future")), Status::Unknown("future".into()));
/// assert_eq!(serde_json::to_string(&Status::Ready)?, "\"ready\"");
/// # Ok::<(), serde_json::Error>(())
/// ```
///
/// Duplicate wire values are compile errors:
///
/// ```compile_fail
/// aster_forge_macros::wire_enum! {
///     enum Duplicate { First => "same", Second => "same" }
/// }
/// ```
///
/// `Unknown` is reserved:
///
/// ```compile_fail
/// aster_forge_macros::wire_enum! {
///     enum Reserved { Unknown => "unknown" }
/// }
/// ```
///
/// Empty known-variant sets are rejected:
///
/// ```compile_fail
/// aster_forge_macros::wire_enum! { enum Empty {} }
/// ```
#[macro_export]
macro_rules! wire_enum {
    (
        $(#[$meta:meta])* $visibility:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(from = "::std::string::String", into = "::std::string::String")]
        $visibility enum $name {
            $($(#[$variant_meta])* $variant,)+
            /// Unrecognized wire string, retained verbatim.
            Unknown(::std::string::String),
        }

        impl ::core::convert::From<::std::string::String> for $name {
            fn from(value: ::std::string::String) -> Self {
                #[deny(unreachable_patterns)]
                match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value),
                }
            }
        }

        impl ::core::convert::From<$name> for ::std::string::String {
            fn from(value: $name) -> Self {
                match value {
                    $($name::$variant => ::std::string::String::from($wire),)+
                    $name::Unknown(value) => value,
                }
            }
        }
    };
}

#[cfg(test)]
mod tests;
