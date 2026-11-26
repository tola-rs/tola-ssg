//! TOML literals and optional examples for generated configuration templates.

use syn::Type;

use crate::config::ty::{is_float, is_int, option_inner, path_ident};

pub fn option_placeholder(ty: &Type) -> &'static str {
    let Some(inner) = option_inner(ty) else {
        return "\"\"";
    };
    match path_ident(inner) {
        Some(id) if id == "Vec" => "[]",
        Some(id) if id == "HashMap" || id == "BTreeMap" => "{}",
        Some(id) if id == "bool" => "false",
        Some(id) if is_int(id) || is_float(id) => "0",
        _ => "\"\"",
    }
}

/// Format an explicit scalar default as valid TOML.
pub fn format_scalar_default_for_type(val: &str, ty: &Type) -> syn::Result<String> {
    let val = val.trim();
    match path_ident(ty) {
        Some(id) if id == "bool" => {
            if matches!(val, "true" | "false") {
                Ok(val.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "boolean config defaults must be true or false",
                ))
            }
        }
        Some(id) if is_int(id) => {
            if matches!(toml_literal(val), Some(toml::de::DeValue::Integer(_))) {
                Ok(val.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "integer config defaults must be valid TOML integer literals",
                ))
            }
        }
        Some(id) if is_float(id) => {
            if matches!(toml_literal(val), Some(toml::de::DeValue::Float(_))) {
                Ok(val.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "floating-point config defaults must be valid TOML float literals",
                ))
            }
        }
        Some(id) if id == "Option" || id == "Vec" || id == "HashMap" || id == "BTreeMap" => {
            Err(syn::Error::new_spanned(
                ty,
                "#[config(default = ...)] requires a scalar field; define composite defaults with Default",
            ))
        }
        _ => Ok(quote_toml_string(val)),
    }
}

fn toml_literal(val: &str) -> Option<toml::de::DeValue<'_>> {
    toml::de::DeValue::parse(val)
        .ok()
        .map(|parsed| parsed.into_inner())
}

fn quote_toml_string(val: &str) -> String {
    let mut quoted = String::with_capacity(val.len() + 2);
    quoted.push('"');
    for ch in val.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{08}' => quoted.push_str("\\b"),
            '\u{0c}' => quoted.push_str("\\f"),
            ch if ch.is_control() => {
                use std::fmt::Write;
                write!(quoted, "\\u{:04X}", ch as u32).expect("writing to a String cannot fail");
            }
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{format_scalar_default_for_type, option_placeholder};
    use syn::parse_quote;

    #[test]
    fn option_placeholders() {
        assert_eq!(option_placeholder(&parse_quote!(Option<Vec<String>>)), "[]");
        assert_eq!(
            option_placeholder(&parse_quote!(
                Option<std::collections::HashMap<String, String>>
            )),
            "{}"
        );
        assert_eq!(option_placeholder(&parse_quote!(Option<bool>)), "false");
        assert_eq!(option_placeholder(&parse_quote!(Option<String>)), "\"\"");
    }

    #[test]
    fn invalid_scalar_defaults() {
        for (val, ty) in [
            ("maybe", parse_quote!(bool)),
            ("1__2", parse_quote!(u16)),
            ("01", parse_quote!(u16)),
            ("1_.0", parse_quote!(f64)),
            ("1.0", parse_quote!(u16)),
            ("1", parse_quote!(f64)),
            ("1\nother = 2", parse_quote!(u16)),
        ] {
            assert!(format_scalar_default_for_type(val, &ty).is_err(), "{val}");
        }
    }
}
