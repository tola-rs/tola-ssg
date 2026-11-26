//! Bundled export declarations and attached documentation.

use tola_typst_syntax::docs::Documentation;
use typst::foundations::{CastInfo, Func, ParamInfo, Repr, Type};
use typst::syntax::ast::{self, AstNode};
use typst::syntax::{Source, Span};

use super::RelatedTarget;

#[derive(Debug)]
pub struct ExportDocumentation {
    pub name: String,
    pub documentation: Documentation,
    pub related: Vec<RelatedTarget>,
    pub declaration: ExportDeclaration,
    pub methods: Vec<Signature>,
}

#[derive(Debug)]
pub enum ExportDeclaration {
    Function(Signature),
    Value(String),
}

impl ExportDeclaration {
    pub fn is_function(&self) -> bool {
        matches!(self, Self::Function(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    Positional,
    Named,
    Rest,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub ty: Option<String>,
    pub required: bool,
    pub default: Option<String>,
    pub docs: Option<String>,
    pub kind: ParameterKind,
}

impl Parameter {
    pub fn ty_spelling(&self) -> &str {
        self.ty.as_deref().unwrap_or(match self.kind {
            ParameterKind::Rest => "arguments",
            ParameterKind::Positional | ParameterKind::Named => "any",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Signature {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub returns: Option<String>,
}

impl Signature {
    pub fn of(name: &str, func: &Func) -> Self {
        let returns = match func.to_element() {
            Some(el) => Some(el.name().to_owned()),
            None => func.returns().map(cast_spelling),
        };
        Self {
            name: name.to_owned(),
            parameters: func.params().map(parameter_of).collect(),
            returns,
        }
    }

    /// Reads callable shape from a source closure AST; types come from doc comments only.
    pub fn from_source(
        name: &str,
        src: &Source,
        closure: &ast::Closure<'_>,
        docs: &Documentation,
    ) -> Option<Self> {
        let parameters = closure
            .params()
            .children()
            .map(|param| {
                let (pname, kind, default) = match param {
                    ast::Param::Pos(pat) => (
                        match pat {
                            ast::Pattern::Normal(ast::Expr::Ident(ident)) => {
                                ident.get().to_string()
                            }
                            _ => source_text(src, pat.span())?,
                        },
                        ParameterKind::Positional,
                        None,
                    ),
                    ast::Param::Named(named) => {
                        let expr = named
                            .to_untyped()
                            .children()
                            .next_back()
                            .and_then(|c| c.cast::<ast::Expr>());
                        (
                            named.name().get().to_string(),
                            ParameterKind::Named,
                            Some(match expr {
                                Some(expr) => source_text(src, expr.span())?,
                                None => "none".to_owned(),
                            }),
                        )
                    }
                    ast::Param::Spread(spread) => (
                        spread
                            .sink_ident()
                            .map_or_else(String::new, |id| id.get().to_string()),
                        ParameterKind::Rest,
                        None,
                    ),
                };
                let doc_param = docs.parameters.iter().rev().find(|p| p.name == pname);
                Some(Parameter {
                    name: pname,
                    ty: (kind != ParameterKind::Rest)
                        .then(|| doc_param.and_then(|p| p.ty.clone()))
                        .flatten(),
                    required: kind == ParameterKind::Positional,
                    default,
                    docs: doc_param
                        .map(|p| p.description.clone())
                        .filter(|d| !d.is_empty()),
                    kind,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            name: name.to_owned(),
            parameters,
            returns: docs.returns.clone(),
        })
    }

    /// Formats as Tinymist's signature block (`let name(...) = return;`).
    pub fn code(&self) -> String {
        let mut out = format!("let {}(", self.name);
        self.write_parameters(&mut out);
        let ret = self.returns.as_deref().unwrap_or("any");
        if ret != self.name {
            out.push_str(" = ");
            out.push_str(ret);
        }
        out.push(';');
        out
    }

    pub fn method_code(&self, receiver: char) -> String {
        let mut out = format!("{receiver}.{}(", self.name);
        self.write_parameters(&mut out);
        if let Some(ret) = &self.returns {
            out.push_str(" -> ");
            out.push_str(ret);
        }
        out
    }

    fn write_parameters(&self, out: &mut String) {
        let mut params = self.positional();
        params.extend(self.rest());
        params.extend(self.named());
        for param in params {
            out.push_str("\n  ");
            if param.kind == ParameterKind::Rest {
                out.push_str("..");
            }
            out.push_str(&param.name);
            out.push_str(": ");
            out.push_str(param.ty_spelling());
            if let Some(default) = &param.default {
                out.push_str(" = ");
                out.push_str(&collapsed_default(default));
            }
            out.push(',');
        }
        if !self.parameters.is_empty() {
            out.push('\n');
        }
        out.push(')');
    }

    pub fn positional(&self) -> Vec<&Parameter> {
        self.parameters
            .iter()
            .filter(|p| p.kind == ParameterKind::Positional)
            .collect()
    }

    pub fn named(&self) -> Vec<&Parameter> {
        let mut named: Vec<_> = self
            .parameters
            .iter()
            .filter(|p| p.kind == ParameterKind::Named)
            .collect();
        named.sort_by(|a, b| a.name.cmp(&b.name));
        named
    }

    pub fn rest(&self) -> Vec<&Parameter> {
        self.parameters
            .iter()
            .filter(|p| p.kind == ParameterKind::Rest)
            .collect()
    }
}

fn source_text(src: &Source, span: Span) -> Option<String> {
    let node = src.find(span)?;
    Some(
        src.text()[node.range()]
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Returns Typst's short code spelling (`int`, `str`, `bool`), never the diagnostic long name.
pub fn type_spelling(ty: &Type) -> String {
    ty.short_name().to_owned()
}

pub fn cast_spelling(cast: &CastInfo) -> String {
    match cast {
        CastInfo::Any => "any".into(),
        CastInfo::Value(val, _) => val.repr().to_string(),
        CastInfo::Type(ty) => type_spelling(ty),
        CastInfo::Union(types) => types
            .iter()
            .map(cast_spelling)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn collapsed_default(default: &str) -> String {
    let default = default.trim();
    let collapsed = if default.len() > 30 {
        match (default.as_bytes().first(), default.as_bytes().last()) {
            (Some(b'{'), Some(b'}')) => "{ .. }",
            (Some(b'`'), Some(b'`')) => "raw",
            (Some(b'['), Some(b']')) => "content",
            _ => default,
        }
    } else {
        default
    };
    collapsed.replace('\n', "\n  ")
}

fn parameter_of(param: ParamInfo) -> Parameter {
    let kind = if param.variadic() {
        ParameterKind::Rest
    } else if param.named() {
        ParameterKind::Named
    } else {
        ParameterKind::Positional
    };
    let Some(native) = param.to_native() else {
        return Parameter {
            name: param.name().unwrap_or("_").to_owned(),
            ty: None,
            required: kind != ParameterKind::Rest && param.required(),
            default: param.default().map(|d| d.repr().to_string()),
            docs: None,
            kind,
        };
    };
    Parameter {
        name: native.name.to_owned(),
        ty: Some(cast_spelling(&native.input)),
        required: kind != ParameterKind::Rest && native.required,
        default: native.default.map(|d| d().repr().to_string()),
        docs: (!native.docs.is_empty()).then(|| native.docs.to_owned()),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::Value;

    fn sig(export: &ExportDocumentation) -> &Signature {
        let ExportDeclaration::Function(sig) = &export.declaration else {
            panic!("{} should be a function", export.name);
        };
        sig
    }

    fn param<'a>(sig: &'a Signature, name: &str) -> &'a Parameter {
        sig.parameters
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("missing parameter `{name}`"))
    }

    fn bare_param(name: &str, kind: ParameterKind) -> Parameter {
        Parameter {
            name: name.to_owned(),
            required: kind == ParameterKind::Positional,
            ty: None,
            default: None,
            docs: None,
            kind,
        }
    }

    #[test]
    fn signature_code_tinymist() {
        let sig = Signature {
            name: "sample".to_owned(),
            parameters: vec![
                Parameter {
                    ty: Some("int".to_owned()),
                    default: Some("2".to_owned()),
                    ..bare_param("scale", ParameterKind::Named)
                },
                Parameter {
                    ty: Some("bool".to_owned()),
                    ..bare_param("alpha", ParameterKind::Named)
                },
                bare_param("value", ParameterKind::Positional),
                bare_param("rest", ParameterKind::Rest),
            ],
            returns: Some("str".to_owned()),
        };
        assert_eq!(
            sig.code(),
            "let sample(\n  value: any,\n  ..rest: arguments,\n  alpha: bool,\n  scale: int = 2,\n) = str;"
        );
    }

    #[test]
    fn signature_code_unknown_return() {
        let sig = Signature {
            name: "sample".to_owned(),
            parameters: vec![],
            returns: None,
        };
        assert_eq!(sig.code(), "let sample() = any;");
    }

    #[test]
    fn signature_code_elides_self_return() {
        let sig = Signature {
            name: "sample".to_owned(),
            parameters: vec![],
            returns: Some("sample".to_owned()),
        };
        assert_eq!(sig.code(), "let sample();");
    }

    #[test]
    fn native_fn_signature() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Image.spec()).unwrap();
        let exports = pkg.export_documentation(&["resize-image".into()]).unwrap();
        let sig = sig(&exports[0]);
        let op = param(sig, "op");
        assert_eq!(op.ty.as_deref(), Some("str"));
        assert_eq!(op.default.as_deref(), Some("\"fill\""));
        assert_eq!(op.kind, ParameterKind::Named);
        assert_eq!(param(sig, "source").kind, ParameterKind::Positional);
        assert_eq!(param(sig, "background").ty.as_deref(), Some("color | none"));
        assert_eq!(sig.returns.as_deref(), Some("dictionary"));
        for hidden in ["engine", "span"] {
            assert!(sig.parameters.iter().all(|p| p.name != hidden));
        }
    }

    #[test]
    fn native_rest_param_cast() {
        use typst::LibraryExt as _;
        let lib = typst::Library::builder().build();
        let Value::Func(table) = lib
            .global
            .scope()
            .get("table")
            .expect("library binds `table`")
            .read()
            .clone()
        else {
            panic!("`table` is a function");
        };
        let sig = Signature::of("table", &table);
        let children = param(&sig, "children");
        assert_eq!(children.kind, ParameterKind::Rest);
        assert_eq!(children.ty_spelling(), "content");
    }

    #[test]
    fn source_fn_default_expr() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Collection.spec()).unwrap();
        let exports = pkg.export_documentation(&["index-by".into()]).unwrap();
        let sig = sig(&exports[0]);
        assert_eq!(param(sig, "key").default.as_deref(), Some("value => value"));
    }

    #[test]
    fn schema_alias_signatures() {
        let pkg = crate::builtin_package(&crate::TolaPackage::Schema.spec()).unwrap();
        let exports = pkg
            .export_documentation(&["schema".into(), "min-length".into()])
            .unwrap();
        assert_eq!(
            param(sig(&exports[0]), "unknown").default.as_deref(),
            Some("\"error\"")
        );
        assert_eq!(param(sig(&exports[1]), "minimum").default, None);
        assert!(exports.iter().all(|e| e.declaration.is_function()));
    }
}
