//! Template generation code for Config derive macro.
//!
//! A template writes a configuration's values and nothing else: what a key means is the
//! declaration's own documentation, which `tola help` prints.

use proc_macro2::TokenStream;
use quote::quote;

use crate::config::attr::{CollectionShape, FieldStatus};
use crate::config::field::ConfigField;
use crate::config::ty::{is_option, vec_elem};
use crate::config::value::{format_scalar_default_for_type, option_placeholder};

pub fn emit_template(fields: &[ConfigField], rt: &syn::Path) -> syn::Result<TokenStream> {
    let fields = fields
        .iter()
        .filter(|f| !f.skip && f.status != FieldStatus::Hidden);
    fields
        .clone()
        .filter(|f| !f.sub)
        .chain(fields.filter(|f| f.sub))
        .map(|f| emit_field_template(f, rt))
        .collect()
}

fn emit_field_template(field: &ConfigField, rt: &syn::Path) -> syn::Result<TokenStream> {
    let ident = &field.rust_ident;
    let (commented, status_msg) = match field.status {
        FieldStatus::Normal => (false, None),
        FieldStatus::Experimental => (
            true,
            Some("# (experimental) this feature may change or be removed\n"),
        ),
        FieldStatus::NotImplemented => (true, Some("# (not implemented)\n")),
        FieldStatus::Deprecated => (
            true,
            Some("# (deprecated) avoid this option in new configuration\n"),
        ),
        FieldStatus::Hidden => return Ok(quote! {}),
    };

    if field.sub {
        if field.collection == Some(CollectionShape::ArrayTable) {
            let elem_ty = vec_elem(&field.ty)
                .expect("array-table element type was validated while parsing the field");
            let nested = emit_nested_template(field, elem_ty, quote! { val });
            return Ok(quote! {
                for (index, val) in config_value.#ident.iter().enumerate() {
                    #nested
                }
            });
        }
        return Ok(emit_nested_template(
            field,
            &field.ty,
            quote! { &config_value.#ident },
        ));
    }

    let status_code = status_msg.map(|msg| quote! { out.push_str(#msg); });
    let prefix = if commented { "# " } else { "" };

    if is_option(&field.ty) && field.default.is_none() {
        let line = format_template_assignment(field, option_placeholder(&field.ty), "# ");
        if commented {
            return Ok(quote! {
                #status_code
                out.push_str(#line);
            });
        }
        let rt_code = emit_runtime_value(field, rt);
        return Ok(quote! {
            #status_code
            if config_value.#ident.is_some() {
                #rt_code
            } else {
                out.push_str(#line);
            }
        });
    }

    if let Some(default_val) = &field.default {
        let formatted = format_scalar_default_for_type(default_val, &field.ty)?;
        let line = format_template_assignment(field, &formatted, prefix);
        return Ok(quote! {
            #status_code
            out.push_str(#line);
        });
    }

    let rt_code = emit_runtime_value(field, rt);
    Ok(quote! {
        #status_code
        out.push_str(#prefix);
        #rt_code
    })
}

fn emit_nested_template(field: &ConfigField, ty: &syn::Type, val: TokenStream) -> TokenStream {
    let propagate = if field.collection == Some(CollectionShape::ArrayTable) {
        let ident = &field.rust_ident;
        quote! {
            .map_err(|error| error.with_array_element(Self::FIELDS.#ident, index))
        }
    } else {
        quote! {}
    };
    quote! {
        let nested = <#ty>::try_template_with_header_from(#val)#propagate?;
        if !nested.is_empty() {
            out.push('\n');
            out.push_str(&nested);
        }
    }
}

pub(super) fn emit_section_header(
    section: TokenStream,
    collection: Option<CollectionShape>,
) -> TokenStream {
    let (open, close) = match collection {
        Some(CollectionShape::ArrayTable) => ("[[", "]]\n"),
        _ => ("[", "]\n"),
    };
    quote! {
        let section = #section;
        if !section.is_empty() {
            out.push_str(#open);
            out.push_str(section);
            out.push_str(#close);
        }
    }
}

fn format_template_assignment(field: &ConfigField, val: &str, prefix: &str) -> String {
    format!("{prefix}{} = {val}\n", field.toml_name)
}

fn emit_runtime_value(field: &ConfigField, rt: &syn::Path) -> TokenStream {
    let ident = &field.rust_ident;
    let toml_name = &field.toml_name;

    quote! {
        out.push_str(#toml_name);
        out.push_str(" = ");
        out.push_str(
            &#rt::serialize_toml_value(&config_value.#ident)
                .map_err(|error| {
                    #rt::ConfigTemplateError::new(Self::FIELDS.#ident, error)
                })?,
        );
        out.push('\n');
    }
}
