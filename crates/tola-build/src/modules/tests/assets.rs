use std::fs;
use std::path::{Path, PathBuf};

use crate::build::{BuildMode, BuildRequest, BuildReuse, BuildTrigger};
use crate::config::SiteConfig;
use crate::config::loading::{BuildOverrides, load_site_config};
use crate::{BuildResources, BuildSession};
use tola_input::InputScope;

fn write(root: &Path, name: &str, value: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn rebuild(session: &mut BuildSession, cfg: &SiteConfig, paths: Vec<PathBuf>) {
    let req = BuildRequest {
        trigger: BuildTrigger::Paths(paths.into()),
        reuse: BuildReuse {
            content_inventory: true,
            typst_compilation: true,
            configured_assets: true,
        },
        ..BuildRequest::new(BuildMode::Development)
    };
    session.build_and_write(cfg, req).unwrap_or_else(|err| {
        panic!(
            "build failed: {err:#}; diagnostics: {:#?}",
            tola_diagnostic::attached(&err)
        )
    });
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    module: PathBuf,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        let module = dir.path().join("module");
        fs::create_dir_all(root.join("content")).unwrap();
        write(
            &module,
            "tola-module.toml",
            "[typst]\nentry=\"lib.typ\"\n[build]\nentry=\"site.typ\"\n[options]\nlegacy=false\nfonts=false\n[assets]\ntrees=[{ source=\"assets\", url-prefix=\"/\" }]\nfiles=[{ source=\"assets/scripts/legacy.js\", url=\"/scripts/legacy.js\", enabled-by=\"legacy\" }, { source=\"assets/fonts/body.woff2\", url=\"/fonts/body.woff2\", enabled-by=\"fonts\" }]\n",
        );
        write(&module, "lib.typ", "#let marker = [Library available]");
        write(&module, "assets/style.css", "module-style");
        write(&module, "assets/scripts/main.js", "main-script");
        write(&module, "assets/scripts/legacy.js", "old-script");
        write(&module, "assets/fonts/body.woff2", "font-data");
        write(
            &module,
            "site.typ",
            r#"
        #import "@tola/module:0.0.0": get-module
        #import "lib.typ": marker
        #let assets = get-module().assets
        #document("index.html", format: "html")[
          #marker
          #for (key, value) in assets.urls { [#key = #value; ] }
          #assert.eq(assets.files.keys().sorted(), assets.urls.keys().sorted())
          #if "style.css" in assets.files { [style-input: #read(assets.files.at("style.css")); ] }
          #if "scripts/main.js" in assets.files { [script-input: #read(assets.files.at("scripts/main.js")); ] }
        ]
        "#,
        );
        write(&root, "content/index.typ", "Content");
        let proj = Self {
            _dir: dir,
            root,
            module,
        };
        proj.config("");
        proj
    }

    fn config(&self, extra: &str) {
        write(
            &self.root,
            "tola.toml",
            &format!(
                "[site]\nbase-path=\"/notes/\"\n[modules.demo]\npath=\"../module\"\n[build]\nentry={{ module=\"demo\" }}\n[typst.fonts]\nsystem=false\n[build.minify]\nhtml=false\ncss=false\njavascript=false\n{extra}"
            ),
        );
    }

    fn loaded(&self) -> SiteConfig {
        load_site_config(
            Some(&self.root.join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    fn session(&self) -> BuildSession {
        BuildSession::with_resources(BuildResources::new().with_input_scope(InputScope::Offline))
    }

    fn html(&self) -> String {
        fs::read_to_string(self.root.join("public/index.html")).unwrap()
    }

    fn build(&self) {
        self.session()
            .build_and_write(&self.loaded(), BuildRequest::default())
            .unwrap();
    }

    fn build_error(&self) -> anyhow::Error {
        self.session()
            .build_and_write(&self.loaded(), BuildRequest::default())
            .expect_err("the build must fail")
    }

    fn asset(&self, path: &str) -> String {
        fs::read_to_string(self.root.join("public/_tola-modules/demo").join(path)).unwrap()
    }

    fn develop(&self) -> (SiteConfig, BuildSession) {
        let cfg = self.loaded();
        let mut session = self.session();
        session
            .build_and_write(&cfg, BuildRequest::new(BuildMode::Development))
            .unwrap();
        (cfg, session)
    }

    fn repoint(&self, path: &str) {
        let cfg_path = self.root.join("tola.toml");
        let text = fs::read_to_string(&cfg_path)
            .unwrap()
            .replace("../module", path);
        fs::write(cfg_path, text).unwrap();
    }
}

#[test]
fn disabled_assets_keep_lib() {
    let proj = Project::new();
    proj.config("[modules.demo.assets]\nenable=false\n[modules.demo.assets.replace]\n\"style.css\"=\"missing.css\"\n");
    proj.build();
    assert!(proj.html().contains("Library available"));
    assert!(!proj.root.join("public/_tola-modules/demo").exists());
}

#[test]
fn selected_assets_publish_replacements() {
    let proj = Project::new();
    write(&proj.root, "overrides/custom.css", "project-style");
    proj.config("[assets]\ncache-busting=true\n[modules.demo.assets.replace]\n\"style.css\"=\"overrides/custom.css\"\n");
    proj.build();
    let out = proj.root.join("public/_tola-modules/demo");
    assert_eq!(
        fs::read_to_string(out.join("style.css")).unwrap(),
        "project-style"
    );
    assert_eq!(
        fs::read_to_string(out.join("scripts/main.js")).unwrap(),
        "main-script"
    );
    assert!(!out.join("scripts/legacy.js").exists());
    assert!(!out.join("fonts/body.woff2").exists());
    let html = proj.html();
    for text in [
        "style-input: project-style",
        "script-input: main-script",
        "/notes/_tola-modules/demo/style.css?h=",
        "/notes/_tola-modules/demo/scripts/main.js?h=",
    ] {
        assert!(html.contains(text), "{text}: {html}");
    }
}

#[test]
fn replacement_skip_and_unknown() {
    let proj = Project::new();
    proj.config("[modules.demo.assets.replace]\n\"scripts/legacy.js\"=\"missing.js\"\n");
    proj.build();
    assert!(
        !proj
            .root
            .join("public/_tola-modules/demo/scripts/legacy.js")
            .exists()
    );

    proj.config("[modules.demo.assets.replace]\n\"missing.css\"=\"custom.css\"\n");
    proj.build_error();
}

#[test]
fn moved_module_keeps_alias() {
    let proj = Project::new();
    proj.build();
    let moved = proj.module.with_file_name("relocated");
    fs::rename(&proj.module, &moved).unwrap();
    write(&moved, "assets/style.css", "moved-style");
    proj.repoint("../relocated");
    proj.build();
    assert_eq!(proj.asset("style.css"), "moved-style");
    assert!(proj.html().contains("style-input: moved-style"));
}

#[test]
fn disabled_file_overrides_tree() {
    let proj = Project::new();
    write(&proj.module, "assets/scripts/bad#name.js", "disabled");
    let manifest = proj.module.join("tola-module.toml");
    let src = fs::read_to_string(&manifest).unwrap().replace(
        "files=[",
        "files=[{ source=\"assets/scripts/bad#name.js\", url=\"/scripts/ignored.js\", enable=false }, ",
    );
    fs::write(manifest, src).unwrap();
    proj.build();
    assert!(
        proj.root
            .join("public/_tola-modules/demo/style.css")
            .exists()
    );
    assert!(
        !proj
            .root
            .join("public/_tola-modules/demo/scripts/ignored.js")
            .exists()
    );
    assert!(
        !proj
            .root
            .join("public/_tola-modules/demo/scripts/bad#name.js")
            .exists()
    );
}

#[test]
fn replacement_change_republishes() {
    let proj = Project::new();
    write(&proj.root, "custom.css", "first replacement");
    proj.config(
        "[assets]\ncache-busting=true\n[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n",
    );
    let (cfg, mut session) = proj.develop();
    let first = proj.html();
    write(&proj.root, "custom.css", "second replacement");
    rebuild(&mut session, &cfg, vec![proj.root.join("custom.css")]);
    let second = proj.html();
    assert!(second.contains("style-input: second replacement"));
    assert_ne!(first, second);
    assert_eq!(proj.asset("style.css"), "second replacement");
}

#[test]
fn module_replacement_keeps_alias_asset() {
    let proj = Project::new();
    fs::rename(&proj.module, proj.root.join("module")).unwrap();
    proj.config("[modules.other]\npath=\"module\"\n[modules.demo.assets.replace]\n\"style.css\"=\"module/assets/style.css\"\n");
    proj.repoint("module");
    proj.build();
    for alias in ["demo", "other"] {
        assert_eq!(
            fs::read_to_string(
                proj.root
                    .join(format!("public/_tola-modules/{alias}/style.css"))
            )
            .unwrap(),
            "module-style"
        );
    }
    assert!(proj.html().contains("style-input: module-style"));
}

#[cfg(unix)]
#[test]
fn retargeted_replacement_rejected() {
    use std::os::unix::fs::symlink;
    let proj = Project::new();
    write(&proj.root, "inside.css", "inside");
    write(proj._dir.path(), "outside.css", "outside");
    symlink("inside.css", proj.root.join("custom.css")).unwrap();
    proj.config("[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n");
    let cfg = proj.loaded();
    let candidate = crate::build::build_site(&cfg, BuildMode::Production).unwrap();
    fs::remove_file(proj.root.join("custom.css")).unwrap();
    symlink(
        proj._dir.path().join("outside.css"),
        proj.root.join("custom.css"),
    )
    .unwrap();
    assert!(crate::build::write_site(&candidate).is_err());
    assert!(!proj.root.join("public/index.html").exists());
}
