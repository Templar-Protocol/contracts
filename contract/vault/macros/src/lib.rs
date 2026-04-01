use proc_macro::TokenStream;

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Error, Ident, Result, Token};

struct DeriveFlags(Punctuated<Ident, Token![,]>);

impl Parse for DeriveFlags {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.is_empty() {
            Ok(Self(Punctuated::new()))
        } else {
            Ok(Self(Punctuated::parse_terminated(input)?))
        }
    }
}

#[proc_macro_attribute]
pub fn vault_derive(args: TokenStream, item: TokenStream) -> TokenStream {
    let flags = parse_macro_input!(args as DeriveFlags);

    let mut with_borsh = false;
    let mut with_borsh_schema = false;
    let mut with_postcard = false;
    let mut with_schemars = false;
    let mut with_serde = false;
    let mut with_std_borsh_schema = false;

    for flag in flags.0 {
        match flag.to_string().as_str() {
            "borsh" => with_borsh = true,
            "borsh_schema" => with_borsh_schema = true,
            "postcard" => with_postcard = true,
            "schemars" => with_schemars = true,
            "serde" => with_serde = true,
            "std_borsh_schema" => with_std_borsh_schema = true,
            _ => {
                return Error::new_spanned(
                    flag,
                    "unsupported vault_derive flag; expected one of: borsh, borsh_schema, postcard, schemars, serde, std_borsh_schema",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    let item = proc_macro2::TokenStream::from(item);
    let borsh_attr = with_borsh.then(|| {
        quote! {
            #[cfg_attr(feature = "borsh", derive(borsh::BorshDeserialize, borsh::BorshSerialize))]
        }
    });
    let postcard_attr = with_postcard.then(|| {
        quote! {
            #[cfg_attr(
                all(feature = "postcard", not(feature = "serde")),
                derive(serde::Serialize, serde::Deserialize)
            )]
        }
    });
    let borsh_schema_attr = with_borsh_schema.then(|| {
        quote! {
            #[cfg_attr(feature = "borsh-schema", derive(borsh::BorshSchema))]
        }
    });
    let schemars_attr = with_schemars.then(|| {
        quote! {
            #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        }
    });
    let serde_attr = with_serde.then(|| {
        quote! {
            #[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
        }
    });
    let std_borsh_schema_attr = with_std_borsh_schema.then(|| {
        quote! {
            #[cfg_attr(all(feature = "borsh", feature = "std"), derive(borsh::BorshSchema))]
        }
    });

    quote! {
        #borsh_attr
        #borsh_schema_attr
        #postcard_attr
        #schemars_attr
        #serde_attr
        #std_borsh_schema_attr
        #[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
        #item
    }
    .into()
}
