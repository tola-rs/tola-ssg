use std::collections::{BTreeMap, BTreeSet};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, ParameterInformation, ParameterLabel, Range, SignatureInformation, TextEdit,
};
use tola_packages::{Parameter, ParameterKind, Signature, type_spelling};
use tola_typst::typst::foundations::{Func, Repr, Value};
use tola_typst_syntax::docs::Documentation as TypstDocs;

use super::super::expression::ExpressionType;
use super::Semantic;

pub(in crate::query) fn function_docs(func: &Func) -> Option<&str> {
    func.docs().filter(|docs| !docs.is_empty())
}

/// One function's signature: Typst metadata, the declaration's docs, then observed types.
pub(in crate::query) fn signature(
    name: &str,
    func: &Func,
    documented: Option<&TypstDocs>,
    observed: &BTreeMap<String, String>,
) -> Signature {
    let mut sig = Signature::of(name, func);
    for param in &mut sig.parameters {
        let doc = documented.and_then(|docs| docs.parameters.iter().find(|p| p.name == param.name));
        if param.ty.is_none() {
            // A rest parameter's value is Typst's `arguments`, which observations never replace.
            let obs = match param.kind {
                ParameterKind::Rest => None,
                _ => observed.get(&param.name).cloned(),
            };
            param.ty = doc.and_then(|d| d.ty.clone()).or(obs);
        }
        if param.docs.is_none() {
            param.docs = doc
                .map(|d| d.description.clone())
                .filter(|desc| !desc.is_empty());
        }
    }
    if sig.returns.is_none() {
        sig.returns = documented.and_then(|docs| docs.returns.clone());
    }
    sig
}

fn parameter_docs(param: &Parameter) -> Option<Documentation> {
    match (param.kind, param.required, param.docs.as_deref()) {
        (ParameterKind::Named, true, Some(docs)) => Some(super::super::markdown_documentation(
            format!("Required. {docs}"),
        )),
        (ParameterKind::Named, true, None) => {
            Some(super::super::markdown_documentation("Required."))
        }
        (_, _, docs) => docs.map(super::super::markdown_documentation),
    }
}

pub(super) fn signature_information(
    func: &Func,
    name: &str,
    method: bool,
    docs: Option<&str>,
) -> (SignatureInformation, Vec<Parameter>) {
    let documented = docs.map(TypstDocs::parse);
    let mut sig = signature(name, func, documented.as_ref(), &BTreeMap::new());
    // A method call writes its receiver outside the argument list.
    if method
        && matches!(
            sig.parameters.first().map(|p| p.kind),
            Some(ParameterKind::Positional)
        )
    {
        sig.parameters.remove(0);
    }
    let ordered = ordered_parameters(&sig);
    let label = inline_signature(&sig);
    let params = ordered
        .iter()
        .map(|param| ParameterInformation {
            label: ParameterLabel::Simple(format!("{}:", param.name)),
            documentation: parameter_docs(param),
        })
        .collect();
    (
        SignatureInformation {
            label,
            parameters: Some(params),
            documentation: docs.map(super::super::markdown_documentation),
            active_parameter: None,
        },
        ordered.into_iter().cloned().collect(),
    )
}

pub(in crate::query) fn inline_signature(sig: &Signature) -> String {
    let ordered = ordered_parameters(sig);
    let mut line = format!("{}(", sig.name);
    for (idx, param) in ordered.iter().enumerate() {
        if idx > 0 {
            line.push_str(", ");
        }
        if param.kind == ParameterKind::Rest {
            line.push_str("..");
        }
        line.push_str(&param.name);
        line.push_str(": ");
        line.push_str(param.ty_spelling());
    }
    line.push(')');
    if let Some(returns) = &sig.returns {
        line.push_str(" -> ");
        line.push_str(returns);
    }
    line
}

fn ordered_parameters(sig: &Signature) -> Vec<&Parameter> {
    let mut ordered = sig.positional();
    ordered.extend(sig.rest());
    ordered.extend(sig.named());
    ordered
}

pub(in crate::query) fn abbreviated(repr: &str) -> String {
    const SHOWN: usize = 8192;
    let total = repr.chars().count();
    if total <= SHOWN {
        return repr.to_owned();
    }
    let taken: String = repr.chars().take(SHOWN).collect();
    // Cut at an entry boundary so the omission marker never interrupts a single item.
    let held = match taken.rfind('\n') {
        Some(at) => taken[..at].to_owned(),
        None => taken.trim_end().to_owned(),
    };
    format!(
        "{held}\n… ({} characters omitted)",
        total - held.chars().count()
    )
}

pub(in crate::query) fn sampled_value(val: &Value) -> String {
    abbreviated(&val.repr())
}

pub(in crate::query) fn description(val: &Value, name: &str) -> String {
    match val {
        Value::Func(func) => inline_signature(&signature(name, func, None, &BTreeMap::new())),
        _ => {
            let repr = abbreviated(&val.repr());
            if names_itself(&repr) {
                repr
            } else {
                format!("{}: {repr}", type_spelling(&val.ty()))
            }
        }
    }
}

fn names_itself(repr: &str) -> bool {
    repr.starts_with('<')
        || repr.find('(').is_some_and(|at| {
            at > 0
                && repr[..at]
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
        })
}

fn expression_description(ty: &ExpressionType, name: &str) -> String {
    match ty.value() {
        Some(val) => description(val, name),
        None => ty.type_name(),
    }
}

pub(in crate::query) fn completion(
    name: &str,
    vals: &[ExpressionType],
    edit: CompletionTextEdit,
) -> CompletionItem {
    let val = super::super::completion::agreed(vals.iter().map(ExpressionType::value)).flatten();
    let kind = match val {
        Some(Value::Func(_)) => CompletionItemKind::FUNCTION,
        Some(Value::Module(_)) => CompletionItemKind::MODULE,
        Some(_) => CompletionItemKind::VARIABLE,
        None if vals.iter().any(|ty| !matches!(ty, ExpressionType::Unknown)) => {
            CompletionItemKind::VARIABLE
        }
        None => CompletionItemKind::FIELD,
    };
    let docs = match val {
        Some(Value::Func(func)) => function_docs(func).map(super::super::markdown_documentation),
        _ => None,
    };
    let desc = match vals {
        [] => None,
        vals => Some(
            vals.iter()
                .map(|val| expression_description(val, name))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    };
    CompletionItem {
        label: name.into(),
        kind: Some(kind),
        label_details: desc.map(|desc| CompletionItemLabelDetails {
            detail: None,
            description: Some(desc),
        }),
        text_edit: Some(edit),
        documentation: docs,
        ..CompletionItem::default()
    }
}

pub(in crate::query) fn parameter_completions(
    sem: &Semantic<'_>,
    funcs: &[(Value, bool)],
    taken: &BTreeSet<String>,
    range: Range,
) -> Vec<CompletionItem> {
    let mut offered = BTreeSet::new();
    let mut items = Vec::new();
    for (val, _) in funcs {
        let Value::Func(func) = val else { continue };
        let documented = sem
            .function_documentation(func, None)
            .map(|docs| TypstDocs::parse(&docs));
        let name = func.name().unwrap_or_default();
        for param in signature(name, func, documented.as_ref(), &BTreeMap::new()).parameters {
            if param.kind != ParameterKind::Named
                || taken.contains(&param.name)
                || !offered.insert(param.name.clone())
            {
                continue;
            }
            let name = param.name.clone();
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(param.ty_spelling().to_owned()),
                }),
                documentation: parameter_docs(&param),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: crate::completion::typst_snippet(&format!("{name}: ${{}}")),
                })),
                insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
                ..CompletionItem::default()
            });
        }
    }
    items
}
