//! Config derive macro - generates FIELDS and template().
//!
//! Combines field path generation and TOML template generation.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, Lit, Meta, Type};

/// Field status for template generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStatus {
    Normal,
    Experimental,
    NotImplemented,
    Deprecated,
    Hidden,
}

/// Parsed field information.
#[derive(Debug)]
pub struct FieldInfo {
    pub name: syn::Ident,
    pub toml_name: String,
    pub doc: Option<String>,
    pub inline_doc: bool,  // show doc as inline comment if single line
    pub status: FieldStatus,
    pub default: Option<String>,
    pub skip: bool,
    pub sub_config: bool,  // explicitly marked nested config for recursive validation
    pub ty: String,
}

/// Generate Config implementation (FIELDS + template).
pub fn derive(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;
    let fields_struct_name = syn::Ident::new(&format!("{}Fields", name), name.span());

    let section = get_section(&input.attrs)
        .unwrap_or_else(|| infer_section(&name.to_string()));

    let section_doc = extract_doc_comment(&input.attrs).unwrap_or_default();

    // Parse section-level status (applies to entire struct)
    let section_status = parse_field_status(&input.attrs);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => return quote! { compile_error!("Config only works on structs with named fields"); },
        },
        _ => return quote! { compile_error!("Config only works on structs"); },
    };

    // Collect field info
    let field_infos: Vec<FieldInfo> = fields
        .iter()
        .filter_map(|field| {
            let ident = field.ident.as_ref()?;
            let attrs = &field.attrs;

            let status = parse_field_status(attrs);
            let skip = has_attr(attrs, "skip");

            Some(FieldInfo {
                name: ident.clone(),
                toml_name: get_custom_name(attrs).unwrap_or_else(|| ident.to_string()),
                doc: extract_doc_comment(attrs),
                inline_doc: has_attr(attrs, "inline_doc"),
                status,
                default: get_default_value(attrs),
                skip,
                sub_config: has_attr(attrs, "sub_config"),
                ty: type_to_string(&field.ty),
            })
        })
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

    // Generate template string (skip hidden and skip fields)
    let template_fields: Vec<_> = field_infos.iter()
        .filter(|f| !f.skip && f.status != FieldStatus::Hidden)
        .collect();

    let template_str = generate_template(&template_fields);

    // Collect own fields (non-sub_config, non-skip) for status checks
    let own_fields: Vec<_> = field_infos.iter()
        .filter(|f| !f.skip && !f.sub_config)
        .collect();

    // Check if we need `default` variable (for section or field status checks)
    let has_section_status = matches!(section_status, FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental);
    let has_field_status = field_infos.iter().any(|f|
        !f.skip && !f.sub_config && matches!(f.status, FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental)
    );
    let needs_default = (has_section_status && !own_fields.is_empty()) || has_field_status;

    // Generate field status checks for special status fields
    // Use PartialEq comparison with default value (not IsSet trait)
    let status_checks: Vec<_> = field_infos.iter()
        .filter(|f| !f.skip && !f.sub_config && matches!(f.status, FieldStatus::NotImplemented | FieldStatus::Deprecated | FieldStatus::Experimental))
        .map(|f| {
            let field_name = &f.name;
            let full_path = if section.is_empty() {
                f.toml_name.clone()
            } else {
                format!("{}.{}", section, f.toml_name)
            };
            let status = match f.status {
                FieldStatus::NotImplemented => quote! { crate::config::types::FieldStatus::NotImplemented },
                FieldStatus::Deprecated => quote! { crate::config::types::FieldStatus::Deprecated },
                FieldStatus::Experimental => quote! { crate::config::types::FieldStatus::Experimental },
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

    // Generate recursive calls for nested Config types (explicitly marked with sub_config)
    let nested_calls: Vec<_> = field_infos.iter()
        .filter(|f| !f.skip && f.sub_config)
        .map(|f| {
            let field_name = &f.name;
            quote! {
                self.#field_name.validate_field_status(diag);
            }
        })
        .collect();

    // Generate section-level status check (if struct has experimental/deprecated/not_implemented)
    // Only check own fields (exclude sub_config fields) to avoid duplicate hints with child sections
    let section_status_check = if has_section_status && !own_fields.is_empty() {
        let status_token = match section_status {
            FieldStatus::NotImplemented => quote! { crate::config::types::FieldStatus::NotImplemented },
            FieldStatus::Deprecated => quote! { crate::config::types::FieldStatus::Deprecated },
            FieldStatus::Experimental => quote! { crate::config::types::FieldStatus::Experimental },
            _ => unreachable!(),
        };

        // Generate comparison for each own field
        let field_checks: Vec<_> = own_fields.iter().map(|f| {
            let field_name = &f.name;
            quote! { self.#field_name != default.#field_name }
        }).collect();

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

    // Generate default variable definition if needed
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
            pub fn template() -> &'static str {
                #template_str
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
                out.push_str(Self::template());
                out
            }

            /// Validate field status (experimental, deprecated, not_implemented).
            #[allow(unused_variables)]
            pub fn validate_field_status(&self, diag: &mut crate::config::ConfigDiagnostics) {
                #default_def
                // Check section-level status first
                #section_status_check
                // Check special status fields
                #(#status_checks)*
                // Recurse into nested Config types
                #(#nested_calls)*
            }
        }
    }
}

/// Generate template string for fields.
fn generate_template(fields: &[&FieldInfo]) -> String {
    fields.iter()
        .map(|f| generate_field_template(f))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate TOML template for a single field.
fn generate_field_template(info: &FieldInfo) -> String {
    let mut lines = Vec::new();

    // Check if doc is single line and inline_doc is enabled
    let is_single_line_doc = info.doc.as_ref().map_or(false, |d| !d.contains('\n'));
    let use_inline = info.inline_doc && is_single_line_doc;

    // Doc comment (only if not using inline style)
    if !use_inline {
        if let Some(ref doc) = info.doc {
            for line in doc.lines() {
                lines.push(format!("# {}", line.trim()));
            }
        }
    }

    // Status comment
    let (prefix, status_comment) = match info.status {
        FieldStatus::Normal => ("", None),
        FieldStatus::Experimental => ("# ", Some("# (experimental) this feature may change or be removed")),
        FieldStatus::NotImplemented => ("# ", Some("# (not implemented)")),
        FieldStatus::Deprecated => ("# ", Some("# (deprecated) this option will be removed in a future version")),
        FieldStatus::Hidden => return String::new(),
    };

    if let Some(comment) = status_comment {
        lines.push(comment.to_string());
    }

    // Check if optional type
    let is_optional = info.ty.starts_with("Option<");

    // Field value
    let default = match &info.default {
        Some(v) => format_default_for_type(v, &info.ty),
        None => infer_default(&info.ty),
    };

    if info.sub_config {
        lines.push(format!("# see [{}] section", to_snake_case(&info.toml_name)));
    } else if is_optional && info.default.is_none() {
        // Optional fields without explicit default are commented out
        if use_inline {
            lines.push(format!("# {} = \"\"  # {}", info.toml_name, info.doc.as_ref().unwrap().trim()));
        } else {
            lines.push(format!("# {} = \"\"", info.toml_name));
        }
    } else {
        let field_line = format!("{}{} = {}", prefix, info.toml_name, default);
        if use_inline {
            lines.push(format!("{}  # {}", field_line, info.doc.as_ref().unwrap().trim()));
        } else {
            lines.push(field_line);
        }
    }

    lines.join("\n")
}

// ============================================================================
// Attribute parsing helpers
// ============================================================================

fn get_section(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "section")
}

fn get_custom_name(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "name")
}

fn get_default_value(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "default")
}

fn get_string_attr(attrs: &[Attribute], key: &str) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        let mut value = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(key) {
                let lit: syn::LitStr = meta.value()?.parse()?;
                value = Some(lit.value());
            }
            Ok(())
        });
        if value.is_some() {
            return value;
        }
    }
    None
}

fn has_attr(attrs: &[Attribute], key: &str) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(key) {
                found = true;
            }
            // Skip value if present (e.g., `default = "en"`)
            if meta.input.peek(syn::Token![=]) {
                let _ = meta.value();
                let _: Option<syn::Lit> = meta.input.parse().ok();
            }
            Ok(())
        });
        if found {
            return true;
        }
    }
    false
}

fn parse_field_status(attrs: &[Attribute]) -> FieldStatus {
    for attr in attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        let mut status = FieldStatus::Normal;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("status") {
                // Parse status = experimental (ident, not string)
                let _: syn::Token![=] = meta.input.parse()?;
                let ident: syn::Ident = meta.input.parse()?;
                status = match ident.to_string().as_str() {
                    "experimental" => FieldStatus::Experimental,
                    "not_implemented" => FieldStatus::NotImplemented,
                    "deprecated" => FieldStatus::Deprecated,
                    "hidden" => FieldStatus::Hidden,
                    _ => FieldStatus::Normal,
                };
            } else if meta.input.peek(syn::Token![=]) {
                // Skip other key = value attributes (e.g., section = "xxx")
                let _: syn::Token![=] = meta.input.parse()?;
                // Try to parse as ident first, then as literal
                if meta.input.parse::<syn::Ident>().is_err() {
                    let _ = meta.input.parse::<syn::Lit>();
                }
            }
            Ok(())
        });
        if status != FieldStatus::Normal {
            return status;
        }
    }
    FieldStatus::Normal
}

fn extract_doc_comment(attrs: &[Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &nv.value {
                    if let Lit::Str(s) = &expr_lit.lit {
                        return Some(s.value());
                    }
                }
            }
            None
        })
        .collect();

    if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n").trim().to_string())
    }
}

// ============================================================================
// Type helpers
// ============================================================================

fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

fn infer_section(name: &str) -> String {
    let name = name
        .strip_suffix("SectionConfig")
        .or_else(|| name.strip_suffix("MetaConfig"))
        .or_else(|| name.strip_suffix("Config"))
        .or_else(|| name.strip_suffix("Settings"))
        .unwrap_or(name);
    to_snake_case(name)
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Format default value based on field type.
/// String/PathBuf/enum types get quoted, others are used as-is.
fn format_default_for_type(value: &str, ty: &str) -> String {
    match ty {
        "String" | "PathBuf" => format!("\"{}\"", value),
        // Enum types (not ending with Config/Settings)
        _ if !ty.starts_with("Option<")
            && !ty.starts_with("Vec<")
            && !ty.ends_with("Config")
            && !ty.ends_with("Settings")
            && !matches!(ty, "bool" | "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32" | "f64")
            => format!("\"{}\"", value),
        _ => value.to_string(),
    }
}

fn infer_default(ty: &str) -> String {
    match ty {
        "String" => "\"\"".to_string(),
        "bool" => "false".to_string(),
        "u16" | "u32" | "u64" | "usize" => "0".to_string(),
        "i16" | "i32" | "i64" | "isize" => "0".to_string(),
        "f32" | "f64" => "0.0".to_string(),
        "PathBuf" => "\"\"".to_string(),
        _ if ty.starts_with("Option<") => "# (optional)".to_string(),
        _ if ty.starts_with("Vec<") => "[]".to_string(),
        _ => "\"\"".to_string(),
    }
}
