use lsp_types::PrepareRenameResponse;
use tola_typst::typst::foundations::{Repr, Str};

use crate::analysis::NameQueryError;
use crate::query::tests::{QuerySession, marked_cursor};

const ENTITY_QUERY: &str = "entry=\"model.typ\"\nlabel=\"entities\"\n";

fn configure_modules(site: &mut QuerySession, modules: &str) {
    let cfg = std::fs::read_to_string(site.site.root().join("tola.toml")).unwrap();
    site.site.write("tola.toml", &format!("{cfg}\n{modules}"));
    site.reload();
}

fn scenario() -> QuerySession {
    let mut site = QuerySession::with_program("#invalid_page_template()");
    site.site.write(
        "theme/lib.typ",
        "#let local(address) = []\n#let subtree(body, address: none) = []\n",
    );
    site.site.write(
        "theme/model.typ",
        r#"
#import "@tola/site:0.0.0": project-path
#metadata((
  (id: "ROOT", name: "Root page", source: project-path("content/target.typ")),
  (id: "INLINE", name: "Inline entity", source: project-path("content/target.typ")),
  (id: "https://foreign.example/ROOT/", name: "Foreign entity", url: "https://foreign.example/ROOT/"),
))<entities>
"#,
    );
    site.site.write(
        "theme/tola-module.toml",
        &format!(
            r#"
[typst]
entry="lib.typ"
[queries.entities]
{ENTITY_QUERY}[[queries.entities.references]]
export="local"
parameter={{positional=0}}
[[queries.entities.definitions]]
export="subtree"
parameter={{named="address"}}
"#
        ),
    );
    site.site.write(
        "content/target.typ",
        "#import \"@tola-modules/notes:0.0.0\": subtree\n😀 #subtree(address: \"INLINE\")[Body]\n",
    );
    configure_modules(&mut site, "[modules.notes]\npath=\"theme\"\n");
    site
}

fn module_scenario() -> QuerySession {
    let mut site = QuerySession::with_program("#unrelated_broken_page_template()");
    for (alias, dir, src, name) in [
        ("shop", "products", "catalog", "Torque wrench"),
        ("terms", "glossary", "glossary", "Glossary entry"),
    ] {
        site.site.write(
            &format!("{dir}/lib.typ"),
            "#let link(code) = []\n#let alias-link = link\n#let define(code, name: \"item\") = (id: code, name: name)\n",
        );
        site.site.write(
            &format!("{dir}/model.typ"),
            &format!(
                r#"
#import "@tola/site:0.0.0": project-path
#import project-path("content/{src}.typ"): product
#metadata((product + (description: "A model-owned description", source: project-path("content/{src}.typ")),))<entities>
"#
            ),
        );
        site.site.write(
            &format!("{dir}/tola-module.toml"),
            &format!(
                r#"
[typst]
entry="lib.typ"
[queries.catalog]
{ENTITY_QUERY}[[queries.catalog.references]]
export="link"
parameter={{positional=0}}
[[queries.catalog.references]]
export="alias-link"
parameter={{positional=0}}
[[queries.catalog.definitions]]
export="define"
parameter={{positional=0}}
"#
            ),
        );
        site.site.write(
            &format!("content/{src}.typ"),
            &format!(
                "#import \"@tola-modules/{alias}:0.0.0\": define\n#let product = define(\"sku:α-42\", name: \"{name}\")\n"
            ),
        );
    }
    site.site.write(
        "content/uses.typ",
        "#import \"@tola-modules/shop:0.0.0\": link as cite\n#cite(\"sku:α-42\")\n#let partial = cite.with(\"sku:α-42\")\n#partial()\n#{ let cite(code) = []; cite(\"sku:α-42\") }\n#import \"@tola-modules/terms:0.0.0\" as glossary\n#glossary.link(\"sku:α-42\")\n",
    );
    configure_modules(
        &mut site,
        "[modules.shop]\npath='products'\n[modules.terms]\npath='glossary'\n",
    );
    site
}

const PRODUCT_REF: &str = "#import \"@tola-modules/shop:0.0.0\": link\n😀 #link(\"sku:α|-42\")";

#[test]
fn entity_hover_and_references_identity() {
    let mut site = module_scenario();
    let hover = site.hover_text(PRODUCT_REF).expect("entity hover");
    assert!(
        hover.contains("Torque wrench") && hover.contains("sku:α-42"),
        "{hover}"
    );
    assert!(
        hover.contains("shop.catalog") && hover.contains("model-owned"),
        "{hover}"
    );
    let refs = site
        .references(PRODUCT_REF, false)
        .expect("entity references");
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert!(
        refs.iter()
            .all(|loc| !loc.uri.as_str().contains("glossary"))
    );
    let decls = site.references(PRODUCT_REF, true).unwrap();
    assert_eq!(decls.len(), refs.len() + 1, "{decls:?}");
    let def = decls
        .iter()
        .find(|loc| loc.uri.as_str().ends_with("content/catalog.typ"))
        .unwrap();
    assert_eq!(def.range.start.line, 1);
    assert_eq!(def.range.start.character, 22);
}

#[test]
fn entity_parameter_conflict_diagnostic() {
    let mut site = module_scenario();
    let lib = std::fs::read_to_string(site.site.root().join("products/lib.typ")).unwrap();
    site.site.write(
        "products/lib.typ",
        &format!("{lib}\n#let other-link = link\n"),
    );
    let manifest =
        std::fs::read_to_string(site.site.root().join("products/tola-module.toml")).unwrap();
    site.site.write(
        "products/tola-module.toml",
        &format!(
            r#"{manifest}
[queries.other]
entry = "model.typ"
label = "entities"
[[queries.other.references]]
export = "other-link"
parameter = {{ positional = 0 }}
"#
        ),
    );
    site.reload();
    let err = site
        .try_reply_with("textDocument/hover", PRODUCT_REF, serde_json::json!({}))
        .expect_err("one argument cannot select two query namespaces");
    let msg = err.to_string();
    assert!(msg.contains("queries.other.references[0]"));
    assert!(msg.contains("queries.catalog.references[0]"));
}

#[test]
fn entity_rename_validates_proposed_model() {
    let mut site = module_scenario();
    let prepared = site
        .prepare_rename(PRODUCT_REF)
        .expect("literal definition supports rename");
    let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
        panic!("placeholder")
    };
    assert_eq!(placeholder, "sku:α-42");
    let edit = site
        .rename(PRODUCT_REF, "part:\"β\\雪")
        .expect("checked entity edits");
    let changes = edit
        .changes
        .expect("connection versions the edits before delivery");
    assert_eq!(changes.values().map(Vec::len).sum::<usize>(), 4);
    let quoted = Str::from("part:\"β\\雪").repr();
    assert!(
        changes
            .values()
            .flatten()
            .all(|edit| edit.new_text == quoted.as_str())
    );
    assert!(
        changes
            .keys()
            .all(|uri| !uri.as_str().ends_with("glossary.typ"))
    );
    assert!(
        std::fs::read_to_string(site.site.root().join("content/catalog.typ"))
            .unwrap()
            .contains("sku:α-42"),
        "rename only proposes edits"
    );
}

#[test]
fn entity_rename_spares_other_identities() {
    for (path, records) in [
        (
            "products/model.typ",
            "(product + (source: source), product + (id: \"related:\" + product.id, source: source))",
        ),
        (
            "glossary/model.typ",
            "(product + (id: \"term:\" + product.id, source: source),)",
        ),
    ] {
        let mut site = module_scenario();
        site.site.write(
            path,
            &format!(
                "#import \"@tola/site:0.0.0\": project-path\n\
                 #import project-path(\"content/catalog.typ\"): product\n\
                 #let source = project-path(\"content/catalog.typ\")\n\
                 #metadata({records})<entities>"
            ),
        );
        assert!(site.hover_text(PRODUCT_REF).is_some(), "{path}");
        assert_eq!(
            site.rename_refusal(PRODUCT_REF, "new:id"),
            "entityRenameRefused",
            "{path}"
        );
    }
}

#[test]
fn entity_rename_refuses_derived_and_dynamic() {
    let mut notes = scenario();
    assert!(
        notes
            .prepare_rename("#import \"@tola-modules/notes:0.0.0\": local\n#local(\"RO|OT\")")
            .is_none()
    );
    assert!(
        notes
            .prepare_rename(
                "#import \"@tola-modules/notes:0.0.0\": local\n#local(\"https://foreign.example/RO|OT/\")"
            )
            .is_none()
    );
    let refusal = notes
        .rename_error(
            "#import \"@tola-modules/notes:0.0.0\": local\n#local(\"IN|LINE\")",
            "changed",
        )
        .expect("hard-coded model cannot be renamed");
    assert!(
        matches!(
            refusal.downcast_ref::<NameQueryError>(),
            Some(NameQueryError::EntityRename(_))
        ),
        "{refusal:?}"
    );
    let mut site = module_scenario();
    site.site.write(
        "content/dynamic.typ",
        "#import \"@tola-modules/shop:0.0.0\": link\n#let code = \"sku:α-42\"\n#link(code)\n",
    );
    assert!(site.prepare_rename(PRODUCT_REF).is_none());
    assert_eq!(
        site.rename_refusal(PRODUCT_REF, "new"),
        "entityRenameRefused"
    );
}

#[test]
fn entity_rename_refuses_shared_definition() {
    let mut site = module_scenario();
    configure_modules(&mut site, "[modules.shop-again]\npath='products'\n");
    site.site.write(
        "content/second-alias.typ",
        "#import \"@tola-modules/shop-again:0.0.0\": link\n#link(\"sku:α-42\")\n",
    );
    assert!(
        site.hover_text(PRODUCT_REF)
            .unwrap()
            .contains("Torque wrench")
    );
    assert_eq!(site.references(PRODUCT_REF, false).unwrap().len(), 3);
    let unchanged = site.rename(PRODUCT_REF, "sku:α-42").unwrap();
    assert!(unchanged.changes.is_none() && unchanged.document_changes.is_none());
    assert!(site.prepare_rename(PRODUCT_REF).is_none());
    let err = site
        .rename_error(PRODUCT_REF, "new:id")
        .expect("shared definition must not produce partial edits");
    assert!(
        matches!(
            err.downcast_ref::<NameQueryError>(),
            Some(NameQueryError::EntityRename(_))
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("shop-again.catalog"), "{err}");
    assert!(
        std::fs::read_to_string(site.site.root().join("content/catalog.typ"))
            .unwrap()
            .contains("sku:α-42")
    );
}

fn shared_definition_scenario(nodes: &str) -> QuerySession {
    let mut site = QuerySession::with_program("#unrelated_broken_page_template()");
    site.site.write(
        "alpha/lib.typ",
        "#let define(code, name: \"item\") = (id: code, name: name)\n#let cite(code) = []\n",
    );
    site.site.write(
        "alpha/model.typ",
        r#"
#import "@tola/site:0.0.0": project-path
#import project-path("content/shared.typ"): entry
#metadata((entry + (source: project-path("content/shared.typ")),))<entities>
"#,
    );
    site.site.write(
        "alpha/tola-module.toml",
        &format!(
            r#"
[typst]
entry="lib.typ"
[queries.nodes]
{ENTITY_QUERY}[[queries.nodes.references]]
export="cite"
parameter={{positional=0}}
[[queries.nodes.definitions]]
export="define"
parameter={{positional=0}}
"#
        ),
    );
    site.site.write(
        "beta/lib.typ",
        &format!("#let nodes{nodes} = items\n#let mark(code) = (id: code, name: \"marked\")\n"),
    );
    site.site.write(
        "beta/model.typ",
        r#"
#import "@tola/site:0.0.0": project-path
#metadata(((id: "SHARED", name: "Beta", source: project-path("content/shared.typ")),))<entities>
"#,
    );
    site.site.write(
        "beta/tola-module.toml",
        &format!(
            r#"
[typst]
entry="lib.typ"
[queries.nodes]
{ENTITY_QUERY}[[queries.nodes.definitions]]
export="mark"
parameter={{positional=0}}
[[queries.nodes.definitions]]
export="nodes"
parameter={{positional=0}}
"#
        ),
    );
    site.site.write(
        "content/shared.typ",
        r#"#import "@tola-modules/alpha:0.0.0": define
#import "@tola-modules/beta:0.0.0": mark, nodes
#let entry = define("SHARED", name: "Shared")
#{ let _ = mark("SHARED"); let _ = nodes("SHARED") }
"#,
    );
    configure_modules(
        &mut site,
        "[modules.alpha]\npath='alpha'\n[modules.beta]\npath='beta'\n",
    );
    site
}

const SHARED_REF: &str = "#import \"@tola-modules/alpha:0.0.0\": cite\n😀 #cite(\"SHAR|ED\")";

#[test]
fn shared_definitions_require_declared_parameters() {
    let mut site = shared_definition_scenario("(..items)");
    let err = site
        .rename_error(SHARED_REF, "public:β")
        .expect("a nonexistent definition parameter must be diagnosed");
    assert!(
        err.to_string()
            .contains("queries.nodes.definitions[1].parameter")
    );

    let mut site = shared_definition_scenario("(first, ..items)");
    assert_eq!(
        site.rename_refusal(SHARED_REF, "public:β"),
        "entityRenameRefused"
    );
}

#[test]
fn unsaved_buffers_share_rename_world() {
    use crate::protocol::SourceReply;
    use crate::sources::OpenSources;
    use serde_json::json;

    let mut site = module_scenario();
    let root = site.config.root();
    let target_path = root.join("content/catalog.typ");
    let caller_path = root.join("content/new-buffer.typ");
    let target_uri = crate::uri::from_file_path(&target_path).unwrap();
    let caller_uri = crate::uri::from_file_path(&caller_path).unwrap();
    let target = "// Unsaved 😀 heading\n#import \"@tola-modules/shop:0.0.0\": define\n#let note = \"🦊\"; #let product = define(\"draft:雪\", name: \"Unsaved product\")";
    let (caller, pos) =
        marked_cursor("#import \"@tola-modules/shop:0.0.0\": link\n😀 #link(\"draft:|雪\")");
    let mut open = OpenSources::new(root);
    for (uri, text, version) in [(&target_uri, target, 7), (&caller_uri, caller.as_str(), 3)] {
        open.apply(lsp_server::Notification::new(
            "textDocument/didOpen".into(),
            json!({
                "textDocument": {"uri": uri, "languageId": "typst", "version": version, "text": text}
            }),
        ))
        .unwrap();
    }
    let query = crate::query::tests::decode(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": caller_uri}, "position": pos, "newName": "public:β"
        }),
    );
    let reply = crate::query::respond(crate::query::QueryRequest {
        session: &mut site.session,
        compilations: &mut site.compilations,
        disk: &mut crate::analysis::DiskSources::default(),
        graphs: &mut crate::analysis::GraphCache::default(),
        bibliographies: &mut site.bibliographies,
        entity_queries: &mut site.entity_queries,
        config: &site.config,
        source_revision: open.revision(),
        overrides: open.overrides(),
        view: &open.view(),
        published: &[],
        query: &query,
        routes_as_hints: false,
        check_progress: crate::protocol::CheckProgress::Failed,
        cancellation: &tola_build::cancellation::BuildCancellation::default(),
        client_root: &crate::uri::ClientRoot::new(root),
    })
    .unwrap();
    let SourceReply::Rename(Some(edit)) = reply else {
        panic!("rename reply")
    };
    let changes = edit.changes.unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[&target_uri][0].range.start.line, 2);
    assert_eq!(changes[&target_uri][0].range.start.character, 40);
    assert_eq!(changes[&caller_uri][0].range.start.character, 9);
    assert!(
        changes
            .values()
            .flatten()
            .all(|edit| edit.new_text == "\"public:β\"")
    );
    assert!(!caller_path.exists());
    assert!(
        std::fs::read_to_string(target_path)
            .unwrap()
            .contains("sku:α-42")
    );
}
