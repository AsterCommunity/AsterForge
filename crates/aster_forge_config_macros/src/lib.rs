//! Derive macros for configuration section validation.

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
use proc_macro_crate::{FoundCrate, crate_name};
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input, parse_quote};

/// Generates unconditional validation calls for every named configuration field.
#[proc_macro_derive(ValidateConfig)]
pub fn validate_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let mut generics = input.generics;
    let config_crate = match crate_name("aster_forge_config") {
        Ok(FoundCrate::Itself) => quote!(::aster_forge_config),
        Ok(FoundCrate::Name(found)) => {
            let ident = syn::Ident::new(&found, proc_macro2::Span::call_site());
            quote!(::#ident)
        }
        Err(error) => {
            return syn::Error::new(name.span(), error)
                .to_compile_error()
                .into();
        }
    };
    let fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            _ => {
                return syn::Error::new_spanned(
                    name,
                    "ValidateConfig requires a struct with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(name, "ValidateConfig requires a struct")
                .to_compile_error()
                .into();
        }
    };
    let names = fields
        .iter()
        .filter_map(|field| field.ident.as_ref())
        .collect::<Vec<_>>();
    for field in &fields {
        let ty = &field.ty;
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #config_crate::ValidatableConfig));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let expanded = quote! {
        impl #impl_generics #config_crate::ConfigSections for #name #ty_generics #where_clause {
            fn validate_sections(&self) -> #config_crate::Result<()> {
                #(
                    #config_crate::ValidatableConfig::validate(&self.#names)?;
                )*
                Ok(())
            }
        }
    };
    expanded.into()
}
