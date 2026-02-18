//! Field information structures and parsing.

use syn::Type;

use crate::config::attr::{
    extract_doc_comment, get_custom_name, get_default_value, get_inline_doc, has_attr,
    parse_field_status,
};

// Re-export FieldStatus for convenience
pub use crate::config::attr::FieldStatus;

/// Parsed field information.
pub struct FieldInfo {
    pub name: syn::Ident,
    pub toml_name: String,
    pub doc: Option<String>,
    pub inline_doc: Option<String>,
    pub status: FieldStatus,
    pub default: Option<String>,
    pub skip: bool,
    pub sub: bool,
    pub ty: Type,
}

impl FieldInfo {
    /// Parse field info from a syn::Field.
    pub fn from_field(field: &syn::Field) -> Option<Self> {
        let ident = field.ident.as_ref()?;
        let attrs = &field.attrs;

        Some(Self {
            name: ident.clone(),
            toml_name: get_custom_name(attrs).unwrap_or_else(|| ident.to_string()),
            doc: extract_doc_comment(attrs),
            inline_doc: get_inline_doc(attrs),
            status: parse_field_status(attrs),
            default: get_default_value(attrs),
            skip: has_attr(attrs, "skip"),
            sub: has_attr(attrs, "sub"),
            ty: field.ty.clone(),
        })
    }
}
