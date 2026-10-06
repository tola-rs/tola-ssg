//! Template generation code for Config derive macro.
//!
//! A template writes a configuration's values and nothing else: what a key means is the
//! declaration's own documentation, which `tola help` prints.

use proc_macro2::TokenStream;
use quote::quote;

use crate::config::attr::{CollectionShape, FieldStatus};
use crate::config::field::ConfigField;
use crate::config::field_type::{is_option_type, vec_element_type};
use crate::config::value::{format_scalar_default_for_type, option_placeholder};

/// Generate TOML template code for the fields.
pub fn generate_template_code(
    fields: &[ConfigField],
    runtime_crate: &syn::Path,
) -> syn::Result<TokenStream> {
    let fields = fields
        .iter()
        .filter(|field| !field.skip && field.status != FieldStatus::Hidden);
    fields
        .clone()
        .filter(|field| !field.sub)
        .chain(fields.filter(|field| field.sub))
        .map(|field| generate_field_template_code(field, runtime_crate))
        .collect()
}

/// Generate TOML template code for one field.
fn generate_field_template_code(
    field: &ConfigField,
    runtime_crate: &syn::Path,
) -> syn::Result<TokenStream> {
    let field_name = &field.rust_ident;
    let (is_commented, status_comment) = match field.status {
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
            let element_ty = vec_element_type(&field.ty)
                .expect("array-table element type was validated while parsing the field");
            let nested_code = generate_nested_template_code(field, element_ty, quote! { value });
            return Ok(quote! {
                for (index, value) in config_value.#field_name.iter().enumerate() {
                    #nested_code
                }
            });
        }
        return Ok(generate_nested_template_code(
            field,
            &field.ty,
            quote! { &config_value.#field_name },
        ));
    }

    let status_code = status_comment.map(|comment| quote! { out.push_str(#comment); });
    let prefix = if is_commented { "# " } else { "" };

    if is_option_type(&field.ty) && field.default.is_none() {
        let line = format_template_assignment(field, option_placeholder(&field.ty), "# ");
        if is_commented {
            return Ok(quote! {
                #status_code
                out.push_str(#line);
            });
        }
        let runtime_code = generate_runtime_value_code(field, runtime_crate);
        return Ok(quote! {
            #status_code
            if config_value.#field_name.is_some() {
                #runtime_code
            } else {
                out.push_str(#line);
            }
        });
    }

    // Explicit template defaults are scalar values known at expansion time.
    if let Some(default_val) = &field.default {
        let formatted = format_scalar_default_for_type(default_val, &field.ty)?;
        let line = format_template_assignment(field, &formatted, prefix);
        return Ok(quote! {
            #status_code
            out.push_str(#line);
        });
    }

    let runtime_code = generate_runtime_value_code(field, runtime_crate);
    Ok(quote! {
        #status_code
        out.push_str(#prefix);
        #runtime_code
    })
}

/// Nested sections render the values their parent supplies.
fn generate_nested_template_code(
    field: &ConfigField,
    field_ty: &syn::Type,
    value: TokenStream,
) -> TokenStream {
    let propagate = if field.collection == Some(CollectionShape::ArrayTable) {
        let field_name = &field.rust_ident;
        quote! {
            .map_err(|error| error.with_array_element(Self::FIELDS.#field_name, index))
        }
    } else {
        quote! {}
    };
    quote! {
        let nested = <#field_ty>::try_template_with_header_from(#value)#propagate?;
        if !nested.is_empty() {
            out.push('\n');
            out.push_str(&nested);
        }
    }
}

pub(super) fn generate_section_header_code(
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

fn format_template_assignment(field: &ConfigField, value: &str, prefix: &str) -> String {
    let toml_name = &field.toml_name;
    format!("{prefix}{toml_name} = {value}\n")
}

fn generate_runtime_value_code(field: &ConfigField, runtime_crate: &syn::Path) -> TokenStream {
    let field_name = &field.rust_ident;
    let toml_name = &field.toml_name;

    quote! {
        out.push_str(#toml_name);
        out.push_str(" = ");
        out.push_str(
            &#runtime_crate::serialize_toml_value(&config_value.#field_name)
                .map_err(|error| {
                    #runtime_crate::ConfigTemplateError::new(Self::FIELDS.#field_name, error)
                })?,
        );
        out.push('\n');
    }
}
