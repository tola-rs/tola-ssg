use std::collections::{BTreeMap, BTreeSet};

use lsp_types::Hover;
use tola_packages::Signature;
use tola_typst::typst::foundations::{Func, Styles, Value};
use tola_typst::typst::syntax::Span;
use tola_typst_syntax::docs::Documentation;

use crate::markdown;
use crate::protocol::markdown_hover;
use crate::sentence::listed;

use super::super::expression::ExpressionType;
use super::super::records::declared::DeclaredOutput;
use super::super::semantic::{self, Semantic};

pub(in crate::query) const SECTION_JOIN: &str = "\n\n---\n\n";
const SHAPE_FIELD_LIMIT: usize = 20;

pub(in crate::query) fn says_something(hover: &Hover) -> bool {
    let says = |marked: &lsp_types::MarkedString| match marked {
        lsp_types::MarkedString::String(text) => !text.trim().is_empty(),
        lsp_types::MarkedString::LanguageString(code) => !code.value.trim().is_empty(),
    };
    match &hover.contents {
        lsp_types::HoverContents::Markup(markup) => !markup.value.trim().is_empty(),
        lsp_types::HoverContents::Scalar(marked) => says(marked),
        lsp_types::HoverContents::Array(marked) => marked.iter().any(says),
    }
}

pub(super) fn observed_type_union(observed: &[(ExpressionType, Option<Styles>)]) -> Option<String> {
    let types = observed
        .iter()
        .map(|(ty, _)| ty.type_name())
        .collect::<BTreeSet<_>>();
    (!types.is_empty()).then(|| types.into_iter().collect::<Vec<_>>().join(" | "))
}

pub(super) fn agreed_value(observed: &[(ExpressionType, Option<Styles>)]) -> Option<&Value> {
    let (ty, _) = observed.first()?;
    let val = ty.value()?;
    observed
        .iter()
        .all(|(ty, _)| ty.value() == Some(val))
        .then_some(val)
}

pub(super) fn described_export(sem: &Semantic<'_>, name: &str, val: &Value, span: Span) -> String {
    let docs = match val {
        Value::Func(func) => semantic::function_docs(func).map(str::to_owned),
        _ => None,
    }
    .or_else(|| sem.source_docs(span));
    let line = match val {
        Value::Func(func) => semantic::signature(name, func, None, &BTreeMap::new()).code(),
        _ => semantic::description(val, name),
    };
    let mut text = typc_fence(&line);
    if let Some(docs) = docs {
        text.push_str(SECTION_JOIN);
        text.push_str(&markdown::docs(&docs));
    }
    text
}

pub(in crate::query) fn typc_fence(code: &str) -> String {
    format!("```typc\n{code}\n```")
}

pub(super) fn sections_hover(
    sections: Vec<String>,
    range: Option<lsp_types::Range>,
) -> Option<Hover> {
    (!sections.is_empty()).then(|| markdown_hover(sections.join(SECTION_JOIN), range))
}

pub(super) fn parameter_section(sig: &Signature) -> Option<String> {
    let mut text = String::new();
    for (heading, kind, group) in [
        ("Positional Parameters", "positional", sig.positional()),
        ("Rest Parameters", "spread right", sig.rest()),
        ("Named Parameters", "named", sig.named()),
    ] {
        if group.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("# ");
        text.push_str(heading);
        for (idx, param) in group.iter().enumerate() {
            text.push_str("\n\n## ");
            text.push_str(&param.name);
            if idx > 0 {
                text.push_str(" (");
                text.push_str(kind);
                text.push(')');
            }
            text.push_str("\n\n");
            text.push_str(&typc_fence(&format!("type: {}", param.ty_spelling())));
            if let Some(docs) = &param.docs {
                text.push_str("\n\n");
                text.push_str(&markdown::docs(docs));
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

fn shape_section(heading: &str, fields: &[String]) -> String {
    let mut section = format!("### {heading}");
    for field in fields.iter().take(SHAPE_FIELD_LIMIT) {
        section.push_str("\n- ");
        section.push_str(field);
    }
    if fields.len() > SHAPE_FIELD_LIMIT {
        section.push_str(&format!(
            "\n… {} more fields",
            fields.len() - SHAPE_FIELD_LIMIT
        ));
    }
    section
}

pub(in crate::query) fn shown_routes<'a>(routes: impl Iterator<Item = &'a str>) -> String {
    let (mut names, remaining) = listed(routes, crate::routes::NAMED_PAGES);
    if remaining > 0 {
        names.push_str(&format!(" and {remaining} more"));
    }
    names
}

pub(super) fn function_sections(
    name: &str,
    named: bool,
    func: &Func,
    docs: Option<&Documentation>,
    observed_params: &BTreeMap<String, String>,
) -> Vec<String> {
    let sig = semantic::signature(name, func, docs, observed_params);
    let mut sections = Vec::new();
    if named {
        sections.push(typc_fence(&sig.code()));
    }
    if let Some(docs) = docs
        && !docs.summary.is_empty()
    {
        sections.push(markdown::docs(&docs.summary));
    }
    sections.extend(parameter_section(&sig));
    sections
}

pub(super) fn declaration_sections(
    docs: Option<String>,
    declared: Option<&DeclaredOutput>,
) -> Vec<String> {
    let mut sections = Vec::new();
    if let Some(declared) = declared {
        sections.push(shape_section("Declared fields", &declared.lines));
    }
    if let Some(summary) = docs
        .map(|d| Documentation::parse(&d))
        .map(|d| d.summary)
        .filter(|s| !s.is_empty())
    {
        sections.push(markdown::docs(&summary));
    }
    if let Some(declared) = declared
        && let Some(desc) = &declared.documentation
    {
        sections.push(markdown::docs(desc));
    }
    sections
}
