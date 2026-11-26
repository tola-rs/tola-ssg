use super::*;
use crate::config::SiteConfigSchema;
use crate::modules;
use std::collections::BTreeMap;
use std::fs;
use tola_input::InputScope;
use tola_module::module_set::LoadedModule;
use typst::foundations::Value;

fn loaded_modules(
    site: &Path,
    decls: &BTreeMap<String, tola_module::ModuleDeclaration>,
    mode: tola_input::InputMode,
) -> std::result::Result<BTreeMap<String, LoadedModule>, tola_module::ModuleLoadError> {
    let roots = crate::inputs::module_roots(site, mode, None, decls, &Default::default())?;
    modules::load_modules_with_roots(site, decls, &roots, mode)
}

struct Site {
    dir: tempfile::TempDir,
    cfg: Arc<SiteConfig>,
    module: LoadedModule,
}

impl Site {
    fn new(model: &str) -> Self {
        Self::with_namespace("forest", model)
    }

    fn with_namespace(namespace: &str, model: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("theme")).unwrap();
        fs::write(root.join("site.typ"), "#unknown_layout_function()").unwrap();
        fs::write(root.join("theme/model.typ"), model).unwrap();
        fs::write(
            root.join("theme/tola-module.toml"),
            "[queries.nodes]\nentry='model.typ'\nlabel='nodes'\n\n",
        )
        .unwrap();
        let decls = BTreeMap::from([(
            namespace.into(),
            tola_module::ModuleDeclaration {
                source: tola_module::ModuleSource::Local {
                    path: "theme".into(),
                },
                ..Default::default()
            },
        )]);
        let cfg = Arc::new(
            SiteConfigSchema {
                modules: decls.clone(),
                ..Default::default()
            }
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap(),
        );
        let module = loaded_modules(
            root,
            &decls,
            tola_input::InputMode::new(InputScope::Offline, false),
        )
        .unwrap()
        .remove(namespace)
        .unwrap();
        Self { dir, cfg, module }
    }

    fn run(&self, overlays: Vec<(PathBuf, Arc<str>)>) -> QueryResult {
        run(
            Arc::clone(&self.cfg),
            &self.module,
            &self.module.queries["nodes"],
            &format!("{}.nodes", self.module.name),
            BuildResources::new().with_input_scope(InputScope::Offline),
            overlays,
            &BuildCancellation::default(),
        )
        .unwrap()
    }
}

#[test]
fn source_reads_preserve_bytes() {
    let site = Site::with_namespace(
        "catalog",
        "#import \"@tola/source:0.0.0\": all-sources\n#metadata(all-sources().map(s => (path:s.path, title:s.meta.title))) <nodes>",
    );
    let src = site.dir.path().join("content/glossary.typ");
    let orig = "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Glossary\"))";
    fs::write(&src, orig).unwrap();
    let res = site.run(Vec::new());
    assert!(res.success, "{:?}", res.diagnostics);
    assert_eq!(res.input.read_coverage, "model-and-entity-sources");
    assert_eq!(res.input.mode, "disk");
    let read = res
        .input
        .reads
        .iter()
        .find(|r| {
            matches!(&r.source, InputSource::File { root, path, provided: false }
            if root == "project" && path == Path::new("content/glossary.typ"))
        })
        .expect("source-analysis reads must not disappear behind all-sources metadata");
    fs::write(&src, "changed after query").unwrap();
    let json = serde_json::to_value(read).unwrap();
    assert_eq!(
        json["digest"],
        format!("blake3:{}", blake3::hash(orig.as_bytes()).to_hex())
    );
    assert_eq!(json["origin"], "disk");
    let rel = Path::new("content/glossary.typ");
    assert!(Path::new(json["diskPath"].as_str().unwrap()).ends_with(rel));
    assert_eq!(Path::new(json["path"].as_str().unwrap()), rel);
}

#[test]
fn failed_query_keeps_reads() {
    let model = "#metadata(json(\"missing.json\"))<nodes>";
    let site = Site::with_namespace("catalog", model);
    let res = site.run(Vec::new());
    assert!(!res.success);
    assert!(
        res.input
            .disk_read_attempts
            .iter()
            .any(|p| p.ends_with("theme/missing.json"))
    );
    let entry = res
        .input
        .reads
        .iter()
        .find(|r| {
            matches!(&r.source, InputSource::File { root, path, .. }
            if root.starts_with("@tola-modules/catalog:") && path == Path::new("model.typ"))
        })
        .expect("a failed query retains its native package entry's successful read");
    assert_eq!(entry.origin, "disk");
    assert!(
        entry
            .disk_path
            .as_ref()
            .unwrap()
            .ends_with("theme/model.typ")
    );
    assert_eq!(
        entry.digest,
        format!("blake3:{}", blake3::hash(model.as_bytes()).to_hex())
    );
    assert!(!res.input.reads.iter().any(|r| {
        matches!(&r.source, InputSource::File { path, .. } if path.ends_with("missing.json"))
    }));
}

#[test]
fn failed_query_missing_generated_input() {
    let model = |path: &str| {
        format!(
            "#import \"@tola/site:0.0.0\": project-path\n#metadata(json(project-path(\"{path}\")))<nodes>"
        )
    };
    let mut site = Site::new(&model("generated/missing.json"));
    let root = site.dir.path();
    let manifest = root.join("theme/tola-module.toml");
    let initial = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            "{initial}\n{}",
            r#"
[options]
disabled = false
[[hooks.before-build]]
name = "prepare"
argv = ["{tola}"]
generates = ["generated"]
[[hooks.before-build]]
name = "disabled-generator"
argv = ["{tola}"]
enabled-by = "disabled"
generates = ["disabled.json"]
[[hooks.before-build]]
name = "unrelated-generator"
argv = ["{tola}"]
generates = ["unrelated.json"]
"#
        ),
    )
    .unwrap();
    site.module = loaded_modules(
        root,
        site.cfg.modules(),
        tola_input::InputMode::new(InputScope::Offline, false),
    )
    .unwrap()
    .remove("forest")
    .unwrap();

    let failed = site.run(Vec::new());
    assert!(!failed.success);
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|d| d.code != tola_diagnostic::codes::query::RESULT
                && d.severity == Severity::Error)
    );
    let summary = failed
        .diagnostics
        .iter()
        .find(|d| d.code == tola_diagnostic::codes::query::RESULT)
        .unwrap();
    assert_eq!(summary.notes.len(), 1);
    assert!(summary.notes[0].contains("generated/missing.json"));
    assert!(summary.notes[0].contains("@module/forest/prepare"));
    assert!(
        summary
            .help
            .iter()
            .any(|h| h.message.contains("tola check"))
    );
    assert!(!root.join("generated").exists());

    let entry = run_file(
        Arc::clone(&site.cfg),
        Path::new("generated/model.typ"),
        "nodes",
        BuildResources::new().with_input_scope(InputScope::Offline),
        Vec::new(),
        &BuildCancellation::default(),
    )
    .unwrap();
    assert!(!entry.success);
    assert!(
        entry
            .diagnostics
            .iter()
            .flat_map(|d| &d.notes)
            .any(|note| note.contains("@module/forest/prepare"))
    );

    fs::create_dir_all(root.join("generated/a-directory")).unwrap();
    for path in ["ordinary.json", "disabled.json", "generated/a-directory"] {
        let failed = site.run(vec![(root.join("theme/model.typ"), Arc::from(model(path)))]);
        assert!(!failed.success);
        assert!(
            !failed
                .diagnostics
                .iter()
                .flat_map(|d| &d.help)
                .any(|h| h.message.contains("tola check"))
        );
    }
}

#[test]
fn read_evidence_keeps_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("glossary.typ");
    let observe = |text: &str| {
        fs::write(&path, text).unwrap();
        let world = tola_typst::TypstWorld::builder(&path, root)
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .unwrap();
        let id = tola_typst::file_id_from_path(&path, root).unwrap();
        let (bytes, accessed) = world.read_file_with_evidence(id);
        assert_eq!(bytes.unwrap().as_slice(), text.as_bytes());
        accessed.reads
    };
    let first = observe("first version");
    let second = observe("second version");
    let old = InputRead::from(&first[0]);
    assert_eq!(
        old.digest,
        format!("blake3:{}", blake3::hash(b"first version").to_hex())
    );
    let mut res = Site::with_namespace("catalog", "#metadata(())<nodes>").run(Vec::new());
    res.input.reads.clear();
    res.input.record(
        first.iter().chain(&first).chain(&second),
        std::iter::empty(),
    );
    assert_eq!(
        res.input.reads.len(),
        2,
        "conflicting byte observations cannot be collapsed to one revision"
    );
    assert_eq!(res.input.reads[0].source, res.input.reads[1].source);
    assert_ne!(res.input.reads[0].digest, res.input.reads[1].digest);
}

#[test]
fn entity_native_source_unsaved_model() {
    let mut site = Site::new("#metadata(())<nodes>");
    let root = site.dir.path();
    fs::write(root.join("content/a.typ"), "saved").unwrap();
    fs::write(root.join("theme/lib.typ"), "#let link(id) = []").unwrap();
    fs::write(
        root.join("theme/tola-module.toml"),
        "[typst]\nentry='lib.typ'\n[queries.nodes]\nentry='model.typ'\nlabel='nodes'\n[[queries.nodes.references]]\nexport='link'\nparameter={positional=0}\n",
    )
    .unwrap();
    site.module = loaded_modules(
        root,
        site.cfg.modules(),
        tola_input::InputMode::new(InputScope::Offline, false),
    )
    .unwrap()
    .remove("forest")
    .unwrap();
    let model = "#import \"@tola/site:0.0.0\": project-path\n#metadata(((id: \"a\", name: \"Unsaved\", source: project-path(\"content/a.typ\")),))<nodes>";
    let overlays = vec![(root.join("theme/model.typ"), Arc::<str>::from(model))];
    let res = run_entities(
        Arc::clone(&site.cfg),
        &site.module,
        &site.module.queries["nodes"],
        "forest.nodes",
        BuildResources::new().with_input_scope(InputScope::Offline),
        overlays.clone(),
        &BuildCancellation::default(),
    )
    .unwrap();
    assert!(res.result.success, "{:?}", res.result.diagnostics);
    assert_eq!(res.entities[0].name, "Unsaved");
    assert!(res.revision.as_ref().unwrap().checked().is_some());
    assert_eq!(
        res.entities[0].source.unwrap().vpath().get_without_slash(),
        "content/a.typ"
    );
    assert!(res.result.data.is_none());
    assert!(!res.source_accessed.is_empty());
    assert!(!root.join("public").exists());
    let cli = site.run(overlays);
    assert!(cli.success, "{:?}", cli.diagnostics);
    let data = cli.data.unwrap();
    assert_eq!(data[0]["name"], "Unsaved");
    assert_eq!(
        data[0]["source"],
        serde_json::json!({"root": "project", "path": "content/a.typ"})
    );
    let invalid = site.run(vec![(
        root.join("theme/model.typ"),
        Arc::from(model.replace("name:", "label:")),
    )]);
    assert!(!invalid.success);
}

#[test]
fn query_encodes_native_paths() {
    let site = Site::new(
        r#"
#import "@tola/site:0.0.0": project-path
#metadata((file: project-path("content/a.typ"), total: 3)) <nodes>
"#,
    );
    fs::write(site.dir.path().join("content/a.typ"), "Body").unwrap();
    let res = site.run(Vec::new());
    assert!(res.success, "{:?}", res.diagnostics);
    assert_eq!(
        res.data.unwrap(),
        serde_json::json!({
            "file": {"root": "project", "path": "content/a.typ"},
            "total": 3,
        })
    );
}

#[test]
fn cancelled_query_stops() {
    let site = Site::new("#metadata(())<nodes>");
    let canceller = crate::cancellation::BuildCanceller::new();
    canceller.cancel();
    let err = run(
        Arc::clone(&site.cfg),
        &site.module,
        &site.module.queries["nodes"],
        "forest.nodes",
        BuildResources::new().with_input_scope(InputScope::Offline),
        Vec::new(),
        &canceller.token(),
    )
    .expect_err("cancelled query");
    assert!(crate::cancellation::is_cancelled(&anyhow::Error::new(err)));
}

#[test]
fn changed_query_requires_resolve() {
    let mut site = Site::new("#metadata((revision:\"v1\"))<nodes>");
    let root = site.dir.path();
    let manifest = root.join("theme/tola-module.toml");
    let first = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, &first).unwrap();
    let resolve = || {
        loaded_modules(
            root,
            site.cfg.modules(),
            tola_input::InputMode::new(InputScope::Offline, false),
        )
        .unwrap()
        .remove("forest")
        .unwrap()
    };
    site.module = resolve();
    let initial = site.run(Vec::new());
    assert_eq!(initial.data, Some(serde_json::json!({"revision":"v1"})));

    fs::write(
        root.join("theme/second.typ"),
        "#metadata((revision:\"v2\"))<updated-nodes>",
    )
    .unwrap();
    fs::write(
        &manifest,
        first
            .replace("entry='model.typ'", "entry='second.typ'")
            .replace("label='nodes'", "label='updated-nodes'\n"),
    )
    .unwrap();
    let Err(failure) = run(
        Arc::clone(&site.cfg),
        &site.module,
        &site.module.queries["nodes"],
        "forest.nodes",
        BuildResources::new().with_input_scope(InputScope::Offline),
        Vec::new(),
        &BuildCancellation::default(),
    ) else {
        panic!("a stale descriptor cannot label newly evaluated data");
    };
    let msg = failure.to_string();
    assert!(
        msg.contains("forest.nodes") && msg.contains("changed"),
        "{msg}"
    );

    site.module = resolve();
    let updated = site.run(Vec::new());
    assert!(updated.success, "{:?}", updated.diagnostics);
    assert_eq!(updated.data, Some(serde_json::json!({"revision":"v2"})));
}

#[test]
fn module_bindings_across_inspections() {
    let site = Site::new("#metadata(())<nodes>");
    let root = site.dir.path();
    fs::write(
        root.join("site.typ"),
        "#import \"@tola/module:0.0.0\": get-module\n#metadata(get-module(\"forest\").options.marker)<current-marker>",
    )
    .unwrap();
    let manifest = root.join("theme/tola-module.toml");
    let src = fs::read_to_string(&manifest).unwrap();
    let marked = |marker: &str| format!("{src}\n[options]\nmarker={marker:?}\n");
    let mut session = SourceDiagnosticSession::with_resources(
        Arc::clone(&site.cfg),
        BuildResources::new().with_input_scope(InputScope::Offline),
    );
    for marker in ["first", "second"] {
        fs::write(&manifest, marked(marker)).unwrap();
        let rev = session
            .inspect(Vec::new(), &BuildCancellation::default())
            .unwrap();
        let bundle = rev
            .checked()
            .and_then(|c| c.bundle())
            .unwrap_or_else(|| panic!("{:?}", rev.diagnostics()));
        let decls = bundle.metadata_declarations("current-marker");
        assert_eq!(decls.len(), 1);
        let Value::Str(current) = decls[0].value() else {
            panic!("the module option must remain a string");
        };
        assert_eq!(current.as_str(), marker);
    }
}
