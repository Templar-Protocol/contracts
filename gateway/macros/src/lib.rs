use proc_macro::TokenStream;

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{
    parenthesized, parse_macro_input, Attribute, Expr, ExprLit, Ident, Lit, LitStr, Meta, Result,
    Token, Type,
};

struct ReadMethodSpecInput {
    attrs: Vec<Attribute>,
    rpc_method: LitStr,
    ident: Ident,
    input: Type,
    output: Type,
}

impl Parse for ReadMethodSpecInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let rpc_method: LitStr = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ident: Ident = input.parse()?;

        let input_content;
        parenthesized!(input_content in input);
        let input_ty: Type = input_content.parse()?;

        let _: Token![->] = input.parse()?;
        let output: Type = input.parse()?;

        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
        }

        Ok(Self {
            attrs,
            rpc_method,
            ident,
            input: input_ty,
            output,
        })
    }
}

struct WriteMethodSpecInput {
    attrs: Vec<Attribute>,
    rpc_method: LitStr,
    ident: Ident,
    input: Type,
}

impl Parse for WriteMethodSpecInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let rpc_method: LitStr = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ident: Ident = input.parse()?;

        let input_content;
        parenthesized!(input_content in input);
        let input_ty: Type = input_content.parse()?;

        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
        }

        Ok(Self {
            attrs,
            rpc_method,
            ident,
            input: input_ty,
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

fn expand_method(
    attrs: Vec<Attribute>,
    rpc_method: LitStr,
    ident: Ident,
    request_ty: proc_macro2::TokenStream,
    output_ty: proc_macro2::TokenStream,
    method_kind: proc_macro2::TokenStream,
) -> TokenStream {
    let doc = cleaned_doc_text(&attrs);
    let summary = summary_from_doc(&doc);
    let deprecated = attrs.iter().any(|attr| attr.path().is_ident("deprecated"));

    quote! {
        #[doc = concat!("RPC method: `", #rpc_method, "`")]
        #[doc = ""]
        #(#attrs)*
        pub struct #ident;

        impl templar_gateway_types::MethodSpec for #ident {
            type Input = #request_ty;
            type Output = #output_ty;

            const RPC_METHOD: &'static str = #rpc_method;
        }

        impl templar_gateway_types::spec::RpcMethodMeta for #ident {
            const KIND: templar_gateway_types::spec::MethodKind = #method_kind;
            const SUMMARY: &'static str = #summary;
            const DESCRIPTION: &'static str = #doc;
            const DEPRECATED: bool = #deprecated;
        }
    }
    .into()
}

#[proc_macro]
pub fn read_method_spec(input: TokenStream) -> TokenStream {
    let ReadMethodSpecInput {
        attrs,
        rpc_method,
        ident,
        input,
        output,
    } = parse_macro_input!(input as ReadMethodSpecInput);

    expand_method(
        attrs,
        rpc_method,
        ident,
        quote!(templar_gateway_types::common::ReadRequest<#input>),
        quote!(#output),
        quote!(templar_gateway_types::spec::MethodKind::Read),
    )
}

#[proc_macro]
pub fn write_method_spec(input: TokenStream) -> TokenStream {
    let WriteMethodSpecInput {
        attrs,
        rpc_method,
        ident,
        input,
    } = parse_macro_input!(input as WriteMethodSpecInput);

    expand_method(
        attrs,
        rpc_method,
        ident,
        quote!(templar_gateway_types::common::WriteRequest<#input>),
        quote!(templar_gateway_types::common::WriteOperationResult),
        quote!(templar_gateway_types::spec::MethodKind::Write),
    )
}
