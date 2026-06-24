//! OpenAPI attribute macros shared by Aster services.
//!
//! The crate keeps route annotations lightweight in production builds while preserving
//! `utoipa::path` metadata when the `openapi` feature is enabled for debug builds. This lets
//! application code keep a single annotation path without pulling OpenAPI generation into normal
//! release binaries.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

extern crate proc_macro;

use proc_macro::TokenStream;

#[cfg(all(feature = "openapi", debug_assertions))]
use quote::quote;

#[cfg(all(feature = "openapi", debug_assertions))]
/// Expands to `#[utoipa::path(...)]` when OpenAPI generation is enabled for debug builds.
#[proc_macro_attribute]
pub fn path(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);

    quote! {
        #[utoipa::path(#attr)]
        #item
    }
    .into()
}

#[cfg(not(all(feature = "openapi", debug_assertions)))]
/// Leaves the annotated item unchanged when OpenAPI generation is disabled.
#[proc_macro_attribute]
pub fn path(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
