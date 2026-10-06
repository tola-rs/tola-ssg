//! Parsed configuration fields shared by path, template, and status generation.

use syn::{Type, ext::IdentExt};

use crate::config::attr::{
    CollectionShape, ConfigAttrScope, ConfigAttrs, FieldStatus, extract_doc_comment,
};
use crate::config::field_type::{
    is_collection_type, is_direct_named_config_type, is_vec_type, vec_element_type,
};
use crate::config::serde::parse_serde_field;

/// A configuration field with validated attributes and its canonical TOML key.
pub struct ConfigField {
    pub rust_ident: syn::Ident,
    pub toml_name: String,
    pub doc: Option<String>,
    pub values: Option<syn::Path>,
    pub status: FieldStatus,
    pub default: Option<String>,
    pub skip: bool,
    pub sub: bool,
    pub collection: Option<CollectionShape>,
    pub ty: Type,
}

impl ConfigField {
    /// Parse a configuration field from its Rust declaration.
    pub fn from_field(field: &syn::Field) -> syn::Result<Option<Self>> {
        let Some(ident) = field.ident.as_ref() else {
            return Ok(None);
        };
        let attrs = ConfigAttrs::parse(&field.attrs, ConfigAttrScope::Field)?;
        if attrs.skip && attrs.collection.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                "collection cannot be combined with #[config(skip)]",
            ));
        }
        match attrs.collection {
            Some(CollectionShape::ArrayTable) if !is_vec_type(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires a Vec<T> field",
                ));
            }
            Some(CollectionShape::ArrayTable)
                if vec_element_type(&field.ty)
                    .is_none_or(|element| !is_direct_named_config_type(element)) =>
            {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires Vec<T> where T is a direct named type that derives Config",
                ));
            }
            Some(CollectionShape::ArrayTable) if !attrs.sub => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires a non-skipped #[config(sub)] Vec<T> field",
                ));
            }
            Some(CollectionShape::Inline) if attrs.sub => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = inline cannot be combined with #[config(sub)]",
                ));
            }
            Some(CollectionShape::Inline) if !is_collection_type(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = inline requires a Vec<T> or Option<Vec<T>> field",
                ));
            }
            None if !attrs.skip && is_collection_type(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Vec<T> fields require #[config(collection = inline)] or #[config(collection = array_table)]",
                ));
            }
            _ => {}
        }
        if attrs.sub && attrs.skip {
            return Err(syn::Error::new_spanned(
                ident,
                "#[config(sub)] cannot be combined with #[config(skip)]",
            ));
        }
        if attrs.sub
            && attrs.collection != Some(CollectionShape::ArrayTable)
            && !is_direct_named_config_type(&field.ty)
        {
            return Err(syn::Error::new_spanned(
                ident,
                "#[config(sub)] requires a direct named field type that derives Config",
            ));
        }
        if attrs.sub {
            refuse_section_documentation(field)?;
        }
        let (serde_name, key_span) = parse_serde_field(&field.attrs, attrs.skip, ident)?;
        let toml_name = match (attrs.name, serde_name) {
            (Some(config_name), Some(serde_name)) if config_name != serde_name => {
                return Err(syn::Error::new_spanned(
                    ident,
                    format!("config name {config_name} does not match serde rename {serde_name}"),
                ));
            }
            (Some(config_name), Some(_)) => config_name,
            (Some(_), None) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "config name requires a matching serde rename",
                ));
            }
            (None, Some(serde_name)) => serde_name,
            (None, None) => ident.unraw().to_string(),
        };
        validate_toml_key(&toml_name, key_span)?;

        Ok(Some(Self {
            rust_ident: ident.clone(),
            toml_name,
            doc: extract_doc_comment(&field.attrs),
            status: attrs.status.unwrap_or(FieldStatus::Normal),
            values: attrs.values,
            default: attrs.default,
            skip: attrs.skip,
            sub: attrs.sub,
            collection: attrs.collection,
            ty: field.ty.clone(),
        }))
    }
}

impl ConfigField {
    /// The section whose documentation this key carries, when the key opens one.
    ///
    /// A `#[config(sub)]` field writes a table, or a table per element for an array table, and
    /// that section's own declaration documents the key.
    pub fn section_type(&self) -> Option<&Type> {
        if !self.sub {
            return None;
        }
        match self.collection {
            Some(CollectionShape::ArrayTable) => vec_element_type(&self.ty),
            _ => Some(&self.ty),
        }
    }
}

/// Reject documentation on the key that opens a section.
///
/// The field path is the section's own table, so the section's declaration is the one place its
/// documentation stays true; a comment here would be a second copy to keep in step.
fn refuse_section_documentation(field: &syn::Field) -> syn::Result<()> {
    const REFUSAL: &str = "a #[config(sub)] field takes its documentation from the section type, so document the section there";
    if let Some(attribute) = field
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("doc"))
    {
        return Err(syn::Error::new_spanned(attribute, REFUSAL));
    }
    Ok(())
}

fn validate_toml_key(key: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if !super::is_toml_bare_key(key) {
        return Err(syn::Error::new(
            span,
            "Config field names must be non-empty TOML bare keys containing only ASCII letters, digits, underscores, or hyphens",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ConfigField;
    use syn::parse_quote;

    #[test]
    fn incompatible_field_declarations_are_refused() {
        let fields: Vec<syn::Field> = vec![
            parse_quote! { #[serde(flatten)] settings: Settings },
            parse_quote! { #[serde(rename(serialize = "other"))] value: String },
            parse_quote! { #[serde(rename(deserialize = "other"))] value: String },
            parse_quote! { #[serde(rename(serialize = "one", deserialize = "two"))] value: String },
            parse_quote! { #[serde(rename = "a.b")] value: String },
            parse_quote! { #[serde(rename = "a/b")] value: String },
            parse_quote! { #[serde(rename = "")] value: String },
            parse_quote! { #[config(name = "a/b")] #[serde(rename = "a/b")] value: String },
            parse_quote! { café: String },
            parse_quote! { #[config(collection = array_table)] feeds: Vec<FeedConfig> },
            parse_quote! { sources: Vec<String> },
            parse_quote! { #[config(sub, collection = inline)] sources: Vec<SourceConfig> },
            parse_quote! { #[config(sub)] enabled: bool },
            parse_quote! { #[config(sub)] nested: Option<NestedConfig> },
            parse_quote! { #[config(sub, collection = array_table)] feeds: Vec<String> },
            parse_quote! {
                #[doc = "A parent section description"]
                #[config(sub)]
                nested: NestedConfig
            },
            parse_quote! { #[config(sub, skip)] nested: NestedConfig },
            parse_quote! { #[config(skip, collection = inline)] values: Vec<String> },
        ];
        for field in fields {
            assert!(ConfigField::from_field(&field).is_err());
        }
    }
}
