//! Syntactic field type inspection shared by validation and template defaults.

use syn::{GenericArgument, Ident, PathArguments, PathSegment, Type};

fn path_type_segment(ty: &Type) -> Option<&PathSegment> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    type_path.path.segments.last()
}

fn container_element_type<'ty>(ty: &'ty Type, container: &str) -> Option<&'ty Type> {
    let segment = path_type_segment(ty)?;
    if segment.ident != container {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(element) = arguments.args.first()? else {
        return None;
    };
    Some(element)
}

pub(super) fn is_vec_type(ty: &Type) -> bool {
    path_type_ident(ty).is_some_and(|name| name == "Vec")
}

pub(super) fn vec_element_type(ty: &Type) -> Option<&Type> {
    container_element_type(ty, "Vec")
}

/// Check nested-section type syntax, rejecting wrappers and scalar types.
/// Rust checks whether the named type derives Config after expansion.
pub(super) fn is_direct_named_config_type(ty: &Type) -> bool {
    path_type_ident(ty).is_some_and(|name| {
        !is_integer_type(name)
            && !is_float_type(name)
            && !["Option", "Vec", "String", "bool", "char"]
                .iter()
                .any(|excluded| name == excluded)
    })
}

/// Check whether the type path ends in `Option`; aliases are not resolved.
pub(super) fn is_option_type(ty: &Type) -> bool {
    path_type_ident(ty).is_some_and(|name| name == "Option")
}

pub(super) fn path_type_ident(ty: &Type) -> Option<&Ident> {
    path_type_segment(ty).map(|segment| &segment.ident)
}

pub(super) fn option_inner_type(ty: &Type) -> Option<&Type> {
    container_element_type(ty, "Option")
}

pub(super) fn is_integer_type(name: &Ident) -> bool {
    [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
    ]
    .iter()
    .any(|integer| name == integer)
}

pub(super) fn is_float_type(name: &Ident) -> bool {
    name == "f32" || name == "f64"
}

pub(super) fn is_collection_type(ty: &Type) -> bool {
    is_vec_type(ty) || option_inner_type(ty).is_some_and(is_vec_type)
}
