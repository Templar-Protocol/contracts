use proc_macro::TokenStream;

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{
    parse_macro_input, Attribute, Expr, ExprLit, Ident, Lit, LitStr, Meta, Result, Token, Type,
};

enum MethodKind {
    Read,
    Write,
}

struct MethodSpecInput {
    attrs: Vec<Attribute>,
    ident: Ident,
    _comma_1: Token![,],
    rpc_method: LitStr,
    _comma_2: Token![,],
    input: Type,
    _comma_3: Token![,],
    output: Type,
}

impl Parse for MethodSpecInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(Self {
            attrs: input.call(Attribute::parse_outer)?,
            ident: input.parse()?,
            _comma_1: input.parse()?,
            rpc_method: input.parse()?,
            _comma_2: input.parse()?,
            input: input.parse()?,
            _comma_3: input.parse()?,
            output: input.parse()?,
        })
    }
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
            lit: Lit::Str(text), ..
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

fn expand_method(input: MethodSpecInput, kind: MethodKind) -> TokenStream {
    let MethodSpecInput {
        attrs,
        ident,
        rpc_method,
        input,
        output,
        ..
    } = input;

    let doc = cleaned_doc_text(&attrs);
    let summary = summary_from_doc(&doc);
    let deprecated = attrs.iter().any(|attr| attr.path().is_ident("deprecated"));

    let request_ty = match kind {
        MethodKind::Read => quote!(crate::common::ReadRequest<#input>),
        MethodKind::Write => quote!(crate::common::WriteRequest<#input>),
    };
    let method_kind = match kind {
        MethodKind::Read => quote!(crate::spec::MethodKind::Read),
        MethodKind::Write => quote!(crate::spec::MethodKind::Write),
    };

    quote! {
        #(#attrs)*
        pub struct #ident;

        impl crate::MethodSpec for #ident {
            type Input = #request_ty;
            type Output = #output;

            const RPC_METHOD: &'static str = #rpc_method;
        }

        impl crate::spec::RpcMethodMeta for #ident {
            const KIND: crate::spec::MethodKind = #method_kind;
            const SUMMARY: &'static str = #summary;
            const DESCRIPTION: &'static str = #doc;
            const DEPRECATED: bool = #deprecated;
        }
    }
    .into()
}

#[proc_macro]
pub fn public_read_method_spec(input: TokenStream) -> TokenStream {
    expand_method(parse_macro_input!(input as MethodSpecInput), MethodKind::Read)
}

#[proc_macro]
pub fn write_method_spec(input: TokenStream) -> TokenStream {
    expand_method(parse_macro_input!(input as MethodSpecInput), MethodKind::Write)
}
