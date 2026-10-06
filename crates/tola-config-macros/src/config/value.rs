//! TOML literals and optional examples for generated configuration templates.

use syn::Type;

use crate::config::field_type::{
    is_float_type, is_integer_type, option_inner_type, path_type_ident,
};

/// Choose a commented TOML example from the optional field's inner type.
pub fn option_placeholder(ty: &Type) -> &'static str {
    let Some(inner) = option_inner_type(ty) else {
        return "\"\"";
    };
    match path_type_ident(inner) {
        Some(name) if name == "Vec" => "[]",
        Some(name) if name == "HashMap" || name == "BTreeMap" => "{}",
        Some(name) if name == "bool" => "false",
        Some(name) if is_integer_type(name) || is_float_type(name) => "0",
        _ => "\"\"",
    }
}

/// Format an explicit scalar config default as valid TOML.
///
/// Attributes contain unescaped values such as `en` or `tailwind@4`.
/// Strings and enums become TOML basic strings; numeric and boolean literals are
/// checked before expansion. Composite defaults use the runtime TOML serializer.
pub fn format_scalar_default_for_type(value: &str, ty: &Type) -> syn::Result<String> {
    let value = value.trim();
    match path_type_ident(ty) {
        Some(name) if name == "bool" => {
            if matches!(value, "true" | "false") {
                Ok(value.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "boolean config defaults must be true or false",
                ))
            }
        }
        Some(name) if is_integer_type(name) => {
            if matches!(toml_literal(value), Some(toml::de::DeValue::Integer(_))) {
                Ok(value.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "integer config defaults must be valid TOML integer literals",
                ))
            }
        }
        Some(name) if is_float_type(name) => {
            if matches!(toml_literal(value), Some(toml::de::DeValue::Float(_))) {
                Ok(value.to_owned())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    "floating-point config defaults must be valid TOML float literals",
                ))
            }
        }
        Some(name)
            if name == "Option" || name == "Vec" || name == "HashMap" || name == "BTreeMap" =>
        {
            Err(syn::Error::new_spanned(
                ty,
                "#[config(default = ...)] requires a scalar field; define composite defaults with Default",
            ))
        }
        _ => Ok(quote_toml_string(value)),
    }
}

fn toml_literal(value: &str) -> Option<toml::de::DeValue<'_>> {
    toml::de::DeValue::parse(value)
        .ok()
        .map(|parsed| parsed.into_inner())
}

fn quote_toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{08}' => quoted.push_str("\\b"),
            '\u{0c}' => quoted.push_str("\\f"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(quoted, "\\u{:04X}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => quoted.push(character),
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
    fn optional_placeholders_follow_type() {
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
    fn defaults_outside_their_scalar_category_are_refused() {
        for (value, ty) in [
            ("maybe", parse_quote!(bool)),
            ("1__2", parse_quote!(u16)),
            ("01", parse_quote!(u16)),
            ("1_.0", parse_quote!(f64)),
            ("1.0", parse_quote!(u16)),
            ("1", parse_quote!(f64)),
            ("1\nother = 2", parse_quote!(u16)),
        ] {
            assert!(
                format_scalar_default_for_type(value, &ty).is_err(),
                "{value}"
            );
        }
    }
}
