//! Syntactic field type inspection shared by validation and template defaults.

use syn::{GenericArgument, Ident, PathArguments, PathSegment, Type};

fn path_seg(ty: &Type) -> Option<&PathSegment> {
    let Type::Path(tp) = ty else {
        return None;
    };
    tp.path.segments.last()
}

fn elem_of<'a>(ty: &'a Type, container: &str) -> Option<&'a Type> {
    let seg = path_seg(ty)?;
    if seg.ident != container {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let GenericArgument::Type(elem) = args.args.first()? else {
        return None;
    };
    Some(elem)
}

pub(super) fn is_vec(ty: &Type) -> bool {
    path_ident(ty).is_some_and(|id| id == "Vec")
}

pub(super) fn vec_elem(ty: &Type) -> Option<&Type> {
    elem_of(ty, "Vec")
}

/// Check nested-section type syntax; Rust checks `Config` impl after expansion.
pub(super) fn is_nested_config(ty: &Type) -> bool {
    path_ident(ty).is_some_and(|id| {
        !is_int(id)
            && !is_float(id)
            && !["Option", "Vec", "String", "bool", "char"]
                .iter()
                .any(|excl| id == excl)
    })
}

pub(super) fn is_option(ty: &Type) -> bool {
    path_ident(ty).is_some_and(|id| id == "Option")
}

pub(super) fn path_ident(ty: &Type) -> Option<&Ident> {
    path_seg(ty).map(|seg| &seg.ident)
}

pub(super) fn option_inner(ty: &Type) -> Option<&Type> {
    elem_of(ty, "Option")
}

pub(super) fn is_int(id: &Ident) -> bool {
    [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
    ]
    .iter()
    .any(|int| id == int)
}

pub(super) fn is_float(id: &Ident) -> bool {
    id == "f32" || id == "f64"
}

pub(super) fn is_collection(ty: &Type) -> bool {
    is_vec(ty) || option_inner(ty).is_some_and(is_vec)
}
