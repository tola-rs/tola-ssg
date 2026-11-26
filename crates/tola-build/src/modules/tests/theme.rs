use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::build::{BuildMode, BuildRequest, BuildReuse, BuildTrigger};
use crate::cancellation::BuildCancellation;
use crate::config::SiteConfig;
use crate::config::loading::{BuildOverrides, load_site_config};
use crate::{BuildResources, BuildSession, modules};
use tola_input::InputScope;

struct Project {
    _workspace: tempfile::TempDir,
    root: PathBuf,
    module: PathBuf,
}

fn write(root: &Path, path: &str, src: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, src).unwrap();
}

impl Project {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("project");
        let module = workspace.path().join("forester");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&module).unwrap();
        write(
            &module,
            "tola-module.toml",
            r#"
        [typst]
        entry = "lib.typ"
        [build]
        entry = "site.typ"
        [assets]
        trees = [{ source = "assets", url-prefix = "/" }]
        [options]
        show-footer = true
        labels = ["Default label"]
        [queries.nodes]
        entry = "model.typ"
        label = "theme-acceptance-nodes"
                "#,
        );
        write(
            &module,
            "lib.typ",
            r#"#import "@tola/source:0.0.0": tola-meta
        #import "helper.typ": helper-marker
        #import "@tola/module:0.0.0": get-module
        #let module-settings = get-module()
        #let package-marker = read("/package-root.txt").trim()
        #let note(title) = tola-meta((title: title))
        #let source-text(source) = read(source.file)
        #let source-content(source) = include source.file
        "#,
        );
        write(
            &module,
            "helper.typ",
            r#"#let helper-marker = read("data.txt").trim()"#,
        );
        write(&module, "components/footer.typ", "Default module footer.");
        write(
            &module,
            "assets/style.css",
            "body { color: rebeccapurple; }",
        );
        write(&module, "data.txt", "Relative module data");
        write(&module, "package-root.txt", "Absolute module data");
        write(
            &module,
            "content/index.typ",
            "WRONG MODULE SOURCE: a project path must not be rebased here.",
        );
        write(
            &module,
            "site.typ",
            r#"#import "@tola/source:0.0.0": all-sources
        #import "lib.typ": helper-marker, package-marker, source-content, module-settings
        #document("index.html", format: "html")[
          #html.elem("link", attrs: (rel: "stylesheet", href: module-settings.assets.urls.at("style.css")))
          #helper-marker / #package-marker
          Labels count: #module-settings.options.labels.len()
          #for source in all-sources() { source-content(source) }
          #if module-settings.options.show-footer { include "components/footer.typ" }
        ]
        "#,
        );
        write(
            &module,
            "model.typ",
            r#"#import "@tola/source:0.0.0": all-sources
        #import "lib.typ": helper-marker, package-marker, source-text, module-settings
        #metadata(all-sources().map(source => (
          path: source.path,
          title: source.meta.title,
          text: source-text(source),
          relative: helper-marker,
          absolute: package-marker,
          alias: module-settings.name,
          options: module-settings.options,
        )))<theme-acceptance-nodes>
        "#,
        );
        write(
            &root,
            "content/index.typ",
            r#"#import "@tola-modules/forester:0.0.0": note
        #note("Project title")
        Project body marker.
        "#,
        );
        let proj = Self {
            _workspace: workspace,
            root,
            module,
        };
        proj.configure("");
        proj
    }

    fn configure(&self, extra: &str) {
        write(
            &self.root,
            "tola.toml",
            &format!(
                r#"[site]
        base-path = "/mounted/"
        [modules.forester]
        path = "../forester"
        [build]
        entry = {{ module = "forester" }}
        [typst.fonts]
        system = false
        {extra}
        "#
            ),
        );
    }

    fn config(&self) -> anyhow::Result<SiteConfig> {
        load_site_config(
            Some(&self.root.join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .map(|loaded| loaded.into_config())
    }

    fn session(&self) -> BuildSession {
        BuildSession::with_resources(BuildResources::new().with_input_scope(InputScope::Offline))
    }

    fn html(&self) -> String {
        fs::read_to_string(self.root.join("public/index.html")).unwrap()
    }

    fn build(&self) -> anyhow::Result<()> {
        self.session()
            .build_and_write(&self.config()?, BuildRequest::default())
            .map(|_| ())
    }

    fn build_error(&self) -> anyhow::Error {
        self.build().expect_err("the build must fail")
    }
}

#[test]
fn sibling_theme_supplies_site() {
    let proj = Project::new();
    let mut initial = fs::read_dir(&proj.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    initial.sort();
    assert_eq!(initial, ["content", "tola.toml"]);
    proj.build().unwrap();
    let html = proj.html();
    for expected in [
        "Relative module data",
        "Absolute module data",
        "Project body marker.",
        "Default module footer.",
        "Labels count: 1",
        "/mounted/_tola-modules/forester/style.css",
    ] {
        assert!(html.contains(expected), "{expected}: {html}");
    }
    assert!(!html.contains("WRONG MODULE SOURCE"));
    assert!(!proj.root.join("site.typ").exists());
    assert!(!proj.root.join("theme").exists());
    let css =
        fs::read_to_string(proj.root.join("public/_tola-modules/forester/style.css")).unwrap();
    assert!(css.contains("body") && css.contains("color"));
}

#[test]
fn query_keeps_project_paths() {
    let proj = Project::new();
    // Query cannot assume before-build has produced the declared asset tree.
    fs::remove_dir_all(proj.module.join("assets")).unwrap();
    let model = proj.module.join("model.typ");
    let src = fs::read_to_string(&model).unwrap().replace(
        "options: module-settings.options,",
        "options: module-settings.options, assets-ready: module-settings.assets != none,",
    );
    fs::write(model, src).unwrap();
    proj.configure(
        r#"[[hooks.before-build]]
        name = "must-not-run-for-a-query"
        argv = ["this-module-test-hook-must-never-be-spawned"]
        "#,
    );
    let cfg = Arc::new(proj.config().unwrap());
    let resolved = super::loaded_modules(
        cfg.root(),
        cfg.modules(),
        tola_input::InputMode::new(InputScope::Offline, false),
    )
    .unwrap();
    let module = &resolved["forester"];
    let res = modules::query::run(
        Arc::clone(&cfg),
        module,
        &module.queries["nodes"],
        "forester.nodes",
        BuildResources::new().with_input_scope(InputScope::Offline),
        Vec::new(),
        &BuildCancellation::default(),
    )
    .unwrap();
    assert!(res.success, "{:?}", res.diagnostics);
    let row = &res.data.unwrap()[0];
    assert_eq!(row["path"], "index.typ");
    assert_eq!(row["title"], "Project title");
    assert_eq!(row["relative"], "Relative module data");
    assert_eq!(row["absolute"], "Absolute module data");
    assert_eq!(row["alias"], "forester");
    assert_eq!(row["assets-ready"], false);
    assert_eq!(row["options"]["show-footer"], true);
    assert_eq!(
        row["options"]["labels"],
        serde_json::json!(["Default label"])
    );
    assert!(
        row["text"]
            .as_str()
            .unwrap()
            .contains("Project body marker.")
    );
    assert!(!proj.root.join("public").exists());
}

#[test]
fn aliases_share_module() {
    let proj = Project::new();
    proj.configure("[modules.another]\npath = '../forester'\n");
    let cfg = proj.config().unwrap();
    let resolved = super::loaded_modules(
        cfg.root(),
        cfg.modules(),
        tola_input::InputMode::new(InputScope::Offline, false),
    )
    .unwrap();
    assert_eq!(resolved["forester"].root, resolved["another"].root);
    assert!(
        super::loaded_modules(
            cfg.root(),
            cfg.modules(),
            tola_input::InputMode::new(InputScope::Pure, false)
        )
        .is_err()
    );
    write(
        &proj.root,
        "content/alias.typ",
        r#"#import "@tola-modules/another:0.0.0" as another
        #another.note("Another imported title")
        Imported alias: #another.module-settings.name
        "#,
    );
    proj.build().unwrap();
    assert!(proj.html().contains("Imported alias: another"));
}

#[test]
fn explicit_options_override_defaults() {
    let proj = Project::new();
    proj.configure("[modules.forester.options]\nshow-footer = false\nlabels = []\n");
    proj.build().unwrap();
    let html = proj.html();
    assert!(html.contains("Labels count: 0"), "{html}");
    assert!(!html.contains("Default module footer."), "{html}");
}

#[test]
fn theme_assets_collision_check() {
    let proj = Project::new();
    let entry = proj.module.join("site.typ");
    let mut src = fs::read_to_string(&entry).unwrap();
    src.push_str("\n#asset(\"_tola-modules/forester/style.css\", \"Conflicting stylesheet\")\n");
    fs::write(entry, src).unwrap();
    let err = proj.build_error();
    assert!(format!("{err:#}").contains("style.css"));
    assert!(!proj.root.join("public/index.html").exists());
}

#[test]
fn missing_theme_reports_path() {
    let proj = Project::new();
    let cfg_path = proj.root.join("tola.toml");
    let src = fs::read_to_string(&cfg_path)
        .unwrap()
        .replace("../forester", "../missing-forester");
    fs::write(&cfg_path, src).unwrap();
    let failure = proj.config().and_then(|cfg| {
        super::loaded_modules(
            cfg.root(),
            cfg.modules(),
            tola_input::InputMode::new(InputScope::Offline, false),
        )
        .map_err(anyhow::Error::new)
    });
    let err = failure.expect_err("a missing module cannot be resolved");
    assert!(format!("{err:#}").contains("missing-forester"));
}

#[test]
fn build_invalidates_module_reads() {
    let proj = Project::new();
    let cfg = proj.config().unwrap();
    let mut session = proj.session();
    session
        .build_and_write(&cfg, BuildRequest::new(BuildMode::Development))
        .unwrap();
    assert!(proj.html().contains("Relative module data"));
    let changed = proj.module.join("data.txt");
    fs::write(&changed, "Edited external module data").unwrap();
    session
        .build_and_write(
            &cfg,
            BuildRequest {
                trigger: BuildTrigger::Paths(Arc::from([changed].as_slice())),
                reuse: BuildReuse {
                    content_inventory: true,
                    typst_compilation: true,
                    configured_assets: true,
                },
                ..BuildRequest::new(BuildMode::Development)
            },
        )
        .unwrap();
    let html = proj.html();
    assert!(html.contains("Edited external module data"), "{html}");
    assert!(!html.contains("Relative module data"), "{html}");
}
