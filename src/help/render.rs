//! Terminal and Markdown rendering directly from typed `HelpPage` and `HelpReference` models.

use std::fmt::Write as _;
use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use super::language::HelpLanguage;
use super::markup::{Block, Inline, Markup};
use super::model::{
    ConfigPage, Example, HelpPage, HelpReference, IndexPage, OverviewPage, PackageExport,
    PackagePage, Parameter, RelatedItem, ScriptPage,
};
use crate::terminal::text::{Style, StyledText};
use crate::terminal::wrap;

/// Render one help page to terminal text.
pub(crate) fn terminal(
    page: &HelpPage,
    language: HelpLanguage,
    columns: Option<usize>,
    color: bool,
) -> String {
    let mut renderer = TerminalRenderer {
        columns: columns.unwrap_or(TerminalRenderer::DEFAULT_COLUMNS).max(1),
        color,
        language,
        blocks: Vec::new(),
    };
    renderer.page(page);
    let mut output = renderer.blocks.join("\n\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

/// Render the complete reference (`tola help --all`) to terminal text.
pub(crate) fn terminal_reference(
    reference: &HelpReference,
    language: HelpLanguage,
    columns: Option<usize>,
    color: bool,
) -> String {
    let mut renderer = TerminalRenderer {
        columns: columns.unwrap_or(TerminalRenderer::DEFAULT_COLUMNS).max(1),
        color,
        language,
        blocks: Vec::new(),
    };
    renderer.overview(&reference.overview);
    for page in &reference.config {
        renderer.rule();
        renderer.config(page);
    }
    for page in &reference.packages {
        renderer.rule();
        renderer.package(page);
    }
    for page in &reference.scripts {
        renderer.rule();
        renderer.script(page);
    }
    let mut output = renderer.blocks.join("\n\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

/// Render one help page to Markdown.
pub(crate) fn markdown(page: &HelpPage, language: HelpLanguage) -> String {
    let mut out = String::new();
    write_markdown_page(&mut out, page, language);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Render the complete reference (`tola help --all --format markdown`) to Markdown.
pub(crate) fn markdown_reference(reference: &HelpReference, language: HelpLanguage) -> String {
    let mut out = String::new();
    write_markdown_overview(&mut out, &reference.overview, language);
    for page in &reference.config {
        out.push_str("\n---\n\n");
        write_markdown_config(&mut out, page, language);
    }
    for page in &reference.packages {
        out.push_str("\n---\n\n");
        write_markdown_package(&mut out, page, language);
    }
    for page in &reference.scripts {
        out.push_str("\n---\n\n");
        write_markdown_script(&mut out, page, language);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn param_group_labels(
    lang: HelpLanguage,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match lang {
        HelpLanguage::English => (
            "Positional parameters",
            "Rest parameters",
            "Named parameters",
            "required",
        ),
        HelpLanguage::SimplifiedChinese => ("位置参数", "可变参数", "命名参数", "必填"),
    }
}

fn script_param_labels(lang: HelpLanguage) -> (&'static str, &'static str) {
    match lang {
        HelpLanguage::English => ("Positional parameters", "Options"),
        HelpLanguage::SimplifiedChinese => ("位置参数", "选项"),
    }
}

struct TerminalRenderer {
    columns: usize,
    color: bool,
    language: HelpLanguage,
    blocks: Vec<String>,
}

impl TerminalRenderer {
    const DEFAULT_COLUMNS: usize = 88;

    fn page(&mut self, page: &HelpPage) {
        match page {
            HelpPage::Overview(page) => self.overview(page),
            HelpPage::Index(page) => self.index(page),
            HelpPage::Config(page) => self.config(page),
            HelpPage::Package(page) => self.package(page),
            HelpPage::Script(page) => self.script(page),
        }
    }

    fn summary_bullet(&self, title: &str, summary: &Markup) -> String {
        let mut text = StyledText::default();
        text.push(&format!("`{title}`"), Style::Link);
        if !summary.is_empty() {
            text.push(self.language.colon(), Style::Plain);
            inline_markup_first_line(summary, &mut text);
        }
        self.bullet_line(&text, "")
    }

    fn push_bullets(&mut self, lines: Vec<String>) {
        let joined = lines.join("\n");
        if !joined.is_empty() {
            self.blocks.push(joined);
        }
    }

    fn overview(&mut self, page: &OverviewPage) {
        self.heading(1, &page.title);
        self.markup(&page.body, 0);
        let bullets = page
            .categories
            .iter()
            .map(|entry| {
                let mut text = StyledText::default();
                text.push(entry.title, Style::Link);
                self.bullet_line(&text, "")
            })
            .collect();
        self.push_bullets(bullets);
    }

    fn index(&mut self, page: &IndexPage) {
        self.heading(1, &page.title);
        self.markup(&page.summary, 0);
        let bullets = page
            .entries
            .iter()
            .map(|e| self.summary_bullet(&e.title, &e.summary))
            .collect();
        self.push_bullets(bullets);
    }

    fn config(&mut self, page: &ConfigPage) {
        self.heading(1, &page.header);
        self.markup(&page.summary, 0);
        if !page.keys.is_empty() {
            let heading = match self.language {
                HelpLanguage::English => "Keys",
                HelpLanguage::SimplifiedChinese => "配置项",
            };
            self.heading(2, heading);
            for key in &page.keys {
                let mut header = StyledText::default();
                header.push(&format!("`{}`", key.name), Style::Literal);
                if let Some(default) = &key.default {
                    let (open, close) = self.language.parentheses();
                    let label = match self.language {
                        HelpLanguage::English => "default: ",
                        HelpLanguage::SimplifiedChinese => "默认：",
                    };
                    header.push(open, Style::Plain);
                    header.push(label, Style::Plain);
                    header.push(&format_default_value(default), Style::Literal);
                    header.push(close, Style::Plain);
                }
                if let Some(values) = &key.values {
                    let label = match self.language {
                        HelpLanguage::English => " — values: ",
                        HelpLanguage::SimplifiedChinese => " — 可选值：",
                    };
                    header.push(label, Style::Plain);
                    header.push(&values.join(", "), Style::Literal);
                }
                self.blocks.push(self.wrapped(&header, ""));
                self.markup_indented(&key.body, "  ", 2);
                for example in &key.examples {
                    self.example(example, "  ");
                }
            }
        }
        if !page.children.is_empty() {
            let heading = match self.language {
                HelpLanguage::English => "Child tables",
                HelpLanguage::SimplifiedChinese => "子表",
            };
            self.heading(2, heading);
            let bullets = page
                .children
                .iter()
                .map(|c| self.summary_bullet(&c.header, &c.summary))
                .collect();
            self.push_bullets(bullets);
        }
        if !page.body.is_empty() {
            self.markup(&page.body, 1);
        }
        for example in &page.examples {
            self.example(example, "");
        }
        self.related(&page.related, 2);
    }

    fn package(&mut self, page: &PackagePage) {
        self.heading(1, &page.title);
        let import_heading = match self.language {
            HelpLanguage::English => "Import",
            HelpLanguage::SimplifiedChinese => "导入",
        };
        self.heading(2, import_heading);
        self.blocks.push(self.code("typst", &page.import, ""));
        if !page.summary.is_empty() {
            self.markup(&page.summary, 0);
        }
        if !page.body.is_empty() {
            self.markup(&page.body, 1);
        }
        for example in &page.examples {
            self.example(example, "");
        }
        if !page.selected {
            let exports_heading = match self.language {
                HelpLanguage::English => "Exports",
                HelpLanguage::SimplifiedChinese => "导出成员",
            };
            self.heading(2, exports_heading);
            let bullets = page
                .exports
                .iter()
                .map(|e| self.summary_bullet(&e.name, &e.summary))
                .collect();
            self.push_bullets(bullets);
        }
        self.related(&page.related, 2);
        for (idx, export) in page.exports.iter().enumerate() {
            if idx > 0 || !page.selected {
                self.rule();
            }
            self.package_export(export);
        }
        if !page.selected && page.exports.len() > 1 {
            let sample = page
                .exports
                .iter()
                .take(2)
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let (label, lang) = match self.language {
                HelpLanguage::English => ("Read selected exports", ""),
                HelpLanguage::SimplifiedChinese => ("只查看指定成员", " --lang zh"),
            };
            let mut hint = StyledText::default();
            hint.push(label, Style::Plain);
            hint.push(self.language.colon(), Style::Plain);
            hint.push(
                &format!("`tola help{lang} \"@tola/{}\" {sample}`", page.name),
                Style::Literal,
            );
            self.blocks.push(self.wrapped(&hint, ""));
        }
    }

    fn package_export(&mut self, export: &PackageExport) {
        self.heading(2, &format!("{} - {}", export.name, export.kind));
        self.blocks.push(self.signature(&export.declaration));
        if !export.summary.is_empty() {
            self.markup(&export.summary, 0);
        }
        if !export.body.is_empty() {
            self.markup(&export.body, 2);
        }
        self.parameter_groups(&export.parameters);
        for example in &export.examples {
            self.example(example, "");
        }
        for method in &export.methods {
            self.heading(3, &format!("`{}`", method.name));
            self.blocks.push(self.signature(&method.declaration));
            self.markup(&method.receiver_note, 0);
            if !method.summary.is_empty() {
                self.markup(&method.summary, 0);
            }
            if !method.body.is_empty() {
                self.markup(&method.body, 3);
            }
            self.parameter_groups(&method.parameters);
            for example in &method.examples {
                self.example(example, "");
            }
        }
        self.related(&export.related, 3);
    }

    fn script(&mut self, page: &ScriptPage) {
        self.heading(1, &page.locator);
        if !page.summary.is_empty() {
            self.markup(&page.summary, 0);
        }
        if !page.body.is_empty() {
            self.markup(&page.body, 1);
        }
        for command in &page.commands {
            self.heading(2, &command.name);
            let invocation = if command.name == "main" {
                format!("tola nu {}", page.locator)
            } else {
                format!("tola nu {} {}", page.locator, command.name)
            };
            if command.usage != invocation {
                self.blocks.push(self.code("sh", &command.usage, ""));
            }
            if !command.summary.is_empty() {
                self.markup(&command.summary, 0);
            }
            if !command.body.is_empty() {
                self.markup(&command.body, 2);
            }
            self.script_parameters(&command.parameters, &command.options);
            for example in &command.examples {
                self.example(example, "");
            }
            let label = match self.language {
                HelpLanguage::English => "Nu help",
                HelpLanguage::SimplifiedChinese => "查看 Nu 帮助",
            };
            let mut text = StyledText::default();
            text.push(label, Style::Plain);
            text.push(self.language.colon(), Style::Plain);
            text.push(&format!("`{invocation} --help`"), Style::Literal);
            self.blocks.push(self.wrapped(&text, ""));
        }
    }

    fn parameter_groups(&mut self, params: &[Parameter]) {
        let (pos, rest, named, req) = param_group_labels(self.language);
        let (open, close) = self.language.parentheses();
        for (heading, kind) in [(pos, "positional"), (rest, "rest"), (named, "named")] {
            let group: Vec<&Parameter> = params.iter().filter(|p| p.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            self.blocks.push(Style::Emphasis.paint(heading, self.color));
            let bullets = group
                .iter()
                .map(|p| {
                    let prefix = if p.kind == "rest" { ".." } else { "" };
                    let mut text = StyledText::default();
                    text.push(&format!("`{prefix}{}`", p.name), Style::Literal);
                    if p.required {
                        text.push(&format!("{open}{req}{close}"), Style::Plain);
                    }
                    if !p.description.is_empty() {
                        text.push(self.language.colon(), Style::Plain);
                        inline_markup_first_line(&p.description, &mut text);
                    }
                    self.bullet_line(&text, "")
                })
                .collect();
            self.push_bullets(bullets);
        }
    }

    fn script_parameters(&mut self, params: &[Parameter], opts: &[Parameter]) {
        let (pos_label, opt_label) = script_param_labels(self.language);
        if !params.is_empty() {
            self.blocks
                .push(Style::Emphasis.paint(pos_label, self.color));
            let bullets = params
                .iter()
                .map(|p| {
                    let prefix = if p.kind == "rest" { "..." } else { "" };
                    let mut text = StyledText::default();
                    text.push(&format!("`{prefix}{}`", p.name), Style::Literal);
                    if !p.description.is_empty() {
                        text.push(self.language.colon(), Style::Plain);
                        inline_markup_first_line(&p.description, &mut text);
                    }
                    self.bullet_line(&text, "")
                })
                .collect();
            self.push_bullets(bullets);
        }
        if !opts.is_empty() {
            self.blocks
                .push(Style::Emphasis.paint(opt_label, self.color));
            let bullets = opts
                .iter()
                .map(|opt| {
                    let mut text = StyledText::default();
                    let label = match opt.short {
                        Some(short) => format!("`{}`, `-{short}`", opt.name),
                        None => format!("`{}`", opt.name),
                    };
                    text.push(&label, Style::Literal);
                    if !opt.description.is_empty() {
                        text.push(self.language.colon(), Style::Plain);
                        inline_markup_first_line(&opt.description, &mut text);
                    }
                    self.bullet_line(&text, "")
                })
                .collect();
            self.push_bullets(bullets);
        }
    }

    fn example(&mut self, example: &Example, indent: &str) {
        if let Some(title) = &example.title {
            let mut text = StyledText::default();
            let prefix = match self.language {
                HelpLanguage::English => "Example: ",
                HelpLanguage::SimplifiedChinese => "示例：",
            };
            text.push(prefix, Style::Lead);
            if let Some(Block::Paragraph(inlines)) = title.blocks.first() {
                push_inlines(inlines, Style::Lead, &mut text);
            } else {
                text.push(&title.plain_text(), Style::Lead);
            }
            self.blocks.push(self.wrapped(&text, indent));
        }
        if let Some(description) = &example.description {
            self.markup_indented(description, indent, 2);
        }
        self.blocks
            .push(self.code(&example.lang, &example.code, indent));
    }

    fn related(&mut self, items: &[RelatedItem], heading_level: u8) {
        if items.is_empty() {
            return;
        }
        self.heading(heading_level, self.language.related());
        let lines = items
            .iter()
            .map(|item| {
                let mut text = StyledText::default();
                text.push(&format!("`{}`", item.display_label()), Style::Link);
                self.bullet_line(&text, "")
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.blocks.push(lines);
    }

    fn heading(&mut self, level: u8, text: &str) {
        let style = heading_style(level);
        let mut styled = StyledText::default();
        styled.push(text, style);
        self.blocks.push(self.wrapped(&styled, ""));
    }

    fn rule(&mut self) {
        self.blocks.push("-".repeat(self.columns));
    }

    fn markup(&mut self, markup: &Markup, heading_offset: u8) {
        self.markup_indented(markup, "", heading_offset);
    }

    fn markup_indented(&mut self, markup: &Markup, indent: &str, heading_offset: u8) {
        let rendered = self.render_blocks(&markup.blocks, indent, heading_offset);
        if !rendered.is_empty() {
            self.blocks.push(rendered);
        }
    }

    fn render_blocks(&self, blocks: &[Block], indent: &str, heading_offset: u8) -> String {
        let mut out = Vec::new();
        for block in blocks {
            let rendered = match block {
                Block::Heading { level, inlines } => {
                    let style = heading_style(level.saturating_add(heading_offset));
                    let mut text = StyledText::default();
                    push_inlines(inlines, style, &mut text);
                    self.wrapped(&text, indent)
                }
                Block::Paragraph(inlines) => {
                    let mut text = StyledText::default();
                    push_inlines(inlines, Style::Plain, &mut text);
                    self.wrapped(&text, indent)
                }
                Block::List { ordered, items } => {
                    self.list(*ordered, items, indent, heading_offset)
                }
                Block::Code { lang, source } => {
                    self.code(lang.as_deref().unwrap_or_default(), source, indent)
                }
                Block::Table { headers, rows } => self.table(headers, rows, indent),
            };
            if !rendered.is_empty() {
                out.push(rendered);
            }
        }
        out.join("\n\n")
    }

    fn list(
        &self,
        ordered: bool,
        items: &[Vec<Block>],
        indent: &str,
        heading_offset: u8,
    ) -> String {
        items
            .iter()
            .enumerate()
            .map(|(position, blocks)| {
                let marker = if ordered {
                    format!("{}. ", position + 1)
                } else {
                    "- ".to_owned()
                };
                let continuation = format!("{indent}{}", " ".repeat(marker.width()));
                let body = self.render_blocks(blocks, &continuation, heading_offset);
                format!(
                    "{indent}{}{}",
                    Style::Emphasis.paint(&marker, self.color),
                    body.strip_prefix(&continuation).unwrap_or(&body)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn bullet_line(&self, text: &StyledText, indent: &str) -> String {
        let marker = "- ";
        let continuation = format!("{indent}  ");
        let body = self.wrapped(text, &continuation);
        format!(
            "{indent}{}{}",
            Style::Emphasis.paint(marker, self.color),
            body.strip_prefix(&continuation).unwrap_or(&body)
        )
    }

    fn wrapped(&self, text: &StyledText, indent: &str) -> String {
        wrap::line_ranges(
            &text.text,
            self.columns.saturating_sub(indent.width()).max(1),
        )
        .into_iter()
        .map(|range| format!("{indent}{}", text.paint(range, self.color)))
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn highlight_theme(&self) -> tola_highlight::ThemeChoice {
        if self.color {
            tola_highlight::ThemeChoice::Terminal
        } else {
            tola_highlight::ThemeChoice::Plain
        }
    }

    fn signature(&self, source: &str) -> String {
        let text = tola_highlight::highlight_typst_signature(source, self.highlight_theme());
        paint_code_lines(source, &text, "")
    }

    fn code(&self, language: &str, source: &str, indent: &str) -> String {
        let language = language.split_whitespace().next().unwrap_or_default();
        let text = tola_highlight::highlight(source, language, self.highlight_theme());
        paint_code_lines(source, &text, indent)
    }

    fn table(&self, headers: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], indent: &str) -> String {
        let mut all_rows: Vec<Vec<StyledText>> = Vec::with_capacity(rows.len() + 1);
        all_rows.push(
            headers
                .iter()
                .map(|cell| {
                    let mut text = StyledText::default();
                    push_inlines(cell, Style::Plain, &mut text);
                    text
                })
                .collect(),
        );
        for row in rows {
            all_rows.push(
                row.iter()
                    .map(|cell| {
                        let mut text = StyledText::default();
                        push_inlines(cell, Style::Plain, &mut text);
                        text
                    })
                    .collect(),
            );
        }
        let column_count = headers.len();
        if column_count == 0 {
            return String::new();
        }
        let available = self.columns.saturating_sub(indent.width());
        let minimum: Vec<usize> = (0..column_count)
            .map(|column| {
                column_minimum(
                    all_rows
                        .iter()
                        .filter_map(|row| row.get(column))
                        .map(|cell| cell.text.as_str()),
                )
            })
            .collect();
        let natural: Vec<usize> = (0..column_count)
            .map(|column| {
                all_rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.text.width())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let separators = (column_count - 1) * 3;
        let Some(widths) = fitted_widths(&minimum, &natural, separators, available) else {
            return all_rows
                .iter()
                .skip(1)
                .map(|row| {
                    all_rows[0]
                        .iter()
                        .zip(row)
                        .map(|(header, cell)| {
                            let mut text = StyledText::default();
                            text.push(&header.text, Style::Emphasis);
                            text.push(": ", Style::Plain);
                            text.append(cell);
                            self.wrapped(&text, indent)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
        };
        let mut lines = Vec::new();
        for (row_number, row) in all_rows.iter().enumerate() {
            let cell_lines: Vec<Vec<Range<usize>>> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| wrap::line_ranges(&cell.text, *width))
                .collect();
            let height = cell_lines.iter().map(Vec::len).max().unwrap_or(1);
            for line_number in 0..height {
                let mut line = indent.to_owned();
                for (column, width) in widths.iter().enumerate() {
                    if column > 0 {
                        line.push_str(" | ");
                    }
                    let range = cell_lines[column].get(line_number).cloned().unwrap_or(0..0);
                    let cell = &row[column];
                    let padding = width.saturating_sub(cell.text[range.clone()].width());
                    let painted = cell.paint(range, self.color);
                    if row_number == 0 {
                        line.push_str(&Style::Emphasis.paint(&painted, self.color));
                    } else {
                        line.push_str(&painted);
                    }
                    if column + 1 < column_count {
                        line.push_str(&" ".repeat(padding));
                    }
                }
                lines.push(line);
            }
            if row_number == 0 {
                lines.push(format!(
                    "{indent}{}",
                    widths
                        .iter()
                        .map(|width| "-".repeat(*width))
                        .collect::<Vec<_>>()
                        .join("-+-")
                ));
            }
        }
        lines.join("\n")
    }
}

fn heading_style(level: u8) -> Style {
    match level {
        1 => Style::Heading1,
        2 => Style::Heading2,
        3 => Style::Heading3,
        _ => Style::Heading4,
    }
}

fn inline_markup_first_line(markup: &Markup, out: &mut StyledText) {
    if let Some(Block::Paragraph(inlines)) = markup.blocks.first() {
        push_inlines(inlines, Style::Plain, out);
    } else {
        out.push(&markup.plain_text(), Style::Plain);
    }
}

fn push_inlines(inlines: &[Inline], style: Style, text: &mut StyledText) {
    for inline in inlines {
        match inline {
            Inline::Text(source) => text.push(source, style),
            Inline::Code(source) => {
                text.push("`", Style::Literal);
                text.push(source, Style::Literal);
                text.push("`", Style::Literal);
            }
            Inline::Strong(children) | Inline::Emph(children) => {
                push_inlines(children, Style::Emphasis, text);
            }
            Inline::Link { url, label } => match label {
                Some(label) => {
                    push_inlines(label, Style::Link, text);
                    text.push(" (", Style::Plain);
                    text.push(url, Style::Link);
                    text.push(")", Style::Plain);
                }
                None => text.push(url, Style::Link),
            },
            Inline::Ref { target, label } => match label {
                Some(label) => push_inlines(label, Style::Link, text),
                None => {
                    text.push("`", Style::Literal);
                    text.push(target, Style::Literal);
                    text.push("`", Style::Literal);
                }
            },
        }
    }
}

pub(crate) fn column_minimum<'a>(cells: impl Iterator<Item = &'a str>) -> usize {
    cells
        .flat_map(str::split_whitespace)
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
        .clamp(12, 20)
}

pub(crate) fn fitted_widths(
    minimum: &[usize],
    natural: &[usize],
    separators: usize,
    available: usize,
) -> Option<Vec<usize>> {
    if minimum.iter().sum::<usize>() + separators > available {
        return None;
    }
    let mut widths = minimum
        .iter()
        .zip(natural)
        .map(|(minimum, natural)| (*natural).max(*minimum))
        .collect::<Vec<_>>();
    while widths.iter().sum::<usize>() + separators > available {
        let column = (0..widths.len())
            .filter(|&column| widths[column] > minimum[column])
            .max_by_key(|&column| widths[column])
            .expect("the minimum widths fit");
        widths[column] -= 1;
    }
    Some(widths)
}

fn format_default_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

// --- Markdown renderer ---

fn write_markdown_page(out: &mut String, page: &HelpPage, language: HelpLanguage) {
    match page {
        HelpPage::Overview(page) => write_markdown_overview(out, page, language),
        HelpPage::Index(page) => write_markdown_index(out, page, language),
        HelpPage::Config(page) => write_markdown_config(out, page, language),
        HelpPage::Package(page) => write_markdown_package(out, page, language),
        HelpPage::Script(page) => write_markdown_script(out, page, language),
    }
}

fn write_markdown_overview(out: &mut String, page: &OverviewPage, _language: HelpLanguage) {
    writeln!(out, "# {}\n", page.title).unwrap();
    write_markdown_blocks(out, &page.body.blocks, "", 1);
    out.push('\n');
    for category in &page.categories {
        writeln!(out, "- `{}`", category.title).unwrap();
    }
}

fn write_markdown_index(out: &mut String, page: &IndexPage, language: HelpLanguage) {
    writeln!(out, "# {}\n", page.title).unwrap();
    write_markdown_blocks(out, &page.summary.blocks, "", 1);
    out.push('\n');
    for entry in &page.entries {
        if entry.summary.is_empty() {
            writeln!(out, "- `{}`", entry.title).unwrap();
        } else {
            writeln!(
                out,
                "- `{}`{}{}",
                entry.title,
                language.colon(),
                markdown_first_line(&entry.summary)
            )
            .unwrap();
        }
    }
}

fn write_markdown_config(out: &mut String, page: &ConfigPage, language: HelpLanguage) {
    writeln!(out, "# {}\n", page.header).unwrap();
    if !page.summary.is_empty() {
        write_markdown_blocks(out, &page.summary.blocks, "", 1);
        out.push('\n');
    }
    if !page.keys.is_empty() {
        let heading = match language {
            HelpLanguage::English => "Keys",
            HelpLanguage::SimplifiedChinese => "配置项",
        };
        writeln!(out, "## {heading}\n").unwrap();
        for key in &page.keys {
            write!(out, "- `{}`", key.name).unwrap();
            if let Some(default) = &key.default {
                let (open, close) = language.parentheses();
                let label = match language {
                    HelpLanguage::English => "default: ",
                    HelpLanguage::SimplifiedChinese => "默认：",
                };
                write!(
                    out,
                    "{open}{label}`{}`{close}",
                    format_default_value(default)
                )
                .unwrap();
            }
            if !key.body.is_empty() {
                writeln!(
                    out,
                    "{}{}",
                    language.colon(),
                    markdown_first_line(&key.body)
                )
                .unwrap();
                if key.body.blocks.len() > 1 {
                    out.push('\n');
                    write_markdown_blocks(out, &key.body.blocks[1..], "  ", 2);
                }
            } else {
                out.push('\n');
            }
            for example in &key.examples {
                out.push('\n');
                write_markdown_example(out, example, language, "  ");
            }
        }
        out.push('\n');
    }
    if !page.children.is_empty() {
        let heading = match language {
            HelpLanguage::English => "Child tables",
            HelpLanguage::SimplifiedChinese => "子表",
        };
        writeln!(out, "## {heading}\n").unwrap();
        for child in &page.children {
            writeln!(
                out,
                "- `{}`{}{}",
                child.header,
                language.colon(),
                markdown_first_line(&child.summary)
            )
            .unwrap();
        }
        out.push('\n');
    }
    if !page.body.is_empty() {
        write_markdown_blocks(out, &page.body.blocks, "", 2);
        out.push('\n');
    }
    for example in &page.examples {
        write_markdown_example(out, example, language, "");
        out.push('\n');
    }
    write_markdown_related(out, &page.related, 2, language);
}

fn write_markdown_package(out: &mut String, page: &PackagePage, language: HelpLanguage) {
    writeln!(out, "# {}\n", page.title).unwrap();
    let import_heading = match language {
        HelpLanguage::English => "Import",
        HelpLanguage::SimplifiedChinese => "导入",
    };
    writeln!(out, "## {import_heading}\n").unwrap();
    out.push_str(&code_block("typst", &page.import));
    out.push('\n');
    if !page.summary.is_empty() {
        write_markdown_blocks(out, &page.summary.blocks, "", 1);
        out.push('\n');
    }
    if !page.body.is_empty() {
        write_markdown_blocks(out, &page.body.blocks, "", 1);
        out.push('\n');
    }
    for example in &page.examples {
        write_markdown_example(out, example, language, "");
        out.push('\n');
    }
    if !page.selected {
        let heading = match language {
            HelpLanguage::English => "Exports",
            HelpLanguage::SimplifiedChinese => "导出成员",
        };
        writeln!(out, "## {heading}\n").unwrap();
        for export in &page.exports {
            writeln!(
                out,
                "- `{}`{}{}",
                export.name,
                language.colon(),
                markdown_first_line(&export.summary)
            )
            .unwrap();
        }
        out.push('\n');
    }
    write_markdown_related(out, &page.related, 2, language);
    for (index, export) in page.exports.iter().enumerate() {
        if index > 0 || !page.selected {
            out.push_str("---\n\n");
        }
        writeln!(out, "## {} - {}\n", export.name, export.kind).unwrap();
        out.push_str(&code_block("typc", &export.declaration));
        out.push('\n');
        if !export.summary.is_empty() {
            write_markdown_blocks(out, &export.summary.blocks, "", 2);
            out.push('\n');
        }
        if !export.body.is_empty() {
            write_markdown_blocks(out, &export.body.blocks, "", 2);
            out.push('\n');
        }
        write_markdown_params(out, &export.parameters, language);
        for example in &export.examples {
            write_markdown_example(out, example, language, "");
            out.push('\n');
        }
        for method in &export.methods {
            writeln!(out, "### `{}`\n", method.name).unwrap();
            out.push_str(&code_block("typc", &method.declaration));
            out.push('\n');
            write_markdown_blocks(out, &method.receiver_note.blocks, "", 3);
            out.push('\n');
            if !method.summary.is_empty() {
                write_markdown_blocks(out, &method.summary.blocks, "", 3);
                out.push('\n');
            }
            if !method.body.is_empty() {
                write_markdown_blocks(out, &method.body.blocks, "", 3);
                out.push('\n');
            }
            write_markdown_params(out, &method.parameters, language);
            for example in &method.examples {
                write_markdown_example(out, example, language, "");
                out.push('\n');
            }
        }
        write_markdown_related(out, &export.related, 3, language);
    }
}

fn paint_code_lines(
    source: &str,
    highlighted: &tola_highlight::Highlighted<'_>,
    indent: &str,
) -> String {
    let mut rendered = String::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let end = offset + line.strip_suffix('\n').unwrap_or(line).len();
        rendered.push_str(indent);
        rendered.push_str("  ");
        highlighted.paint(offset..end, &mut rendered);
        if line.ends_with('\n') {
            rendered.push('\n');
        }
        offset += line.len();
    }
    if rendered.ends_with('\n') {
        rendered.pop();
    }
    rendered
}

fn write_markdown_script(out: &mut String, page: &ScriptPage, language: HelpLanguage) {
    writeln!(out, "# {}\n", page.locator).unwrap();
    if !page.summary.is_empty() {
        write_markdown_blocks(out, &page.summary.blocks, "", 1);
        out.push('\n');
    }
    if !page.body.is_empty() {
        write_markdown_blocks(out, &page.body.blocks, "", 1);
        out.push('\n');
    }
    for command in &page.commands {
        writeln!(out, "## `{}`\n", command.name).unwrap();
        let invocation = if command.name == "main" {
            format!("tola nu {}", page.locator)
        } else {
            format!("tola nu {} {}", page.locator, command.name)
        };
        if command.usage != invocation {
            out.push_str(&code_block("sh", &command.usage));
            out.push('\n');
        }
        if !command.summary.is_empty() {
            write_markdown_blocks(out, &command.summary.blocks, "", 2);
            out.push('\n');
        }
        if !command.body.is_empty() {
            write_markdown_blocks(out, &command.body.blocks, "", 2);
            out.push('\n');
        }
        let (pos_label, opt_label) = script_param_labels(language);
        if !command.parameters.is_empty() {
            writeln!(out, "**{pos_label}**\n").unwrap();
            for p in &command.parameters {
                let prefix = if p.kind == "rest" { "..." } else { "" };
                writeln!(
                    out,
                    "- `{prefix}{}`{}{}",
                    p.name,
                    language.colon(),
                    markdown_first_line(&p.description)
                )
                .unwrap();
            }
            out.push('\n');
        }
        if !command.options.is_empty() {
            writeln!(out, "**{opt_label}**\n").unwrap();
            for opt in &command.options {
                let label = match opt.short {
                    Some(short) => format!("`{}`, `-{short}`", opt.name),
                    None => format!("`{}`", opt.name),
                };
                writeln!(
                    out,
                    "- {label}{}{}",
                    language.colon(),
                    markdown_first_line(&opt.description)
                )
                .unwrap();
            }
            out.push('\n');
        }
        for example in &command.examples {
            write_markdown_example(out, example, language, "");
            out.push('\n');
        }
    }
}

fn write_markdown_params(out: &mut String, params: &[Parameter], lang: HelpLanguage) {
    let (pos, rest, named, req) = param_group_labels(lang);
    let (open, close) = lang.parentheses();
    for (heading, kind) in [(pos, "positional"), (rest, "rest"), (named, "named")] {
        let group: Vec<&Parameter> = params.iter().filter(|p| p.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        writeln!(out, "**{heading}**\n").unwrap();
        for p in group {
            let prefix = if p.kind == "rest" { ".." } else { "" };
            write!(out, "- `{prefix}{}`", p.name).unwrap();
            if p.required {
                write!(out, "{open}{req}{close}").unwrap();
            }
            if !p.description.is_empty() {
                writeln!(
                    out,
                    "{}{}",
                    lang.colon(),
                    markdown_first_line(&p.description)
                )
                .unwrap();
            } else {
                out.push('\n');
            }
        }
        out.push('\n');
    }
}

fn write_markdown_example(
    out: &mut String,
    example: &Example,
    language: HelpLanguage,
    indent: &str,
) {
    if let Some(title) = &example.title {
        let prefix = match language {
            HelpLanguage::English => "Example: ",
            HelpLanguage::SimplifiedChinese => "示例：",
        };
        writeln!(out, "{indent}**{prefix}{}**\n", markdown_first_line(title)).unwrap();
    }
    if let Some(description) = &example.description {
        write_markdown_blocks(out, &description.blocks, indent, 2);
        out.push('\n');
    }
    for line in code_block(&example.lang, &example.code).lines() {
        writeln!(out, "{indent}{line}").unwrap();
    }
}

fn write_markdown_related(
    out: &mut String,
    items: &[RelatedItem],
    level: usize,
    language: HelpLanguage,
) {
    if items.is_empty() {
        return;
    }
    writeln!(out, "{} {}\n", "#".repeat(level), language.related()).unwrap();
    for item in items {
        writeln!(out, "- `{}`", item.display_label()).unwrap();
    }
    out.push('\n');
}

fn write_markdown_blocks(out: &mut String, blocks: &[Block], indent: &str, heading_offset: u8) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match block {
            Block::Heading { level, inlines } => {
                let hashes = "#".repeat(level.saturating_add(heading_offset).min(6) as usize);
                writeln!(out, "{indent}{hashes} {}", markdown_inlines(inlines)).unwrap();
            }
            Block::Paragraph(inlines) => {
                writeln!(out, "{indent}{}", markdown_inlines(inlines)).unwrap();
            }
            Block::List { ordered, items } => {
                for (position, item_blocks) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", position + 1)
                    } else {
                        "- ".to_owned()
                    };
                    if let Some((first, rest)) = item_blocks.split_first() {
                        let first_line = match first {
                            Block::Paragraph(inlines) => markdown_inlines(inlines),
                            _ => String::new(),
                        };
                        writeln!(out, "{indent}{marker}{first_line}").unwrap();
                        if !rest.is_empty() {
                            let child_indent = format!("{indent}{}", " ".repeat(marker.len()));
                            write_markdown_blocks(out, rest, &child_indent, heading_offset);
                        }
                    }
                }
            }
            Block::Code { lang, source } => {
                for line in code_block(lang.as_deref().unwrap_or_default(), source).lines() {
                    writeln!(out, "{indent}{line}").unwrap();
                }
            }
            Block::Table { headers, rows } => {
                let header_line = headers
                    .iter()
                    .map(|cell| markdown_inlines(cell))
                    .collect::<Vec<_>>()
                    .join(" | ");
                let sep = vec!["---"; headers.len()].join(" | ");
                writeln!(out, "{indent}| {header_line} |").unwrap();
                writeln!(out, "{indent}| {sep} |").unwrap();
                for row in rows {
                    let line = row
                        .iter()
                        .map(|cell| markdown_inlines(cell))
                        .collect::<Vec<_>>()
                        .join(" | ");
                    writeln!(out, "{indent}| {line} |").unwrap();
                }
            }
        }
    }
}

fn markdown_first_line(markup: &Markup) -> String {
    match markup.blocks.first() {
        Some(Block::Paragraph(inlines)) => markdown_inlines(inlines),
        _ => markup.plain_text(),
    }
}

fn markdown_inlines(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Code(code) => {
                let ticks = longest_backtick_run(code) + 1;
                let fence = "`".repeat(ticks);
                if code.starts_with('`') || code.ends_with('`') {
                    write!(out, "{fence} {code} {fence}").unwrap();
                } else {
                    write!(out, "{fence}{code}{fence}").unwrap();
                }
            }
            Inline::Strong(inner) => write!(out, "**{}**", markdown_inlines(inner)).unwrap(),
            Inline::Emph(inner) => write!(out, "*{}*", markdown_inlines(inner)).unwrap(),
            Inline::Link { url, label } => match label {
                Some(label) => write!(out, "[{}]({url})", markdown_inlines(label)).unwrap(),
                None => out.push_str(url),
            },
            Inline::Ref { target, label } => match label {
                Some(label) => out.push_str(&markdown_inlines(label)),
                None => write!(out, "`{target}`").unwrap(),
            },
        }
    }
    out
}

fn longest_backtick_run(source: &str) -> usize {
    source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

pub(super) fn code_block(language: &str, source: &str) -> String {
    let fence = "`".repeat((longest_backtick_run(source) + 1).max(3));
    let newline = if source.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{source}{newline}{fence}\n")
}
