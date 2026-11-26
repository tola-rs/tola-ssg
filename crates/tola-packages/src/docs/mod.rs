//! Bundled declaration documentation parsed from package AST and static natives.

use tola_typst_syntax::docs::Documentation;
use typst::foundations::{CastInfo, Repr};
use typst::syntax::{LinkedNode, Source, ast};

use crate::BuiltinPackage;

mod declaration;
mod related;
mod resolution;

use resolution::{Declaration, Declarations, DeclaredDocumentation, source_function};

#[derive(Debug, thiserror::Error)]
pub enum ExportDocumentationError {
    #[error("`{name}` is not an export of {package}")]
    NotAnExport { name: String, package: String },
    #[error("builtin source `{0}` is absent")]
    MissingSource(String),
    #[error("unknown builtin native `{0}`")]
    UnknownNative(String),
    #[error("invalid bundled package import: {0}")]
    InvalidImport(String),
    #[error("bundled declaration `{0}` has no source binding")]
    MissingBinding(String),
    #[error("bundled dictionary has no member `{0}`")]
    MissingMember(String),
    #[error("bundled member `{0}` has no source dictionary")]
    MissingDictionary(String),
    #[error("bundled member `{0}` is not declared by a dictionary")]
    MemberOutsideDictionary(String),
    #[error("{0}")]
    InvalidDeclaration(&'static str),
    #[error(
        "`{name}` documents parameter `{parameter}`, which the bundled declaration does not have"
    )]
    UnknownParameter { name: String, parameter: String },
}

pub use declaration::{
    ExportDeclaration, ExportDocumentation, Parameter, ParameterKind, Signature, cast_spelling,
    type_spelling,
};
pub use related::RelatedTarget;

impl BuiltinPackage {
    /// Entrypoint's opening `//` block without its leading package identity prefix.
    pub fn overview(&self) -> Option<String> {
        let (_, src) = self.files().find(|(path, _)| *path == "lib.typ")?;
        let identity = format!("{} - ", self.spec());
        let lines: Vec<_> = src
            .lines()
            .map_while(|line| {
                let comment = line.strip_prefix("//")?;
                if comment.starts_with('/') {
                    return None;
                }
                Some(comment.strip_prefix(' ').unwrap_or(comment))
            })
            .enumerate()
            .map(|(i, line)| {
                if i == 0 {
                    line.strip_prefix(&identity).unwrap_or(line)
                } else {
                    line
                }
            })
            .collect();
        let out = lines.join("\n");
        (!out.trim().is_empty()).then(|| out.trim_end().to_owned())
    }

    /// Describes selected exports in request order using bundled sources and static natives.
    pub fn export_documentation(
        &self,
        names: &[String],
    ) -> Result<Vec<ExportDocumentation>, ExportDocumentationError> {
        for name in names {
            if !self.exports().any(|export| export == name) {
                return Err(ExportDocumentationError::NotAnExport {
                    name: name.clone(),
                    package: self.spec().to_string(),
                });
            }
        }
        let sources = self
            .files()
            .filter(|(path, _)| path.ends_with(".typ"))
            .map(|(path, src)| (path, Source::detached(src.into_owned())))
            .collect();
        let decls = Declarations {
            sources,
            natives: crate::library::native_scope(),
        };
        names
            .iter()
            .map(|name| {
                let file = decls.file("lib.typ")?;
                let (kind, docs) = decls.binding(file, LinkedNode::new(file.1.root()), name)?;
                let declared = docs.unwrap_or_else(|| DeclaredDocumentation {
                    documentation: Documentation::parse(""),
                    related: Vec::new(),
                });
                let methods = match &kind {
                    Declaration::Native(func) => match func.returns() {
                        Some(CastInfo::Type(ty)) => crate::datalog::methods(*ty)
                            .map(|(mname, func)| {
                                let mut sig = Signature::of(mname, func);
                                sig.parameters.remove(0);
                                sig
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    Declaration::Export(export) => export.methods.clone(),
                    _ => Vec::new(),
                };
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
                    Declaration::Constant(val) => {
                        ExportDeclaration::Value(format!("{name}: {} = {}", val.ty(), val.repr()))
                    }
                };
                Ok(ExportDocumentation {
                    name: name.clone(),
                    documentation: declared.documentation,
                    related: declared.related,
                    declaration,
                    methods,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_fns_document_params() {
        for pkg in crate::builtin_packages() {
            let names: Vec<_> = pkg.exports().map(str::to_owned).collect();
            for export in pkg.export_documentation(&names).unwrap() {
                let ExportDeclaration::Function(sig) = &export.declaration else {
                    continue;
                };
                assert!(
                    sig.returns.is_some(),
                    "{} `{}` documents no return type",
                    pkg.spec(),
                    export.name
                );
                for param in &sig.parameters {
                    assert!(
                        param.ty.is_some() || param.kind == ParameterKind::Rest,
                        "{} `{}` documents no type for `{}`",
                        pkg.spec(),
                        export.name,
                        param.name
                    );
                }
            }
        }
    }

    #[test]
    fn schema_exports_hide_initializers() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Schema.spec()).unwrap();
        for export in pkg
            .export_documentation(&["email".into(), "any".into()])
            .unwrap()
        {
            assert!(!export.declaration.is_function());
            let ExportDeclaration::Value(decl) = &export.declaration else {
                panic!("{} should be a value", export.name);
            };
            assert_eq!(
                decl,
                &format!("{}: value (defined by package)", export.name)
            );
            assert!(!export.documentation.summary.is_empty());
        }
    }

    #[test]
    fn code_themes_export_value() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Code.spec()).unwrap();
        let exports = pkg.export_documentation(&["code-themes".into()]).unwrap();
        let ExportDeclaration::Value(decl) = &exports[0].declaration else {
            panic!("code-themes should be a value");
        };
        for theme in crate::theme_names() {
            let file = crate::theme_file(theme).expect("offered theme has a file");
            assert!(
                decl.contains(&format!("{theme}: path(\"/{file}\")")),
                "{decl}"
            );
        }
    }

    #[test]
    fn overview_paragraphs() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Document.spec()).unwrap();
        let overview = pkg.overview().unwrap();
        assert!(!overview.starts_with(&pkg.spec().to_string()));
        assert!(overview.contains("\n\n"));
        assert!(!overview.contains("///"));
    }

    #[test]
    fn related_targets_valid() {
        for pkg in crate::builtin_packages() {
            let names: Vec<_> = pkg.exports().map(str::to_owned).collect();
            for export in pkg.export_documentation(&names).unwrap() {
                for target in &export.related {
                    match target {
                        RelatedTarget::Export(name) => assert!(
                            names.contains(name),
                            "`{}` relates `{name}`, which it does not export",
                            export.name
                        ),
                        RelatedTarget::Package(name) => assert!(
                            crate::builtin_packages().any(|p| p.spec().name.as_str() == name),
                            "`{}` relates `@tola/{name}`, which no site can import",
                            export.name
                        ),
                    }
                }
            }
        }
    }
}
