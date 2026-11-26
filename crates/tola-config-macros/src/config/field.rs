//! Parsed configuration fields shared by path, template, and status generation.

use syn::{Type, ext::IdentExt};

use crate::config::attr::{
    CollectionShape, ConfigAttrScope, ConfigAttrs, FieldStatus, extract_doc_comment,
};
use crate::config::serde::parse_serde_field;
use crate::config::ty::{is_collection, is_nested_config, is_vec, vec_elem};

/// Validated configuration field with its canonical TOML key.
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
    pub fragment_with: Option<syn::Path>,
    pub deserialize_skip: bool,
}

impl ConfigField {
    pub fn from_field(field: &syn::Field) -> syn::Result<Self> {
        Self::parse(field, false)
    }

    pub fn from_fragment_field(field: &syn::Field) -> syn::Result<Self> {
        Self::parse(field, true)
    }

    fn parse(field: &syn::Field, fragment: bool) -> syn::Result<Self> {
        let ident = field
            .ident
            .as_ref()
            .expect("a named field has an identifier");
        let attr = ConfigAttrs::parse(&field.attrs, ConfigAttrScope::Field)?;
        if attr.skip && attr.collection.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                "collection cannot be combined with #[config(skip)]",
            ));
        }
        match attr.collection {
            Some(CollectionShape::ArrayTable) if !is_vec(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires a Vec<T> field",
                ));
            }
            Some(CollectionShape::ArrayTable)
                if vec_elem(&field.ty).is_none_or(|el| !is_nested_config(el)) =>
            {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires Vec<T> where T is a direct named type that derives Config",
                ));
            }
            Some(CollectionShape::ArrayTable) if !attr.sub => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = array_table requires a non-skipped #[config(sub)] Vec<T> field",
                ));
            }
            Some(CollectionShape::Inline) if attr.sub => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = inline cannot be combined with #[config(sub)]",
                ));
            }
            Some(CollectionShape::Inline) if !is_collection(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "collection = inline requires a Vec<T> or Option<Vec<T>> field",
                ));
            }
            None if !fragment && !attr.skip && is_collection(&field.ty) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Vec<T> fields require #[config(collection = inline)] or #[config(collection = array_table)]",
                ));
            }
            _ => {}
        }
        if attr.sub && attr.skip {
            return Err(syn::Error::new_spanned(
                ident,
                "#[config(sub)] cannot be combined with #[config(skip)]",
            ));
        }
        if attr.sub
            && attr.collection != Some(CollectionShape::ArrayTable)
            && !is_nested_config(&field.ty)
        {
            return Err(syn::Error::new_spanned(
                ident,
                "#[config(sub)] requires a direct named field type that derives Config",
            ));
        }
        if attr.sub {
            refuse_section_doc(field)?;
        }
        let deserialize_skip =
            super::serde::fragment_field(&field.attrs, attr.fragment_with.is_some())?;
        let (serde_name, key_span) = parse_serde_field(&field.attrs, attr.skip || fragment, ident)?;
        let toml_name = match (attr.name, serde_name) {
            (Some(cfg_name), Some(serde_name)) if cfg_name != serde_name => {
                return Err(syn::Error::new_spanned(
                    ident,
                    format!("config name {cfg_name} does not match serde rename {serde_name}"),
                ));
            }
            (Some(cfg_name), Some(_)) => cfg_name,
            (Some(_), None) => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "config name requires a matching serde rename",
                ));
            }
            (None, Some(serde_name)) => serde_name,
            (None, None) => ident.unraw().to_string(),
        };
        if !fragment {
            validate_toml_key(&toml_name, key_span)?;
        }

        Ok(Self {
            rust_ident: ident.clone(),
            toml_name,
            doc: extract_doc_comment(&field.attrs),
            status: attr.status.unwrap_or(FieldStatus::Normal),
            values: attr.values,
            default: attr.default,
            skip: attr.skip,
            sub: attr.sub,
            collection: attr.collection,
            ty: field.ty.clone(),
            fragment_with: attr.fragment_with,
            deserialize_skip,
        })
    }
}

impl ConfigField {
    pub fn section_type(&self) -> Option<&Type> {
        if !self.sub {
            return None;
        }
        match self.collection {
            Some(CollectionShape::ArrayTable) => vec_elem(&self.ty),
            _ => Some(&self.ty),
        }
    }
}

fn refuse_section_doc(field: &syn::Field) -> syn::Result<()> {
    const MSG: &str = "a #[config(sub)] field takes its documentation from the section type, so document the section there";
    if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("doc")) {
        return Err(syn::Error::new_spanned(attr, MSG));
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
    fn invalid_fields_refused() {
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
