//! Generate Config field paths, fallible TOML templates, and status validation.

mod attr;
mod field;
mod serde;
mod template;
mod ty;
mod value;

use std::borrow::Cow;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use attr::{
    CollectionShape, ConfigAttrScope, ConfigAttrs, FieldStatus, extract_doc_comment, infer_section,
};
use field::ConfigField;
use serde::reject_serde_rename_all;
use template::{emit_section_header, emit_template};

fn is_toml_bare_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn pascal_case(name: &syn::Ident) -> String {
    name.to_string()
        .split('_')
        .map(|seg| {
            let mut chars = seg.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

pub fn derive(input: &DeriveInput) -> TokenStream {
    match expand(input) {
        Ok(tokens) => tokens,
        Err(err) => err.into_compile_error(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    reject_generic(input)?;
    let name = &input.ident;
    let fields_ty = syn::Ident::new(&format!("{}Fields", pascal_case(name)), name.span());

    let attr = ConfigAttrs::parse(&input.attrs, ConfigAttrScope::Struct)?;
    let section = match attr.section {
        Some(sec) => sec,
        None => {
            let sec = infer_section(&name.to_string());
            attr::validate_section_path(&sec, name.span())?;
            sec
        }
    };
    let rt: syn::Path = attr
        .crate_path
        .unwrap_or_else(|| syn::parse_quote!(::tola_config));

    let section_doc = extract_doc_comment(&input.attrs).unwrap_or_default();
    let section_status = attr.status.unwrap_or(FieldStatus::Normal);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Ok(
                    quote! { compile_error!("Config only works on structs with named fields"); },
                );
            }
        },
        _ => return Ok(quote! { compile_error!("Config only works on structs"); }),
    };

    reject_serde_rename_all(&input.attrs)?;

    let cfg_fields: Vec<ConfigField> = fields
        .iter()
        .map(ConfigField::from_field)
        .collect::<syn::Result<_>>()?;

    let path_fields: Vec<_> = cfg_fields.iter().filter(|f| !f.skip).collect();

    let field_defs = path_fields.iter().map(|f| {
        let ident = &f.rust_ident;
        quote! { pub #ident: #rt::FieldPath, }
    });

    let field_paths: Vec<_> = path_fields
        .iter()
        .map(|f| {
            if section.is_empty() {
                Cow::Borrowed(f.toml_name.as_str())
            } else {
                Cow::Owned(format!("{section}.{}", f.toml_name))
            }
        })
        .collect();

    let field_inits = path_fields.iter().zip(&field_paths).map(|(f, path)| {
        let ident = &f.rust_ident;
        quote! { #ident: #rt::FieldPath::new(#path), }
    });

    let declared_fields = path_fields.iter().zip(&field_paths).map(|(f, path)| {
        let doc = declared_documentation_code(f, &rt);
        quote! { (#rt::FieldPath::new(#path), #doc) }
    });

    // Explicit value sets are TOML literals; otherwise only booleans have a finite set.
    let declared_values = path_fields
        .iter()
        .zip(&field_paths)
        .filter_map(|(f, path)| {
            if let Some(vals) = &f.values {
                return Some(quote! { (#rt::FieldPath::new(#path), #vals) });
            }
            let inner = ty::option_inner(&f.ty).unwrap_or(&f.ty);
            let is_bool = ty::path_ident(inner).is_some_and(|id| id == "bool");
            is_bool.then(|| quote! { (#rt::FieldPath::new(#path), &["true", "false"]) })
        });

    let fragment_code = generate_fragment(name, &cfg_fields, &rt);
    let template_code = emit_template(&cfg_fields, &rt)?;

    let mut visible = cfg_fields
        .iter()
        .filter(|f| !f.skip && f.status != FieldStatus::Hidden);
    // A section whose visible fields are all nested sections leaves the header to them; an empty
    // section still writes its own.
    let has_header = attr.collection == Some(CollectionShape::ArrayTable)
        || visible.clone().next().is_none()
        || visible.any(|f| !f.sub);

    let header_code =
        has_header.then(|| emit_section_header(quote! { Self::TEMPLATE_SECTION }, attr.collection));

    let status_checks: Vec<TokenStream> = cfg_fields
        .iter()
        .filter(|f| !f.skip)
        .filter_map(|f| {
            let ident = &f.rust_ident;
            let status = emit_field_status(f.status, &rt)?;
            Some(quote! {
                if diag.is_present(Self::FIELDS.#ident.as_str()) {
                    #rt::status::check_field_status(
                        Self::FIELDS.#ident.as_str(),
                        #status,
                        diag,
                    );
                }
            })
        })
        .collect();

    let nested_calls: Vec<TokenStream> = cfg_fields
        .iter()
        .filter(|f| !f.skip && f.sub)
        .map(|f| {
            let ident = &f.rust_ident;
            if f.collection == Some(CollectionShape::ArrayTable) {
                quote! {
                    for (idx, val) in self.#ident.iter().enumerate() {
                        diag.with_array_element(Self::FIELDS.#ident.as_str(), idx, |diag| {
                            val.validate_field_status(diag);
                        });
                    }
                }
            } else {
                quote! {
                    self.#ident.validate_field_status(diag);
                }
            }
        })
        .collect();

    let section_status_check = emit_field_status(section_status, &rt).map(|status_tok| {
        quote! {
            if diag.is_present(#section) {
                #rt::status::check_field_status(#section, #status_tok, diag);
            }
        }
    });

    let uses_diag =
        section_status_check.is_some() || !status_checks.is_empty() || !nested_calls.is_empty();
    let diag = if uses_diag {
        quote!(diag)
    } else {
        quote!(_diag)
    };

    Ok(quote! {
        #fragment_code
        /// Generated field path accessors.
        pub struct #fields_ty {
            #(#field_defs)*
        }

        impl #name {
            /// Field paths for diagnostic messages.
            pub const FIELDS: #fields_ty = #fields_ty {
                #(#field_inits)*
            };

            /// Declared fields in declaration order with their doc comments.
            pub const DECLARED_FIELDS: &'static [(#rt::FieldPath, Option<&'static str>)] =
                &[#(#declared_fields),*];

            /// Accepted TOML literals for fields with finite value sets.
            pub const DECLARED_VALUES: &'static [(#rt::FieldPath, &'static [&'static str])] =
                &[#(#declared_values),*];

            /// Section name for TOML output.
            pub const TEMPLATE_SECTION: &'static str = #section;

            /// Section documentation.
            pub const TEMPLATE_DOC: &'static str = #section_doc;

            /// Generate a TOML template from this type's default value.
            pub fn try_template() -> Result<String, #rt::ConfigTemplateError> {
                let default = Self::default();
                Self::try_template_from(&default)
            }

            /// Generate TOML template from an explicit configuration value.
            pub fn try_template_from(value: &Self) -> Result<String, #rt::ConfigTemplateError> {
                let config_value = value;
                let mut out = String::new();
                #template_code
                Ok(out)
            }

            /// Generate a TOML template with its section header from this type's default value.
            pub fn try_template_with_header() -> Result<String, #rt::ConfigTemplateError> {
                let default = Self::default();
                Self::try_template_with_header_from(&default)
            }

            /// Generate TOML template with a section header from an explicit configuration value.
            pub fn try_template_with_header_from(value: &Self) -> Result<String, #rt::ConfigTemplateError> {
                let mut out = String::new();
                #header_code
                out.push_str(&Self::try_template_from(value)?);
                Ok(out)
            }

            /// Validate field status (experimental, deprecated, not_implemented).
            pub fn validate_field_status(&self, #diag: &mut #rt::ConfigDiagnostics) {
                #section_status_check
                #(#status_checks)*
                #(#nested_calls)*
            }
        }
    })
}

fn declared_documentation_code(field: &ConfigField, rt: &syn::Path) -> TokenStream {
    if let Some(sec) = field.section_type() {
        return quote! { #rt::section_documentation(<#sec>::TEMPLATE_DOC) };
    }
    match field.doc.as_deref() {
        Some(doc) => quote! { Some(#doc) },
        None => quote! { None },
    }
}

fn emit_field_status(status: FieldStatus, rt: &syn::Path) -> Option<TokenStream> {
    match status {
        FieldStatus::NotImplemented => Some(quote! { #rt::FieldStatus::NotImplemented }),
        FieldStatus::Deprecated => Some(quote! { #rt::FieldStatus::Deprecated }),
        FieldStatus::Experimental => Some(quote! { #rt::FieldStatus::Experimental }),
        FieldStatus::Normal | FieldStatus::Hidden => None,
    }
}

pub fn derive_fragment(input: &DeriveInput) -> TokenStream {
    match expand_fragment(input) {
        Ok(tokens) => tokens,
        Err(err) => err.into_compile_error(),
    }
}

fn expand_fragment(input: &DeriveInput) -> syn::Result<TokenStream> {
    reject_generic(input)?;
    let attr = ConfigAttrs::parse(&input.attrs, ConfigAttrScope::Struct)?;
    let rt = attr
        .crate_path
        .unwrap_or_else(|| syn::parse_quote!(::tola_config));
    reject_serde_rename_all(&input.attrs)?;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Fragment requires a named struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "Fragment requires named fields",
        ));
    };
    let fields = fields
        .named
        .iter()
        .map(ConfigField::from_fragment_field)
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(generate_fragment(&input.ident, &fields, &rt))
}

fn generate_fragment(name: &syn::Ident, fields: &[ConfigField], rt: &syn::Path) -> TokenStream {
    let arms = fields.iter().filter(|f| !f.deserialize_skip).map(|f| {
        let key = &f.toml_name;
        let ty = &f.ty;
        let check = if let Some(adapter) = &f.fragment_with {
            quote! { #adapter(val, ctx); }
        } else {
            quote! { <#ty as #rt::ValidateFragment>::validate_fragment(val, ctx); }
        };
        quote! { #key => ctx.key(key, |ctx| { #check }), }
    });
    quote! {
        impl #rt::ValidateFragment for #name {
            fn validate_fragment(val: &#rt::toml::Value, ctx: &mut #rt::FragmentContext<'_>) {
                let #rt::toml::Value::Table(tbl) = val else {
                    #rt::wrong_type(val, "table", ctx);
                    return;
                };
                for (key, val) in tbl {
                    match key.as_str() {
                        #(#arms)*
                        _ => ctx.key(key, |ctx| ctx.issue(#rt::FragmentIssueKind::UnknownKey)),
                    }
                }
            }
        }
    }
}

fn reject_generic(input: &DeriveInput) -> syn::Result<()> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "fragment derives do not support generic records",
        ));
    }
    Ok(())
}
