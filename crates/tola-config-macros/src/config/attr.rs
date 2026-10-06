//! Parse Config attributes for field paths, templates, and status validation.

use syn::{Attribute, Lit, Meta};

/// Field status used by template generation and status diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStatus {
    Normal,
    Experimental,
    NotImplemented,
    Deprecated,
    Hidden,
}

/// TOML representation for a collection-valued config field or section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionShape {
    Inline,
    ArrayTable,
}

/// The attributes accepted on a config struct or field.
#[derive(Clone, Default)]
pub struct ConfigAttrs {
    pub section: Option<String>,
    /// Runtime crate path used by generated configuration support code.
    /// Defaults to `::tola_config`; a renamed dependency can supply its own path.
    pub crate_path: Option<syn::Path>,
    pub name: Option<String>,
    pub default: Option<String>,
    pub values: Option<syn::Path>,
    pub status: Option<FieldStatus>,
    pub skip: bool,
    pub sub: bool,
    pub collection: Option<CollectionShape>,
}

/// Attribute location, used to reject options on the wrong declaration kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAttrScope {
    Struct,
    Field,
}

impl ConfigAttrs {
    pub fn parse(attrs: &[Attribute], scope: ConfigAttrScope) -> syn::Result<Self> {
        let mut parsed = Self::default();
        let mut seen_tola_config = false;
        let mut seen_status = false;
        let mut seen_hidden = false;

        for attr in attrs {
            let is_config = attr.path().is_ident("config");
            let is_tola_config = attr.path().is_ident("tola_config");
            if !is_config && !is_tola_config {
                continue;
            }
            if is_tola_config {
                if scope != ConfigAttrScope::Struct {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "tola_config is only valid on a config struct",
                    ));
                }
                if seen_tola_config {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "duplicate tola_config attribute",
                    ));
                }
                if parsed.crate_path.is_some() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "tola_config cannot be combined with config(crate = ... )",
                    ));
                }
                seen_tola_config = true;
                parsed.crate_path = Some(syn::parse_quote!(tola_config));
                if matches!(attr.meta, Meta::Path(_)) {
                    continue;
                }
            }

            attr.parse_nested_meta(|meta| {
                let key = meta
                    .path
                    .get_ident()
                    .ok_or_else(|| meta.error("config attribute keys must be identifiers"))?;

                match key.to_string().as_str() {
                    "section" => {
                        require_scope(scope, ConfigAttrScope::Struct, key)?;
                        ensure_not_seen(parsed.section.is_some(), key)?;
                        let section = parse_string_value(&meta, key)?;
                        validate_section_path(&section, key.span())?;
                        parsed.section = Some(section);
                    }
                    "crate" => {
                        if is_tola_config {
                            return Err(meta.error("tola_config selects the tola_config runtime crate and does not accept crate = ..."));
                        }
                        require_scope(scope, ConfigAttrScope::Struct, key)?;
                        if parsed.crate_path.is_some() {
                            if seen_tola_config {
                                return Err(meta.error("tola_config cannot be combined with config(crate = ... )"));
                            }
                            return Err(meta.error("duplicate crate config attribute"));
                        }
                        parsed.crate_path = Some(meta.value()?.parse()?);
                    }
                    "name" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_not_seen(parsed.name.is_some(), key)?;
                        parsed.name = Some(parse_string_value(&meta, key)?);
                    }
                    "default" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_not_seen(parsed.default.is_some(), key)?;
                        parsed.default = Some(parse_string_value(&meta, key)?);
                    }
                    "values" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_not_seen(parsed.values.is_some(), key)?;
                        parsed.values = Some(meta.value()?.parse()?);
                    }
                    "status" => {
                        ensure_not_seen(seen_status, key)?;
                        seen_status = true;
                        if seen_hidden {
                            return Err(meta.error(
                                "status cannot be combined with the hidden flag",
                            ));
                        }
                        let status = parse_status_value(&meta)?;
                        if scope == ConfigAttrScope::Struct && status == FieldStatus::Hidden {
                            return Err(meta.error("hidden status is only valid on a config field"));
                        }
                        parsed.status = Some(status);
                    }
                    "skip" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_flag(&meta, key)?;
                        ensure_not_seen(parsed.skip, key)?;
                        parsed.skip = true;
                    }
                    "sub" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_flag(&meta, key)?;
                        ensure_not_seen(parsed.sub, key)?;
                        parsed.sub = true;
                    }
                    "collection" => {
                        ensure_not_seen(parsed.collection.is_some(), key)?;
                        let collection = parse_collection_shape(&meta)?;
                        if scope == ConfigAttrScope::Struct
                            && collection != CollectionShape::ArrayTable
                        {
                            return Err(meta.error(
                                "config struct sections only support collection = array_table",
                            ));
                        }
                        parsed.collection = Some(collection);
                    }
                    "hidden" => {
                        require_scope(scope, ConfigAttrScope::Field, key)?;
                        ensure_flag(&meta, key)?;
                        if seen_status {
                            return Err(meta.error(
                                "the hidden flag cannot be combined with status",
                            ));
                        }
                        if seen_hidden {
                            return Err(meta.error("duplicate hidden config attribute"));
                        }
                        seen_hidden = true;
                        parsed.status = Some(FieldStatus::Hidden);
                    }
                    _ => {
                        return Err(meta.error(format!(
                            "unknown config attribute {key}; expected section, crate, name, default, values, status, skip, sub, collection, or hidden"
                        )));
                    }
                }

                Ok(())
            })?;
        }

        Ok(parsed)
    }
}

/// Infer a section name from a config type name.
pub fn infer_section(name: &str) -> String {
    let name = name
        .strip_suffix("SectionConfig")
        .or_else(|| name.strip_suffix("MetaConfig"))
        .or_else(|| name.strip_suffix("Config"))
        .or_else(|| name.strip_suffix("Settings"))
        .unwrap_or(name);
    to_snake_case(name)
}

/// Convert PascalCase to snake_case.
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

fn require_scope(
    actual: ConfigAttrScope,
    expected: ConfigAttrScope,
    key: &syn::Ident,
) -> syn::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        let expected_name = match expected {
            ConfigAttrScope::Struct => "struct",
            ConfigAttrScope::Field => "field",
        };
        Err(syn::Error::new_spanned(
            key,
            format!("{key} is only valid on a config {expected_name}"),
        ))
    }
}

fn ensure_not_seen(seen: bool, key: &syn::Ident) -> syn::Result<()> {
    if seen {
        Err(syn::Error::new_spanned(
            key,
            format!("duplicate {key} config attribute"),
        ))
    } else {
        Ok(())
    }
}

fn ensure_flag(meta: &syn::meta::ParseNestedMeta<'_>, key: &syn::Ident) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        return Err(meta.error(format!("{key} is a flag and does not take a value")));
    }
    Ok(())
}

fn parse_string_value(
    meta: &syn::meta::ParseNestedMeta<'_>,
    key: &syn::Ident,
) -> syn::Result<String> {
    let value = meta.value()?;
    let literal: syn::LitStr = value.parse().map_err(|error| {
        syn::Error::new(error.span(), format!("{key} expects a string literal"))
    })?;
    Ok(literal.value())
}

pub(super) fn validate_section_path(section: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if section.is_empty() {
        return Ok(());
    }
    if section
        .split('.')
        .any(|part| !super::is_toml_bare_key(part))
    {
        return Err(syn::Error::new(
            span,
            "Config sections must contain non-empty dot-separated TOML bare keys containing only ASCII letters, digits, underscores, or hyphens",
        ));
    }
    Ok(())
}

fn parse_status_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<FieldStatus> {
    let name = parse_choice_name(
        meta,
        "status expects experimental, not_implemented, deprecated, or hidden",
    )?;

    match name.as_str() {
        "experimental" => Ok(FieldStatus::Experimental),
        "not_implemented" => Ok(FieldStatus::NotImplemented),
        "deprecated" => Ok(FieldStatus::Deprecated),
        "hidden" => Ok(FieldStatus::Hidden),
        _ => Err(meta.error(format!(
            "unknown config status {name}; expected experimental, not_implemented, deprecated, or hidden"
        ))),
    }
}

fn parse_collection_shape(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<CollectionShape> {
    let name = parse_choice_name(meta, "collection expects inline or array_table")?;

    match name.as_str() {
        "inline" => Ok(CollectionShape::Inline),
        "array_table" => Ok(CollectionShape::ArrayTable),
        _ => Err(meta.error(format!(
            "unknown collection shape {name}; expected inline or array_table"
        ))),
    }
}

/// Status and collection choices accept either an identifier or a string.
fn parse_choice_name(meta: &syn::meta::ParseNestedMeta<'_>, expected: &str) -> syn::Result<String> {
    let value = meta.value()?;
    if value.peek(syn::Ident) {
        Ok(value.parse::<syn::Ident>()?.to_string())
    } else if value.peek(syn::LitStr) {
        Ok(value.parse::<syn::LitStr>()?.value())
    } else {
        Err(meta.error(expected))
    }
}

/// Extract doc comment from #[doc = "..."] attributes.
pub fn extract_doc_comment(attrs: &[Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(expr_lit) = &nv.value
                && let Lit::Str(s) = &expr_lit.lit
            {
                return Some(s.value());
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

#[cfg(test)]
mod tests {
    use super::{CollectionShape, ConfigAttrScope, ConfigAttrs, FieldStatus};
    use quote::ToTokens;
    use syn::parse_quote;

    #[test]
    fn section_names_come_from_type_names() {
        for (name, expected) in [
            ("BuildSectionConfig", "build"),
            ("SiteMetaConfig", "site"),
            ("CacheConfig", "cache"),
            ("EditorSettings", "editor"),
            ("PackageStore", "package_store"),
        ] {
            assert_eq!(super::infer_section(name), expected);
        }
    }

    /// Each accepted spelling reaches the parsed attribute field it names.
    #[test]
    fn accepted_attributes_parse() {
        let attrs = vec![parse_quote!(#[config(
            name = "title",
            default = "hello",
            status = "experimental",
            sub
        )])];
        let parsed = ConfigAttrs::parse(&attrs, ConfigAttrScope::Field).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("title"));
        assert_eq!(parsed.default.as_deref(), Some("hello"));
        assert_eq!(parsed.status, Some(FieldStatus::Experimental));
        assert!(parsed.sub);

        let attrs = vec![parse_quote!(#[tola_config(section = "site")])];
        let parsed = ConfigAttrs::parse(&attrs, ConfigAttrScope::Struct).unwrap();
        assert_eq!(parsed.section.as_deref(), Some("site"));
        assert_eq!(
            parsed.crate_path.unwrap().to_token_stream().to_string(),
            "tola_config"
        );

        let struct_attrs = vec![parse_quote!(#[config(collection = array_table)])];
        assert!(
            ConfigAttrs::parse(&struct_attrs, ConfigAttrScope::Struct)
                .unwrap()
                .collection
                == Some(CollectionShape::ArrayTable)
        );

        let field_attrs = vec![parse_quote!(#[config(collection = "inline")])];
        assert!(
            ConfigAttrs::parse(&field_attrs, ConfigAttrScope::Field)
                .unwrap()
                .collection
                == Some(CollectionShape::Inline)
        );
    }

    #[test]
    fn incompatible_attributes_are_refused() {
        for (attrs, scope) in [
            (
                vec![parse_quote!(#[config(collection = inline)])],
                ConfigAttrScope::Struct,
            ),
            (
                vec![
                    parse_quote!(#[tola_config]),
                    parse_quote!(#[config(crate = other_config)]),
                ],
                ConfigAttrScope::Struct,
            ),
            (
                vec![parse_quote!(#[config(stauts = experimental)])],
                ConfigAttrScope::Field,
            ),
            (
                vec![parse_quote!(#[config(status = someday)])],
                ConfigAttrScope::Struct,
            ),
            (vec![parse_quote!(#[config(sub)])], ConfigAttrScope::Struct),
            (
                vec![
                    parse_quote!(#[config(name = "one")]),
                    parse_quote!(#[config(name = "two")]),
                ],
                ConfigAttrScope::Field,
            ),
        ] {
            assert!(ConfigAttrs::parse(&attrs, scope).is_err());
        }
    }

    #[test]
    fn sections_require_bare_key_components() {
        for section in ["", "site.child", "site-name.child_1", "123"] {
            assert!(super::validate_section_path(section, proc_macro2::Span::call_site()).is_ok());
        }
        for section in [
            "site!",
            "site..child",
            ".site",
            "site.",
            "site child",
            "站点",
            "\"site\"",
        ] {
            let literal = syn::LitStr::new(section, proc_macro2::Span::call_site());
            let attrs = vec![parse_quote!(#[config(section = #literal)])];
            assert!(ConfigAttrs::parse(&attrs, ConfigAttrScope::Struct).is_err());
        }
    }
}
