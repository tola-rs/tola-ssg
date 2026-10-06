//! Bundled declaration documentation, without evaluating package or site code.

use anyhow::{Result, bail};
use tola_typst_syntax::docs::Documentation;
use typst::foundations::Repr;
use typst::syntax::{LinkedNode, Source, ast};

use crate::BuiltinPackage;

mod declaration;
mod related;
mod resolution;

use resolution::{Declaration, Declarations, DeclaredDocumentation, source_function};

pub use declaration::{
    ExportDeclaration, ExportDocumentation, Parameter, ParameterKind, Signature, cast_spelling,
    type_spelling,
};
pub use related::RelatedTarget;
pub use tola_typst_syntax::docs::{DocumentationSegment, documentation_segments};

impl BuiltinPackage {
    /// The entrypoint's opening `//` block, without its repeated package identity.
    /// Export documentation begins at `///` and is kept separate from the overview.
    pub fn overview(&self) -> Option<String> {
        let (_, source) = self.files().find(|(path, _)| *path == "lib.typ")?;
        let identity = format!("{} - ", self.spec());
        let lines = source
            .lines()
            .map_while(|line| {
                let comment = line.strip_prefix("//")?;
                if comment.starts_with('/') {
                    return None;
                }
                Some(comment.strip_prefix(' ').unwrap_or(comment))
            })
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    line.strip_prefix(&identity).unwrap_or(line)
                } else {
                    line
                }
            })
            .collect::<Vec<_>>();
        let overview = lines.join("\n");
        (!overview.trim().is_empty()).then(|| overview.trim_end().to_owned())
    }

    /// Describe selected exports in request order, using only bundled source and static natives.
    /// All names are checked before any declaration is returned.
    pub fn export_documentation(&self, names: &[String]) -> Result<Vec<ExportDocumentation>> {
        for name in names {
            if !self.exports().any(|export| export == name) {
                bail!("`{name}` is not an export of {}", self.spec());
            }
        }
        let sources = self
            .files()
            .filter(|(path, _)| path.ends_with(".typ"))
            .map(|(path, source)| (path, Source::detached(source.into_owned())))
            .collect::<Vec<_>>();
        let declarations = Declarations {
            sources,
            natives: crate::library::native_scope(),
        };
        names
            .iter()
            .map(|name| {
                let file = declarations.file("lib.typ")?;
                let (kind, docs) =
                    declarations.binding(file, LinkedNode::new(file.1.root()), name)?;
                let declared = docs.unwrap_or_else(|| DeclaredDocumentation {
                    documentation: Documentation::parse(""),
                    related: Vec::new(),
                });
                let declaration = match kind {
                    Declaration::Native(func) => {
                        ExportDeclaration::Function(Signature::of(name, &func))
                    }
                    Declaration::Source(file, node) => match node.cast::<ast::Closure>() {
                        Some(closure) => ExportDeclaration::Function(source_function(
                            name,
                            file,
                            &closure,
                            &declared.documentation,
                        )?),
                        None => {
                            ExportDeclaration::Value(format!("{name}: value (defined by package)"))
                        }
                    },
                    Declaration::Export(export) => export.declaration,
                    Declaration::Site => {
                        ExportDeclaration::Value(format!("{name}: dictionary (site values)"))
                    }
                    Declaration::Constant(value) => ExportDeclaration::Value(format!(
                        "{name}: {} = {}",
                        value.ty(),
                        value.repr()
                    )),
                };
                Ok(ExportDocumentation {
                    name: name.clone(),
                    documentation: declared.documentation,
                    related: declared.related,
                    declaration,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled function names the type of every parameter and of the value it returns, so
    /// no rendered signature has to leave a type out.
    #[test]
    fn bundled_functions_document_every_parameter_type() {
        for package in crate::builtin_packages() {
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            for export in package.export_documentation(&names).unwrap() {
                let ExportDeclaration::Function(signature) = &export.declaration else {
                    continue;
                };
                assert!(
                    signature.returns.is_some(),
                    "{} `{}` documents no return type",
                    package.spec(),
                    export.name
                );
                for parameter in &signature.parameters {
                    // A source spread's sink type is Typst's `arguments`; nothing declares it.
                    assert!(
                        parameter.ty.is_some() || parameter.kind == ParameterKind::Rest,
                        "{} `{}` documents no type for `{}`",
                        package.spec(),
                        export.name,
                        parameter.name
                    );
                }
            }
        }
    }

    #[test]
    fn schema_values_hide_private_initializers() {
        let package = crate::builtin_package(&crate::TolaPackage::Schema.spec()).unwrap();
        let exports = package
            .export_documentation(&["email".into(), "any".into()])
            .unwrap();
        for export in exports {
            assert!(!export.declaration.is_function());
            let ExportDeclaration::Value(declaration) = &export.declaration else {
                panic!("{} should be a value", export.name)
            };
            assert_eq!(
                declaration,
                &format!("{}: value (defined by package)", export.name)
            );
            assert!(!export.documentation.summary.is_empty());
        }
    }

    #[test]
    fn constant_declarations_show_theme_choices() {
        let package = crate::builtin_package(&crate::TolaPackage::Code.spec()).unwrap();
        let exports = package
            .export_documentation(&["code-themes".into()])
            .unwrap();
        let ExportDeclaration::Value(declaration) = &exports[0].declaration else {
            panic!("code-themes should be a value")
        };
        for theme in crate::theme_names() {
            let file = crate::theme_file(theme).expect("every offered theme names a file");
            assert!(
                declaration.contains(&format!("{theme}: path(\"/{file}\")")),
                "{declaration}"
            );
        }
    }

    #[test]
    fn overview_keeps_module_paragraphs() {
        let package = crate::builtin_package(&crate::TolaPackage::Document.spec()).unwrap();
        let overview = package.overview().unwrap();
        assert!(!overview.starts_with(&package.spec().to_string()));
        assert!(overview.contains("\n\n"));
        assert!(overview.contains("document context"));
        assert!(!overview.contains("///"));
    }

    /// Every related target names an export of its own package or a package a site can import.
    #[test]
    fn related_targets_resolve() {
        for package in crate::builtin_packages() {
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            for export in package.export_documentation(&names).unwrap() {
                for related in &export.related {
                    match related {
                        RelatedTarget::Export(name) => assert!(
                            names.contains(name),
                            "`{}` relates `{name}`, which it does not export",
                            export.name
                        ),
                        RelatedTarget::Package(name) => assert!(
                            crate::builtin_packages()
                                .any(|package| package.spec().name.as_str() == name),
                            "`{}` relates `@tola/{name}`, which no site can import",
                            export.name
                        ),
                    }
                }
            }
        }
    }
}
