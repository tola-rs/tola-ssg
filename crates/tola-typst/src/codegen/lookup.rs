//! Element and function lookup across Typst library scopes.

use typst::Library;
use typst::foundations::{Func, Module};

/// Find element functions of this name across global, module, and element sub-scopes.
pub(crate) fn find_element_funcs<'a>(
    library: &'a Library,
    func_name: &'a str,
) -> impl Iterator<Item = Func> + 'a {
    find_all_funcs(library, func_name).filter(|func| func.to_element().is_some())
}

/// Resolves `grid.cell`-style names that exist only inside a parent element's scope.
pub(crate) fn find_element_in_scope(parent_func: &Func, func_name: &str) -> Option<Func> {
    parent_func
        .scope()?
        .get(func_name)?
        .read()
        .clone()
        .cast::<Func>()
        .ok()
        .filter(|f| f.to_element().is_some())
}

/// Find all functions with the given name across all scopes.
pub(crate) fn find_all_funcs<'a>(
    library: &'a Library,
    func_name: &'a str,
) -> impl Iterator<Item = Func> + 'a {
    let global_scope = library.global.scope();

    let global_func = global_scope
        .get(func_name)
        .and_then(|binding| binding.read().clone().cast::<Func>().ok());

    let module_funcs = global_scope.iter().filter_map(move |(_, binding)| {
        let module = binding.read().clone().cast::<Module>().ok()?;
        let func_binding = module.scope().get(func_name)?;
        func_binding.read().clone().cast::<Func>().ok()
    });

    let sub_scope_funcs = global_scope.iter().flat_map(move |(_, binding)| {
        let func = binding.read().clone().cast::<Func>().ok();
        func.into_iter().flat_map(move |f| {
            f.scope().into_iter().flat_map(move |scope| {
                scope
                    .get(func_name)
                    .and_then(|b| b.read().clone().cast::<Func>().ok())
            })
        })
    });

    global_func
        .into_iter()
        .chain(module_funcs)
        .chain(sub_scope_funcs)
}
