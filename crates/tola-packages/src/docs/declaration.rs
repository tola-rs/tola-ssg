//! What one bundled export declares, and the documentation attached to it.

use tola_typst_syntax::docs::Documentation;
use typst::foundations::{CastInfo, Func, ParamInfo, Repr, Type};

use super::RelatedTarget;

/// An export's documentation and the declaration visible without evaluating package code.
#[derive(Debug)]
pub struct ExportDocumentation {
    /// The name a caller imports, including any export alias.
    pub name: String,
    /// The documentation the export carries, split into its summary, parameters, and return.
    pub documentation: Documentation,
    /// The targets the block's `Related:` line names, in the order it writes them; empty when it
    /// writes none.
    pub related: Vec<RelatedTarget>,
    /// What the declaration exposes to a caller.
    pub declaration: ExportDeclaration,
}
/// What an export declares, as bundled source and static natives expose it.
#[derive(Debug)]
pub enum ExportDeclaration {
    /// A function, with the parameters its callers pass and the value it returns.
    Function(Signature),
    /// A value, written as its type and representation.
    Value(String),
}

impl ExportDeclaration {
    /// Whether the export is a function.
    pub fn is_function(&self) -> bool {
        matches!(self, Self::Function(_))
    }
}

/// How callers pass one parameter of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    /// Passed by position.
    Positional,
    /// Passed by name, with the default its declaration carries.
    Named,
    /// Collects the arguments that remain (`..name`), as Typst's `arguments` value.
    Rest,
}

/// One parameter of a function, as every surface renders it.
#[derive(Debug, Clone)]
pub struct Parameter {
    /// The parameter's name, without any `..` a rest parameter carries.
    pub name: String,
    /// The values the parameter accepts, in Typst's code spelling, when a declaration or a
    /// checked observation names them.
    pub ty: Option<String>,
    /// The declared default, written as the declaration spells it.
    pub default: Option<String>,
    /// What the documentation says about the parameter.
    pub docs: Option<String>,
    /// How callers pass it.
    pub kind: ParameterKind,
}

impl Parameter {
    /// The type this parameter renders, in Typst's code spelling: the type its declaration or a
    /// checked observation names, `arguments` for a rest parameter's sink, or `any` when nothing
    /// names one.
    pub fn ty_spelling(&self) -> &str {
        self.ty.as_deref().unwrap_or(match self.kind {
            ParameterKind::Rest => "arguments",
            ParameterKind::Positional | ParameterKind::Named => "any",
        })
    }
}

/// A function's callable shape: the name a caller imports, the parameters callers pass, and the
/// value it returns.
#[derive(Debug, Clone)]
pub struct Signature {
    /// The name a caller imports.
    pub name: String,
    /// The parameters callers pass, in declaration order.
    pub parameters: Vec<Parameter>,
    /// The value the function returns, when its declaration or a checked observation names one.
    pub returns: Option<String>,
}

impl Signature {
    /// The signature Typst's own metadata declares for one function. A closure's parameters carry
    /// no type here: only a documented annotation or a checked observation names one. An element
    /// function returns the element it builds, which Tinymist spells as that element's own name,
    /// so a binding that aliases one still answers the element.
    pub fn of(name: &str, func: &Func) -> Self {
        let returns = match func.to_element() {
            Some(element) => Some(element.name().to_owned()),
            None => func.returns().map(cast_spelling),
        };
        Self {
            name: name.to_owned(),
            parameters: func.params().map(parameter_of).collect(),
            returns,
        }
    }

    /// The block Tinymist prints for a function: `let name(`, every parameter on its own line,
    /// then `) = return;`. An unknown return spells `any`; a return that spells the function's own
    /// name is left out, as Tinymist leaves it. No fence, no indentation, no trailing newline.
    pub fn code(&self) -> String {
        let mut block = format!("let {}(", self.name);
        let mut parameters = self.positional();
        parameters.extend(self.rest());
        parameters.extend(self.named());
        for parameter in parameters {
            block.push_str("\n  ");
            if parameter.kind == ParameterKind::Rest {
                block.push_str("..");
            }
            block.push_str(&parameter.name);
            block.push_str(": ");
            block.push_str(parameter.ty_spelling());
            if let Some(default) = &parameter.default {
                block.push_str(" = ");
                block.push_str(&collapsed_default(default));
            }
            block.push(',');
        }
        if !self.parameters.is_empty() {
            block.push('\n');
        }
        block.push(')');
        let returns = self.returns.as_deref().unwrap_or("any");
        if returns != self.name {
            block.push_str(" = ");
            block.push_str(returns);
        }
        block.push(';');
        block
    }

    /// The positional parameters, in declaration order.
    pub fn positional(&self) -> Vec<&Parameter> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.kind == ParameterKind::Positional)
            .collect()
    }

    /// The named parameters, ordered by name as Tinymist orders them.
    pub fn named(&self) -> Vec<&Parameter> {
        let mut named = self
            .parameters
            .iter()
            .filter(|parameter| parameter.kind == ParameterKind::Named)
            .collect::<Vec<_>>();
        named.sort_by(|left, right| left.name.cmp(&right.name));
        named
    }

    /// The rest parameters, in declaration order.
    pub fn rest(&self) -> Vec<&Parameter> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.kind == ParameterKind::Rest)
            .collect()
    }
}

/// The one spelling of a type's name for every surface: Typst's code spelling (`int`, `str`,
/// `bool`), never the diagnostic long name (`integer`, `string`) a `Type` displays as.
pub fn type_spelling(ty: &Type) -> String {
    ty.short_name().to_owned()
}

/// The spelling of the values one native declaration accepts, in Typst's code spelling.
pub fn cast_spelling(cast: &CastInfo) -> String {
    match cast {
        CastInfo::Any => "any".into(),
        CastInfo::Value(value, _) => value.repr().to_string(),
        CastInfo::Type(ty) => type_spelling(ty),
        CastInfo::Union(types) => types
            .iter()
            .map(cast_spelling)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}
/// One declared default as a signature spells it: Tinymist collapses a long block, raw, or
/// content default, and indents the lines of a multi-line one.
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

/// One parameter as Typst's own metadata declares it: a native's cast type, default, and
/// documentation, or a closure parameter's evaluated default.
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
            default: param.default().map(|default| default.repr().to_string()),
            docs: None,
            kind,
        };
    };
    Parameter {
        name: native.name.to_owned(),
        ty: Some(cast_spelling(&native.input)),
        default: native.default.map(|default| default().repr().to_string()),
        docs: (!native.docs.is_empty()).then(|| native.docs.to_owned()),
        kind,
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use typst::foundations::Value;
    fn signature(export: &ExportDocumentation) -> &Signature {
        let ExportDeclaration::Function(signature) = &export.declaration else {
            panic!("{} should be a function", export.name)
        };
        signature
    }

    pub(crate) fn parameter<'a>(signature: &'a Signature, name: &str) -> &'a Parameter {
        signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == name)
            .unwrap_or_else(|| panic!("missing parameter `{name}`"))
    }

    /// One parameter nothing declares but how callers pass it.
    fn undeclared_parameter(name: &str, kind: ParameterKind) -> Parameter {
        Parameter {
            name: name.to_owned(),
            ty: None,
            default: None,
            docs: None,
            kind,
        }
    }

    #[test]
    fn signature_code_prints_the_tinymist_block() {
        let signature = Signature {
            name: "sample".to_owned(),
            parameters: vec![
                Parameter {
                    ty: Some("int".to_owned()),
                    default: Some("2".to_owned()),
                    ..undeclared_parameter("scale", ParameterKind::Named)
                },
                Parameter {
                    ty: Some("bool".to_owned()),
                    ..undeclared_parameter("alpha", ParameterKind::Named)
                },
                undeclared_parameter("value", ParameterKind::Positional),
                undeclared_parameter("rest", ParameterKind::Rest),
            ],
            returns: Some("str".to_owned()),
        };
        assert_eq!(
            signature.code(),
            "let sample(\n  value: any,\n  ..rest: arguments,\n  alpha: bool,\n  scale: int = 2,\n) = str;"
        );
    }

    #[test]
    fn signature_code_spells_unknown_returns_any() {
        let signature = Signature {
            name: "sample".to_owned(),
            parameters: vec![],
            returns: None,
        };
        assert_eq!(signature.code(), "let sample() = any;");
    }

    #[test]
    fn signature_code_omits_returns_matching_the_name() {
        let signature = Signature {
            name: "sample".to_owned(),
            parameters: vec![],
            returns: Some("sample".to_owned()),
        };
        assert_eq!(signature.code(), "let sample();");
    }

    #[test]
    fn native_parameters_carry_types_and_documentation() {
        let package = crate::builtin_package(&crate::TolaPackage::Image.spec()).unwrap();
        let exports = package
            .export_documentation(&["resize-image".into()])
            .unwrap();
        let signature = signature(&exports[0]);
        let op = parameter(signature, "op");
        assert_eq!(op.ty.as_deref(), Some("str"));
        assert_eq!(op.default.as_deref(), Some("\"fill\""));
        assert!(op.kind == ParameterKind::Named);
        assert!(parameter(signature, "path").kind == ParameterKind::Positional);
        assert_eq!(
            parameter(signature, "background").ty.as_deref(),
            Some("color | none")
        );
        assert_eq!(signature.returns.as_deref(), Some("dictionary"));
        for hidden in ["engine", "span"] {
            assert!(
                signature
                    .parameters
                    .iter()
                    .all(|parameter| parameter.name != hidden)
            );
        }
    }

    /// A native rest parameter renders the cast its declaration carries, not the `arguments` sink
    /// a source spread falls back to.
    #[test]
    fn native_rest_parameter_renders_its_cast() {
        use typst::LibraryExt as _;
        let library = typst::Library::builder().build();
        let table = library
            .global
            .scope()
            .get("table")
            .expect("the library binds `table`")
            .read()
            .clone();
        let Value::Func(table) = table else {
            panic!("`table` is a function")
        };
        let signature = Signature::of("table", &table);
        let children = parameter(&signature, "children");
        assert!(children.kind == ParameterKind::Rest);
        assert_eq!(children.ty_spelling(), "content");
    }

    #[test]
    fn source_defaults_remain_source_expressions() {
        let package = crate::builtin_package(&crate::TolaPackage::Collection.spec()).unwrap();
        let exports = package.export_documentation(&["index-by".into()]).unwrap();
        let signature = signature(&exports[0]);
        assert_eq!(
            parameter(signature, "key").default.as_deref(),
            Some("value => value")
        );
    }
    #[test]
    fn schema_aliases_keep_source_signatures() {
        let package = crate::builtin_package(&crate::TolaPackage::Schema.spec()).unwrap();
        let exports = package
            .export_documentation(&["schema".into(), "min-length".into()])
            .unwrap();
        assert_eq!(
            parameter(signature(&exports[0]), "unknown")
                .default
                .as_deref(),
            Some("\"error\"")
        );
        let minimum = parameter(signature(&exports[1]), "minimum");
        assert_eq!(minimum.default, None);
        assert!(
            exports
                .iter()
                .all(|export| export.declaration.is_function())
        );
    }
}
