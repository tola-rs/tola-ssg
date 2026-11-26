use std::sync::Arc;

use lsp_types::{
    CompletionTextEdit, DocumentHighlightKind, GotoDefinitionResponse, PrepareRenameResponse, Uri,
};
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceCompilation, SourceDiagnosticSession};
use tola_typst::typst::World;
use tola_typst::typst::syntax::Source;

use super::{hover, locations, referenced, rename, typed_name};
use crate::position;
use crate::query::tests::*;

const SITE_PROGRAM: &str =
    "#for source in (\"content/page.typ\",) {\n  document(\"page.html\")[#include source]\n}\n";

fn site_with_program(program: &str) -> SiteDirectory {
    let site = SiteDirectory::new();
    site.write("site.typ", program);
    site
}

fn label_hover(site: &SiteDirectory, rel: &str, marked: &str) -> String {
    let root = resolved_root(site);
    let path = root.join(rel);
    let text = marked.replace('|', "");
    let pos = marked.find('|').expect("a cursor marker");
    site.write(rel, &text);
    let cfg = load_configuration(&root).expect("site configuration");
    let cancel = BuildCancellation::new();
    let overrides = vec![(path.clone(), Arc::<str>::from(text.as_str()))];
    let mut session = SourceDiagnosticSession::with_resources(
        Arc::clone(&cfg),
        BuildResources::new()
            .with_network_access(tola_input::NetworkAccess::Denied)
            .without_system_fonts(),
    );
    let rev = session
        .inspect(overrides.clone(), &cancel)
        .expect("a completed check");
    let alone;
    let comp = match rev.checked() {
        Some(checked) if checked.bundle().is_some() => checked,
        _ => {
            alone = session
                .inspect_source(rel, overrides, &cancel)
                .expect("a prepared world")
                .into_checked();
            alone.as_ref().expect("a compilation for the file")
        }
    };
    let uri = crate::uri::from_file_path(&path).expect("the file's uri");
    let id = crate::identity::file_id(&uri, &root).expect("the file's id");
    let src = comp.world().source(id).expect("the file's source");
    let hover = hover(comp, &cfg, &src, pos, &cancel)
        .expect("a label hover")
        .expect("a label answer");
    match hover.contents {
        lsp_types::HoverContents::Markup(markup) => markup.value,
        _ => panic!("a label hover is markdown"),
    }
}

fn spelled_site() -> (
    SiteDirectory,
    SourceCompilation,
    crate::uri::ClientRoot,
    Uri,
) {
    let site = site_with_program(
        r#"#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document("page.html")[#include source.file]
}
"#,
    );
    site.write(
        "content/page.typ",
        "#figure([Body]) <intro>\n\nSee @intro\n",
    );
    let cfg = load_configuration(&resolved_root(&site)).expect("site configuration");
    let cancel = BuildCancellation::new();
    let mut session = SourceDiagnosticSession::with_resources(
        Arc::clone(&cfg),
        BuildResources::new()
            .with_network_access(tola_input::NetworkAccess::Denied)
            .without_system_fonts(),
    );
    let rev = session
        .inspect(Vec::new(), &cancel)
        .expect("a completed check");
    let comp = rev.into_checked().expect("the site compiles");
    let root = resolved_root(&site);
    let spelled = root.join("spelled");
    let client_root = crate::uri::ClientRoot::with_resolved(&spelled, &root);
    let expected = crate::uri::from_file_path(&spelled.join("content/page.typ")).unwrap();
    (site, comp, client_root, expected)
}

#[test]
fn reference_prefix_and_delimiter_edits() {
    let src = Source::detached("See @name\n");
    let ref_ = referenced(&src, 4).expect("the marker's reference");
    assert_eq!(typed_name(&src, &ref_, 4), "");
    assert_eq!(typed_name(&src, &ref_, 8), "nam");
    let src = Source::detached("#ref(<name>)");
    let pos = src.text().find('>').unwrap() + 1;
    let ref_ = referenced(&src, pos).expect("the label literal");
    assert_eq!(typed_name(&src, &ref_, pos), "name");

    for marked in ["See |@old", "#ref(|<old>)", "#ref(<old>|)"] {
        let pos = marked.find('|').unwrap();
        let src = Source::detached(marked.replace('|', ""));
        let ref_ = referenced(&src, pos).expect("the selected reference");
        let edit = ref_.completion_edit(&src, pos, "new".to_owned()).unwrap();
        let (range, text) = match edit {
            CompletionTextEdit::Edit(e) => (e.range, e.new_text),
            CompletionTextEdit::InsertAndReplace(e) => (e.replace, e.new_text),
        };
        let start = position::byte_offset(src.lines(), range.start).unwrap();
        let end = position::byte_offset(src.lines(), range.end).unwrap();
        assert!(start <= pos && pos <= end);
        let mut changed = src.text().to_owned();
        changed.replace_range(start..end, &text);
        assert_eq!(changed, src.text().replace("old", "new"));
    }
}

#[test]
fn label_hover_states() {
    let broken = site_with_program("#let broken = )\n");
    let local = label_hover(
        &broken,
        "content/page.typ",
        "#figure([Body]) <int|ro>\n\nSee @intro\n\n#undefined-fn()\n",
    );
    assert!(local.contains("`figure`"), "{local}");
    assert!(local.contains("declared in this file"), "{local}");
    assert!(!local.contains("no source declares the label"), "{local}");

    let compiled = site_with_program(SITE_PROGRAM);
    let missing = label_hover(&compiled, "content/page.typ", "See @missing|\n");
    assert!(
        missing.contains("no source declares the label"),
        "{missing}"
    );

    let mut site = QuerySession::new();
    let ref_hover = site
        .hover_text("#metadata((kind: \"note\")) <introduction>\nSee @intro|duction")
        .expect("reference hover");
    assert!(ref_hover.contains("metadata"), "{ref_hover}");
    assert!(ref_hover.contains("content/document.typ"), "{ref_hover}");
    assert!(ref_hover.contains("/document/"), "{ref_hover}");

    let code_hover = site
        .hover_text("#figure([A]) <intro|duction>\n")
        .expect("code label hover");
    assert!(code_hover.contains("<introduction>"), "{code_hover}");
    assert!(code_hover.contains("figure"), "{code_hover}");
    assert!(code_hover.contains("content/document.typ"), "{code_hover}");
}

#[test]
fn label_client_root_uri() {
    let (_site, comp, client_root, expected) = spelled_site();
    let cancel = BuildCancellation::new();

    let found = locations(&comp, "intro", true, &cancel, &client_root).expect("locations");
    assert!(!found.is_empty(), "the label's spellings");
    assert!(found.iter().all(|loc| loc.uri == expected), "{found:?}");

    let ren = rename(&comp, "intro", "outro", &cancel, &client_root)
        .expect("a rename")
        .expect("edits");
    let changes = ren.changes.expect("edits by URI");
    assert_eq!(changes.keys().collect::<Vec<_>>(), [&expected]);
}

#[test]
fn loop_keyword_highlights() {
    let mut site = QuerySession::new();
    let highlights = site
        .highlights("#for x in (1, 2) {\n  if x == 1 { br|eak }\n  continue\n}\n")
        .expect("highlights for the loop");
    assert_eq!(highlights.len(), 3, "{highlights:?}");
    assert_eq!(
        (
            highlights[0].range.start.line,
            highlights[0].range.start.character
        ),
        (0, 0)
    );
    assert_eq!(
        (
            highlights[1].range.start.line,
            highlights[1].range.start.character
        ),
        (1, 14)
    );
    assert!(
        highlights.iter().all(|h| h.kind.is_none()),
        "{highlights:?}"
    );
}

#[test]
fn label_owning_document_and_includes() {
    let mut site = QuerySession::with_program(
        "#document(\"a.html\")[#include \"content/a.typ\"]\n#document(\"b.html\")[#include \"content/document.typ\"]\n",
    );
    site.site
        .write("content/a.typ", "#figure([First]) <intro>\nSee @intro\n");
    let marked = "#figure([Second]) <intro>\nSee @int|ro\n";
    let Some(GotoDefinitionResponse::Scalar(loc)) = site.definition(marked) else {
        panic!("the label has a definition");
    };
    assert!(
        loc.uri.as_str().ends_with("content/document.typ"),
        "{loc:?}"
    );
    let refs = site
        .references(marked, true)
        .expect("the label has references");
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(
        refs.iter()
            .all(|l| l.uri.as_str().ends_with("content/document.typ")),
        "{refs:?}"
    );

    let mut site = QuerySession::new();
    site.site.write(
        "content/labels.typ",
        "#figure([Parameters]) <tbl:appendix-parameters>\n",
    );
    let src = "#include \"labels.typ\"\nSee @tbl:appendix-parameters\n";
    let items = site.completion(&src.replace("parameters", "para|"));
    assert!(items.iter().any(|i| i.label == "tbl:appendix-parameters"));
    let Some(GotoDefinitionResponse::Scalar(loc)) =
        site.definition(&src.replace("parameters", "para|meters"))
    else {
        panic!("the included label has a definition");
    };
    assert!(loc.uri.as_str().ends_with("content/labels.typ"));
}

#[test]
fn label_and_binding_references() {
    let mut site = QuerySession::new();
    let text = "#figure([Body]) <intro>\nSee @int|ro and #ref(<intro>)\n";
    for (include_decl, expected) in [(true, 3), (false, 2)] {
        let refs = site
            .references(text, include_decl)
            .expect("the spellings of the label");
        assert_eq!(refs.len(), expected, "{refs:?}");
        assert!(
            refs.iter()
                .all(|l| l.uri.as_str().ends_with("document.typ")),
            "{refs:?}"
        );
    }

    let bound = "#let factor = 2\n#let twice = factor * fa|ctor\n";
    assert_eq!(site.references(bound, false).expect("uses").len(), 2);
    let all = site.references(bound, true).expect("all");
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(
        (all[0].range.start.line, all[0].range.start.character),
        (0, 5),
        "declaration comes first: {all:?}"
    );
}

#[test]
fn label_rename_invalid_names() {
    let mut site = QuerySession::new();
    let src = "#figure([Body]) <intro>\nSee @int|ro\n";
    for name in [
        "two words",
        "bad>name",
        "trailing.",
        ":leading",
        "",
        "line\nbreak",
    ] {
        assert!(site.rename(src, name).is_none(), "{name}");
    }
    assert!(site.rename(src, "章节-二").is_some());
}

#[test]
fn rename_all_spellings() {
    let mut site = QuerySession::new();
    for (text, new_name, expected) in [
        (
            "#figure([Body]) <introduction>\nSee @intro|duction and @introduction\n",
            "overview",
            3,
        ),
        ("#let factor = 2\n#f|actor\n", "scale", 2),
    ] {
        let edit = site.rename(text, new_name).expect("the rename edits");
        let changes = edit.changes.expect("edits by file");
        assert_eq!(changes.len(), 1, "{changes:?}");
        let edits = changes.values().flatten().collect::<Vec<_>>();
        assert_eq!(edits.len(), expected, "{edits:?}");
        assert!(edits.iter().all(|e| e.new_text == new_name), "{edits:?}");
    }
}

#[test]
fn rename_spares_outer_binding() {
    let mut site = QuerySession::new();
    let edit = site
        .rename(
            "#let scale = 3\n#let render(scale) = sca|le\n#render(2)\n",
            "factor",
        )
        .expect("the rename edits");
    let mut files = edit.changes.expect("edits by file").into_values();
    let edits = files.next().expect("one edited file");
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert!(edits.iter().all(|e| e.range.start.line == 1), "{edits:?}");
}

#[test]
fn label_and_binding_highlights() {
    let mut site = QuerySession::new();
    let highlights = site
        .highlights("#figure([Body]) <introduction>\nSee @intro|duction and @introduction\n")
        .expect("the spellings of the label");
    assert_eq!(highlights.len(), 3, "{highlights:?}");
    assert_eq!(
        (
            highlights[0].kind,
            highlights[0].range.start.line,
            highlights[0].range.start.character,
            highlights[0].range.end.line,
            highlights[0].range.end.character,
        ),
        (Some(DocumentHighlightKind::WRITE), 0, 17, 0, 29),
        "{highlights:?}"
    );
    assert!(
        highlights[1..]
            .iter()
            .all(|h| h.kind == Some(DocumentHighlightKind::READ)),
        "{highlights:?}"
    );
    assert_eq!(
        (
            highlights[1].range.start.line,
            highlights[1].range.start.character,
            highlights[1].range.end.line,
            highlights[1].range.end.character,
        ),
        (1, 5, 1, 17),
        "{highlights:?}"
    );

    let bound = site
        .highlights("#let factor = 2\n#f|actor\n")
        .expect("bound highlights");
    assert_eq!(bound.len(), 2, "{bound:?}");
    assert_eq!(bound[0].kind, Some(DocumentHighlightKind::WRITE));
    assert_eq!(bound[1].kind, Some(DocumentHighlightKind::READ));
    assert_eq!(
        (bound[1].range.start.line, bound[1].range.start.character),
        (1, 1),
        "{bound:?}"
    );
}

#[test]
fn prepare_rename_placeholder() {
    let mut site = QuerySession::new();
    for (text, expected) in [
        (
            "#metadata((kind: \"note\")) <introduction>\nSee @intro|duction\n",
            "introduction",
        ),
        ("#let factor = 2\n#f|actor\n", "factor"),
    ] {
        let Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) =
            site.prepare_rename(text)
        else {
            panic!("no rename preparation for {text:?}");
        };
        assert_eq!(placeholder, expected);
    }
}

#[test]
fn undeclared_label_no_rename() {
    let mut site = QuerySession::new();
    assert!(site.prepare_rename("See @intro|duction\n").is_none());
    assert!(site.rename("See @intro|duction\n", "overview").is_none());
}
