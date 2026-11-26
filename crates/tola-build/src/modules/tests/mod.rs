mod assets;
mod theme;

use super::*;
use std::fs;
use tola_input::InputScope;
use tola_module::declaration::*;
use tola_module::module_set::{LoadedModule, ModuleLoadError};

fn loaded_modules(
    site: &Path,
    decls: &BTreeMap<String, ModuleDeclaration>,
    mode: InputMode,
) -> std::result::Result<BTreeMap<String, LoadedModule>, ModuleLoadError> {
    let roots = crate::inputs::module_roots(site, mode, None, decls, &Default::default())?;
    load_modules_with_roots(site, decls, &roots, mode)
}

fn theme_site(manifest: &str) -> (tempfile::TempDir, BTreeMap<String, ModuleDeclaration>) {
    let site = tempfile::tempdir().unwrap();
    let mod_dir = site.path().join("theme");
    fs::create_dir(&mod_dir).unwrap();
    fs::write(mod_dir.join("model.typ"), "#metadata(())<forest-nodes>").unwrap();
    fs::write(mod_dir.join("tola-module.toml"), manifest).unwrap();
    (
        site,
        BTreeMap::from([(
            "forest".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: "theme".into(),
                },
                ..Default::default()
            },
        )]),
    )
}

const VALID: &str = r#"
[queries.nodes]
entry = "model.typ"
label = "forest-nodes"
[commands.list]
help = "List nodes"
query = "nodes"
projection = ["id", "name"]
[commands.edit]
help = "Edit a node"
argv = ["editor", "--wait"]
"#;

#[test]
fn tarball_supplies_entry() {
    use crate::config::loading::{BuildOverrides, load_site_config};
    use crate::{BuildResources, BuildSession};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("content")).unwrap();
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(root.join("theme.tar.gz")).unwrap(),
        flate2::Compression::fast(),
    );
    let mut archive = tar::Builder::new(gzip);
    for (name, text) in [
        ("theme/tola-module.toml", "[build]\nentry = 'site.typ'\n"),
        (
            "theme/site.typ",
            "#document(\"index.html\", format: \"html\")[A packaged site]",
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(text.len() as u64);
        archive
            .append_data(&mut header, name, text.as_bytes())
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
    fs::write(
        root.join("tola.toml"),
        "[build]\nentry = { module = 'theme' }\n[modules.theme]\ntarball = { path = 'theme.tar.gz' }\n[typst.fonts]\nsystem = false\n",
    )
    .unwrap();
    let cfg = load_site_config(
        Some(&root.join("tola.toml")),
        tola_typst::PackageLocations::default(),
        &BuildOverrides::default(),
    )
    .unwrap()
    .into_config();
    let mut session =
        BuildSession::with_resources(BuildResources::new().with_input_scope(InputScope::Offline));
    session
        .build_and_write(&cfg, crate::build::BuildRequest::default())
        .unwrap();
    assert!(root.join("public/index.html").is_file());
    assert!(matches!(
        tola_input::InputLock::read(root).unwrap().unwrap().modules["theme"],
        tola_input::LockedModule::Tarball { .. }
    ));
    fs::remove_dir_all(tola_input::layout::modules(root)).unwrap();
    let resources = BuildResources::new().with_input_scope(InputScope::Pure);
    let prepared = prepare_site(&cfg, resources.input_mode(), &Default::default()).unwrap();
    let boundary = resources.source_boundary(prepared.config());
    assert!(boundary.check(&prepared.build().entry).is_ok());
    assert!(boundary.check(&root.join(".tola/unrelated.typ")).is_err());
    BuildSession::with_resources(resources)
        .build_and_write(&cfg, crate::build::BuildRequest::default())
        .unwrap();
}

#[test]
fn options_merge_with_defaults() {
    let manifest = r#"
[options]
show = true
items = ["default"]
nested = { count = 2, date = 2026-01-01, dev = "run" }

[[hooks.generate-outputs]]
name = "render"
nu = "render.nu"
args = { option = "items" }
order = { option = "nested.count" }
dev = { option = "nested.dev" }
outputs = [{ file = "result.txt" }]
"#;
    let (site, mut decls) = theme_site(manifest);
    decls.get_mut("forest").unwrap().options =
        toml::from_str("show = false\nitems = []\nnested = { count = 3, dev = 'reuse' }").unwrap();
    let modules =
        loaded_modules(site.path(), &decls, InputMode::new(InputScope::Pure, false)).unwrap();
    let opts = &modules["forest"].options;
    assert_eq!(opts["show"].as_bool(), Some(false));
    assert!(opts["items"].as_array().unwrap().is_empty());
    assert_eq!(opts["nested"]["count"].as_integer(), Some(3));
    assert_eq!(opts["nested"]["date"].as_str(), Some("2026-01-01"));
    let hooks = tola_module::hooks::hook_stages(site.path(), &modules).unwrap();
    let hook = &hooks.entries()[0];
    assert_eq!(hook.dev, tola_module::DevParticipation::Reuse);
    assert_eq!(hook.order, 3);
    assert!(matches!(&hook.action, tola_module::Action::Nu { args, .. } if args.is_empty()));
}

#[test]
fn local_module_skips_command_exec() {
    let (site, decls) = theme_site(VALID);
    let modules =
        loaded_modules(site.path(), &decls, InputMode::new(InputScope::Pure, false)).unwrap();
    let forest = &modules["forest"];
    assert_eq!(
        forest.root,
        site.path().join("theme").canonicalize().unwrap()
    );
    assert_eq!(forest.queries["nodes"].label, "forest-nodes");
    assert_eq!(
        forest.commands["edit"].action().unwrap(),
        CommandAction::Execute(tola_module::Action::Argv(vec![
            "editor".into(),
            "--wait".into(),
        ]))
    );
}

#[test]
fn rejects_escape_allows_distinct_aliases() {
    let (site, mut decls) = theme_site(&VALID.replace("model.typ", "../outside.typ"));
    assert!(
        loaded_modules(
            site.path(),
            &decls,
            InputMode::new(InputScope::Online, false)
        )
        .is_err()
    );
    fs::write(site.path().join("theme/tola-module.toml"), VALID).unwrap();
    decls.insert(
        "other".into(),
        ModuleDeclaration {
            source: ModuleSource::Local {
                path: "theme".into(),
            },
            ..Default::default()
        },
    );
    assert_eq!(
        loaded_modules(
            site.path(),
            &decls,
            InputMode::new(InputScope::Online, false)
        )
        .unwrap()
        .len(),
        2
    );
}

#[test]
fn rejects_unknown_command_fields() {
    let (site, decls) = theme_site(&VALID.replace("argv = [", "typo = true\nargv = ["));
    let err = loaded_modules(
        site.path(),
        &decls,
        InputMode::new(InputScope::Online, false),
    )
    .unwrap_err();
    assert!(format!("{:#}", anyhow::Error::new(err)).contains("unknown field"));
}

#[cfg(unix)]
#[test]
fn query_entry_symlink_escape() {
    use std::os::unix::fs::symlink;
    let (site, decls) = theme_site(VALID);
    let path = site.path().join("theme/model.typ");
    fs::remove_file(&path).unwrap();
    fs::write(site.path().join("outside.typ"), "outside").unwrap();
    symlink(site.path().join("outside.typ"), path).unwrap();
    let mode = InputMode::new(InputScope::Online, false);
    assert!(matches!(
        loaded_modules(site.path(), &decls, mode),
        Err(ModuleLoadError::Roots(
            tola_module::inputs::ModuleRootError::Tree {
                source: tola_input::TreeHashError::Escapes { .. },
                ..
            }
        )),
    ));
}

#[test]
fn pure_mode_rejects_external_root() {
    let (external, _) = theme_site(VALID);
    let site = tempfile::tempdir().unwrap();
    let decls = BTreeMap::from([(
        "forest".into(),
        ModuleDeclaration {
            source: ModuleSource::Local {
                path: external.path().join("theme"),
            },
            ..Default::default()
        },
    )]);
    assert!(
        loaded_modules(
            site.path(),
            &decls,
            InputMode::new(InputScope::Online, false)
        )
        .is_ok()
    );
    assert!(loaded_modules(site.path(), &decls, InputMode::new(InputScope::Pure, false)).is_err());
}
