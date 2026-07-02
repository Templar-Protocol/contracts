use proc_macro::TokenStream;

use quote::{format_ident, quote};
use syn::{
    parse_macro_input, punctuated::Punctuated, Attribute, Data, DeriveInput, Expr, ExprLit, Fields,
    GenericArgument, Lit, LitStr, Meta, PathArguments, Token, Type,
};

fn has_constructor_default(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut has_default = false;

    for attr in attrs.iter().filter(|attr| attr.path().is_ident("method")) {
        let entries = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for entry in entries {
            match entry {
                Meta::Path(path) => {
                    if path.is_ident("default") {
                        has_default = true;
                    } else {
                        return Err(syn::Error::new_spanned(
                            path,
                            "expected `default` for field-level `#[method(...)]`",
                        ));
                    }
                }
                other => return Err(syn::Error::new_spanned(other, "expected `default`")),
            }
        }
    }

    Ok(has_default)
}

fn option_inner_ty(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}

fn cleaned_doc_text(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();

    for attr in attrs {
        let Meta::NameValue(name_value) = &attr.meta else {
            continue;
        };
        if !name_value.path.is_ident("doc") {
            continue;
        }
        let Expr::Lit(ExprLit {
            lit: Lit::Str(text),
            ..
        }) = &name_value.value
        else {
            continue;
        };

        lines.push(text.value().trim_start().to_owned());
    }

    lines.join("\n").trim().to_owned()
}

fn summary_from_doc(doc: &str) -> String {
    let paragraph = doc
        .split("\n\n")
        .find(|paragraph| !paragraph.trim().is_empty())
        .unwrap_or_default();

    paragraph
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn constructor_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Ok(quote! {});
    };
    let Fields::Named(fields) = &data.fields else {
        return Ok(quote! {});
    };

    let mut new_params = Vec::new();
    let mut initializers = Vec::new();
    let mut setters = Vec::new();

    for field in &fields.named {
        let field_ident = field.ident.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(field, "MethodSpec constructors require named fields")
        })?;
        let has_constructor_default = has_constructor_default(&field.attrs)?;
        let option_inner = option_inner_ty(&field.ty);

        if !has_constructor_default {
            let field_ty = &field.ty;
            new_params.push(quote!(#field_ident: #field_ty));
            initializers.push(quote!(#field_ident));
            continue;
        }

        let default_value = if option_inner.is_some() {
            quote!(None)
        } else {
            quote!(::core::default::Default::default())
        };
        initializers.push(quote!(#field_ident: #default_value));

        let setter_ident = format_ident!("with_{}", field_ident);
        let setter_ty = option_inner.unwrap_or(&field.ty);
        let setter_value = if option_inner.is_some() {
            quote!(Some(value))
        } else {
            quote!(value)
        };
        setters.push(quote! {
            #[must_use]
            pub fn #setter_ident(mut self, value: #setter_ty) -> Self {
                self.#field_ident = #setter_value;
                self
            }
        });
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            #[must_use]
            pub fn new(#(#new_params),*) -> Self {
                Self { #(#initializers),* }
            }

            #(#setters)*
        }
    })
}

/// Attach a gateway method spec to an operation struct.
///
/// The operation struct *is* the method input. A `#[method(..)]` helper
/// attribute declares the RPC method name and direction; the struct's own doc
/// comment supplies the RPC summary/description:
///
/// ```ignore
/// /// Get market configuration.
/// #[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
/// #[method(read = "market.getConfiguration", output = MarketConfiguration)]
/// pub struct GetConfiguration { pub market_id: AccountId }
///
/// /// Withdraw static yield.
/// #[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
/// #[method(write = "market.withdrawStaticYield")]
/// pub struct WithdrawStaticYield { /* .. */ }
/// ```
///
/// The derive also emits an inherent `new(...)` constructor. Fields are
/// constructor parameters unless they opt into `Default::default()` with
/// `#[method(default)]`; opted-in fields also get a fluent `with_<field>(...)`
/// setter. Public fields remain available for explicit struct literals.
///
/// Reads require `output = <Type>`; writes take no output (their output is
/// always `WriteOperationResult`).
#[proc_macro_derive(MethodSpec, attributes(method))]
pub fn derive_method_spec(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;

    let mut method_attrs = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("method"));
    let method_attr = method_attrs.next().ok_or_else(|| {
        syn::Error::new_spanned(
            ident,
            "deriving `MethodSpec` requires a `#[method(read = \"..\", output = ..)]` \
             or `#[method(write = \"..\")]` attribute",
        )
    })?;
    if let Some(duplicate) = method_attrs.next() {
        return Err(syn::Error::new_spanned(
            duplicate,
            "duplicate `#[method(...)]` attribute",
        ));
    }

    let mut read_rpc: Option<LitStr> = None;
    let mut write_rpc: Option<LitStr> = None;
    let mut output: Option<Type> = None;
    method_attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("read") {
            if read_rpc.replace(meta.value()?.parse()?).is_some() {
                return Err(meta.error("duplicate `read` entry"));
            }
        } else if meta.path.is_ident("write") {
            if write_rpc.replace(meta.value()?.parse()?).is_some() {
                return Err(meta.error("duplicate `write` entry"));
            }
        } else if meta.path.is_ident("output") {
            if output.replace(meta.value()?.parse()?).is_some() {
                return Err(meta.error("duplicate `output` entry"));
            }
        } else {
            return Err(meta.error("expected `read`, `write`, or `output`"));
        }
        Ok(())
    })?;

    let (rpc_method, output_ty, method_kind) = match (read_rpc, write_rpc) {
        (Some(rpc), None) => {
            let output = output.ok_or_else(|| {
                syn::Error::new_spanned(method_attr, "read methods require `output = <Type>`")
            })?;
            (
                rpc,
                quote!(#output),
                quote!(templar_gateway_types::spec::MethodKind::Read),
            )
        }
        (None, Some(rpc)) => {
            if output.is_some() {
                return Err(syn::Error::new_spanned(
                    method_attr,
                    "write methods do not take an `output` (it is always `WriteOperationResult`)",
                ));
            }
            (
                rpc,
                quote!(templar_gateway_types::common::WriteOperationResult),
                quote!(templar_gateway_types::spec::MethodKind::Write),
            )
        }
        (Some(_), Some(_)) => {
            return Err(syn::Error::new_spanned(
                method_attr,
                "specify exactly one of `read` or `write`",
            ))
        }
        (None, None) => {
            return Err(syn::Error::new_spanned(
                method_attr,
                "specify one of `read = \"..\"` or `write = \"..\"`",
            ))
        }
    };

    let doc = cleaned_doc_text(&input.attrs);
    let summary = summary_from_doc(&doc);
    let deprecated = input
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("deprecated"));
    let constructor_impl = constructor_impl(input)?;

    Ok(quote! {
        #constructor_impl

        impl templar_gateway_types::MethodSpec for #ident {
            type Output = #output_ty;

            const RPC_METHOD: &'static str = #rpc_method;
        }

        impl templar_gateway_types::spec::RpcMethodMeta for #ident {
            const KIND: templar_gateway_types::spec::MethodKind = #method_kind;
            const SUMMARY: &'static str = #summary;
            const DESCRIPTION: &'static str = #doc;
            const DEPRECATED: bool = #deprecated;
        }
    })
}
