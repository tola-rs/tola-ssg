//! The bibliography entries the site's citations name.
//!
//! Typst reads `@key` in markup as a reference: a label of the site answers it first, and the
//! bibliographies the site names answer it when no label does, which is what a citation is. Every
//! entry is read from the files those `bibliography(…)` calls name, by the crates Typst itself reads
//! them with, so an editor answer and a published page agree.
//!
//! A compiled site realizes every bibliography as an element that says which files it reads and
//! which style and language it renders with — whether the call spells a string, a path, an array,
//! or a value bound elsewhere. The editor reads exactly those, and renders each entry the way the
//! call that declares it renders. A site whose program did not compile is searched for the calls
//! its sources spell.
//!
//! An answer has the bibliography item, not the citation: a citation renders as the number the
//! page happens to give it, and an author hovering a key asks which work it names. An entry whose
//! call the site did not compile answers with its own fields instead.

mod keys;
mod render;
mod sources;

use anyhow::Result;
use keys::key_location;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Hover, Location};
use render::{rendered, unread_note};
pub(crate) use sources::BibliographyCache;
use sources::{Bibliography, spelled_sources};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;
use tola_typst::typst::syntax::{FileId, LinkedNode, Source};

/// The hover one citation answers with, or `None` when the site names no bibliography at all.
///
/// A site that names none leaves `@key` to its labels, which answer for themselves. A site that
/// names one answers which entry a key reaches, and says so when none reaches it: the author who
/// wrote the key is reading a citation, not a missing label.
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
    key_location(compilation.world(), &declared, client_root)
}

/// The entries a citation may name, each filling the key the author is typing.
///
/// An entry also completes by its title: keys are hard to remember, and accepting the title writes
/// the key. The typed prefix matches without regard to case, which is how authors spell keys.
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
        if !key_matches
            && !title
                .as_deref()
                .is_some_and(|title| starts_with_ignoring_case(title, typed))
        {
            continue;
        }
        let edit = super::completion::completion_edit(
            source,
            cursor,
            reference.name.clone(),
            key.to_owned(),
        )?;
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

/// Every place the site spells the citation key the request names.
///
/// A bibliography can be cited from any page, so the spellings are the ones every source the site
/// holds has. The entry in its bibliography file is the declaration, which the request decides
/// whether to include.
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
        && let Some(location) = key_location(world, &declared, client_root)?
    {
        found.push(location);
    }
    for id in spelled_sources(compilation, config, request.open, cancellation)? {
        cancellation.ensure_active()?;
        let Ok(source) = world.source(id) else {
            continue;
        };
        let Some(uri) = crate::identity::client_uri(id, client_root) else {
            continue;
        };
        let mut spellings = Vec::new();
        super::labels::collect_spellings(
            &LinkedNode::new(source.root()),
            request.key,
            &mut spellings,
        );
        found.extend(spellings.into_iter().filter_map(|spelling| {
            Some(Location {
                uri: uri.clone(),
                range: crate::position::utf16_range(source.lines(), spelling.range)?,
            })
        }));
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

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::{CompletionTextEdit, GotoDefinitionResponse, Position};

    /// A citation completes the keys the site's bibliographies declare.
    #[test]
    fn citations_complete_declared_keys() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @knu|\n");
        let labels = completion_labels(&items);
        assert!(labels.contains(&"knuth1984"), "{labels:?}");
    }

    /// A citation answers the entry a bibliography declares, and says so when none reaches the key.
    #[test]
    fn citations_answer_their_bibliography() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let program = "#bibliography(\"refs.bib\")\n\nSee @knuth|1984.\n";
        let hover = site.hover_text(program).expect("a citation hover");
        // The site's own style renders the reference, not the raw `.bib` fields.
        assert!(hover.contains("D. E. Knuth"), "{hover}");
        assert!(hover.contains("Literate Programming"), "{hover}");
        assert!(hover.contains("content/refs.bib"), "{hover}");
        assert!(
            site.definition(program).is_some(),
            "a definition inside the bibliography"
        );

        let near_miss = site
            .hover_text("#bibliography(\"refs.bib\")\n\nSee @knuth|2000.\n")
            .expect("a hover for a key no entry reaches");
        assert!(
            near_miss.contains("no bibliography declares"),
            "{near_miss}"
        );
    }

    /// A name the library declares answers with the signature it takes, then the documentation Typst
    /// has for it.
    #[test]
    fn library_name_answers_its_signature() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#set te|xt(lang: \"de\")\n")
            .expect("a hover for the standard library");
        assert!(hover.contains("text("), "{hover}");
        assert!(hover.contains("Customizes the look"), "{hover}");
    }

    /// A YAML bibliography answers like any other: the entry renders, and its key is reachable.
    #[test]
    fn yaml_bibliography_answers_its_entry() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.yml",
            "tarry:\n    type: Book\n    title: Harry Potter and the Order of the Phoenix\n    author: Rowling, J. K.\n    date: 2003-06-21\n",
        );
        let program = "#bibliography(\"refs.yml\")\n\nSee @tar|ry.\n";
        let hover = site.hover_text(program).expect("a citation hover");
        assert!(hover.contains("Harry Potter"), "{hover}");
        assert!(hover.contains("content/refs.yml"), "{hover}");
        assert!(
            site.definition(program).is_some(),
            "a definition inside the YAML bibliography"
        );
    }

    /// Every bibliography the site compiles answers, not only the one its first call names.
    #[test]
    fn every_compiled_bibliography_answers() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        site.site.write(
            "content/other.bib",
            "@article{onlyb,\n  title = {Second File Work},\n  author = {Doe, Jane},\n  year = {2001},\n}\n",
        );
        // The site realizes this document's call first, and `other.typ` names a second bibliography.
        site.site.write(
            "content/other.typ",
            "= Other\n\n#bibliography(\"other.bib\")\n",
        );
        let program = "#bibliography(\"refs.bib\")\n\nSee @only|b.\n";
        let hover = site
            .hover_text(program)
            .expect("a hover on the second bibliography");
        assert!(hover.contains("Second File Work"), "{hover}");
        assert!(hover.contains("content/other.bib"), "{hover}");
        assert!(
            site.definition(program).is_some(),
            "a definition inside the second bibliography"
        );
        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @only|\n");
        let labels = completion_labels(&items);
        assert!(labels.contains(&"onlyb"), "{labels:?}");
    }

    /// A call's sources may be an array, a value bound elsewhere, or a `path(…)`, and the editor
    /// reads what the site compiled either way.
    #[test]
    fn bibliographies_read_arrays_values_and_paths() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        site.site.write(
            "content/other.bib",
            "@article{onlyb,\n  title = {Second File Work},\n  author = {Doe, Jane},\n  year = {2001},\n}\n",
        );
        let array = "#bibliography((\"refs.bib\", \"other.bib\"))\n\nSee @only|b.\n";
        let hover = site.hover_text(array).expect("a hover on an array source");
        assert!(hover.contains("Second File Work"), "{hover}");
        let bound =
            "#let refs = (\"refs.bib\", \"other.bib\")\n#bibliography(refs)\n\nSee @only|b.\n";
        let hover = site.hover_text(bound).expect("a hover on a bound source");
        assert!(hover.contains("Second File Work"), "{hover}");
        let spelled = "#bibliography(path(\"refs.bib\"))\n\nSee @knuth|1984.\n";
        let hover = site.hover_text(spelled).expect("a hover on a path source");
        assert!(hover.contains("Literate Programming"), "{hover}");
    }

    /// A site whose program did not compile still reads the bibliography calls its sources spell.
    #[test]
    fn failed_site_reads_spelled_bibliography_sources() {
        let mut site = QuerySession::with_program("#let broken =\n");
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        site.site.write(
            "content/other.bib",
            "@article{onlyb,\n  title = {Second File Work},\n  author = {Doe, Jane},\n  year = {2001},\n}\n",
        );
        site.site.write(
            "content/array.typ",
            "= Array\n\n#bibliography((\"refs.bib\",))\n",
        );
        site.site.write(
            "content/spelled.typ",
            "= Spelled\n\n#bibliography(path(\"other.bib\"))\n",
        );
        let hover = site
            .hover_text("See @knuth|1984 and @onlyb.\n")
            .expect("a hover through an array source");
        assert!(hover.contains("Literate Programming"), "{hover}");
        let hover = site
            .hover_text("See @knuth1984 and @only|b.\n")
            .expect("a hover through a path source");
        assert!(hover.contains("Second File Work"), "{hover}");
    }

    /// A bibliography renders its terms in the language its own call is written in.
    #[test]
    fn hover_renders_terms_in_the_call_language() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E. and Muster, Max},\n  year = {1984},\n}\n",
        );
        let german = site
            .hover_text(
                "#set text(lang: \"de\")\n#bibliography(\"refs.bib\")\n\nSee @knuth|1984.\n",
            )
            .expect("a hover in a German bibliography");
        assert!(german.contains("und"), "{german}");
        let english = site
            .hover_text("#bibliography(\"refs.bib\")\n\nSee @knuth|1984.\n")
            .expect("a hover in the default language");
        assert!(english.contains(" and "), "{english}");
    }
    /// A YAML key after multibyte text still points at the key itself: the scanner counts
    /// characters, and a range that addresses bytes has both mapped.
    #[test]
    fn yaml_bibliography_keys_address_their_bytes() {
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
    fn bibliographies_render_with_their_own_style() {
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
        assert!(plain.contains("D. E. Knuth"), "{plain}");
        assert!(!plain.contains("[B]"), "{plain}");
    }

    /// A bibliography parsed once is reused while its bytes are unchanged, and read again when the
    /// file changes.
    #[test]
    fn reused_bibliography_parses_follow_changed_bytes() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let program = "#bibliography(\"refs.bib\")\n\nSee @knuth|1984.\n";
        let first = site.hover_text(program).expect("a citation hover");
        assert!(first.contains("Literate Programming"), "{first}");
        assert_eq!(site.hover_text(program).as_deref(), Some(first.as_str()));
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Edited Title},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        site.revision += 1;
        let edited = site
            .hover_text(program)
            .expect("a citation hover after the file changed");
        assert!(edited.contains("Edited Title"), "{edited}");
    }

    /// A citation completes by its key, without regard to case, and by the entry's title.
    #[test]
    fn citation_completion_offers_keys_and_titles() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/refs.bib",
            "@article{knuth1984,\n  title = {Literate Programming},\n  author = {Knuth, Donald E.},\n  year = {1984},\n}\n",
        );
        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @Kn|\n");
        let labels = completion_labels(&items);
        assert!(labels.contains(&"knuth1984"), "{labels:?}");
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

        let items = site.completion("#bibliography(\"refs.bib\")\n\nSee @liT|\n");
        let titled = item_labeled(&items, "Literate Programming");
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
