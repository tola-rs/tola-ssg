//! Native access to module records supplied by the build host.

use typst::diag::{SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Args, Dict, Str, Value, func};
use typst::syntax::{FileId, Span, VirtualRoot};

fn caller_alias(id: FileId) -> Option<String> {
    match id.root() {
        VirtualRoot::Package(pkg) if pkg.namespace.as_str() == "tola-modules" => {
            Some(pkg.name.to_string())
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
    let Ok(val) = modules.get(name) else {
        bail!(
            span,
            "module `{name}` is not configured for this compilation"
        );
    };
    match val {
        Value::Dict(dict) => Ok(dict.clone()),
        _ => bail!(span, "Tola received an invalid module record"),
    }
}

#[func]
pub(super) fn get_module(
    engine: &mut Engine,
    args: &mut Args,
    span: Span,
    /// Configured alias; omit inside `@tola-modules/*` to use the calling instance.
    #[external]
    #[default]
    name: Str,
) -> SourceResult<Dict> {
    let name = args.eat::<Str>()?;
    args.take().finish()?;
    let alias = match name {
        Some(name) => name.to_string(),
        None => {
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
    fn caller_alias_matches_module_pkg() {
        let id = |root| FileId::new(RootedPath::new(root, VirtualPath::new("/lib.typ").unwrap()));
        let pkg = "@tola-modules/notes:0.0.0".parse().unwrap();
        assert_eq!(
            caller_alias(id(VirtualRoot::Package(pkg))),
            Some("notes".into())
        );
        let builtin = "@tola/source:0.0.0".parse().unwrap();
        assert_eq!(caller_alias(id(VirtualRoot::Package(builtin))), None);
        assert_eq!(caller_alias(id(VirtualRoot::Project)), None);
    }
}
