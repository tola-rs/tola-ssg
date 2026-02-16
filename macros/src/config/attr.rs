//! Attribute parsing helpers for Config derive macro.

use syn::{Attribute, Lit, Meta};

/// Field status for template generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStatus {
    Normal,
    Experimental,
    NotImplemented,
    Deprecated,
    Hidden,
}

/// Get section name from #[config(section = "xxx")].
pub fn get_section(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "section")
}

/// Get custom field name from #[config(name = "xxx")].
pub fn get_custom_name(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "name")
}

/// Get default value from #[config(default = "xxx")].
pub fn get_default_value(attrs: &[Attribute]) -> Option<String> {
    get_string_attr(attrs, "default")
}

/// Get string value from #[config(key = "value")].
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

/// Check if attribute has a flag like #[config(skip)].
pub fn has_attr(attrs: &[Attribute], key: &str) -> bool {
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

/// Parse field status from #[config(status = experimental)].
pub fn parse_field_status(attrs: &[Attribute]) -> FieldStatus {
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

/// Extract doc comment from #[doc = "..."] attributes.
pub fn extract_doc_comment(attrs: &[Attribute]) -> Option<String> {
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
