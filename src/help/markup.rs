//! Declarative `.typ` documentation parser built directly on `typst::syntax`.
//!
//! No Typst evaluator is invoked: each documentation file begins with one
//! document-level `#show:` template rule (`overview`, `config`, `package`, or
//! `script`) so page prose stays flat at column 0, followed by ordinary calls
//! for individual entries (`#key`, `#export`, `#method`, `#command`, `#example`).
//!
//! TODO: This restricted, syntax-only specialization currently serves offline
//! help pages only. As Typst evolves, revisit whether a similar declarative or
//! segmented syntax-level path can be exposed and generalized to accelerate
//! site compilation and incremental builds.

use std::collections::BTreeMap;
use std::ops::Range;

use anyhow::{Result, bail, ensure};
use serde::{Serialize, Serializer};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, SyntaxKind, ast};

use super::model::Example;

/// Whether a documentation example should be compiled by the example test suite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExampleCheck {
    Default,
    Site,
}

/// Validated Typst markup: serialized verbatim as a Typst string in JSON, and
/// rendered directly from its parsed blocks for terminal, Markdown, and search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Markup {
    pub(crate) typst: String,
    pub(crate) blocks: Vec<Block>,
}

impl Markup {
    pub(crate) fn is_empty(&self) -> bool {
        self.typst.is_empty()
    }

    pub(crate) fn parse(source: &str) -> Result<Self> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        let parsed = Source::detached(trimmed);
        ensure_no_syntax_errors(&parsed, "inline markup")?;
        let children: Vec<LinkedNode<'_>> = LinkedNode::new(parsed.root()).children().collect();
        let (blocks, _) = parse_markup_slice(&parsed, &children, false)?;
        Ok(Self {
            typst: trimmed.to_owned(),
            blocks,
        })
    }

    /// Plain text for search indexing and summaries.
    pub(crate) fn plain_text(&self) -> String {
        let mut parts = Vec::new();
        collect_plain_blocks(&self.blocks, &mut parts);
        parts.join(" ")
    }

    /// Every `#ref("target")` target mentioned inside this markup.
    #[allow(dead_code)]
    pub(crate) fn refs(&self) -> Vec<&str> {
        let mut out = Vec::new();
        collect_refs_blocks(&self.blocks, &mut out);
        out
    }
}

impl Serialize for Markup {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.typst)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Code {
        lang: Option<String>,
        source: String,
    },
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emph(Vec<Inline>),
    Link {
        url: String,
        label: Option<Vec<Inline>>,
    },
    Ref {
        target: String,
        label: Option<Vec<Inline>>,
    },
}

#[derive(Debug)]
pub(crate) struct OverviewDoc {
    pub body: Markup,
}

pub(crate) struct ConfigDoc {
    pub summary: Markup,
    pub body: Markup,
    pub examples: Vec<Example>,
    pub related: Vec<String>,
    pub keys: BTreeMap<String, KeyDoc>,
}

pub(crate) struct KeyDoc {
    pub body: Markup,
    pub examples: Vec<Example>,
}

pub(crate) struct PackageDoc {
    pub summary: Markup,
    pub body: Markup,
    pub examples: Vec<Example>,
    pub related: Vec<String>,
    pub exports: BTreeMap<String, ExportDoc>,
}

pub(crate) struct ExportDoc {
    pub summary: Markup,
    pub body: Markup,
    pub params: BTreeMap<String, Markup>,
    pub methods: BTreeMap<String, MethodDoc>,
    pub examples: Vec<Example>,
    pub related: Vec<String>,
}

pub(crate) struct MethodDoc {
    pub summary: Markup,
    pub body: Markup,
    pub params: BTreeMap<String, Markup>,
    pub examples: Vec<Example>,
}

pub(crate) struct ScriptDoc {
    pub summary: Markup,
    pub body: Markup,
    pub commands: BTreeMap<String, CommandDoc>,
}

pub(crate) struct CommandDoc {
    pub summary: Markup,
    pub body: Markup,
    pub params: BTreeMap<String, Markup>,
    pub examples: Vec<Example>,
}

struct DocumentHeader<'a> {
    kind: String,
    call: Option<LinkedNode<'a>>,
    rest: Vec<LinkedNode<'a>>,
    rest_range: Range<usize>,
}

/// Extract the single leading `#show: <kind>(...)` document header and return
/// all remaining top-level nodes after it.
fn parse_document_header<'a>(source: &'a Source, label: &str) -> Result<DocumentHeader<'a>> {
    let root = LinkedNode::new(source.root());
    let children: Vec<LinkedNode<'a>> = root.children().collect();
    let mut header_idx = None;
    for (index, child) in children.iter().enumerate() {
        match child.kind() {
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment
            | SyntaxKind::Hash => {}
            SyntaxKind::ShowRule => {
                header_idx = Some(index);
                break;
            }
            other => bail!(
                "{label}: document must begin with `#show: ...`, found {:?} ({:?})",
                other,
                &source.text()[child.range()]
            ),
        }
    }
    let idx = header_idx
        .ok_or_else(|| anyhow::anyhow!("{label}: missing leading `#show: ...` document header"))?;
    let rule = children[idx]
        .cast::<ast::ShowRule>()
        .ok_or_else(|| anyhow::anyhow!("{label}: invalid `#show` rule"))?;
    ensure!(
        rule.selector().is_none(),
        "{label}: `#show` with a selector is not allowed in help docs"
    );
    let (kind, call) = match rule.transform() {
        ast::Expr::Ident(ident) => (ident.as_str().to_owned(), None),
        ast::Expr::FuncCall(call) => {
            let node = source
                .find(call.span())
                .ok_or_else(|| anyhow::anyhow!("{label}: missing show call node"))?;
            (callee_name(&node)?, Some(node))
        }
        _ => bail!("{label}: `#show:` must name `overview`, `config`, `package`, or `script`"),
    };

    let mut start = idx + 1;
    if let Some(next) = children.get(start)
        && next.kind() == SyntaxKind::Semicolon
    {
        start += 1;
    }
    let rest = children[start..].to_vec();
    for child in &rest {
        ensure!(
            child.kind() != SyntaxKind::ShowRule,
            "{label}: only one document-level `#show:` header is allowed per file"
        );
    }
    let rest_range = match (rest.first(), rest.last()) {
        (Some(first), Some(last)) => first.range().start..last.range().end,
        _ => 0..0,
    };
    Ok(DocumentHeader {
        kind,
        call,
        rest,
        rest_range,
    })
}

/// Partition top-level nodes into page body nodes (everything not matching
type ExtractedItems<'a> = Vec<(String, LinkedNode<'a>)>;

/// `item_calls`) and the extracted top-level item calls.
fn split_body_and_items<'a>(
    source: &'a Source,
    nodes: &[LinkedNode<'a>],
    full_range: Range<usize>,
    item_calls: &[&str],
    allow_examples: bool,
) -> Result<(Markup, Vec<Example>, ExtractedItems<'a>)> {
    let mut body_nodes: Vec<LinkedNode<'a>> = Vec::new();
    let mut skip_ranges: Vec<Range<usize>> = Vec::new();
    let mut items: Vec<(String, LinkedNode<'a>)> = Vec::new();

    let mut i = 0;
    while i < nodes.len() {
        let child = &nodes[i];
        if child.kind() == SyntaxKind::Hash
            && let Some(next) = nodes.get(i + 1)
            && next.kind() == SyntaxKind::FuncCall
            && let Ok(name) = callee_name(next)
            && item_calls.contains(&name.as_str())
        {
            let start = child.range().start;
            let mut end = next.range().end;
            let mut advance = 2;
            if let Some(semi) = nodes.get(i + 2)
                && semi.kind() == SyntaxKind::Semicolon
            {
                end = semi.range().end;
                advance = 3;
            }
            skip_ranges.push(start..end);
            items.push((name, next.clone()));
            i += advance;
            continue;
        }
        if child.kind() == SyntaxKind::FuncCall
            && let Ok(name) = callee_name(child)
            && item_calls.contains(&name.as_str())
        {
            let start = child.range().start;
            let mut end = child.range().end;
            let mut advance = 1;
            if let Some(semi) = nodes.get(i + 1)
                && semi.kind() == SyntaxKind::Semicolon
            {
                end = semi.range().end;
                advance = 2;
            }
            skip_ranges.push(start..end);
            items.push((name, child.clone()));
            i += advance;
            continue;
        }
        body_nodes.push(child.clone());
        i += 1;
    }

    if allow_examples {
        for (index, child) in body_nodes.iter().enumerate() {
            if child.kind() == SyntaxKind::FuncCall
                && callee_name(child).ok().as_deref() == Some("example")
            {
                let start = if index > 0 && body_nodes[index - 1].kind() == SyntaxKind::Hash {
                    body_nodes[index - 1].range().start
                } else {
                    child.range().start
                };
                let mut end = child.range().end;
                if let Some(next) = body_nodes.get(index + 1)
                    && next.kind() == SyntaxKind::Semicolon
                {
                    end = next.range().end;
                }
                skip_ranges.push(start..end);
            }
        }
    }
    skip_ranges.sort_by_key(|range| range.start);

    let (blocks, examples) = parse_markup_slice(source, &body_nodes, allow_examples)?;
    let typst = slice_excluding(source.text(), full_range, &skip_ranges);
    Ok((Markup { typst, blocks }, examples, items))
}

pub(crate) fn parse_overview(label: &str, text: &str) -> Result<OverviewDoc> {
    let source = Source::detached(text);
    ensure_no_syntax_errors(&source, label)?;
    let header = parse_document_header(&source, label)?;
    ensure!(
        header.kind == "overview",
        "{label}: expected `#show: overview`, found `#show: {}`",
        header.kind
    );
    if let Some(call) = &header.call {
        let args = parse_args(&source, call, label)?;
        ensure!(
            args.positional.is_empty() && args.named.is_empty(),
            "{label}: `overview` accepts no arguments"
        );
    }
    let (body, _, _) = split_body_and_items(&source, &header.rest, header.rest_range, &[], false)?;
    Ok(OverviewDoc { body })
}

pub(crate) fn parse_config(label: &str, text: &str) -> Result<ConfigDoc> {
    let source = Source::detached(text);
    ensure_no_syntax_errors(&source, label)?;
    let header = parse_document_header(&source, label)?;
    ensure!(
        header.kind == "config",
        "{label}: expected `#show: config(...)`, found `#show: {}`",
        header.kind
    );
    let call = header
        .call
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{label}: `#show: config(...)` requires arguments"))?;
    let mut args = parse_args(&source, call, label)?;
    ensure!(
        args.positional.is_empty(),
        "{label}: `#show: config(...)` takes named arguments only"
    );
    let summary = take_required_markup(&source, &mut args.named, "summary", label)?;
    let related = take_optional_string_list(&mut args.named, "related", label)?;
    ensure_no_extra_named(&args.named, "config", label)?;

    let (body, examples, items) =
        split_body_and_items(&source, &header.rest, header.rest_range, &["key"], true)?;
    let mut keys = BTreeMap::new();
    for (_, item_call) in items {
        let item_args = parse_args(&source, &item_call, label)?;
        ensure_no_extra_named(&item_args.named, "key", label)?;
        ensure!(
            item_args.positional.len() == 2,
            "{label}: `#key` requires `(\"name\")[...]`"
        );
        let key_name = expect_string(&item_args.positional[0], "key name", label)?;
        let (key_body, key_examples) =
            parse_content_block_with_examples(&source, &item_args.positional[1], label)?;
        ensure!(
            keys.insert(
                key_name.clone(),
                KeyDoc {
                    body: key_body,
                    examples: key_examples,
                },
            )
            .is_none(),
            "{label}: duplicate `#key(\"{key_name}\")`"
        );
    }
    Ok(ConfigDoc {
        summary,
        body,
        examples,
        related,
        keys,
    })
}

pub(crate) fn parse_package(label: &str, text: &str) -> Result<PackageDoc> {
    let source = Source::detached(text);
    ensure_no_syntax_errors(&source, label)?;
    let header = parse_document_header(&source, label)?;
    ensure!(
        header.kind == "package",
        "{label}: expected `#show: package(...)`, found `#show: {}`",
        header.kind
    );
    let call = header
        .call
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{label}: `#show: package(...)` requires arguments"))?;
    let mut args = parse_args(&source, call, label)?;
    ensure!(
        args.positional.is_empty(),
        "{label}: `#show: package(...)` takes named arguments only"
    );
    let summary = take_required_markup(&source, &mut args.named, "summary", label)?;
    let related = take_optional_string_list(&mut args.named, "related", label)?;
    ensure_no_extra_named(&args.named, "package", label)?;

    let (body, examples, items) = split_body_and_items(
        &source,
        &header.rest,
        header.rest_range,
        &["export", "method"],
        true,
    )?;
    let mut exports: BTreeMap<String, ExportDoc> = BTreeMap::new();
    let mut current_export: Option<String> = None;

    for (kind, item_call) in items {
        match kind.as_str() {
            "export" => {
                let mut item_args = parse_args(&source, &item_call, label)?;
                let summary =
                    take_required_markup(&source, &mut item_args.named, "summary", label)?;
                let params =
                    take_optional_markup_dict(&source, &mut item_args.named, "params", label)?;
                let related = take_optional_string_list(&mut item_args.named, "related", label)?;
                ensure_no_extra_named(&item_args.named, "export", label)?;
                ensure!(
                    matches!(item_args.positional.len(), 1 | 2),
                    "{label}: `#export` requires `(\"name\", ...)` with optional `[...]` body"
                );
                let export_name = expect_string(&item_args.positional[0], "export name", label)?;
                let (export_body, export_examples) =
                    if let Some(content) = item_args.positional.get(1) {
                        parse_content_block_with_examples(&source, content, label)?
                    } else {
                        (Markup::default(), Vec::new())
                    };
                ensure!(
                    exports
                        .insert(
                            export_name.clone(),
                            ExportDoc {
                                summary,
                                body: export_body,
                                params,
                                methods: BTreeMap::new(),
                                examples: export_examples,
                                related,
                            },
                        )
                        .is_none(),
                    "{label}: duplicate `#export(\"{export_name}\")`"
                );
                current_export = Some(export_name);
            }
            "method" => {
                let owner = current_export.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("{label}: `#method(...)` must follow an `#export(...)`")
                })?;
                let mut item_args = parse_args(&source, &item_call, label)?;
                let summary =
                    take_optional_markup(&source, &mut item_args.named, "summary", label)?
                        .unwrap_or_default();
                let params =
                    take_optional_markup_dict(&source, &mut item_args.named, "params", label)?;
                ensure_no_extra_named(&item_args.named, "method", label)?;
                ensure!(
                    matches!(item_args.positional.len(), 1 | 2),
                    "{label}: `#method` requires `(\"name\", ...)` with optional `[...]` body"
                );
                let method_name = expect_string(&item_args.positional[0], "method name", label)?;
                let (method_body, method_examples) =
                    if let Some(content) = item_args.positional.get(1) {
                        parse_content_block_with_examples(&source, content, label)?
                    } else {
                        (Markup::default(), Vec::new())
                    };
                let export_doc = exports.get_mut(owner).expect("current export exists");
                ensure!(
                    export_doc
                        .methods
                        .insert(
                            method_name.clone(),
                            MethodDoc {
                                summary,
                                body: method_body,
                                params,
                                examples: method_examples,
                            },
                        )
                        .is_none(),
                    "{label}: duplicate `#method(\"{method_name}\")` under `{owner}`"
                );
            }
            _ => unreachable!(),
        }
    }

    Ok(PackageDoc {
        summary,
        body,
        examples,
        related,
        exports,
    })
}

pub(crate) fn parse_script(label: &str, text: &str) -> Result<ScriptDoc> {
    let source = Source::detached(text);
    ensure_no_syntax_errors(&source, label)?;
    let header = parse_document_header(&source, label)?;
    ensure!(
        header.kind == "script",
        "{label}: expected `#show: script(...)`, found `#show: {}`",
        header.kind
    );
    let call = header
        .call
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{label}: `#show: script(...)` requires arguments"))?;
    let mut args = parse_args(&source, call, label)?;
    ensure!(
        args.positional.is_empty(),
        "{label}: `#show: script(...)` takes named arguments only"
    );
    let summary = take_required_markup(&source, &mut args.named, "summary", label)?;
    ensure_no_extra_named(&args.named, "script", label)?;

    let (body, _, items) = split_body_and_items(
        &source,
        &header.rest,
        header.rest_range,
        &["command"],
        false,
    )?;
    let mut commands = BTreeMap::new();
    for (_, item_call) in items {
        let mut item_args = parse_args(&source, &item_call, label)?;
        let summary = take_required_markup(&source, &mut item_args.named, "summary", label)?;
        let params = take_optional_markup_dict(&source, &mut item_args.named, "params", label)?;
        ensure_no_extra_named(&item_args.named, "command", label)?;
        ensure!(
            matches!(item_args.positional.len(), 1 | 2),
            "{label}: `#command` requires `(\"name\", ...)` with optional `[...]` body"
        );
        let command_name = expect_string(&item_args.positional[0], "command name", label)?;
        let (cmd_body, cmd_examples) = if let Some(content) = item_args.positional.get(1) {
            parse_content_block_with_examples(&source, content, label)?
        } else {
            (Markup::default(), Vec::new())
        };
        ensure!(
            commands
                .insert(
                    command_name.clone(),
                    CommandDoc {
                        summary,
                        body: cmd_body,
                        params,
                        examples: cmd_examples,
                    },
                )
                .is_none(),
            "{label}: duplicate `#command(\"{command_name}\")`"
        );
    }

    Ok(ScriptDoc {
        summary,
        body,
        commands,
    })
}

fn ensure_no_syntax_errors(source: &Source, label: &str) -> Result<()> {
    let (errors, _) = source.root().errors_and_warnings();
    if errors.is_empty() {
        return Ok(());
    }
    let messages = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    bail!("{label}: Typst syntax error: {messages}")
}

fn callee_name(call: &LinkedNode<'_>) -> Result<String> {
    let Some(func) = call.cast::<ast::FuncCall>() else {
        bail!("expected function call");
    };
    match func.callee() {
        ast::Expr::Ident(ident) => Ok(ident.as_str().to_owned()),
        ast::Expr::FieldAccess(access) => {
            if let ast::Expr::Ident(target) = access.target() {
                Ok(format!("{}.{}", target.as_str(), access.field().as_str()))
            } else {
                bail!("unsupported complex callee")
            }
        }
        _ => bail!("unsupported function callee"),
    }
}

struct ParsedArgs<'a> {
    positional: Vec<LinkedNode<'a>>,
    named: BTreeMap<String, LinkedNode<'a>>,
}

fn parse_args<'a>(
    source: &'a Source,
    call: &LinkedNode<'a>,
    label: &str,
) -> Result<ParsedArgs<'a>> {
    let Some(func) = call.cast::<ast::FuncCall>() else {
        bail!("{label}: expected function call");
    };
    let mut positional = Vec::new();
    let mut named = BTreeMap::new();
    for arg in func.args().items() {
        match arg {
            ast::Arg::Pos(expr) => {
                let node = source
                    .find(expr.span())
                    .ok_or_else(|| anyhow::anyhow!("{label}: missing positional arg node"))?;
                positional.push(node);
            }
            ast::Arg::Named(pair) => {
                let key = pair.name().as_str().to_owned();
                let node = source
                    .find(pair.expr().span())
                    .ok_or_else(|| anyhow::anyhow!("{label}: missing named arg `{key}`"))?;
                ensure!(
                    named.insert(key.clone(), node).is_none(),
                    "{label}: duplicate named argument `{key}`"
                );
            }
            ast::Arg::Spread(_) => bail!("{label}: spread arguments are not allowed in help docs"),
        }
    }
    Ok(ParsedArgs { positional, named })
}

fn ensure_no_extra_named(
    named: &BTreeMap<String, LinkedNode<'_>>,
    call: &str,
    label: &str,
) -> Result<()> {
    if let Some(key) = named.keys().next() {
        bail!("{label}: unexpected argument `{key}` in `{call}`");
    }
    Ok(())
}

fn expect_string(node: &LinkedNode<'_>, context: &str, label: &str) -> Result<String> {
    let Some(literal) = node.cast::<ast::Str>() else {
        bail!("{label}: expected string literal for {context}");
    };
    Ok(literal.get().to_string())
}

fn take_required_markup(
    source: &Source,
    named: &mut BTreeMap<String, LinkedNode<'_>>,
    key: &str,
    label: &str,
) -> Result<Markup> {
    let node = named
        .remove(key)
        .ok_or_else(|| anyhow::anyhow!("{label}: missing required `{key}: [...]`"))?;
    parse_content_block(source, &node, label)
}

fn take_optional_markup(
    source: &Source,
    named: &mut BTreeMap<String, LinkedNode<'_>>,
    key: &str,
    label: &str,
) -> Result<Option<Markup>> {
    let Some(node) = named.remove(key) else {
        return Ok(None);
    };
    Ok(Some(parse_content_block(source, &node, label)?))
}

fn take_optional_string_list(
    named: &mut BTreeMap<String, LinkedNode<'_>>,
    key: &str,
    label: &str,
) -> Result<Vec<String>> {
    let Some(node) = named.remove(key) else {
        return Ok(Vec::new());
    };
    let Some(array) = node.cast::<ast::Array>() else {
        bail!("{label}: `{key}` must be an array of target strings");
    };
    let mut values = Vec::new();
    for item in array.items() {
        let ast::ArrayItem::Pos(ast::Expr::Str(literal)) = item else {
            bail!("{label}: `{key}` items must be string literals");
        };
        values.push(literal.get().to_string());
    }
    Ok(values)
}

fn take_optional_markup_dict(
    source: &Source,
    named: &mut BTreeMap<String, LinkedNode<'_>>,
    key: &str,
    label: &str,
) -> Result<BTreeMap<String, Markup>> {
    let Some(node) = named.remove(key) else {
        return Ok(BTreeMap::new());
    };
    let Some(dict) = node.cast::<ast::Dict>() else {
        bail!("{label}: `{key}` must be a dictionary of parameter descriptions");
    };
    let mut map = BTreeMap::new();
    for item in dict.items() {
        let (param_name, expr) = match item {
            ast::DictItem::Named(pair) => (pair.name().as_str().to_owned(), pair.expr()),
            ast::DictItem::Keyed(pair) => match pair.key() {
                ast::Expr::Str(literal) => (literal.get().to_string(), pair.expr()),
                _ => bail!("{label}: `{key}` dictionary keys must be identifiers or strings"),
            },
            ast::DictItem::Spread(_) => bail!("{label}: spread is not allowed in `{key}`"),
        };
        let value = source
            .find(expr.span())
            .ok_or_else(|| anyhow::anyhow!("{label}: missing value for `{param_name}`"))?;
        let markup = parse_content_block(source, &value, label)?;
        ensure!(
            map.insert(param_name.clone(), markup).is_none(),
            "{label}: duplicate parameter `{param_name}` in `{key}`"
        );
    }
    Ok(map)
}

fn parse_content_block(source: &Source, node: &LinkedNode<'_>, label: &str) -> Result<Markup> {
    ensure!(
        node.kind() == SyntaxKind::ContentBlock,
        "{label}: expected content block `[...]`"
    );
    let markup_node = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Markup)
        .ok_or_else(|| anyhow::anyhow!("{label}: empty content block node"))?;
    let children: Vec<LinkedNode<'_>> = markup_node.children().collect();
    let (blocks, _) = parse_markup_slice(source, &children, false)?;
    let typst = slice_excluding(source.text(), markup_node.range(), &[]);
    Ok(Markup { typst, blocks })
}

fn parse_content_block_with_examples(
    source: &Source,
    node: &LinkedNode<'_>,
    label: &str,
) -> Result<(Markup, Vec<Example>)> {
    ensure!(
        node.kind() == SyntaxKind::ContentBlock,
        "{label}: expected content block `[...]`"
    );
    let markup_node = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Markup)
        .ok_or_else(|| anyhow::anyhow!("{label}: empty content block node"))?;
    let children: Vec<LinkedNode<'_>> = markup_node.children().collect();
    let (markup, examples, _) =
        split_body_and_items(source, &children, markup_node.range(), &[], true)?;
    Ok((markup, examples))
}

fn slice_excluding(text: &str, full_range: Range<usize>, skip_ranges: &[Range<usize>]) -> String {
    let mut raw = String::new();
    let mut cursor = full_range.start;
    for range in skip_ranges {
        if range.end <= cursor || range.start >= full_range.end {
            continue;
        }
        let start = range.start.max(cursor);
        let end = range.end.min(full_range.end);
        if cursor < start {
            raw.push_str(&text[cursor..start]);
        }
        cursor = end;
    }
    if cursor < full_range.end {
        raw.push_str(&text[cursor..full_range.end]);
    }
    dedent_and_trim(&raw)
}

fn dedent_and_trim(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let Some(first) = first else {
        return String::new();
    };
    let last = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or(first);
    let slice = &lines[first..=last];
    let shared_indent = slice
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let mut output = Vec::with_capacity(slice.len());
    let mut prev_blank = false;
    for line in slice {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() {
            if !prev_blank {
                output.push("");
            }
            prev_blank = true;
        } else {
            output.push(trimmed_end.get(shared_indent..).unwrap_or(trimmed_end));
            prev_blank = false;
        }
    }
    output.join("\n")
}

fn parse_markup_slice(
    source: &Source,
    children: &[LinkedNode<'_>],
    allow_examples: bool,
) -> Result<(Vec<Block>, Vec<Example>)> {
    let mut blocks = Vec::new();
    let mut examples = Vec::new();
    let mut paragraph: Vec<Inline> = Vec::new();
    let mut pending_list: Option<(bool, Vec<Vec<Block>>)> = None;

    let flush_paragraph = |paragraph: &mut Vec<Inline>, blocks: &mut Vec<Block>| {
        trim_inlines(paragraph);
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(paragraph)));
        }
    };
    let flush_list = |pending_list: &mut Option<(bool, Vec<Vec<Block>>)>,
                      blocks: &mut Vec<Block>| {
        if let Some((ordered, items)) = pending_list.take() {
            blocks.push(Block::List { ordered, items });
        }
    };

    for child in children {
        match child.kind() {
            SyntaxKind::LineComment
            | SyntaxKind::BlockComment
            | SyntaxKind::Hash
            | SyntaxKind::Semicolon => {}
            SyntaxKind::Parbreak => {
                flush_paragraph(&mut paragraph, &mut blocks);
                flush_list(&mut pending_list, &mut blocks);
            }
            SyntaxKind::Heading => {
                flush_paragraph(&mut paragraph, &mut blocks);
                flush_list(&mut pending_list, &mut blocks);
                let heading = child.cast::<ast::Heading>().expect("heading node");
                let level = heading.depth().get().min(u8::MAX as usize) as u8;
                let body = source
                    .find(heading.body().span())
                    .ok_or_else(|| anyhow::anyhow!("missing heading body"))?;
                let inlines = parse_inline_markup(source, body)?;
                blocks.push(Block::Heading { level, inlines });
            }
            SyntaxKind::ListItem => {
                flush_paragraph(&mut paragraph, &mut blocks);
                let item = child.cast::<ast::ListItem>().expect("list item");
                let body = source
                    .find(item.body().span())
                    .ok_or_else(|| anyhow::anyhow!("missing list item body"))?;
                let item_children: Vec<LinkedNode<'_>> = body.children().collect();
                let (item_blocks, _) = parse_markup_slice(source, &item_children, false)?;
                match &mut pending_list {
                    Some((false, items)) => items.push(item_blocks),
                    _ => {
                        flush_list(&mut pending_list, &mut blocks);
                        pending_list = Some((false, vec![item_blocks]));
                    }
                }
            }
            SyntaxKind::EnumItem => {
                flush_paragraph(&mut paragraph, &mut blocks);
                let item = child.cast::<ast::EnumItem>().expect("enum item");
                let body = source
                    .find(item.body().span())
                    .ok_or_else(|| anyhow::anyhow!("missing enum item body"))?;
                let item_children: Vec<LinkedNode<'_>> = body.children().collect();
                let (item_blocks, _) = parse_markup_slice(source, &item_children, false)?;
                match &mut pending_list {
                    Some((true, items)) => items.push(item_blocks),
                    _ => {
                        flush_list(&mut pending_list, &mut blocks);
                        pending_list = Some((true, vec![item_blocks]));
                    }
                }
            }
            SyntaxKind::Raw => {
                let raw = child.cast::<ast::Raw>().expect("raw node");
                if raw.block() {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    flush_list(&mut pending_list, &mut blocks);
                    blocks.push(Block::Code {
                        lang: raw.lang().map(|lang| lang.get().to_string()),
                        source: raw_text(raw),
                    });
                } else {
                    flush_list(&mut pending_list, &mut blocks);
                    paragraph.push(Inline::Code(raw_inline_text(raw)));
                }
            }
            SyntaxKind::FuncCall => {
                let name = callee_name(child)?;
                match name.as_str() {
                    "example" => {
                        ensure!(
                            allow_examples,
                            "`#example(...)` is not allowed in this context"
                        );
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_list(&mut pending_list, &mut blocks);
                        examples.push(parse_example(source, child)?);
                    }
                    "table" => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_list(&mut pending_list, &mut blocks);
                        blocks.push(parse_table(source, child)?);
                    }
                    "ref" | "link" => {
                        flush_list(&mut pending_list, &mut blocks);
                        paragraph.push(parse_inline_call(source, child, &name)?);
                    }
                    other => bail!("unsupported function call `#{other}` in help markup"),
                }
            }
            SyntaxKind::Space => {
                if !paragraph.is_empty() {
                    push_inline_text(&mut paragraph, " ");
                }
            }
            _ => {
                flush_list(&mut pending_list, &mut blocks);
                parse_inline_node(source, child.clone(), &mut paragraph)?;
            }
        }
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    flush_list(&mut pending_list, &mut blocks);
    Ok((blocks, examples))
}

fn parse_example(source: &Source, call: &LinkedNode<'_>) -> Result<Example> {
    let mut args = parse_args(source, call, "#example")?;
    let title = take_optional_markup(source, &mut args.named, "title", "#example")?;
    let check = match args.named.remove("check") {
        None => None,
        Some(node) => match expect_string(&node, "check", "#example")?.as_str() {
            "default" => Some(ExampleCheck::Default),
            "site" => Some(ExampleCheck::Site),
            other => bail!("`#example` check must be \"default\" or \"site\", found `{other}`"),
        },
    };
    ensure_no_extra_named(&args.named, "example", "#example")?;
    ensure!(
        args.positional.len() == 1,
        "`#example` requires one `[...]` body containing a code block"
    );
    let body_node = &args.positional[0];
    ensure!(
        body_node.kind() == SyntaxKind::ContentBlock,
        "`#example` body must be a `[...]` content block"
    );
    let markup_node = body_node
        .children()
        .find(|child| child.kind() == SyntaxKind::Markup)
        .ok_or_else(|| anyhow::anyhow!("empty `#example` content block"))?;
    let children: Vec<LinkedNode<'_>> = markup_node.children().collect();
    let (blocks, _) = parse_markup_slice(source, &children, false)?;
    let mut code_blocks = Vec::new();
    let mut prose_blocks = Vec::new();
    for block in blocks {
        match block {
            Block::Code { lang, source } => code_blocks.push((lang, source)),
            other => prose_blocks.push(other),
        }
    }
    ensure!(
        code_blocks.len() == 1,
        "`#example` must contain exactly one fenced code block (found {})",
        code_blocks.len()
    );
    let (lang, code) = code_blocks.remove(0);
    let description = if prose_blocks.is_empty() {
        None
    } else {
        let mut skip = Vec::new();
        for child in markup_node.children() {
            if child.kind() == SyntaxKind::Raw
                && let Some(raw) = child.cast::<ast::Raw>()
                && raw.block()
            {
                skip.push(child.range());
            }
        }
        let typst = slice_excluding(source.text(), markup_node.range(), &skip);
        Some(Markup {
            typst,
            blocks: prose_blocks,
        })
    };
    Ok(Example {
        title,
        description,
        lang: lang.unwrap_or_default(),
        code,
        check,
    })
}

fn parse_table(source: &Source, call: &LinkedNode<'_>) -> Result<Block> {
    let mut args = parse_args(source, call, "#table")?;
    let columns = match args.named.remove("columns") {
        None => None,
        Some(node) => {
            let Some(int) = node.cast::<ast::Int>() else {
                bail!("`#table(columns: ...)` must be a positive integer");
            };
            let count = int.get();
            ensure!(count > 0, "`#table(columns: ...)` must be positive");
            Some(count as usize)
        }
    };
    ensure_no_extra_named(&args.named, "table", "#table")?;

    let mut headers: Option<Vec<Vec<Inline>>> = None;
    let mut cells: Vec<Vec<Inline>> = Vec::new();
    for item in &args.positional {
        if item.kind() == SyntaxKind::FuncCall && callee_name(item)? == "table.header" {
            ensure!(headers.is_none(), "duplicate `table.header` in `#table`");
            let header_args = parse_args(source, item, "table.header")?;
            ensure!(
                header_args.named.is_empty(),
                "`table.header` takes positional cells only"
            );
            let mut row = Vec::with_capacity(header_args.positional.len());
            for cell in &header_args.positional {
                row.push(parse_cell_inlines(source, cell)?);
            }
            headers = Some(row);
        } else {
            cells.push(parse_cell_inlines(source, item)?);
        }
    }

    let (headers, body_cells) = match headers {
        Some(headers) => (headers, cells),
        None => {
            let count = columns.ok_or_else(|| {
                anyhow::anyhow!("`#table` requires `columns: N` or `table.header(...)`")
            })?;
            ensure!(
                cells.len() >= count,
                "`#table` has fewer cells than its column count"
            );
            let mut iter = cells.into_iter();
            let headers = iter.by_ref().take(count).collect();
            (headers, iter.collect())
        }
    };
    let column_count = columns.unwrap_or(headers.len());
    ensure!(
        column_count > 0 && headers.len() == column_count,
        "`#table` header cell count must match `columns`"
    );
    ensure!(
        body_cells.len() % column_count == 0,
        "`#table` body cell count ({}) is not divisible by columns ({column_count})",
        body_cells.len()
    );
    let rows = body_cells
        .chunks(column_count)
        .map(<[Vec<Inline>]>::to_vec)
        .collect();
    Ok(Block::Table { headers, rows })
}

fn parse_cell_inlines(source: &Source, node: &LinkedNode<'_>) -> Result<Vec<Inline>> {
    ensure!(
        node.kind() == SyntaxKind::ContentBlock,
        "`#table` cells must be `[...]` content blocks"
    );
    let markup = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Markup)
        .ok_or_else(|| anyhow::anyhow!("empty table cell"))?;
    parse_inline_markup(source, markup)
}

fn parse_inline_markup(source: &Source, markup_node: LinkedNode<'_>) -> Result<Vec<Inline>> {
    let mut inlines = Vec::new();
    for child in markup_node.children() {
        parse_inline_node(source, child, &mut inlines)?;
    }
    trim_inlines(&mut inlines);
    Ok(inlines)
}

fn parse_inline_node(source: &Source, node: LinkedNode<'_>, out: &mut Vec<Inline>) -> Result<()> {
    match node.kind() {
        SyntaxKind::LineComment
        | SyntaxKind::BlockComment
        | SyntaxKind::Hash
        | SyntaxKind::Semicolon => {}
        SyntaxKind::Text => push_inline_text(out, &source.text()[node.range()]),
        SyntaxKind::Space => push_inline_text(out, " "),
        SyntaxKind::Linebreak => push_inline_text(out, "\n"),
        SyntaxKind::Escape => {
            let escaped = node.cast::<ast::Escape>().expect("escape node");
            push_inline_text(out, &escaped.get().to_string());
        }
        SyntaxKind::Shorthand => {
            let shorthand = node.cast::<ast::Shorthand>().expect("shorthand node");
            push_inline_text(out, &shorthand.get().to_string());
        }
        SyntaxKind::SmartQuote => {
            let quote = node.cast::<ast::SmartQuote>().expect("smart quote");
            push_inline_text(out, if quote.double() { "\"" } else { "'" });
        }
        SyntaxKind::Raw => {
            let raw = node.cast::<ast::Raw>().expect("raw node");
            ensure!(!raw.block(), "fenced code block not allowed inline");
            out.push(Inline::Code(raw_inline_text(raw)));
        }
        SyntaxKind::Strong => {
            let strong = node.cast::<ast::Strong>().expect("strong node");
            let body = source
                .find(strong.body().span())
                .ok_or_else(|| anyhow::anyhow!("missing strong body"))?;
            out.push(Inline::Strong(parse_inline_markup(source, body)?));
        }
        SyntaxKind::Emph => {
            let emph = node.cast::<ast::Emph>().expect("emph node");
            let body = source
                .find(emph.body().span())
                .ok_or_else(|| anyhow::anyhow!("missing emph body"))?;
            out.push(Inline::Emph(parse_inline_markup(source, body)?));
        }
        SyntaxKind::Link => {
            let link = node.cast::<ast::Link>().expect("link node");
            out.push(Inline::Link {
                url: link.get().to_string(),
                label: None,
            });
        }
        SyntaxKind::FuncCall => {
            let name = callee_name(&node)?;
            out.push(parse_inline_call(source, &node, &name)?);
        }
        other => bail!(
            "unsupported inline syntax {:?} ({:?}) in help markup",
            other,
            &source.text()[node.range()]
        ),
    }
    Ok(())
}

fn parse_inline_call(source: &Source, call: &LinkedNode<'_>, name: &str) -> Result<Inline> {
    let args = parse_args(source, call, name)?;
    ensure!(
        args.named.is_empty(),
        "`#{name}` does not accept named arguments"
    );
    match name {
        "ref" => {
            ensure!(
                matches!(args.positional.len(), 1 | 2),
                "`#ref` expects `(target)` or `(target)[label]`"
            );
            let target = expect_string(&args.positional[0], "ref target", "#ref")?;
            let label = match args.positional.get(1) {
                Some(content) => Some(parse_cell_inlines(source, content)?),
                None => None,
            };
            Ok(Inline::Ref { target, label })
        }
        "link" => {
            ensure!(
                matches!(args.positional.len(), 1 | 2),
                "`#link` expects `(url)` or `(url)[label]`"
            );
            let url = expect_string(&args.positional[0], "link url", "#link")?;
            let label = match args.positional.get(1) {
                Some(content) => Some(parse_cell_inlines(source, content)?),
                None => None,
            };
            Ok(Inline::Link { url, label })
        }
        other => bail!("unsupported inline call `#{other}` in help markup"),
    }
}

fn raw_text(raw: ast::Raw<'_>) -> String {
    raw.lines()
        .map(|line| line.get().as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn raw_inline_text(raw: ast::Raw<'_>) -> String {
    raw.lines()
        .map(|line| line.get().as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_inline_text(inlines: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if text == " "
        && let Some(Inline::Text(existing)) = inlines.last()
        && existing.ends_with(' ')
    {
        return;
    }
    match inlines.last_mut() {
        Some(Inline::Text(existing)) => existing.push_str(text),
        _ => inlines.push(Inline::Text(text.to_owned())),
    }
}

fn trim_inlines(inlines: &mut Vec<Inline>) {
    while let Some(Inline::Text(text)) = inlines.first_mut() {
        let trimmed = text.trim_start_matches(' ');
        if trimmed.is_empty() {
            inlines.remove(0);
        } else {
            if trimmed.len() != text.len() {
                *text = trimmed.to_owned();
            }
            break;
        }
    }
    while let Some(Inline::Text(text)) = inlines.last_mut() {
        let trimmed = text.trim_end_matches(' ');
        if trimmed.is_empty() {
            inlines.pop();
        } else {
            if trimmed.len() != text.len() {
                *text = trimmed.to_owned();
            }
            break;
        }
    }
}

fn collect_plain_blocks(blocks: &[Block], out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => {
                let text = plain_inlines(inlines);
                if !text.is_empty() {
                    out.push(text);
                }
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_plain_blocks(item, out);
                }
            }
            Block::Code { source, .. } => {
                if !source.is_empty() {
                    out.push(source.clone());
                }
            }
            Block::Table { headers, rows } => {
                for row in std::iter::once(headers).chain(rows) {
                    let line = row
                        .iter()
                        .map(|cell| plain_inlines(cell))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !line.is_empty() {
                        out.push(line);
                    }
                }
            }
        }
    }
}

pub(crate) fn plain_inlines(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::Strong(inner) | Inline::Emph(inner) => out.push_str(&plain_inlines(inner)),
            Inline::Link { url, label } => match label {
                Some(label) => out.push_str(&plain_inlines(label)),
                None => out.push_str(url),
            },
            Inline::Ref { target, label } => match label {
                Some(label) => out.push_str(&plain_inlines(label)),
                None => out.push_str(target),
            },
        }
    }
    out
}

#[allow(dead_code)]
fn collect_refs_blocks<'a>(blocks: &'a [Block], out: &mut Vec<&'a str>) {
    for block in blocks {
        match block {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => {
                collect_refs_inlines(inlines, out);
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_refs_blocks(item, out);
                }
            }
            Block::Table { headers, rows } => {
                for cell in headers.iter().chain(rows.iter().flatten()) {
                    collect_refs_inlines(cell, out);
                }
            }
            Block::Code { .. } => {}
        }
    }
}

#[allow(dead_code)]
fn collect_refs_inlines<'a>(inlines: &'a [Inline], out: &mut Vec<&'a str>) {
    for inline in inlines {
        match inline {
            Inline::Ref { target, label } => {
                out.push(target.as_str());
                if let Some(label) = label {
                    collect_refs_inlines(label, out);
                }
            }
            Inline::Strong(inner) | Inline::Emph(inner) => collect_refs_inlines(inner, out),
            Inline::Link {
                label: Some(label), ..
            } => collect_refs_inlines(label, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_document_show_header_with_entry_calls() {
        let doc = parse_package(
            "test.typ",
            r#"
#show: package(
  summary: [Manage *site* data.],
  related: ("[site]",),
)

See #ref("[site]") for details.

#export(
  "relation",
  summary: [Create a relation.],
  params: (
    name: [Relation name.],
  ),
  related: ("@tola/source",),
)[
  Defines a relation with a fixed arity.

  #example(title: [Declare a relation], check: "default")[
    ```typst
    #import "@tola/datalog:0.0.0": relation
    ```
  ]
]

#method(
  "atom",
  summary: [Emit a fact.],
  params: (value: [Term value.]),
)[
  Adds one tuple.
]
"#,
        )
        .unwrap();
        assert_eq!(doc.summary.typst, "Manage *site* data.");
        assert_eq!(doc.body.refs(), vec!["[site]"]);
        let export = &doc.exports["relation"];
        assert_eq!(export.body.typst, "Defines a relation with a fixed arity.");
        assert_eq!(export.examples.len(), 1);
        assert_eq!(export.examples[0].check, Some(ExampleCheck::Default));
        assert_eq!(export.examples[0].lang, "typst");
        assert_eq!(export.methods["atom"].body.typst, "Adds one tuple.");
    }

    #[test]
    fn rejects_selector_show_rules() {
        let err = parse_overview("test.typ", "#show heading: it => it\nHello").unwrap_err();
        assert!(err.to_string().contains("selector"));
    }

    #[test]
    fn rejects_multiple_show_headers() {
        let err =
            parse_overview("test.typ", "#show: overview\nHi\n#show: overview\nBye").unwrap_err();
        assert!(err.to_string().contains("only one"));
    }
}
