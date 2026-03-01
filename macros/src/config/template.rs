//! Template generation code for Config derive macro.

use proc_macro2::TokenStream;
use quote::quote;

use crate::config::field::{FieldInfo, FieldStatus};
use crate::config::types::{format_default_for_type, type_to_string};

/// Generate template code (TokenStream) for fields
pub fn generate_template_code(fields: &[&FieldInfo]) -> TokenStream {
    let field_codes: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_field_template_code(f))
        .collect();

    quote! {
        #(#field_codes)*
    }
}

/// Generate TOML template code for a single field
fn generate_field_template_code(info: &FieldInfo) -> TokenStream {
    let field_name = &info.name;
    let toml_name = &info.toml_name;

    // Check if inline_doc is specified
    let has_inline = info.inline_doc.is_some();

    // Doc comment code (always output if present)
    let doc_code = if let Some(ref doc) = info.doc {
        let doc_lines: Vec<_> = doc.lines().map(|l| format!("# {}\n", l.trim())).collect();
        let doc_str = doc_lines.join("");
        quote! { out.push_str(#doc_str); }
    } else {
        quote! {}
    };

    // Status handling
    let (is_commented, status_comment) = match info.status {
        FieldStatus::Normal => (false, None),
        FieldStatus::Experimental => (
            true,
            Some("# (experimental) this feature may change or be removed\n"),
        ),
        FieldStatus::NotImplemented => (true, Some("# (not implemented)\n")),
        FieldStatus::Deprecated => (
            true,
            Some("# (deprecated) this option will be removed in a future version\n"),
        ),
        FieldStatus::Hidden => return quote! {},
    };

    let status_code = if let Some(comment) = status_comment {
        quote! { out.push_str(#comment); }
    } else {
        quote! {}
    };

    // Check if optional type
    let ty_str = type_to_string(&info.ty);
    let is_optional = ty_str.starts_with("Option<");

    // For sub fields - output the sub config's template_with_header
    if info.sub {
        let field_ty = &info.ty;
        return quote! {
            out.push('\n');
            #doc_code
            out.push_str(&<#field_ty>::template_with_header());
        };
    }

    // For optional fields without explicit default - comment out
    if is_optional && info.default.is_none() {
        if has_inline {
            let inline_comment = info.inline_doc.as_ref().unwrap();
            let line = format!("# {} = \"\"  # {}\n", toml_name, inline_comment);
            return quote! {
                #doc_code
                out.push_str(#line);
            };
        } else {
            let line = format!("# {} = \"\"\n", toml_name);
            return quote! {
                #doc_code
                out.push_str(#line);
            };
        }
    }

    // For fields with explicit default value (compile-time known)
    if let Some(ref default_val) = info.default {
        let formatted = format_default_for_type(default_val, &ty_str);
        let prefix = if is_commented { "# " } else { "" };

        if has_inline {
            let inline_comment = info.inline_doc.as_ref().unwrap();
            let line = format!(
                "{}{} = {}  # {}\n",
                prefix, toml_name, formatted, inline_comment
            );
            return quote! {
                #doc_code
                #status_code
                out.push_str(#line);
            };
        } else {
            let line = format!("{}{} = {}\n", prefix, toml_name, formatted);
            return quote! {
                #doc_code
                #status_code
                out.push_str(#line);
            };
        }
    }

    // For fields using Default::default() - runtime value
    let prefix = if is_commented { "# " } else { "" };

    if has_inline {
        let inline_comment = info.inline_doc.as_ref().unwrap();
        quote! {
            #doc_code
            #status_code
            out.push_str(#prefix);
            out.push_str(#toml_name);
            out.push_str(" = ");
            out.push_str(&toml::Value::try_from(default.#field_name.clone())
                .map(|v| v.to_string())
                .unwrap_or_default());
            out.push_str("  # ");
            out.push_str(#inline_comment);
            out.push('\n');
        }
    } else {
        quote! {
            #doc_code
            #status_code
            out.push_str(#prefix);
            out.push_str(#toml_name);
            out.push_str(" = ");
            out.push_str(&toml::Value::try_from(default.#field_name.clone())
                .map(|v| v.to_string())
                .unwrap_or_default());
            out.push('\n');
        }
    }
}
