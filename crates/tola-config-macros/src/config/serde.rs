//! Resolve explicit serde names without changing configuration key semantics.

use syn::{Attribute, Expr, Lit, Meta, Token, ext::IdentExt, punctuated::Punctuated};

/// Reject `serde(rename_all)`; Config requires explicit `serde(rename = ...)`
/// so field paths and template keys match deserialization.
pub(super) fn reject_serde_rename_all(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        for meta in serde_metadata(attr)? {
            if meta.path().is_ident("rename_all") {
                return Err(syn::Error::new_spanned(
                    meta,
                    "Config derives require explicit #[serde(rename = ...)] keys; rename_all is not supported",
                ));
            }
        }
    }
    Ok(())
}

/// Parse the outer metadata once, including unrelated nested serde attributes.
fn serde_metadata(attr: &Attribute) -> syn::Result<Punctuated<Meta, Token![,]>> {
    attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
}

/// Resolve both serde directions against the Rust name before selecting a key.
pub(super) fn parse_serde_field(
    attrs: &[Attribute],
    config_skip: bool,
    ident: &syn::Ident,
) -> syn::Result<(Option<String>, proc_macro2::Span)> {
    let mut serialize = None;
    let mut deserialize = None;
    let mut rename_span = ident.span();

    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        for meta in serde_metadata(attr)? {
            let path = meta.path();
            let unsupported = path.is_ident("flatten")
                || path.is_ident("skip")
                || path.is_ident("skip_serializing")
                || path.is_ident("skip_deserializing");
            if unsupported && !config_skip {
                return Err(syn::Error::new_spanned(
                    meta,
                    "serde flatten/skip fields require #[config(skip)] to omit them from generated configuration",
                ));
            }
            if !path.is_ident("rename") {
                continue;
            }
            use syn::spanned::Spanned;
            rename_span = meta.span();
            match &meta {
                Meta::NameValue(value) => {
                    let name = parse_rename_literal(&value.value, &value.path)?;
                    set_rename(&mut serialize, name.clone(), &meta)?;
                    set_rename(&mut deserialize, name, &meta)?;
                }
                Meta::List(list) => {
                    let directions =
                        list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
                    for direction in directions {
                        let Meta::NameValue(value) = direction else {
                            return Err(syn::Error::new_spanned(
                                direction,
                                "serde rename directions must use serialize = or deserialize =",
                            ));
                        };
                        let target = if value.path.is_ident("serialize") {
                            &mut serialize
                        } else if value.path.is_ident("deserialize") {
                            &mut deserialize
                        } else {
                            return Err(syn::Error::new_spanned(
                                value.path,
                                "serde rename direction must be serialize or deserialize",
                            ));
                        };
                        set_rename(
                            target,
                            parse_rename_literal(&value.value, &value.path)?,
                            &value,
                        )?;
                    }
                }
                Meta::Path(_) => {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "serde rename requires a value",
                    ));
                }
            }
        }
    }

    let rust_name = ident.unraw().to_string();
    let serialize_name = serialize.as_deref().unwrap_or(&rust_name);
    let deserialize_name = deserialize.as_deref().unwrap_or(&rust_name);
    if serialize_name != deserialize_name {
        return Err(syn::Error::new(
            rename_span,
            "serde serialize and deserialize names must match for Config, including an omitted direction's Rust field name",
        ));
    }
    Ok((serialize.or(deserialize), rename_span))
}

fn parse_rename_literal<T: quote::ToTokens>(expr: &Expr, span: &T) -> syn::Result<String> {
    if let Expr::Lit(expr_lit) = expr
        && let Lit::Str(value) = &expr_lit.lit
    {
        return Ok(value.value());
    }
    Err(syn::Error::new_spanned(
        span,
        "serde rename must be a string literal",
    ))
}

fn set_rename<T: quote::ToTokens>(
    target: &mut Option<String>,
    value: String,
    span: &T,
) -> syn::Result<()> {
    if target.is_some() {
        return Err(syn::Error::new_spanned(span, "duplicate serde rename"));
    }
    *target = Some(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::reject_serde_rename_all;
    use syn::parse_quote;

    #[test]
    fn rename_all_is_rejected() {
        let attrs = vec![parse_quote!(#[serde(rename_all = "kebab-case")])];
        assert!(reject_serde_rename_all(&attrs).is_err());
    }
}
