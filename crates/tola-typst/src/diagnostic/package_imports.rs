//! Static package references in site files read by one compilation.
//!
//! Import and include syntax gives authors files to inspect when Typst points only into a
//! package. These references are navigation hints, not proof that a statement executed.

use std::collections::{BTreeMap, BTreeSet};

use typst::syntax::ast::{self, Expr};
use typst::syntax::{DiagSpan, Source};

/// Site files containing literal imports or includes of packages the compilation read.
#[derive(Debug, Default)]
pub(crate) struct PackageImporters {
    files: BTreeMap<String, Vec<String>>,
}

impl PackageImporters {
    /// Sorted source navigation hints for `package`, independent of statement execution.
    pub(crate) fn importing(&self, package: &str) -> &[String] {
        self.files.get(package).map_or(&[], Vec::as_slice)
    }

    /// `packages` limits navigation hints to packages this compilation actually read.
    pub(crate) fn collect(
        sources: impl IntoIterator<Item = (String, Source)>,
        packages: &BTreeSet<String>,
    ) -> Self {
        let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (path, source) in sources {
            if !packages
                .iter()
                .any(|package| source.text().contains(package.as_str()))
            {
                continue;
            }
            for package in imported_packages(&source) {
                if !packages.contains(&package) {
                    continue;
                }
                let importers = files.entry(package).or_default();
                if !importers.contains(&path) {
                    importers.push(path.clone());
                }
            }
        }
        for importers in files.values_mut() {
            importers.sort();
        }
        Self { files }
    }
}

/// The package identity a Typst module path names, when it names one.
///
/// A package path is `@namespace/name:version`, optionally followed by the path inside
/// the package. Only the identity is returned.
pub(crate) fn package_specifier(module_path: &str) -> Option<&str> {
    let rest = module_path.strip_prefix('@')?;
    let mut segments = rest.split('/');
    let namespace = segments.next()?;
    let name = segments.next()?;
    if namespace.is_empty() || name.is_empty() || !name.contains(':') {
        return None;
    }
    Some(&module_path[..1 + namespace.len() + 1 + name.len()])
}

/// The literal package reference enclosing `span`, when it belongs to an import or include.
pub(crate) fn imported_package(source: &Source, span: DiagSpan) -> Option<String> {
    let range = diagnostic_range(source, span)?;
    let mut node = Some(node_covering(source, &range)?);
    while let Some(current) = node {
        if let Some(package) = referenced_package(current.get()) {
            return Some(package);
        }
        node = current.parent().cloned();
    }
    None
}

/// The deepest node whose range covers `range`.
fn node_covering<'a>(
    source: &'a Source,
    range: &std::ops::Range<usize>,
) -> Option<typst::syntax::LinkedNode<'a>> {
    let mut node = typst::syntax::LinkedNode::new(source.root());
    loop {
        let child = node.children().find(|child| {
            let child = child.range();
            child.start <= range.start && range.end <= child.end
        });
        match child {
            Some(child) => node = child,
            None => return Some(node),
        }
    }
}

/// The byte range a diagnostic span covers in `source`.
fn diagnostic_range(source: &Source, span: DiagSpan) -> Option<std::ops::Range<usize>> {
    match span.get() {
        typst::syntax::DiagSpanKind::Number { num, sub_range, .. } => source.range(num, sub_range),
        typst::syntax::DiagSpanKind::Range { range, .. } => Some(range),
        typst::syntax::DiagSpanKind::Detached => None,
    }
}

/// Package identities named by literal imports or includes, in syntax order.
pub(crate) fn imported_packages(source: &Source) -> Vec<String> {
    let mut packages = Vec::new();
    collect_imported_packages(source.root(), &mut packages);
    packages
}

fn referenced_package(node: &typst::syntax::SyntaxNode) -> Option<String> {
    let source = node
        .cast::<ast::ModuleImport>()
        .map(|import| import.source())
        .or_else(|| {
            node.cast::<ast::ModuleInclude>()
                .map(|include| include.source())
        })?;
    let Expr::Str(module) = source else {
        return None;
    };
    package_specifier(module.get().as_str()).map(str::to_owned)
}

fn collect_imported_packages(node: &typst::syntax::SyntaxNode, packages: &mut Vec<String>) {
    if let Some(package) = referenced_package(node)
        && !packages.contains(&package)
    {
        packages.push(package);
    }
    for child in node.children() {
        collect_imported_packages(child, packages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    #[test]
    fn specifier_drops_package_relative_path() {
        assert_eq!(
            package_specifier("@preview/cetz:0.3.4"),
            Some("@preview/cetz:0.3.4")
        );
        assert_eq!(
            package_specifier("@preview/cetz:0.3.4/src/canvas.typ"),
            Some("@preview/cetz:0.3.4")
        );
        assert_eq!(package_specifier("templates/page.typ"), None);
        assert_eq!(package_specifier("@preview/cetz"), None);
        assert_eq!(package_specifier("@/cetz:0.3.4"), None);
    }

    #[test]
    fn imports_ignore_non_import_text() {
        let file = source(
            "#import \"@preview/cetz:0.3.4\": canvas\n\
             #let example = \"@preview/other:1.0.0\"\n\
             // #import \"@preview/commented:1.0.0\": canvas\n",
        );

        assert_eq!(imported_packages(&file), ["@preview/cetz:0.3.4"]);
    }

    #[test]
    fn nested_block_imports_are_found() {
        let file = source(
            "#let figure() = {\n  import \"@preview/cetz:0.3.4\": canvas\n  canvas(())\n}\n",
        );

        assert_eq!(imported_packages(&file), ["@preview/cetz:0.3.4"]);
    }

    #[test]
    fn includes_name_package_identity() {
        let file = source(
            "#include \"@local/diag:1.0.0\"\n\
             #include \"@local/diag:1.0.0/part.typ\"\n\
             #include \"helper.typ\"\n\
             #let example = \"@local/other:1.0.0\"\n",
        );
        assert_eq!(imported_packages(&file), ["@local/diag:1.0.0"]);
        let span = DiagSpan::from_range(file.id(), 10..15);
        assert_eq!(
            imported_package(&file, span).as_deref(),
            Some("@local/diag:1.0.0")
        );
    }

    #[test]
    fn importers_name_each_importing_file_once() {
        let importers = PackageImporters::collect(
            [
                (
                    "content/a.typ".to_owned(),
                    source(
                        "#import \"@preview/cetz:0.3.4\": canvas\n#import \"@preview/cetz:0.3.4\": other\n",
                    ),
                ),
                (
                    "templates/page.typ".to_owned(),
                    source("#import \"@preview/cetz:0.3.4\": canvas"),
                ),
            ],
            &["@preview/cetz:0.3.4".to_owned()].into_iter().collect(),
        );

        assert_eq!(
            importers.importing("@preview/cetz:0.3.4"),
            ["content/a.typ", "templates/page.typ"]
        );
    }
}
