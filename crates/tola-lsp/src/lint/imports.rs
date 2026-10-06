//! Whether an import a source can see could introduce a name its index cannot enumerate.

use tola_typst_syntax::names::{DeclarationKind, Import, ImportSource, Initializer, SourceNames};

/// Whether an import this scope can see at `at` could supply a name the index cannot enumerate.
///
/// Only a wildcard's interface, or a bare import of a module whose identity this index cannot
/// establish, reaches beyond the declarations `resolve` answers; an explicit item or an alias is a
/// declaration. The import must be written before the spelling, because its names become visible
/// only at its own end.
pub(super) fn could_supply_unknown(names: &SourceNames, scope: usize, at: usize) -> bool {
    names.imports().iter().enumerate().any(|(index, import)| {
        import.range.end <= at
            && scope_reaches(names, import.scope, scope)
            && may_bind_unknown(names, index, import)
    })
}

/// Whether one statement can introduce a name this index does not hold.
fn may_bind_unknown(names: &SourceNames, index: usize, import: &Import) -> bool {
    if import.wildcard {
        return true;
    }
    if !import.items.is_empty() || is_aliased(names, index) {
        return false;
    }
    match &import.source {
        // A single-segment source names its own binding, which is a declaration.
        ImportSource::Name(path) => path.segments.len() > 1,
        ImportSource::Unknown => true,
        ImportSource::Path(_) => false,
    }
}

/// Whether the statement renames the module it imports (`import x as y`).
fn is_aliased(names: &SourceNames, index: usize) -> bool {
    names.declarations().iter().any(|declaration| {
        matches!(
            declaration.kind,
            DeclarationKind::Import {
                explicit_alias: true,
                ..
            }
        ) && matches!(
            &declaration.initializer,
            Initializer::Import { import, .. } if *import == index
        )
    })
}

/// Whether `outer` is `inner` or one of its enclosing scopes.
fn scope_reaches(names: &SourceNames, outer: usize, inner: usize) -> bool {
    let mut scope = Some(inner);
    while let Some(current) = scope {
        if current == outer {
            return true;
        }
        scope = names.scopes()[current].parent;
    }
    false
}
