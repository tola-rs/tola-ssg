//! Config derive macro - generates FIELDS and template().
//!
//! Combines field path generation and TOML template generation.

mod attr;
mod field;
mod template;
mod types;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use attr::{extract_doc_comment, get_section, parse_field_status};
use field::{FieldInfo, FieldStatus};
use template::generate_template_code;
use types::infer_section;

/// Generate Config implementation (FIELDS + template).
pub fn derive(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;
    let fields_struct_name = syn::Ident::new(&format!("{}Fields", name), name.span());

    let section =
        get_section(&input.attrs).unwrap_or_else(|| infer_section(&name.to_string()));

    let section_doc = extract_doc_comment(&input.attrs).unwrap_or_default();

    // Parse section-level status (applies to entire struct)
    let section_status = parse_field_status(&input.attrs);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return quote! { compile_error!("Config only works on structs with named fields"); }
            }
        },
        _ => return quote! { compile_error!("Config only works on structs"); },
    };

    // Collect field info
    let field_infos: Vec<FieldInfo> = fields
        .iter()
        .filter_map(FieldInfo::from_field)
        .collect();

    // Generate FIELDS struct (skip fields with #[config(skip)])
    let fields_for_path: Vec<_> = field_infos.iter().filter(|f| !f.skip).collect();

    let field_defs = fields_for_path.iter().map(|f| {
        let name = &f.name;
        quote! { pub #name: crate::config::FieldPath, }
    });

    let field_inits = fields_for_path.iter().map(|f| {
        let name = &f.name;
        let full_path = if section.is_empty() {
            f.toml_name.clone()
        } else {
            format!("{}.{}", section, f.toml_name)
        };
        quote! { #name: crate::config::FieldPath::new(#full_path), }
    });

    // Generate template code (skip hidden and skip fields)
    let template_fields: Vec<_> = field_infos
        .iter()
        .filter(|f| !f.skip && f.status != FieldStatus::Hidden)
        .collect();

    let template_code = generate_template_code(&template_fields);

    // Collect own fields (non-sub, non-skip) for status checks
    let own_fields: Vec<_> = field_infos
        .iter()
        .filter(|f| !f.skip && !f.sub)
        .collect();

    // Check if we need `default` variable (for section or field status checks)
    let has_section_status = matches!(
        section_status,
        FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental
    );
    let has_field_status = field_infos.iter().any(|f| {
        !f.skip
            && !f.sub
            && matches!(
                f.status,
                FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental
            )
    });
    let needs_default = (has_section_status && !own_fields.is_empty()) || has_field_status;

    // Generate field status checks for special status fields
    let status_checks: Vec<_> = field_infos
        .iter()
        .filter(|f| {
            !f.skip
                && !f.sub
                && matches!(
                    f.status,
                    FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental
                )
        })
        .map(|f| {
            let field_name = &f.name;
            let full_path = if section.is_empty() {
                f.toml_name.clone()
            } else {
                format!("{}.{}", section, f.toml_name)
            };
            let status = match f.status {
                FieldStatus::NotImplemented => {
                    quote! { crate::config::types::FieldStatus::NotImplemented }
                }
                FieldStatus::Deprecated => {
                    quote! { crate::config::types::FieldStatus::Deprecated }
                }
                FieldStatus::Experimental => {
                    quote! { crate::config::types::FieldStatus::Experimental }
                }
                _ => unreachable!(),
            };
            quote! {
                if self.#field_name != default.#field_name {
                    crate::config::types::check_field_status(
                        #full_path,
                        #status,
                        diag,
                    );
                }
            }
        })
        .collect();

    // Generate recursive calls for nested Config types
    let nested_calls: Vec<_> = field_infos
        .iter()
        .filter(|f| !f.skip && f.sub)
        .map(|f| {
            let field_name = &f.name;
            quote! {
                self.#field_name.validate_field_status(diag);
            }
        })
        .collect();

    // Generate section-level status check
    let section_status_check = if has_section_status && !own_fields.is_empty() {
        let status_token = match section_status {
            FieldStatus::NotImplemented => {
                quote! { crate::config::types::FieldStatus::NotImplemented }
            }
            FieldStatus::Deprecated => quote! { crate::config::types::FieldStatus::Deprecated },
            FieldStatus::Experimental => {
                quote! { crate::config::types::FieldStatus::Experimental }
            }
            _ => unreachable!(),
        };

        let field_checks: Vec<_> = own_fields
            .iter()
            .map(|f| {
                let field_name = &f.name;
                quote! { self.#field_name != default.#field_name }
            })
            .collect();

        quote! {
            if #(#field_checks)||* {
                crate::config::types::check_section_status(
                    #section,
                    #status_token,
                    diag,
                );
            }
        }
    } else {
        quote! {}
    };

    let default_def = if needs_default {
        quote! { let default = Self::default(); }
    } else {
        quote! {}
    };

    quote! {
        /// Generated field path accessors.
        #[allow(non_camel_case_types)]
        pub struct #fields_struct_name {
            #(#field_defs)*
        }

        impl #name {
            /// Field paths for diagnostic messages.
            pub const FIELDS: #fields_struct_name = #fields_struct_name {
                #(#field_inits)*
            };

            /// Section name for TOML output.
            pub const TEMPLATE_SECTION: &'static str = #section;

            /// Section documentation.
            pub const TEMPLATE_DOC: &'static str = #section_doc;

            /// Generate TOML template for this config section.
            pub fn template() -> String {
                let default = Self::default();
                let mut out = String::new();
                #template_code
                out
            }

            /// Generate TOML template with section header.
            pub fn template_with_header() -> String {
                let mut out = String::new();
                let doc = Self::TEMPLATE_DOC;
                if !doc.is_empty() {
                    for line in doc.lines() {
                        out.push_str("# ");
                        out.push_str(line.trim());
                        out.push('\n');
                    }
                }
                let section = Self::TEMPLATE_SECTION;
                if !section.is_empty() {
                    out.push('[');
                    out.push_str(section);
                    out.push_str("]\n");
                }
                out.push_str(&Self::template());
                out
            }

            /// Validate field status (experimental, deprecated, not_implemented).
            #[allow(unused_variables)]
            pub fn validate_field_status(&self, diag: &mut crate::config::ConfigDiagnostics) {
                #default_def
                #section_status_check
                #(#status_checks)*
                #(#nested_calls)*
            }
        }
    }
}
