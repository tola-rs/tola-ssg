//! Assembly of the generated Typst program.

use super::features::{HeadFragment, SeoCode, SeoOutput};

pub(super) const SITE_PATH: &str = "site.typ";
pub(super) const SCHEMA_PATH: &str = "site/schema.typ";
pub(super) const SCHEMA_SOURCE: &str = include_str!("templates/schema.typ");
pub(super) const PAGE_PATH: &str = "site/page.typ";
const PAGE_SOURCE: &str = include_str!("templates/page.typ");
pub(super) const NOT_FOUND_PATH: &str = "site/not-found.typ";
const NOT_FOUND_SOURCE: &str = include_str!("templates/not-found.typ");
pub(super) const SELECTION_PATH: &str = "site/selection.typ";
pub(super) const SELECTION_SOURCE: &str = include_str!("templates/selection.typ");
pub(super) const SEO_PATH: &str = "site/seo.typ";
const SEO_SOURCE: &str = include_str!("templates/seo.typ");

pub(super) fn page_source(head: &[HeadFragment]) -> String {
    page_template(PAGE_SOURCE, head)
}

pub(super) fn not_found_source(head: &[HeadFragment]) -> String {
    page_template(NOT_FOUND_SOURCE, head)
}

fn page_template(source: &str, head: &[HeadFragment]) -> String {
    let imports = fragment_imports(head);
    let source = replace_template(source, "{{imports}}\n", &imports);
    replace_template(&source, "{{head}}\n", &fragment_entries(head))
}

pub(super) fn seo_source(seo: &[SeoCode]) -> String {
    let source = replace_template(SEO_SOURCE, "{{imports}}", &seo_imports(seo));
    let source = replace_template(&source, "{{entries}}", &seo_entries(seo));
    replace_template(&source, "{{outputs}}", &seo_outputs(seo))
}

fn seo_imports(seo: &[SeoCode]) -> String {
    let mut lines: Vec<(&'static str, Vec<&'static str>)> = vec![
        ("@tola/site:0.0.0", vec!["site"]),
        ("@tola/web:0.0.0", vec!["head-metadata"]),
    ];
    // A feature's own names first, then the names its shared values read: a value and its import
    // come from one declaration, so a feature never repeats it.
    let names = seo.iter().flat_map(|code| {
        code.imports
            .iter()
            .chain(code.prelude.iter().flat_map(|value| value.imports()))
    });
    for (source, names) in names {
        let (source, names) = (*source, *names);
        if !lines.iter().any(|(existing, _)| *existing == source) {
            lines.push((source, Vec::new()));
        }
        let named = &mut lines
            .iter_mut()
            .find(|(existing, _)| *existing == source)
            .expect("the line was just added")
            .1;
        for name in names {
            if !named.contains(name) {
                named.push(name);
            }
        }
    }
    lines
        .iter()
        .map(|(source, names)| format!("#import \"{source}\": {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn seo_entries(seo: &[SeoCode]) -> String {
    let contributing = seo
        .iter()
        .filter(|code| !code.entries.trim().is_empty())
        .collect::<Vec<_>>();
    if contributing.is_empty() {
        return String::new();
    }
    let mut needed = Vec::new();
    for code in &contributing {
        for value in code.prelude {
            if !needed.contains(value) {
                needed.push(*value);
            }
        }
    }
    let mut out = String::from("\n  if page != none {\n");
    for value in needed {
        out.push_str(value.bindings());
    }
    for code in contributing {
        out.push_str(code.entries);
        out.push('\n');
    }
    out.push_str("  }\n");
    out
}

fn seo_outputs(seo: &[SeoCode]) -> String {
    seo.iter()
        .filter_map(|code| code.output)
        .map(|output| format!("\n\n{}", output.definition))
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn site_program(outputs: &[SeoOutput]) -> String {
    let mut source = include_str!("templates/site-imports.typ").to_owned();
    if !outputs.is_empty() {
        let names = outputs
            .iter()
            .map(|output| output.export)
            .collect::<Vec<_>>()
            .join(", ");
        source.push_str(&format!("#import \"site/seo.typ\": {names}\n"));
    }
    source.push_str(include_str!("templates/site.typ"));
    for output in outputs {
        source.push_str(&format!("#{}(\"{}\", pages)\n", output.export, output.file));
    }
    source
}

fn fragment_imports(head: &[HeadFragment]) -> String {
    head.iter()
        .map(|fragment| fragment.import)
        .filter(|import| !import.is_empty())
        .map(|import| format!("{import}\n"))
        .collect()
}

fn fragment_entries(head: &[HeadFragment]) -> String {
    head.iter()
        .flat_map(|fragment| fragment.head.lines())
        .map(|line| format!("        {line}\n"))
        .collect()
}

fn replace_template(source: &str, anchor: &str, replacement: &str) -> String {
    assert!(
        source.matches(anchor).count() == 1,
        "the scaffold template must contain exactly one `{anchor}`"
    );
    source.replacen(anchor, replacement, 1)
}
