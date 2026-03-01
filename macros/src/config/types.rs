//! Type helper functions for Config derive macro.

use quote::quote;
use syn::Type;

/// Convert syn::Type to string representation
pub fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

/// Infer section name from struct name
pub fn infer_section(name: &str) -> String {
    let name = name
        .strip_suffix("SectionConfig")
        .or_else(|| name.strip_suffix("MetaConfig"))
        .or_else(|| name.strip_suffix("Config"))
        .or_else(|| name.strip_suffix("Settings"))
        .unwrap_or(name);
    to_snake_case(name)
}

/// Convert PascalCase to snake_case
pub fn to_snake_case(s: &str) -> String {
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

/// Format default value based on field type
/// String/PathBuf/enum types get quoted, others are used as-is
pub fn format_default_for_type(value: &str, ty: &str) -> String {
    match ty {
        "String" | "PathBuf" => format!("\"{}\"", value),
        // Enum types (not ending with Config/Settings)
        _ if !ty.starts_with("Option<")
            && !ty.starts_with("Vec<")
            && !ty.ends_with("Config")
            && !ty.ends_with("Settings")
            && !matches!(
                ty,
                "bool"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "usize"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "isize"
                    | "f32"
                    | "f64"
            ) =>
        {
            format!("\"{}\"", value)
        }
        _ => value.to_string(),
    }
}
