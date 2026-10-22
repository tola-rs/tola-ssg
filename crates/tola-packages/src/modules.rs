//! Native access to the module records supplied by the build host.

use typst::diag::{SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Dict, Smart, Str, Value, func};
use typst::syntax::{FileId, Span, VirtualRoot};

fn caller_alias(id: FileId) -> Option<String> {
    match id.root() {
        VirtualRoot::Package(package) if package.namespace.as_str() == "tola-modules" => {
            Some(package.name.to_string())
        }
        _ => None,
    }
}

fn lookup(engine: &Engine, name: &str, span: Span) -> SourceResult<Dict> {
    let Some(modules) = super::library::module_records(engine) else {
        bail!(
            span,
            "Tola could not read module records for this compilation"
        );
    };
    let Ok(value) = modules.get(name) else {
        bail!(
            span,
            "module `{name}` is not configured for this compilation"
        );
    };
    match value {
        Value::Dict(record) => Ok(record.clone()),
        _ => bail!(span, "Tola received an invalid module record"),
    }
}

#[func]
pub(super) fn get_module(
    engine: &mut Engine,
    span: Span,
    /// The module alias declared by the site, or `auto` for the calling module.
    #[default(Smart::Auto)]
    name: Smart<Str>,
) -> SourceResult<Dict> {
    if super::library::in_module_validation(engine) {
        bail!(
            span,
            "module configuration is unavailable while validating options schemas"
        );
    }
    let alias = match name {
        Smart::Custom(name) => name.to_string(),
        Smart::Auto => {
            let Some(alias) = span.id().and_then(caller_alias) else {
                bail!(
                    span,
                    "`get-module()` must be called from a configured module package"
                );
            };
            alias
        }
    };
    lookup(engine, &alias, span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::syntax::{RootedPath, VirtualPath};

    #[test]
    fn caller_is_the_public_module_alias_only() {
        let id = |root| FileId::new(RootedPath::new(root, VirtualPath::new("/lib.typ").unwrap()));
        let package = "@tola-modules/notes:0.0.0".parse().unwrap();
        assert_eq!(
            caller_alias(id(VirtualRoot::Package(package))),
            Some("notes".into())
        );
        let builtin = "@tola/source:0.0.0".parse().unwrap();
        assert_eq!(caller_alias(id(VirtualRoot::Package(builtin))), None);
        assert_eq!(caller_alias(id(VirtualRoot::Project)), None);
    }
}
