//! The structured form of one `tola help` page.
//!
//! The page builders write the Markdown once; the plain renderer parses it through
//! `terminal::documentation`, and the interactive view reads it through this model. `PageId`
//! names a page, `selector()` spells the `tola help` argument that shows it, and the two codecs
//! give that page and its anchors a `tola://` spelling for cross-references.

use std::collections::BTreeSet;
use std::iter::Peekable;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

/// The reserved scheme of a cross-reference between help pages.
pub(crate) const CROSS_REFERENCE_SCHEME: &str = "tola://";

/// One page a `tola help` builder writes, before it is parsed.
pub(crate) struct HelpPage {
    pub id: PageId,
    sections: Vec<HelpSection>,
}

enum HelpSection {
    Markdown(String),
    /// The generated declaration heading precedes every heading in its documentation.
    Export(String),
}

impl HelpPage {
    pub(crate) fn new(id: PageId, markdown: String) -> Self {
        Self {
            id,
            sections: vec![HelpSection::Markdown(markdown)],
        }
    }

    pub(crate) fn push_export(&mut self, markdown: String) {
        self.sections.push(HelpSection::Export(markdown));
    }

    pub(crate) fn markdown(&self) -> String {
        self.sections
            .iter()
            .map(|section| match section {
                HelpSection::Markdown(markdown) | HelpSection::Export(markdown) => {
                    markdown.as_str()
                }
            })
            .collect()
    }
}

/// The identity of one `tola help` page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageId {
    /// The bare `tola help` index.
    Overview,
    /// One configuration table: `[site]`, `[[build.hooks.before-build]]`.
    Table {
        section: String,
        /// Whether the section is an array of tables, as its selector spells it.
        array: bool,
    },
    /// One bundled package: `@tola/address`.
    Package { name: String },
    /// One bundled package narrowed to the selected exports, in request order.
    PackageSelection { name: String, exports: Vec<String> },
}

impl PageId {
    /// The arguments `tola help` shows this page with, without the shell quoting a table selector
    /// needs; `None` for the overview, which is the bare command.
    pub(crate) fn selector(&self) -> Option<String> {
        match self {
            Self::Overview => None,
            Self::Table {
                section,
                array: false,
            } => Some(format!("[{section}]")),
            Self::Table {
                section,
                array: true,
            } => Some(format!("[[{section}]]")),
            Self::Package { name } => Some(name.clone()),
            Self::PackageSelection { name, exports } if exports.is_empty() => Some(name.clone()),
            Self::PackageSelection { name, exports } => {
                Some(format!("{name} {}", exports.join(" ")))
            }
        }
    }

    /// The `tola://` reference naming this page, without any fragment.
    fn uri(&self) -> String {
        match self {
            Self::Overview => format!("{CROSS_REFERENCE_SCHEME}overview"),
            Self::Table {
                section,
                array: false,
            } => {
                format!("{CROSS_REFERENCE_SCHEME}table/[{section}]")
            }
            Self::Table {
                section,
                array: true,
            } => {
                format!("{CROSS_REFERENCE_SCHEME}table/[[{section}]]")
            }
            Self::Package { name } => format!("{CROSS_REFERENCE_SCHEME}package/{name}"),
            Self::PackageSelection { name, exports } if exports.is_empty() => {
                format!("{CROSS_REFERENCE_SCHEME}selection/{name}")
            }
            Self::PackageSelection { name, exports } => {
                format!(
                    "{CROSS_REFERENCE_SCHEME}selection/{name}:{}",
                    exports.join(",")
                )
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct HelpDocument {
    pub id: PageId,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadingRole {
    Documentation,
    Export,
}

/// One block-level element of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        spans: Vec<Inline>,
        anchor: Anchor,
        role: HeadingRole,
    },
    Paragraph {
        spans: Vec<Inline>,
        /// Whether the paragraph introduces the code block that follows it.
        lead: bool,
    },
    /// A list; each item holds its own blocks.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    /// A code block: the fence's language word (empty when it carries none) and its source text,
    /// exactly as the fences hold it.
    Code {
        language: String,
        source: String,
    },
    /// A table; its first row heads the rest.
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// A block quote, holding its own blocks.
    Quote {
        blocks: Vec<Block>,
    },
    Rule,
}

/// One inline span of a paragraph or heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Link {
        label: Vec<Inline>,
        target: LinkTarget,
    },
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// Another help page.
    Page(PageId),
    /// One heading on a help page.
    PageAnchor(PageId, Anchor),
    /// A URL outside the help pages.
    External(String),
}

impl LinkTarget {
    /// The URL a link carries, written on `page`: its `tola://` reference, or its own external URL
    /// unchanged. A same-page anchor omits the page selector.
    pub(crate) fn uri(&self, page: &PageId) -> String {
        match self {
            Self::Page(target) => target.uri(),
            Self::PageAnchor(target, anchor) if target == page => {
                format!("{CROSS_REFERENCE_SCHEME}anchor#{}", anchor.as_str())
            }
            Self::PageAnchor(target, anchor) => format!("{}#{}", target.uri(), anchor.as_str()),
            Self::External(url) => url.clone(),
        }
    }

    /// The target one URL names when written on `page`. A URL outside the reserved scheme is an
    /// external target; a malformed `tola://` reference has no target at all.
    pub(crate) fn from_uri(uri: &str, page: &PageId) -> Option<Self> {
        if let Some(fragment) = uri.strip_prefix('#') {
            return (!fragment.is_empty())
                .then(|| Self::PageAnchor(page.clone(), Anchor(fragment.to_owned())));
        }
        let Some(reference) = uri.strip_prefix(CROSS_REFERENCE_SCHEME) else {
            return Some(Self::External(uri.to_owned()));
        };
        let (id, anchor) = match reference.split_once('#') {
            None => (reference, None),
            Some((_, "")) => return None,
            Some((id, anchor)) => (id, Some(Anchor(anchor.to_owned()))),
        };
        let target = match (id, &anchor) {
            // A fragment without a page names an anchor on the page the link sits on.
            ("anchor", Some(_)) => page.clone(),
            ("anchor", None) => return None,
            _ => page_id(id)?,
        };
        Some(match anchor {
            Some(anchor) => Self::PageAnchor(target, anchor),
            None => Self::Page(target),
        })
    }
}

/// The page one `tola://` reference names, without its fragment.
fn page_id(reference: &str) -> Option<PageId> {
    if reference == "overview" {
        return Some(PageId::Overview);
    }
    let (authority, path) = reference.split_once('/')?;
    match authority {
        "table" => {
            let (section, array) = match path
                .strip_prefix("[[")
                .and_then(|rest| rest.strip_suffix("]]"))
            {
                Some(section) => (section, true),
                None => (path.strip_prefix('[')?.strip_suffix(']')?, false),
            };
            (!section.is_empty()).then_some(PageId::Table {
                section: section.to_owned(),
                array,
            })
        }
        "package" => (!path.is_empty()).then_some(PageId::Package {
            name: path.to_owned(),
        }),
        "selection" => {
            let (name, exports) = match path.split_once(':') {
                Some((name, exports)) => (name, exports.split(',').map(str::to_owned).collect()),
                None => (path, Vec::new()),
            };
            (!name.is_empty()).then_some(PageId::PackageSelection {
                name: name.to_owned(),
                exports,
            })
        }
        _ => None,
    }
}

/// Heading slugs are unique throughout a parsed document, including nested blocks.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Anchor(String);

impl Anchor {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The slug of one heading text: runs of separators fold into a single `-`, letters fold to
/// lowercase, and only letters, digits, and `.`/`[`/`]` are kept.
pub(crate) fn anchor(heading: &str) -> Anchor {
    let mut slug = String::new();
    let mut separated = false;
    for character in heading.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '[' | ']') {
            if separated && !slug.is_empty() {
                slug.push('-');
            }
            separated = false;
            slug.extend(character.to_lowercase());
        } else {
            separated = true;
        }
    }
    Anchor(slug)
}

impl HelpDocument {
    /// Parses the Markdown of one page the builder wrote.
    pub(crate) fn parse(page: HelpPage) -> Self {
        let markdown = page.markdown();
        let mut offset = 0;
        let mut exports = Vec::new();
        for section in &page.sections {
            let start = offset;
            match section {
                HelpSection::Markdown(markdown) => offset += markdown.len(),
                HelpSection::Export(markdown) => {
                    offset += markdown.len();
                    exports.push(start..offset);
                }
            }
        }
        let mut exports = exports.into_iter().peekable();
        let nodes = nodes(
            &mut Parser::new_ext(&markdown, Options::ENABLE_TABLES).into_offset_iter(),
            &mut exports,
        );
        let mut document_blocks = blocks(&nodes, &page.id);
        let mut used = BTreeSet::new();
        // An export index names its declaration even when preceding prose repeats that heading.
        unique_anchors(&mut document_blocks, &mut used, HeadingRole::Export);
        unique_anchors(&mut document_blocks, &mut used, HeadingRole::Documentation);
        Self {
            id: page.id,
            blocks: document_blocks,
        }
    }

    pub(crate) fn contains_anchor(&self, anchor: &Anchor) -> bool {
        contains_anchor(&self.blocks, anchor)
    }
}

fn contains_anchor(blocks: &[Block], wanted: &Anchor) -> bool {
    blocks.iter().any(|block| match block {
        Block::Heading { anchor, .. } => anchor == wanted,
        Block::List { items, .. } => items.iter().any(|blocks| contains_anchor(blocks, wanted)),
        Block::Quote { blocks } => contains_anchor(blocks, wanted),
        _ => false,
    })
}

fn unique_anchors(blocks: &mut [Block], used: &mut BTreeSet<Anchor>, selected: HeadingRole) {
    for block in blocks {
        match block {
            Block::Heading { anchor, role, .. } if *role == selected => {
                let base = if anchor.as_str().is_empty() {
                    "section".to_owned()
                } else {
                    anchor.as_str().to_owned()
                };
                let mut candidate = Anchor(base.clone());
                let mut suffix = 2;
                while !used.insert(candidate.clone()) {
                    candidate = Anchor(format!("{base}-{suffix}"));
                    suffix += 1;
                }
                *anchor = candidate;
            }
            Block::List { items, .. } => {
                for blocks in items {
                    unique_anchors(blocks, used, selected);
                }
            }
            Block::Quote { blocks } => unique_anchors(blocks, used, selected),
            _ => {}
        }
    }
}

/// One Markdown fragment as a tree of balanced events.
enum Node<'a> {
    Heading {
        level: pulldown_cmark::HeadingLevel,
        children: Vec<Node<'a>>,
        role: HeadingRole,
    },
    Branch(Tag<'a>, Vec<Node<'a>>),
    Leaf(Event<'a>),
}

fn nodes<'a>(
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    exports: &mut Peekable<impl Iterator<Item = Range<usize>>>,
) -> Vec<Node<'a>> {
    let mut tree = Vec::new();
    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                while exports
                    .peek()
                    .is_some_and(|export| export.end <= range.start)
                {
                    exports.next();
                }
                let role = if exports
                    .peek()
                    .is_some_and(|export| export.contains(&range.start))
                {
                    exports.next();
                    HeadingRole::Export
                } else {
                    HeadingRole::Documentation
                };
                tree.push(Node::Heading {
                    level,
                    children: nodes(events, exports),
                    role,
                });
            }
            Event::Start(tag) => tree.push(Node::Branch(tag, nodes(events, exports))),
            Event::End(_) => break,
            event => tree.push(Node::Leaf(event)),
        }
    }
    tree
}

fn blocks(nodes: &[Node<'_>], page: &PageId) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut inline_start = 0;
    for (position, _) in nodes.iter().enumerate() {
        let Some(block) = block(nodes, position, page) else {
            continue;
        };
        if inline_start < position {
            blocks.push(inline_paragraph(&nodes[inline_start..position], page));
        }
        blocks.push(block);
        inline_start = position + 1;
    }
    if inline_start < nodes.len() {
        blocks.push(inline_paragraph(&nodes[inline_start..], page));
    }
    blocks
}

fn block(nodes: &[Node<'_>], position: usize, page: &PageId) -> Option<Block> {
    match &nodes[position] {
        Node::Branch(Tag::Paragraph, children) => {
            let spans = inline(children, page);
            let lead = matches!(
                nodes.get(position + 1),
                Some(Node::Branch(Tag::CodeBlock(_), _))
            ) && plain_text(&spans).trim_end().ends_with([':', '：']);
            Some(Block::Paragraph { spans, lead })
        }
        Node::Heading {
            level,
            children,
            role,
        } => {
            let spans = inline(children, page);
            Some(Block::Heading {
                level: *level as u8,
                anchor: anchor(&plain_text(&spans)),
                spans,
                role: *role,
            })
        }
        Node::Branch(Tag::CodeBlock(kind), children) => Some(Block::Code {
            language: fence_language(kind),
            source: code_source(children),
        }),
        Node::Branch(Tag::List(start), children) => Some(Block::List {
            start: *start,
            items: children
                .iter()
                .filter_map(|node| match node {
                    Node::Branch(Tag::Item, children) => Some(blocks(children, page)),
                    _ => None,
                })
                .collect(),
        }),
        Node::Branch(Tag::Table(_), children) => {
            let mut rows = children
                .iter()
                .filter_map(|node| match node {
                    Node::Branch(Tag::TableHead | Tag::TableRow, cells) => Some(cells),
                    _ => None,
                })
                .map(|cells| {
                    cells
                        .iter()
                        .filter_map(|cell| match cell {
                            Node::Branch(Tag::TableCell, children) => Some(inline(children, page)),
                            _ => None,
                        })
                        .collect()
                })
                .collect::<Vec<Vec<Vec<Inline>>>>();
            let headers = if rows.is_empty() {
                Vec::new()
            } else {
                rows.remove(0)
            };
            Some(Block::Table { headers, rows })
        }
        Node::Branch(Tag::BlockQuote(_), children) => Some(Block::Quote {
            blocks: blocks(children, page),
        }),
        // Raw HTML has no structured form of its own; its text reads as a paragraph.
        Node::Branch(Tag::HtmlBlock, children) => Some(inline_paragraph(children, page)),
        Node::Leaf(Event::Rule) => Some(Block::Rule),
        _ => None,
    }
}

/// The paragraph a run of inline nodes that no paragraph wraps reads as.
fn inline_paragraph(nodes: &[Node<'_>], page: &PageId) -> Block {
    Block::Paragraph {
        spans: inline(nodes, page),
        lead: false,
    }
}

fn inline(nodes: &[Node<'_>], page: &PageId) -> Vec<Inline> {
    let mut spans = Vec::new();
    for node in nodes {
        match node {
            Node::Branch(Tag::Emphasis, children) => {
                spans.push(Inline::Emph(inline(children, page)));
            }
            Node::Branch(Tag::Strong, children) => {
                spans.push(Inline::Strong(inline(children, page)));
            }
            Node::Branch(Tag::Link { dest_url, .. }, children) => spans.push(Inline::Link {
                label: inline(children, page),
                target: LinkTarget::from_uri(dest_url, page)
                    .unwrap_or_else(|| LinkTarget::External(dest_url.to_string())),
            }),
            // Every other group — an image included — contributes the text it holds.
            Node::Branch(_, children) | Node::Heading { children, .. } => {
                spans.extend(inline(children, page))
            }
            Node::Leaf(Event::Text(text) | Event::Html(text) | Event::InlineHtml(text)) => {
                push_text(&mut spans, text);
            }
            Node::Leaf(Event::Code(source)) => spans.push(Inline::Code(source.to_string())),
            Node::Leaf(Event::SoftBreak) => push_text(&mut spans, " "),
            Node::Leaf(Event::HardBreak) => push_text(&mut spans, "\n"),
            Node::Leaf(_) => {}
        }
    }
    spans
}

fn push_text(spans: &mut Vec<Inline>, text: &str) {
    match spans.last_mut() {
        Some(Inline::Text(existing)) => existing.push_str(text),
        _ => spans.push(Inline::Text(text.to_owned())),
    }
}

/// The text of inline spans, with each code span and label unwrapped.
fn plain_text(spans: &[Inline]) -> String {
    let mut text = String::new();
    for span in spans {
        match span {
            Inline::Text(value) | Inline::Code(value) => text.push_str(value),
            Inline::Emph(children) | Inline::Strong(children) => {
                text.push_str(&plain_text(children));
            }
            Inline::Link { label, .. } => text.push_str(&plain_text(label)),
        }
    }
    text
}

fn fence_language(kind: &CodeBlockKind<'_>) -> String {
    match kind {
        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
        CodeBlockKind::Indented => String::new(),
    }
}

fn code_source(children: &[Node<'_>]) -> String {
    children
        .iter()
        .filter_map(|node| match node {
            Node::Leaf(Event::Text(text)) => Some(text.as_ref()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_selectors_preserve_selection() {
        for (page, selector) in [
            (PageId::Overview, None),
            (
                PageId::Table {
                    section: "site".to_owned(),
                    array: false,
                },
                Some("[site]"),
            ),
            (
                PageId::Table {
                    section: "build.hooks.before-build".to_owned(),
                    array: true,
                },
                Some("[[build.hooks.before-build]]"),
            ),
            (
                PageId::Package {
                    name: "@tola/schema".to_owned(),
                },
                Some("@tola/schema"),
            ),
            (
                PageId::PackageSelection {
                    name: "@tola/source".to_owned(),
                    exports: vec!["parse-sources".to_owned(), "all-sources".to_owned()],
                },
                Some("@tola/source parse-sources all-sources"),
            ),
        ] {
            assert_eq!(page.selector().as_deref(), selector);
        }
    }

    #[test]
    fn heading_slugs_keep_domain_spelling() {
        for (heading, slug) in [
            ("slugify - function", "slugify-function"),
            (
                "[build.hooks] - configuration table",
                "[build.hooks]-configuration-table",
            ),
            ("Map-shaped [site.extra]", "map-shaped-[site.extra]"),
            ("Import in Typst", "import-in-typst"),
            ("名称 — 类型", "名称-类型"),
        ] {
            assert_eq!(anchor(heading).as_str(), slug);
        }
    }

    #[test]
    fn link_targets_round_trip() {
        let page = PageId::Package {
            name: "@tola/address".to_owned(),
        };
        for target in [
            LinkTarget::Page(PageId::Overview),
            LinkTarget::Page(PageId::Table {
                section: "site".to_owned(),
                array: false,
            }),
            LinkTarget::Page(PageId::Table {
                section: "build.hooks.before-build".to_owned(),
                array: true,
            }),
            LinkTarget::Page(PageId::Package {
                name: "@tola/schema".to_owned(),
            }),
            LinkTarget::Page(PageId::PackageSelection {
                name: "@tola/source".to_owned(),
                exports: Vec::new(),
            }),
            LinkTarget::Page(PageId::PackageSelection {
                name: "@tola/source".to_owned(),
                exports: vec!["parse-sources".to_owned()],
            }),
            LinkTarget::PageAnchor(page.clone(), anchor("Related")),
            LinkTarget::PageAnchor(
                PageId::Package {
                    name: "@tola/schema".to_owned(),
                },
                anchor("parse-sources"),
            ),
            LinkTarget::External("https://typst.app/docs".to_owned()),
        ] {
            assert_eq!(
                LinkTarget::from_uri(&target.uri(&page), &page),
                Some(target)
            );
        }
        for uri in ["tola://table/", "tola://anchor", "tola://anchor#", "#"] {
            assert_eq!(LinkTarget::from_uri(uri, &page), None);
        }
    }

    #[test]
    fn nested_headings_have_unique_anchors() {
        let document =
            document("# Same\n\n> ## Same\n\n- ### Same-2\n\n## Same\n\n# !!!\n\n> # ???\n");
        for name in [
            "same",
            "same-2",
            "same-2-2",
            "same-3",
            "section",
            "section-2",
        ] {
            assert!(document.contains_anchor(&anchor(name)), "missing {name}");
        }
        let mut names = BTreeSet::new();
        collect_anchors(&document.blocks, &mut names);
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn export_fragments_preserve_markdown() {
        let prefix = "# Package\n\nOverview.\n";
        let function = "\n## callable - function\n\n```typst-code\nlet callable();\n```\n";
        let value = "\n---\n\n## constant - value\n\nThe value.\n";
        let mut page = HelpPage::new(PageId::Overview, prefix.to_owned());
        page.push_export(function.to_owned());
        page.push_export(value.to_owned());
        assert_eq!(page.markdown(), [prefix, function, value].concat());
    }

    #[test]
    fn reference_links_cross_export_fragments() {
        let mut page = HelpPage::new(
            PageId::Overview,
            "Read [the overview][destination].\n\n".to_owned(),
        );
        page.push_export("## callable - function\n\n[destination]: tola://overview\n".to_owned());
        let document = HelpDocument::parse(page);
        let Block::Paragraph { spans, .. } = &document.blocks[0] else {
            panic!("a paragraph")
        };
        assert!(spans.iter().any(|span| matches!(
            span,
            Inline::Link {
                target: LinkTarget::Page(PageId::Overview),
                ..
            }
        )));
    }

    #[test]
    fn exports_keep_primary_anchors() {
        let mut page = HelpPage::new(
            PageId::Overview,
            "# Package\n\n## callable - function\n\nOverview example.\n".to_owned(),
        );
        page.push_export(
            "\n## callable - function\n\n### Parameters\n\n> ## callable - function\n\n## misleading - value\n\n```typ\n## fenced - function\n```\n"
                .to_owned(),
        );
        page.push_export("\n---\n\n## constant - value\n\nThe value.\n".to_owned());
        let document = HelpDocument::parse(page);
        let primary = document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Heading {
                    anchor,
                    role: HeadingRole::Export,
                    ..
                } => Some(anchor.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(primary, ["callable-function", "constant-value"]);
        let ordinary = document
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Heading {
                    anchor,
                    spans,
                    role: HeadingRole::Documentation,
                    ..
                } if plain_text(spans) == "callable - function" => Some(anchor),
                _ => None,
            })
            .unwrap();
        assert_eq!(ordinary.as_str(), "callable-function-2");
        assert!(document.contains_anchor(&anchor("callable-function-3")));
        let mut names = BTreeSet::new();
        collect_anchors(&document.blocks, &mut names);
    }

    #[test]
    fn fragments_target_current_page() {
        let document = document(
            "# [Details](#details)\n\nSee [this](#details).\n\n| Topic |\n| --- |\n| [details](#details) |\n",
        );
        let target = LinkTarget::PageAnchor(document.id.clone(), anchor("details"));
        let Block::Heading { spans, .. } = &document.blocks[0] else {
            panic!("heading")
        };
        assert!(
            matches!(&spans[0], Inline::Link { target: heading_target, .. } if heading_target == &target)
        );
        let Block::Paragraph { spans, .. } = &document.blocks[1] else {
            panic!("paragraph")
        };
        assert!(spans.iter().any(|span| matches!(span, Inline::Link { target: paragraph_target, .. } if paragraph_target == &target)));
        let Block::Table { rows, .. } = &document.blocks[2] else {
            panic!("table")
        };
        assert!(
            matches!(&rows[0][0][0], Inline::Link { target: cell_target, .. } if cell_target == &target)
        );
    }

    #[test]
    fn lists_keep_starting_ordinal() {
        let document = document("7. first\n8. second\n");
        assert!(
            matches!(&document.blocks[0], Block::List { start: Some(7), items } if items.len() == 2)
        );
    }

    fn document(markdown: &str) -> HelpDocument {
        HelpDocument::parse(HelpPage::new(PageId::Overview, markdown.to_owned()))
    }

    fn collect_anchors(blocks: &[Block], names: &mut BTreeSet<Anchor>) {
        for block in blocks {
            match block {
                Block::Heading { anchor, .. } => assert!(names.insert(anchor.clone())),
                Block::List { items, .. } => {
                    for blocks in items {
                        collect_anchors(blocks, names);
                    }
                }
                Block::Quote { blocks } => collect_anchors(blocks, names),
                _ => {}
            }
        }
    }
}
