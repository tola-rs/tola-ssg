//! Generate Config field paths, fallible TOML templates, and status validation.

mod attr;
mod field;
mod field_type;
mod serde;
mod template;
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
use template::{generate_section_header_code, generate_template_code};

fn is_toml_bare_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Generate Config field paths, template methods, and status validation.
pub fn derive(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;
    let fields_struct_name = syn::Ident::new(&format!("{}Fields", name), name.span());

    let struct_attrs = match ConfigAttrs::parse(&input.attrs, ConfigAttrScope::Struct) {
        Ok(attrs) => attrs,
        Err(error) => return error.into_compile_error(),
    };
    let section = match struct_attrs.section {
        Some(section) => section,
        None => {
            let section = infer_section(&name.to_string());
            if let Err(error) = attr::validate_section_path(&section, name.span()) {
                return error.into_compile_error();
            }
            section
        }
    };
    let runtime_crate: syn::Path = struct_attrs
        .crate_path
        .unwrap_or_else(|| syn::parse_quote!(::tola_config));

    let section_doc = extract_doc_comment(&input.attrs).unwrap_or_default();

    let section_status = struct_attrs.status.unwrap_or(FieldStatus::Normal);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return quote! { compile_error!("Config only works on structs with named fields"); };
            }
        },
        _ => return quote! { compile_error!("Config only works on structs"); },
    };

    if let Err(error) = reject_serde_rename_all(&input.attrs) {
        return error.into_compile_error();
    }

    let config_fields: Vec<ConfigField> = match fields
        .iter()
        .filter_map(|field| ConfigField::from_field(field).transpose())
        .collect()
    {
        Ok(fields) => fields,
        Err(error) => return error.into_compile_error(),
    };

    let fields_for_path = config_fields.iter().filter(|f| !f.skip).collect::<Vec<_>>();

    let field_defs = fields_for_path.iter().map(|f| {
        let name = &f.rust_ident;
        quote! { pub #name: #runtime_crate::FieldPath, }
    });

    let field_paths = fields_for_path
        .iter()
        .map(|f| {
            if section.is_empty() {
                Cow::Borrowed(f.toml_name.as_str())
            } else {
                Cow::Owned(format!("{}.{}", section, f.toml_name))
            }
        })
        .collect::<Vec<_>>();

    let field_inits = fields_for_path.iter().zip(&field_paths).map(|(f, path)| {
        let name = &f.rust_ident;
        quote! { #name: #runtime_crate::FieldPath::new(#path), }
    });

    let declared_fields = fields_for_path.iter().zip(&field_paths).map(|(f, path)| {
        let documentation = declared_documentation_code(f, &runtime_crate);
        quote! { (#runtime_crate::FieldPath::new(#path), #documentation) }
    });

    // Explicit value sets are TOML literals; otherwise only booleans have a finite set.
    let declared_values = fields_for_path
        .iter()
        .zip(&field_paths)
        .filter_map(|(f, path)| {
            if let Some(values) = &f.values {
                return Some(quote! { (#runtime_crate::FieldPath::new(#path), #values) });
            }
            let ty = field_type::option_inner_type(&f.ty).unwrap_or(&f.ty);
            let boolean = field_type::path_type_ident(ty).is_some_and(|name| name == "bool");
            boolean.then(|| quote! { (#runtime_crate::FieldPath::new(#path), &["true", "false"]) })
        });

    let template_code = match generate_template_code(&config_fields, &runtime_crate) {
        Ok(code) => code,
        Err(error) => return error.into_compile_error(),
    };

    let visible_fields = config_fields
        .iter()
        .filter(|field| !field.skip && field.status != FieldStatus::Hidden);
    let template_has_header = struct_attrs.collection == Some(CollectionShape::ArrayTable)
        || visible_fields.clone().any(|field| !field.sub)
        || !visible_fields.clone().any(|field| field.sub);

    let section_header_code = template_has_header.then(|| {
        let header = generate_section_header_code(
            quote! { Self::TEMPLATE_SECTION },
            struct_attrs.collection,
        );
        quote! {
            #header
        }
    });

    // Status diagnostics depend on explicit TOML presence, not differences from defaults.
    let status_checks = config_fields.iter().filter(|f| !f.skip).filter_map(|f| {
        let field_name = &f.rust_ident;
        let status = generate_runtime_status_code(f.status, &runtime_crate)?;
        Some(quote! {
            if diag.is_present(Self::FIELDS.#field_name.as_str()) {
                #runtime_crate::status::check_field_status(
                    Self::FIELDS.#field_name.as_str(),
                    #status,
                    diag,
                );
            }
        })
    });

    let nested_calls = config_fields.iter().filter(|f| !f.skip && f.sub).map(|f| {
        let field_name = &f.rust_ident;
        if f.collection == Some(CollectionShape::ArrayTable) {
            quote! {
                for (index, value) in self.#field_name.iter().enumerate() {
                    diag.with_array_element(Self::FIELDS.#field_name.as_str(), index, |diag| {
                        value.validate_field_status(diag);
                    });
                }
            }
        } else {
            quote! {
                self.#field_name.validate_field_status(diag);
            }
        }
    });

    let section_status_check =
        generate_runtime_status_code(section_status, &runtime_crate).map(|status_token| {
            quote! {
                if diag.is_present(#section) {
                    #runtime_crate::status::check_field_status(
                        #section,
                        #status_token,
                        diag,
                    );
                }
            }
        });

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

            /// Every field this section declares, in declaration order: its path and the
            /// documentation its Rust declaration carries.
            pub const DECLARED_FIELDS: &'static [(#runtime_crate::FieldPath, Option<&'static str>)] =
                &[#(#declared_fields),*];

            /// The values a declared field accepts, when its type states them.
            pub const DECLARED_VALUES: &'static [(#runtime_crate::FieldPath, &'static [&'static str])] =
                &[#(#declared_values),*];

            /// Section name for TOML output.
            pub const TEMPLATE_SECTION: &'static str = #section;

            /// Section documentation.
            pub const TEMPLATE_DOC: &'static str = #section_doc;

            /// Generate a TOML template from this type's default value.
            pub fn try_template() -> Result<String, #runtime_crate::ConfigTemplateError> {
                let default = Self::default();
                Self::try_template_from(&default)
            }

            /// Generate TOML template from an explicit configuration value.
            pub fn try_template_from(value: &Self) -> Result<String, #runtime_crate::ConfigTemplateError> {
                let config_value = value;
                let mut out = String::new();
                #template_code
                Ok(out)
            }

            /// Generate a TOML template with its section header from this
            /// type's default value.
            pub fn try_template_with_header() -> Result<String, #runtime_crate::ConfigTemplateError> {
                let default = Self::default();
                Self::try_template_with_header_from(&default)
            }

            /// Generate TOML template with a section header from an explicit
            /// configuration value. Nested sections use the value supplied by
            /// their parent, so field-specific defaults are preserved.
            pub fn try_template_with_header_from(value: &Self) -> Result<String, #runtime_crate::ConfigTemplateError> {
                let mut out = String::new();
                #section_header_code
                out.push_str(&Self::try_template_from(value)?);
                Ok(out)
            }

            /// Validate field status (experimental, deprecated, not_implemented).
            #[allow(unused_variables)]
            pub fn validate_field_status(&self, diag: &mut #runtime_crate::ConfigDiagnostics) {
                #section_status_check
                #(#status_checks)*
                #(#nested_calls)*
            }
        }
    }
}

/// The documentation one declared key carries.
///
/// A key that opens a section carries the documentation of the section's own declaration, so the
/// table and its header answer with one text; every other key carries the comment its declaration
/// wrote.
fn declared_documentation_code(field: &ConfigField, runtime_crate: &syn::Path) -> TokenStream {
    if let Some(section) = field.section_type() {
        return quote! { #runtime_crate::section_documentation(<#section>::TEMPLATE_DOC) };
    }
    match field.doc.as_deref() {
        Some(documentation) => quote! { Some(#documentation) },
        None => quote! { None },
    }
}

fn generate_runtime_status_code(
    status: FieldStatus,
    runtime_crate: &syn::Path,
) -> Option<TokenStream> {
    match status {
        FieldStatus::NotImplemented => Some(quote! { #runtime_crate::FieldStatus::NotImplemented }),
        FieldStatus::Deprecated => Some(quote! { #runtime_crate::FieldStatus::Deprecated }),
        FieldStatus::Experimental => Some(quote! { #runtime_crate::FieldStatus::Experimental }),
        FieldStatus::Normal | FieldStatus::Hidden => None,
    }
}
