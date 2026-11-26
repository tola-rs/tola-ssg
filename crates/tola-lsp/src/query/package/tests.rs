use std::path::Path;
use std::sync::Arc;

use lsp_types::request as lsp_request;
use lsp_types::request::Request as LspRequest;
use lsp_types::{CompletionResponse, GotoDefinitionResponse, Location, Position, Range, Uri};
use serde_json::json;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::SiteConfig;
use tola_packages::builtin_package;
use tola_typst::typst::syntax::package::PackageSpec;
use tola_typst::typst::syntax::{RootedPath, Source, VirtualPath, VirtualRoot};

use super::*;
use crate::packages::PackageAccess;
use crate::position;
use crate::protocol::{SourceQuery, SourceReply};
use crate::published::PublishedPackage;
use crate::query::tests::*;

fn site_cfg(root: &Path) -> Arc<SiteConfig> {
    std::fs::write(root.join("tola.toml"), "").unwrap();
    load_configuration(root).expect("site configuration")
}

fn site_src(text: &str) -> Source {
    let id = RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new("content/document.typ").unwrap(),
    )
    .intern();
    Source::new(id, text.into())
}

fn marked_src(marked: &str) -> (Source, usize) {
    let pos = marked.find('|').expect("a query cursor");
    (site_src(&marked.replacen('|', "", 1)), pos)
}

fn pkg_completions(cfg: &SiteConfig, published: &[PublishedPackage], marked: &str) -> Vec<String> {
    let (src, pos) = marked_src(marked);
    let lsp_pos = position::utf16_range(src.lines(), pos..pos).unwrap().start;
    let query = SourceQuery::decode(lsp_server::Request {
        id: 1.into(),
        method: <lsp_types::request::Completion as LspRequest>::METHOD.into(),
        params: json!({
            "textDocument": { "uri": "file:///site/content/document.typ" },
            "position": lsp_pos,
        }),
    })
    .expect("a completion query")
    .1;
    let pkgs = PackageAccess::new(
        cfg,
        Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
        &tola_build::BuildResources::new(),
        published,
    );
    match respond(PackageQuery {
        config: cfg,
        package_access: &pkgs,
        source: &src,
        cursor: pos,
        query: &query,
        client_root: &crate::uri::ClientRoot::new(cfg.root()),
        cancellation: &BuildCancellation::new(),
        overrides: &[],
        indexed_names: None,
    })
    .expect("a reply")
    {
        Some(SourceReply::Completion(CompletionResponse::Array(items))) => {
            items.into_iter().map(|item| item.label).collect()
        }
        other => panic!("a completion, got {other:?}"),
    }
}

fn import_completions(published: &[PublishedPackage], marked: &str) -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let cfg = site_cfg(root.path());
    pkg_completions(&cfg, published, marked)
}

#[test]
fn package_definition_client_root() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = dir.path().canonicalize().unwrap();
    let spelled = dir.path().join("aliased");
    let cfg = site_cfg(&resolved);
    let spec: PackageSpec = "@tola/site:0.0.0".parse().unwrap();
    let pkg = builtin_package(&spec).expect("a builtin package");
    let id = entrypoint(&pkg, &manifest(&pkg).expect("a manifest")).expect("an entry");
    let embedded = crate::identity::embedded_source(id).expect("a builtin package source");
    let view = tola_input::layout::builtin_packages(&resolved);
    let mirror = crate::identity::package_view_path(id, &view).unwrap();
    std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
    std::fs::write(&mirror, embedded.as_bytes()).unwrap();
    let client_root = crate::uri::ClientRoot::with_resolved(&spelled, &resolved);
    let expected =
        crate::uri::from_file_path(&spelled.join(mirror.strip_prefix(&resolved).unwrap())).unwrap();

    let (src, pos) = marked_src("#import \"@tola/site:0.0.0|\"\n");
    let lsp_pos = position::utf16_range(src.lines(), pos..pos).unwrap().start;
    let query = SourceQuery::decode(lsp_server::Request {
        id: 1.into(),
        method: <lsp_types::request::GotoDefinition as LspRequest>::METHOD.into(),
        params: json!({
            "textDocument": { "uri": "file:///site/content/document.typ" },
            "position": lsp_pos,
        }),
    })
    .expect("a definition query")
    .1;
    let pkgs = PackageAccess::new(
        &cfg,
        Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
        &tola_build::BuildResources::new(),
        &[],
    );
    let reply = respond(PackageQuery {
        config: &cfg,
        package_access: &pkgs,
        source: &src,
        cursor: pos,
        query: &query,
        client_root: &client_root,
        cancellation: &BuildCancellation::new(),
        overrides: &[],
        indexed_names: None,
    })
    .expect("a reply");
    let Some(SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(loc)))) = reply else {
        panic!("a definition: {reply:?}");
    };
    assert_eq!(loc.uri, expected);

    let mut mirrored = GotoDefinitionResponse::Scalar(Location {
        uri: crate::uri::from_file_path(&mirror).unwrap(),
        range: Range::default(),
    });
    package_source_definition(
        &mut mirrored,
        &view,
        &client_root,
        &BuildCancellation::new(),
    )
    .unwrap();
    assert_eq!(
        mirrored,
        GotoDefinitionResponse::Scalar(Location {
            uri: expected,
            range: Range::default(),
        })
    );
}

#[test]
fn unsaved_paths_dedup_without_disk_writes() {
    let mut site = QuerySession::new();
    let root = site.config.root().to_path_buf();
    let doc = root.join("content/document.typ");
    let draft = root.join("content/draft.typ");
    site.site.write("content/existing.typ", "Existing\n");
    let (text, pos) = marked_cursor("#include \"dr|\"\n");
    let query = SourceQuery::decode(lsp_server::Request {
        id: 1.into(),
        method: lsp_request::Completion::METHOD.into(),
        params: json!({
            "textDocument": { "uri": crate::uri::from_file_path(&doc).unwrap() },
            "position": pos,
        }),
    })
    .unwrap()
    .1;
    let overrides = [
        (doc, Arc::from(text)),
        (draft.clone(), Arc::from("Draft\n")),
        (root.join("content/existing.typ"), Arc::from("Unsaved\n")),
        (root.join("public/hidden.typ"), Arc::from("Generated\n")),
    ];
    let reply = crate::query::respond(crate::query::QueryRequest {
        session: &mut site.session,
        compilations: &mut site.compilations,
        disk: &mut crate::analysis::DiskSources::default(),
        graphs: &mut crate::analysis::GraphCache::default(),
        bibliographies: &mut site.bibliographies,
        entity_queries: &mut crate::compiler::EntityQueries::default(),
        config: &site.config,
        source_revision: site.revision,
        overrides: &overrides,
        view: &crate::sources::SourceView::default(),
        published: &[],
        query: &query,
        routes_as_hints: false,
        check_progress: crate::protocol::CheckProgress::Failed,
        cancellation: &BuildCancellation::new(),
        client_root: &crate::uri::ClientRoot::new(&root),
    })
    .unwrap();
    let SourceReply::Completion(CompletionResponse::Array(items)) = reply else {
        panic!("a path completion reply");
    };
    let labels = completion_labels(&items);
    assert!(labels.contains(&"draft.typ"), "{labels:?}");
    assert_eq!(labels.iter().filter(|l| **l == "existing.typ").count(), 1);
    assert!(!labels.iter().any(|l| l.ends_with("hidden.typ")));
    assert!(!draft.exists());
}

#[test]
fn local_package_definition_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cfg = site_cfg(&root);
    let pkg_root = root.join("module");
    std::fs::create_dir(&pkg_root).unwrap();
    let entry = pkg_root.join("lib.typ");
    std::fs::write(&entry, "#let value = 1\n#value\n").unwrap();
    let spec = "@local/demo:0.1.0".parse().unwrap();
    let pkg = tola_typst::LocalPackage::new(spec, &pkg_root, VirtualPath::new("lib.typ").unwrap())
        .unwrap();
    let mut pkgs = PackageAccess::new(
        &cfg,
        Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
        &tola_build::BuildResources::new(),
        &[],
    );
    pkgs.local_packages = Arc::new(tola_typst::LocalPackages::new([pkg], ["local"]).unwrap());
    let (src, pos) = marked_src("#import \"@local/de|mo:0.1.0\"\n");
    let query = SourceQuery::decode(lsp_server::Request::new(
        1.into(),
        lsp_request::GotoDefinition::METHOD.into(),
        json!({
            "textDocument": {"uri": crate::uri::from_file_path(&root.join("content/document.typ")).unwrap()},
            "position": position::utf16_range(src.lines(), pos..pos).unwrap().start,
        }),
    ))
    .unwrap()
    .1;
    let reply = respond(PackageQuery {
        config: &cfg,
        package_access: &pkgs,
        source: &src,
        cursor: pos,
        query: &query,
        client_root: &crate::uri::ClientRoot::new(&root),
        cancellation: &BuildCancellation::new(),
        overrides: &[],
        indexed_names: None,
    })
    .unwrap();
    let Some(SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(loc)))) = reply else {
        panic!("a local import names its source file");
    };
    assert_eq!(loc.uri, crate::uri::from_file_path(&entry).unwrap());
    assert_eq!(loc.range, Range::default());
}

#[test]
fn published_versions_query_isolated() {
    let idx = [published("query-local", "0.2.0")];
    let marked = "#import \"@preview/query-local:|\"\n";
    assert_eq!(import_completions(&idx, marked), ["0.2.0"]);
    assert!(import_completions(&[], marked).is_empty());
}

#[test]
fn offline_local_package_completions() {
    let root = tempfile::tempdir().unwrap();
    let pkgs = root.path().join("packages");
    std::fs::create_dir_all(pkgs.join("preview/supplied/1.2.3")).unwrap();
    std::fs::write(root.path().join("tola.toml"), "").unwrap();
    let cfg = tola_build::config::loading::load_site_config(
        Some(&root.path().join("tola.toml")),
        tola_typst::PackageLocations::from_absolute_roots(None, None)
            .unwrap()
            .with_declared_root(pkgs)
            .unwrap(),
        &tola_build::config::loading::BuildOverrides::default(),
    )
    .unwrap()
    .into_config();
    assert_eq!(
        pkg_completions(&cfg, &[], "#import \"@preview/sup|\n#unknown"),
        ["@preview/supplied:1.2.3"]
    );
}

#[test]
fn package_name_and_version_completions() {
    let idx = [
        published("cetz", "0.3.3"),
        published("cetz", "0.3.4"),
        published("cetz-other", "1.0.0"),
    ];
    assert_eq!(
        import_completions(&idx, "#import \"@preview/ce|\"\n"),
        ["@preview/cetz-other:1.0.0", "@preview/cetz:0.3.4"],
    );
    assert_eq!(
        import_completions(&idx, "#import \"@preview/cetz:|\"\n"),
        ["0.3.3", "0.3.4"],
    );
    assert_eq!(
        import_completions(&[], "#import \"@tola/site:|\"\n"),
        ["0.0.0"],
    );
}

#[test]
fn version_completions_numeric_order() {
    let idx = [published("cetz", "0.10.0"), published("cetz", "0.9.0")];
    assert_eq!(
        import_completions(&idx, "#import \"@preview/cetz:|\"\n"),
        ["0.9.0", "0.10.0"]
    );
}

#[test]
fn builtin_package_fixes() {
    for (marked, expected, span_len) in [
        (
            "#import \"@tola/s|it:0.0.0\": site\n",
            "@tola/site:0.0.0",
            Some("@tola/sit:0.0.0".len()),
        ),
        (
            "#import \"@tola/i|con:0.0.1\": icon\n",
            "@tola/icon:0.0.0",
            None,
        ),
    ] {
        let (src, pos) = marked_src(marked);
        let edit = replacement(&src, pos).expect("one fix");
        assert_eq!(edit.new_text, expected);
        if let Some(len) = span_len {
            assert_eq!(
                (edit.range.start.character, edit.range.end.character),
                (9, 9 + len as u32),
            );
        }
    }
}

#[test]
fn resolving_specs_no_fix() {
    for marked in [
        "#import \"@tola/s|ite:0.0.0\": site\n",
        "#import \"@tola/d|ocument:0.0.0\": current-document\n",
        "#import \"@preview/c|etz:0.3.1\": cetz\n",
        "#import \"@tola/z|zzzzzzz:0.0.0\": other\n",
        "#import \"templates/p|age.typ\": page\n",
    ] {
        let (src, pos) = marked_src(marked);
        assert!(replacement(&src, pos).is_none(), "{marked:?}");
    }
}

#[test]
fn mirror_rewrites_package_uris() {
    let dir = tempfile::tempdir().unwrap();
    let srcs = dir.path().join("package-sources");
    let client_root = crate::uri::ClientRoot::new(dir.path());
    let uri: Uri = "tola-package:/tola/document/0.0.0/lib.typ".parse().unwrap();
    let id = crate::identity::file_id(&uri, dir.path()).unwrap();
    let path = srcs.join("tola/document/0.0.0/lib.typ");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        crate::identity::embedded_source(id).unwrap().as_bytes(),
    )
    .unwrap();
    let range = Range::new(Position::new(1, 2), Position::new(3, 4));
    let orig = Location { uri, range };
    for mut def in [
        GotoDefinitionResponse::Scalar(orig.clone()),
        GotoDefinitionResponse::Array(vec![orig.clone()]),
    ] {
        package_source_definition(&mut def, &srcs, &client_root, &BuildCancellation::new())
            .unwrap();
        let loc = match &def {
            GotoDefinitionResponse::Scalar(loc) => loc,
            GotoDefinitionResponse::Array(locs) => &locs[0],
            _ => unreachable!(),
        };
        assert_eq!(loc.uri, crate::uri::from_file_path(&path).unwrap());
        assert_eq!(loc.range, range);
    }

    std::fs::write(&path, "modified source").unwrap();
    let mut projected = GotoDefinitionResponse::Scalar(orig.clone());
    package_source_definition(
        &mut projected,
        &srcs,
        &client_root,
        &BuildCancellation::new(),
    )
    .unwrap();
    assert_eq!(projected, GotoDefinitionResponse::Scalar(orig));
}

#[test]
fn package_doc_completes_without_world() {
    let mut site = QuerySession::with_program("#let broken = absent\n");
    let pkg: lsp_types::Uri = "tola-package:/tola/site/0.0.0/lib.typ".parse().unwrap();
    let id = crate::identity::file_id(&pkg, site.site.root()).expect("a builtin package");
    let text = crate::identity::embedded_source(id).expect("a builtin source");
    let src = Source::detached(text.to_string());
    let lines = src.lines();
    let import = text
        .lines()
        .position(|line| line.starts_with("#import"))
        .expect("the package's import line");
    let at = position::byte_offset(lines, Position::new(import as u32, 5)).expect("a cursor");
    let pos = position::utf16_range(lines, at..at)
        .expect("a position")
        .start;
    let reply = site
        .try_reply_at(
            lsp_request::Completion::METHOD,
            pkg.as_str(),
            pos,
            None,
            json!({}),
        )
        .expect("an answered completion");
    assert_eq!(reply, json!([]));
}

#[test]
fn package_hover_lists_versions() {
    let mut site = QuerySession::new();
    let builtin = site
        .hover_text("#import \"@tola/ico|n:0.0.1\": icon\n")
        .expect("builtin hover");
    assert!(builtin.contains("@tola/icon:0.0.0"), "{builtin}");

    let preview = site
        .hover_text("#import \"@preview/abse|nt:0.1.0\": absent\nBody")
        .expect("preview hover");
    assert!(preview.contains("@preview/absent"), "{preview}");
}
