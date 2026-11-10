//! The bibliography entries the site's citations name, read and rendered as Typst reads them.

mod keys;
mod render;
mod sources;

use anyhow::Result;
use keys::key_location;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Hover, Location, Uri,
};
use render::{rendered, unread_note};
pub(crate) use sources::BibliographyCache;
use sources::{Bibliography, spelled_sources};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;
use tola_typst::typst::syntax::{FileId, LinkedNode, Source};

/// The hover one citation answers with; a site naming no bibliography leaves `@key` to its labels.
pub(super) fn hover(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    open: FileId,
    key: &str,
    cache: &mut BibliographyCache,
    cancellation: &BuildCancellation,
) -> Result<Option<Hover>> {
    let bibliography = Bibliography::read(compilation, config, open, cache, cancellation)?;
    if !bibliography.named {
        return Ok(None);
    }
    let described = match bibliography.entry(key) {
        Some(declared) => format!(
            "{}\n\nDeclared in `{}`.",
            rendered(&declared),
            declared.file.vpath().get_without_slash()
        ),
        None => {
            let mut described = format!("no bibliography declares `{key}`; add the entry to one");
            if !bibliography.unreadable.is_empty() {
                described.push_str("\n\n");
                described.push_str(&unread_note(&bibliography.unreadable));
            }
            described
        }
    };
    Ok(Some(crate::protocol::markdown_hover(described, None)))
}

/// Where one citation's entry is written, or `None` when no bibliography declares `key`.
pub(super) fn definition(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    open: FileId,
    key: &str,
    cache: &mut BibliographyCache,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<Location>> {
    let bibliography = Bibliography::read(compilation, config, open, cache, cancellation)?;
    let Some(declared) = bibliography.entry(key) else {
        return Ok(None);
    };
    Ok(key_location(compilation.world(), &declared, client_root))
}

/// The entries a citation may name, by key or title prefix, ignoring case.
pub(super) fn completions(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    source: &Source,
    cursor: usize,
    cache: &mut BibliographyCache,
    cancellation: &BuildCancellation,
) -> Result<Vec<CompletionItem>> {
    let Some(reference) = super::labels::referenced(source, cursor) else {
        return Ok(Vec::new());
    };
    let typed = super::labels::typed_name(source, &reference, cursor);
    let bibliography = Bibliography::read(compilation, config, source.id(), cache, cancellation)?;
    let mut items = Vec::new();
    for declared in bibliography.entries() {
        let key = declared.entry.key();
        let key_matches = starts_with_ignoring_case(key, typed);
        let title = declared.entry.title().map(ToString::to_string);
        let title_matches = title
            .as_deref()
            .is_some_and(|title| starts_with_ignoring_case(title, typed));
        if !key_matches && !title_matches {
            continue;
        }
        let edit = reference.completion_edit(source, cursor, key.to_owned())?;
        if key_matches {
            items.push(CompletionItem {
                label: key.to_owned(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some(declared.described.to_owned()),
                text_edit: Some(edit.clone()),
                ..Default::default()
            });
        }
        let Some(title) = title else {
            continue;
        };
        items.push(CompletionItem {
            label: title.clone(),
            kind: Some(CompletionItemKind::CONSTANT),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(key.to_owned()),
            }),
            filter_text: Some(title),
            detail: Some(declared.described.to_owned()),
            text_edit: Some(edit),
            ..Default::default()
        });
    }
    Ok(items)
}

/// Whether `text` has `prefix`, ignoring the case keys are written in.
fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// The citation one references request asks about.
pub(super) struct CitationRequest<'a> {
    /// The source the request came from, whose own spellings always count.
    pub(super) open: FileId,
    /// The key the cursor names.
    pub(super) key: &'a str,
    /// Whether the entry's own declaration is one of the spellings.
    pub(super) include_declaration: bool,
}

/// Every place the site spells the key; its bibliography entry is the declaration.
pub(super) fn references(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    request: &CitationRequest<'_>,
    cache: &mut BibliographyCache,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Vec<Location>> {
    let bibliography = Bibliography::read(compilation, config, request.open, cache, cancellation)?;
    let Some(declared) = bibliography.entry(request.key) else {
        return Ok(Vec::new());
    };
    let world = compilation.world();
    let mut found = Vec::new();
    if request.include_declaration
        && let Some(location) = key_location(world, &declared, client_root)
    {
        found.push(location);
    }
    for id in spelled_sources(compilation, config, request.open, cancellation)? {
        cancellation.ensure_active()?;
        let Ok(source) = world.source(id) else {
            continue;
        };
        let Some(uri) = crate::identity::client_uri(id, config.get_root(), client_root).ok() else {
            continue;
        };
        found.extend(spellings_in(&source, &uri, request.key));
    }
    found.sort_by(|left, right| {
        (
            left.uri.as_str(),
            left.range.start.line,
            left.range.start.character,
        )
            .cmp(&(
                right.uri.as_str(),
                right.range.start.line,
                right.range.start.character,
            ))
    });
    found.dedup();
    Ok(found)
}

/// Every place one source spells `key`, as a location in `uri`.
fn spellings_in(source: &Source, uri: &Uri, key: &str) -> Vec<Location> {
    let mut spellings = Vec::new();
    super::labels::collect_spellings(&LinkedNode::new(source.root()), key, &mut spellings);
    spellings
        .into_iter()
        .filter_map(|spelling| {
            let range = crate::position::utf16_range(source.lines(), spelling.range)?;
            Some(Location {
                uri: uri.clone(),
                range,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::{CompletionTextEdit, GotoDefinitionResponse, Position};

    #[test]
    fn citation_keys_ignore_case() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        for marked in [
            "#bibliography(\"refs.bib\")\n\nSee @knu|\n",
            "#bibliography(\"refs.bib\")\n\nSee @Kn|\n",
        ] {
            let items = site.completion(marked);
            assert!(
                completion_labels(&items).contains(&"knuth1984"),
                "{marked}: {items:?}"
            );
        }
    }

    /// A YAML key after multibyte text still points at the key itself: the scanner counts
    /// characters, and a range that addresses bytes has both mapped.
    #[test]
    fn yaml_keys_address_bytes() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.yml",
            "x:\n  type: article\n  title: Über alles\n  author: Müller, Jürgen\n  date: 1990\n\nzweiter:\n  type: article\n  title: Zweite\n  author: Schmidt, Anna\n  date: 1995\n",
        );
        let definition = site
            .definition("#bibliography(\"refs.yml\")\n\nSee @zwei|ter.\n")
            .expect("a definition inside the YAML bibliography");
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("a scalar definition");
        };
        assert_eq!(
            (location.range.start.line, location.range.start.character),
            (6, 0)
        );
        assert_eq!(
            (location.range.end.line, location.range.end.character),
            (6, 7)
        );
    }

    /// Each bibliography renders its entries with the style its own call names.
    #[test]
    fn bibliographies_keep_their_style() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        site.site.write(
            "content/other.bib",
            "@article{onlyb,\n  title = {Second File Work},\n  author = {Doe, Jane},\n  year = {2001},\n}\n",
        );
        site.site.write(
            "content/style.csl",
            r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">
  <info><title>Probe</title><id>probe</id><updated>2020-01-01T00:00:00+00:00</updated></info>
  <citation><layout><text variable="title" prefix="[C]"/></layout></citation>
  <bibliography><layout><text variable="title" prefix="[B]"/></layout></bibliography>
</style>
"#,
        );
        site.site.write(
            "content/other.typ",
            "#bibliography(\"other.bib\", style: \"style.csl\")\n",
        );
        let styled = site
            .hover_text("= Doc\n\n#bibliography(\"refs.bib\")\n\nSee @only|b.\n")
            .expect("a hover on the styled bibliography");
        let styled = crate::markdown::plain(&styled);
        assert!(styled.contains("[B]Second File Work"), "{styled}");
        let plain = site
            .hover_text("= Doc\n\n#bibliography(\"refs.bib\")\n\nSee @knuth|1984.\n")
            .expect("a hover on the site's own style");
        let plain = crate::markdown::plain(&plain);
        assert!(plain.contains("Knuth"), "{plain}");
        assert!(!plain.contains("[B]"), "{plain}");
    }

    #[test]
    fn title_completion_writes_the_key() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @liT|\n");
        let titled = item_labeled(&items, "Literate Programming");
        assert_eq!(titled.filter_text.as_deref(), Some("Literate Programming"));
        let Some(lsp_types::CompletionItemLabelDetails { description, .. }) = &titled.label_details
        else {
            panic!("the title item names the key it writes");
        };
        assert_eq!(description.as_deref(), Some("knuth1984"));
        let Some(CompletionTextEdit::Edit(edit)) = &titled.text_edit else {
            panic!("the title item edits the key in place");
        };
        assert_eq!(edit.new_text, "knuth1984");
        assert_eq!(edit.range.start, Position::new(2, 5));
        assert_eq!(edit.range.end, Position::new(2, 8));

        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @unrelated|\n");
        assert!(!completion_labels(&items).contains(&"Literate Programming"));
    }

    /// A citation key answers where the site spells it, and its declaration when the request asks
    /// for one.
    #[test]
    fn citation_keys_answer_their_references() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let program = "#bibliography(\"refs.bib\")\n\nSee @knu|th1984.\n";
        let uses = site
            .references(program, false)
            .expect("the citation's uses");
        assert_eq!(uses.len(), 1, "{uses:?}");
        assert!(
            uses[0].uri.as_str().ends_with("content/document.typ"),
            "{:?}",
            uses[0].uri
        );
        let all = site
            .references(program, true)
            .expect("the citation's uses and declaration");
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(
            all.iter()
                .any(|location| location.uri.as_str().ends_with("content/refs.bib")),
            "{all:?}"
        );
    }

    /// A bibliography whose text no reader understood is named, so the author reads the diagnostic
    /// pointing into it instead of hunting a misspelled key.
    #[test]
    fn unreadable_bibliography_is_named() {
        let mut site = QuerySession::new();
        site.site
            .write("content/bad.bib", "@article{broken,\n  title = {Unclosed\n");
        let hover = site
            .hover_text("#bibliography(\"bad.bib\")\n\nSee @bro|ken.\n")
            .expect("a hover for a key of an unreadable bibliography");
        assert!(hover.contains("content/bad.bib"), "{hover}");
        assert!(hover.contains("Tola could not read"), "{hover}");
    }
}
