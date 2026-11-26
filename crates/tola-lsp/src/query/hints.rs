//! Positional parameter hints for Tola's own functions, and the current source's checked routes.

use std::ops::Range as ByteRange;

use anyhow::Result;
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, InlayHintTooltip, Position, Range};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::SiteConfig;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::package::PackageSpec;
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind, ast};

use super::expression::single_value;
use super::semantic::Semantic;
use crate::position;
use crate::published::PublishedPackage;

/// Clients with lenses show these checked routes there instead of as annotations.
pub(super) fn route_hints(
    compilation: &SourceCompilation,
    config: &SiteConfig,
    source: &Source,
    viewport: Range,
    cancellation: &BuildCancellation,
) -> Result<Vec<InlayHint>> {
    if viewport.start.line > 0 {
        return Ok(Vec::new());
    }
    let routes = crate::routes::site_routes(compilation, config, source.id(), cancellation)?;
    let pages = crate::routes::named_pages(&routes);
    if pages.named.is_empty() {
        return Ok(Vec::new());
    }
    let Some(at) = opening_line_end(source) else {
        return Ok(Vec::new());
    };
    let mut hints: Vec<InlayHint> = pages
        .named
        .iter()
        .map(|route| page_hint(route.route.clone(), route.url.clone(), at))
        .collect();
    if pages.remaining > 0 {
        hints.push(page_hint(format!("+{} pages", pages.remaining), None, at));
    }
    Ok(hints)
}

/// Where the line that opens a source ends, which is where its pages are annotated.
fn opening_line_end(source: &Source) -> Option<Position> {
    let text = source.text();
    let opening = text.find(['\n', '\r']).map_or(text, |end| &text[..end]);
    position::checked_position(0, position::utf16_len(opening))
}

fn page_hint(label: String, url: Option<String>, at: Position) -> InlayHint {
    InlayHint {
        position: at,
        label: InlayHintLabel::String(label),
        tooltip: url
            .map(|url| InlayHintTooltip::String(format!("Page from current source: {url}"))),
        padding_left: Some(true),
        kind: None,
        text_edits: None,
        padding_right: None,
        data: None,
    }
}

/// The parameter hints for the calls inside `range`.
pub(super) fn hints(
    compilation: &SourceCompilation,
    published: &[PublishedPackage],
    source: &Source,
    range: Range,
    cancellation: &BuildCancellation,
) -> Result<Vec<InlayHint>> {
    let lines = source.lines();
    let Ok(start) = position::byte_offset(lines, range.start) else {
        return Ok(Vec::new());
    };
    // A viewport can end past the document, which asks for hints to its end.
    let end = position::byte_offset(lines, range.end)
        .unwrap_or_else(|_| source.text().len())
        .min(source.text().len());
    let mut semantics = Semantic::new(compilation, cancellation);
    let mut hints = Vec::new();
    walk(
        &LinkedNode::new(source.root()),
        source,
        &(start..end),
        &mut semantics,
        &mut hints,
    )?;
    package_version_hints(source, start..end, published, &mut hints);
    Ok(hints)
}

fn walk(
    node: &LinkedNode<'_>,
    source: &Source,
    range: &ByteRange<usize>,
    semantics: &mut Semantic<'_>,
    hints: &mut Vec<InlayHint>,
) -> Result<()> {
    if overlaps(node, range) {
        hints.extend(call_hints(node, source, semantics)?);
    }
    for child in node.children() {
        walk(&child, source, range, semantics, hints)?;
    }
    Ok(())
}

/// Whether one node's source range touches the viewport.
fn overlaps(node: &LinkedNode<'_>, range: &ByteRange<usize>) -> bool {
    node.range().start <= range.end && range.start <= node.range().end
}

/// The hints one call names: each positional argument takes the parameter at its own index.
fn call_hints(
    call: &LinkedNode<'_>,
    source: &Source,
    semantics: &mut Semantic<'_>,
) -> Result<Vec<InlayHint>> {
    let Some(func_call) = call.cast::<ast::FuncCall>() else {
        return Ok(Vec::new());
    };
    let Some(callee) = call.find(func_call.callee().span()) else {
        return Ok(Vec::new());
    };
    let Some(Value::Func(func)) = single_value(
        semantics
            .known_values(&callee)?
            .into_iter()
            .map(|(value, _)| value)
            .collect(),
    ) else {
        return Ok(Vec::new());
    };
    // A content block is the call's body rather than a value of the parameter it sits in, and a
    // named argument names itself, so only the values left take a parameter by position.
    let values: Vec<ast::Expr<'_>> = func_call
        .args()
        .items()
        .filter_map(|item| match item {
            ast::Arg::Pos(value) => Some(value),
            ast::Arg::Named(_) | ast::Arg::Spread(_) => None,
        })
        .filter(|value| !matches!(value, ast::Expr::ContentBlock(_)))
        .collect();
    // A standard-library call documents itself; only Tola's own functions keep their hints.
    if values.len() == 1 && !semantics.tola_origin(&callee)? {
        return Ok(Vec::new());
    }
    let parameters = func.params().collect::<Vec<_>>();
    let mut hints = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        let Some(parameter) = parameters.get(index) else {
            break;
        };
        let Some(name) = parameter.name() else {
            continue;
        };
        if parameter.variadic() {
            // A variadic parameter names the first of its arguments; the rest are its own.
            if let Some(hint) = hint(source, call, value, format!("..{name}:"))? {
                hints.push(hint);
            }
            break;
        }
        if let Some(hint) = hint(source, call, value, format!("{name}:"))? {
            hints.push(hint);
        }
    }
    Ok(hints)
}

/// The hint one argument takes, or `None` when its text is not addressable.
fn hint(
    source: &Source,
    call: &LinkedNode<'_>,
    value: ast::Expr<'_>,
    label: String,
) -> Result<Option<InlayHint>> {
    let Some(argument) = call.find(value.span()) else {
        return Ok(None);
    };
    let Some(position) = position::utf16_range(source.lines(), argument.range()) else {
        return Ok(None);
    };
    Ok(Some(InlayHint {
        position: position.start,
        label: InlayHintLabel::String(label),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(true),
        data: None,
    }))
}

/// The version status of every published package a string inside `range` imports.
fn package_version_hints(
    source: &Source,
    range: ByteRange<usize>,
    published: &[PublishedPackage],
    hints: &mut Vec<InlayHint>,
) {
    if published.is_empty() {
        return;
    }
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        if overlaps(&node, &range)
            && node.kind() == SyntaxKind::Str
            && matches!(
                node.parent_kind(),
                Some(SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude)
            )
            && let Some(literal) = node.cast::<ast::Str>()
            && let Ok(spec) = literal.get().parse::<PackageSpec>()
            && let Some(label) = version_label(published, &spec)
            && let Some(end) =
                position::utf16_range(source.lines(), node.range().end..node.range().end)
        {
            hints.push(InlayHint {
                position: end.start,
                label: InlayHintLabel::String(label),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: Some(true),
                padding_right: None,
                data: None,
            });
        }
        for child in node.children() {
            stack.push(child);
        }
    }
}

/// What the published index says about one package specification: the label its import states.
fn version_label(published: &[PublishedPackage], spec: &PackageSpec) -> Option<String> {
    // Only `@preview` is published; another namespace names no index entry to speak about.
    if spec.namespace != "preview" {
        return None;
    }
    let versions = super::package::published_versions(published, &spec.namespace, &spec.name)
        .unwrap_or_default();
    if !versions.contains(&spec.version) {
        return Some("version not found".to_owned());
    }
    // `versions` has the import's own version, so a last entry always exists.
    match versions.last() {
        Some(latest) if *latest != spec.version => Some(format!("→ {latest} available")),
        _ => Some("√ latest".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::*;

    #[test]
    fn import_version_hints() {
        let index = [
            published("cetz", "0.3.1"),
            published("cetz", "0.3.2"),
            published("tablex", "0.0.4"),
        ];
        let src = Source::detached(
            "#import \"@preview/cetz:0.3.1\": canvas\n\
             #import \"@preview/cetz:0.3.2\": canvas\n\
             #import \"@preview/tablex:0.0.3\": table\n\
             #import \"@preview/absent:1.0.0\": nothing\n\
             #import \"@local/pkg:1.0.0\": local\n\
             #let text = \"@preview/cetz:0.3.1\"\n",
        );
        let mut hints = Vec::new();
        package_version_hints(&src, 0..src.text().len(), &index, &mut hints);
        let by_line: std::collections::BTreeMap<u32, String> = hints
            .iter()
            .map(|h| {
                (
                    h.position.line,
                    hint_labels(std::slice::from_ref(h)).pop().unwrap(),
                )
            })
            .collect();
        assert_eq!(by_line.len(), 4, "{hints:?}");
        assert!(by_line[&0].contains("0.3.2"), "{:?}", by_line[&0]);
        assert!(!by_line[&1].contains("available"), "{:?}", by_line[&1]);
        assert!(by_line[&2].contains("not found"), "{:?}", by_line[&2]);
        assert!(by_line[&3].contains("not found"), "{:?}", by_line[&3]);
        assert!(!by_line.contains_key(&4));
        assert!(!by_line.contains_key(&5));
    }

    #[test]
    fn version_hints_respect_viewport() {
        let index = [published("cetz", "0.3.2")];
        let src = Source::detached("#import \"@preview/cetz:0.3.1\": canvas\n");
        let mut inside = Vec::new();
        package_version_hints(&src, 0..src.text().len(), &index, &mut inside);
        assert_eq!(inside.len(), 1, "{inside:?}");
        let mut outside = Vec::new();
        package_version_hints(&src, 0..0, &index, &mut outside);
        assert!(outside.is_empty(), "{outside:?}");
    }

    #[test]
    fn route_hints_fallback_when_no_lenses() {
        let mut site = QuerySession::two_documents();
        site.capabilities = lsp_types::ClientCapabilities {
            text_document: Some(lsp_types::TextDocumentClientCapabilities {
                inlay_hint: Some(lsp_types::InlayHintClientCapabilities::default()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            hint_labels(&site.inlay_hints("The document the site realizes.|\n")),
            ["/a.html", "/b.html"]
        );

        site.capabilities = announcing_client();
        assert!(
            site.inlay_hints("The document the site realizes.|\n")
                .is_empty()
        );
    }

    #[test]
    fn ambiguous_callees_omit_hints() {
        for (expr, expected) in [
            (
                "if false { calc.pow } else { calc.rem }",
                Vec::<&str>::new(),
            ),
            (
                "if false { calc.pow } else { calc.pow }",
                vec!["base:", "exponent:"],
            ),
        ] {
            let mut site = QuerySession::with_program(crate::query::tests::FAILED_PROGRAM);
            let hints = site.inlay_hints(&format!("#panic(\"stop\")\n#let f = {expr}\n#f(1, 2)|"));
            assert_eq!(hint_labels(&hints), expected, "{expr}: {hints:?}");
        }
    }

    #[test]
    fn parameter_hints_positional_and_variadic() {
        let cases: [(&str, &[&str], Option<Position>); 4] = [
            (
                "#(calc.max(1, 2, 3))|\n",
                &["..values:"],
                Some(Position::new(0, 11)),
            ),
            (
                "#(calc.pow(2, 3))|\n",
                &["base:", "exponent:"],
                Some(Position::new(0, 11)),
            ),
            ("#align(center)[Body]|\n", &[], None),
            (
                "#import \"@tola/address:0.0.0\": join-route\n#let build(segments) = join-route(|segments)\n",
                &["segments:"],
                Some(Position::new(1, 34)),
            ),
        ];
        for (src, expected, pos) in cases {
            let mut site = QuerySession::new();
            let hints = site.inlay_hints(src);
            assert_eq!(hint_labels(&hints), expected, "{src:?}");
            assert_eq!(hints.first().map(|h| h.position), pos, "{src:?}: {hints:?}");
        }
    }
}
