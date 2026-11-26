use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::RecursiveMode;
use tempfile::TempDir;
use tola_build::build::InputPathScope;
use tola_build::config::SiteConfig;

use super::super::boundary::{
    ClassifiedPath, CoverageCertainty, DirectoryWatch, EventBoundary, WatchSelection,
};
use super::super::event::is_structural;
use super::super::producer::{
    ChangeAdmission, ProducerKind, ProducerSet, WatchRequirement, WatchRequirements, WatchScope,
    dedupe_requirements,
};
use super::super::tests::{local_packages, resolve_schema};
use super::{RootSet, collect_hook_requirements, collect_roots, selection_for_observed};

fn configured_site(root: &Path, src: &str) -> SiteConfig {
    let schema = tola_build::config::SiteConfigSchema::parse(
        &root.join("tola.toml"),
        src,
        tola_input::InputScope::Online,
    )
    .unwrap();
    resolve_schema(root, schema, local_packages())
}

fn default_site() -> (TempDir, PathBuf, SiteConfig) {
    let (dir, root) = canonical_dir();
    std::fs::create_dir_all(root.join("content")).unwrap();
    let cfg = configured_site(&root, "");
    (dir, root, cfg)
}

fn classifies(sel: &WatchSelection, path: &Path, producer: ProducerKind) -> bool {
    sel.boundary().classify(path).producers().contains(producer)
}

#[cfg(unix)]
fn has_assets_requirement(reqs: &[WatchRequirement], path: &Path, scope: WatchScope) -> bool {
    has_requirement(reqs, path, scope, ProducerKind::SiteAssets, true)
}

fn has_requirement(
    reqs: &[WatchRequirement],
    path: &Path,
    scope: WatchScope,
    producer: ProducerKind,
    required: bool,
) -> bool {
    reqs.iter().any(|req| {
        req.path == path
            && req.scope == scope
            && req.producers.contains(producer)
            && req.required == required
    })
}

fn canonical_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

fn exact(path: PathBuf, producer: ProducerKind) -> WatchRequirement {
    WatchRequirement::exact_for(path, ProducerSet::one(producer))
}

fn recursive(path: PathBuf, producer: ProducerKind) -> WatchRequirement {
    WatchRequirement::recursive_for(path, ProducerSet::one(producer))
}

fn selection_for(desired: Vec<WatchRequirement>) -> WatchSelection {
    selection_for_observed(desired, &mut BTreeMap::new(), Arc::default())
}

fn root_set(desired: Vec<WatchRequirement>) -> RootSet {
    RootSet::with_hook_outputs(desired, Arc::default())
}

fn install(roots: &mut RootSet, sel: WatchSelection, epoch: u64) {
    let update = roots.directory_update(&sel, false);
    roots.commit(sel, update, epoch);
}

fn watches(sel: &WatchSelection, path: &Path, mode: RecursiveMode) -> bool {
    sel.directories
        .iter()
        .any(|dir| dir.path() == path && dir.recursive_mode() == mode)
}

fn accepts(boundary: &EventBoundary, path: &Path) -> bool {
    !boundary.classify(path).is_empty()
}

#[test]
fn grouping_preserves_classification() {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};
    let mut desired = Vec::new();
    for prefix in [
        "", "site", "site/a", "site/a/b", "site/a-b", "site/aa", "other",
    ] {
        for scope in [
            WatchScope::Exact,
            WatchScope::Children,
            WatchScope::Recursive,
        ] {
            for producer in [
                ProducerKind::Content,
                ProducerKind::Fonts,
                ProducerKind::Hooks,
            ] {
                desired.push(WatchRequirement::for_scope(
                    PathBuf::from(prefix),
                    scope,
                    ProducerSet::one(producer),
                ));
            }
        }
    }
    let mut cases = desired
        .iter()
        .cloned()
        .map(|req| vec![req])
        .collect::<Vec<_>>();
    cases.push(desired);
    for mut desired in cases {
        dedupe_requirements(&mut desired);
        let boundary = EventBoundary {
            desired: desired.into(),
            ..Default::default()
        };
        for path in [
            "",
            "site",
            "site/a",
            "site/a/b",
            "site/a/b/c.typ",
            "site/a/b/c.css",
            "site/a-b/file.typ",
            "site/aa/file.typ",
            "other/file.typ",
            "unrelated",
        ] {
            let path = Path::new(path);
            for kind in [
                notify::EventKind::Any,
                notify::EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                notify::EventKind::Create(CreateKind::File),
                notify::EventKind::Remove(RemoveKind::Folder),
            ] {
                let mut expected = ClassifiedPath::default();
                for req in boundary.desired.iter() {
                    if req.path.starts_with(path)
                        || (req.matches(path)
                            && (is_structural(kind)
                                || req.admission == ChangeAdmission::Scope
                                || tola_build::has_typ_extension(path)))
                    {
                        expected.merge(req, false);
                    }
                }
                assert_eq!(
                    boundary.classify_change(path, kind),
                    expected,
                    "{path:?} {kind:?}"
                );
                let expected_artifact = boundary.desired.iter().any(|req| {
                    req.path.starts_with(path)
                        || (req.matches(path) && req.admission == ChangeAdmission::Scope)
                });
                assert_eq!(
                    boundary.admits_editor_artifact(path, false),
                    expected_artifact
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_retarget() {
    use std::os::unix::fs::symlink;
    let (_dir, root) = canonical_dir();
    let site = root.join("site");
    let first = root.join("first");
    let second = root.join("second");
    for path in [&site, &first, &second] {
        std::fs::create_dir_all(path).unwrap();
    }
    let link = site.join("theme");
    symlink(&first, &link).unwrap();
    let mut roots = root_set(vec![recursive(link.clone(), ProducerKind::TypstReads)]);
    let selected = roots.initial_selection();
    assert!(watches(&selected, &first, RecursiveMode::Recursive));
    assert!(accepts(&selected.boundary(), &first.join("lib.typ")));
    install(&mut roots, selected, 1);

    std::fs::remove_file(&link).unwrap();
    symlink(&second, &link).unwrap();
    let retarget = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::Any))
        .add_path(link);
    assert_eq!(roots.boundary().invalidated_by(&retarget), ProducerSet::ALL);

    let selected = roots.current_selection();
    assert!(watches(&selected, &second, RecursiveMode::Recursive));
    install(&mut roots, selected, 2);
    assert!(accepts(&roots.boundary(), &second.join("lib.typ")));
    assert!(!accepts(&roots.boundary(), &first.join("lib.typ")));
}

#[cfg(unix)]
#[test]
fn hook_rerun_symlink_target() {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};
    use std::os::unix::fs::symlink;

    let (_dir, root) = canonical_dir();
    let cfg = configured_site(
        &root,
        "[[hooks.before-build]]\nname = \"prepare\"\nargv = [\"unused\"]\n\
         rerun-on = { include = [\"input-link\"] }\n",
    );
    for name in ["first", "second"] {
        std::fs::create_dir_all(root.join(name).join("nested")).unwrap();
        std::fs::write(root.join(name).join("input.txt"), name).unwrap();
    }
    let redirect = root.join("redirect");
    let first = root.join("first/input.txt");
    let second = root.join("second/input.txt");
    symlink("first/nested", &redirect).unwrap();
    symlink("redirect/../input.txt", root.join("input-link")).unwrap();
    let roots = RootSet::from_config(&cfg, &WatchRequirements::default()).unwrap();
    let hooks = ProducerSet::one(ProducerKind::Hooks);
    let write = notify::EventKind::Modify(ModifyKind::Data(DataChange::Any));
    let assert_watched = |sel: &WatchSelection, target: &Path, kinds: &[notify::EventKind]| {
        assert!(watches(
            sel,
            target.parent().unwrap(),
            RecursiveMode::NonRecursive
        ));
        let boundary = sel.boundary();
        for &kind in kinds {
            assert_eq!(boundary.classify_change(target, kind).producers(), hooks);
        }
    };
    let selected = roots.initial_selection();
    assert_watched(&selected, &first, &[write]);
    assert!(
        selected
            .boundary()
            .classify_change(&root.join("input.txt"), write)
            .is_empty()
    );

    std::fs::remove_file(&first).unwrap();
    let selected = roots.current_selection();
    assert_watched(
        &selected,
        &first,
        &[
            notify::EventKind::Remove(RemoveKind::File),
            notify::EventKind::Create(CreateKind::File),
        ],
    );
    std::fs::write(&first, "recreated").unwrap();

    std::fs::remove_file(&redirect).unwrap();
    symlink("second/nested", &redirect).unwrap();
    let retarget =
        notify::Event::new(notify::EventKind::Create(CreateKind::Any)).add_path(redirect);
    assert_eq!(
        selected.boundary().invalidated_by(&retarget),
        ProducerSet::ALL
    );
    let selected = roots.current_selection();
    assert_watched(&selected, &second, &[write]);
    assert!(
        selected
            .boundary()
            .classify_change(&first, write)
            .is_empty()
    );
}

#[test]
fn output_root_unwatched() {
    let (_dir, root, cfg) = default_site();
    let output = root.join("public");

    let roots = collect_roots(&cfg, &WatchRequirements::default()).unwrap();

    assert!(!output.exists());
    assert!(!roots.iter().any(|watch| watch.path == output));
}

#[test]
fn site_entry_exact() {
    let (_dir, root, cfg) = default_site();
    let entry = root.join("site.typ");
    std::fs::write(&entry, "#document(\"index.html\")[Hello]").unwrap();

    let roots = collect_roots(&cfg, &WatchRequirements::default()).unwrap();

    assert!(has_requirement(
        &roots,
        &entry,
        WatchScope::Exact,
        ProducerKind::TypstReads,
        true,
    ));
}

#[test]
fn font_root_recursive() {
    let (_dir, root) = canonical_dir();
    let fonts = root.join("fonts");
    std::fs::create_dir_all(root.join("content")).unwrap();
    let cfg = configured_site(&root, "[typst.fonts]\npaths = [\"fonts\"]\n");

    let sel = selection_for(collect_roots(&cfg, &WatchRequirements::default()).unwrap());
    let boundary = sel.boundary();

    assert!(has_requirement(
        &sel.desired,
        &fonts,
        WatchScope::Recursive,
        ProducerKind::Fonts,
        false,
    ));
    assert!(accepts(&boundary, &fonts));
    assert!(accepts(&boundary, &fonts.join("site.woff2")));
}

#[test]
fn package_checks_availability() {
    let (_dir, root, cfg) = default_site();
    let package_data = root.join("package-data");
    let cache = root.join("package-cache");
    let missing = package_data.join("local/example/1.0.0");
    let selected = cache.join("local/example/1.0.0");
    std::fs::create_dir_all(&selected).unwrap();
    let locations =
        tola_typst::PackageLocations::from_absolute_roots(Some(package_data), Some(cache)).unwrap();
    let store = tola_typst::PackageStore::new(locations, tola_typst::PackageFetchMode::LocalOnly);
    let pkg = "@local/example:1.0.0".parse().unwrap();
    let prepared = store.prepare(&pkg).unwrap();
    let mut dynamic = WatchRequirements::default();
    dynamic.package_checks(prepared.checks().iter().cloned());

    let sel = selection_for(collect_roots(&cfg, &dynamic).unwrap());
    let boundary = sel.boundary();

    assert!(has_requirement(
        &sel.desired,
        &missing,
        WatchScope::Exact,
        ProducerKind::PackageResolution,
        false,
    ));
    assert!(has_requirement(
        &sel.desired,
        &selected,
        WatchScope::Exact,
        ProducerKind::PackageResolution,
        true,
    ));
    assert_eq!(
        boundary.classify(&missing).rebuild_scope(),
        crate::dev::watch::fs::RebuildScope::FullTypst
    );
    assert_eq!(
        boundary.classify(&selected).rebuild_scope(),
        crate::dev::watch::fs::RebuildScope::FullTypst
    );
}

#[test]
fn physical_reads_exact() {
    let (_dir, root, cfg) = default_site();
    let templates = root.join("theme/templates");
    std::fs::create_dir_all(&templates).unwrap();
    let template = templates.join("document.typ");
    std::fs::write(&template, "#let document-template(body) = body").unwrap();

    let mut dynamic = WatchRequirements::default();
    dynamic.observe_paths(
        ProducerKind::TypstReads,
        InputPathScope::Exact,
        true,
        [template.clone()],
    );
    let roots = collect_roots(&cfg, &dynamic).unwrap();

    assert!(has_requirement(
        &roots,
        &template,
        WatchScope::Exact,
        ProducerKind::TypstReads,
        true,
    ));
    assert!(!roots.iter().any(|watch| {
        watch.path == root
            && watch.scope == WatchScope::Recursive
            && watch.producers.contains(ProducerKind::TypstReads)
    }));
}

#[test]
fn subscription_event_admission() {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

    let (_dir, root) = canonical_dir();
    let cfg = configured_site(
        &root,
        "[assets]\ntrees = [{ source = \"static\", url-prefix = \"/assets\", include = [\"static/web/**/*.css\", \"unrelated/**/*.css\"], exclude = [\"static/web/private\"] }]\n\
         [[hooks.before-build]]\nname = \"prepare\"\nargv = [\"true\"]\n\
         rerun-on = { include = [\"scripts/**/*.mjs\", \"static/web/**/*.mjs\", \"future/tools/*.mjs\"], exclude = [\"scripts/private\"] }\n",
    );
    let content = root.join("content");
    let static_tree = root.join("static/web");
    let scripts = root.join("scripts");
    let read = root.join("inputs/data.json");
    let content_read = content.join("data.json");
    for dir in [&content, &static_tree, &scripts] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::create_dir_all(read.parent().unwrap()).unwrap();
    let mut dynamic = WatchRequirements::default();
    dynamic.observe_paths(
        ProducerKind::TypstReads,
        InputPathScope::Exact,
        true,
        [read.clone(), content_read],
    );
    let sel = selection_for(collect_roots(&cfg, &dynamic).unwrap());
    let boundary = sel.boundary();
    let future = root.join("future/tools");
    assert!(!future.exists());
    assert!(has_requirement(
        &sel.desired,
        &future,
        WatchScope::Recursive,
        ProducerKind::Hooks,
        false,
    ));
    assert!(has_requirement(
        &sel.desired,
        &static_tree,
        WatchScope::Recursive,
        ProducerKind::SiteAssets,
        false,
    ));
    assert!(!sel.desired.iter().any(|req| {
        req.producers.contains(ProducerKind::SiteAssets)
            && req.path.starts_with(root.join("unrelated"))
    }));

    let content_root =
        ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::TypstReads);
    let reads = ProducerSet::one(ProducerKind::TypstReads);
    let data_change = || notify::EventKind::Modify(ModifyKind::Data(DataChange::Any));
    let cases = [
        (
            content.join("extra.txt"),
            data_change(),
            ProducerSet::default(),
        ),
        (
            content.join("image.png"),
            data_change(),
            ProducerSet::default(),
        ),
        (content.join("page.typ"), data_change(), content_root),
        (
            content.join("nested"),
            notify::EventKind::Create(CreateKind::Folder),
            content_root,
        ),
        (
            content.join("extra.txt"),
            notify::EventKind::Any,
            content_root,
        ),
        (read, data_change(), reads),
        (
            scripts.join("search.mjs"),
            data_change(),
            ProducerSet::one(ProducerKind::Hooks),
        ),
        (
            static_tree.join("app.css"),
            data_change(),
            ProducerSet::one(ProducerKind::SiteAssets),
        ),
        (
            static_tree.join("build.mjs"),
            notify::EventKind::Create(CreateKind::File),
            ProducerSet::one(ProducerKind::Hooks),
        ),
        (
            static_tree.join("notes.txt"),
            notify::EventKind::Create(CreateKind::File),
            ProducerSet::default(),
        ),
        (
            root.join("unrelated/site.css"),
            data_change(),
            ProducerSet::default(),
        ),
        (
            static_tree.join("private/local.css"),
            data_change(),
            ProducerSet::default(),
        ),
        (
            scripts.join("private"),
            notify::EventKind::Create(CreateKind::Folder),
            ProducerSet::default(),
        ),
        (
            scripts.join("new"),
            notify::EventKind::Create(CreateKind::Folder),
            ProducerSet::one(ProducerKind::Hooks),
        ),
        (
            scripts.join("renamed"),
            notify::EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            ProducerSet::one(ProducerKind::Hooks),
        ),
        (
            future.parent().unwrap().to_path_buf(),
            notify::EventKind::Remove(RemoveKind::Folder),
            ProducerSet::one(ProducerKind::Hooks),
        ),
        (
            future.join("new.mjs"),
            notify::EventKind::Create(CreateKind::File),
            ProducerSet::one(ProducerKind::Hooks),
        ),
    ];
    for (path, kind, expected) in cases {
        assert_eq!(
            boundary.classify_change(&path, kind).producers(),
            expected,
            "{} under {kind:?}",
            path.display()
        );
    }
}

#[test]
fn recursive_parent_coverage() {
    let (_dir, root) = canonical_dir();
    let content = root.join("content");
    let nested = content.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut roots = root_set(vec![recursive(content.clone(), ProducerKind::Content)]);
    let initial = roots.initial_selection();
    install(&mut roots, initial, 1);
    roots.confirm_current_observation();
    let mut desired = roots.selection.desired.clone();
    for (producer, name) in [
        (ProducerKind::TypstReads, "theme.typ"),
        (ProducerKind::Fonts, "theme-font.woff2"),
        (ProducerKind::SiteAssets, "image.svg"),
    ] {
        let file = nested.join(name);
        std::fs::write(&file, "input").unwrap();
        desired.push(exact(file, producer));
    }
    let selected = selection_for(desired);
    assert!(roots.extends_observation(&selected));
    assert!(
        roots
            .directory_update(&selected, false)
            .retains_directories()
    );
    install(&mut roots, selected, 2);
    assert!(
        roots
            .coverage(ProducerKind::Content)
            .unwrap()
            .covers_current_revision()
    );
    for producer in [
        ProducerKind::TypstReads,
        ProducerKind::Fonts,
        ProducerKind::SiteAssets,
    ] {
        assert_eq!(
            roots.coverage(producer).unwrap().certainty(),
            CoverageCertainty::AcceptedEvents
        );
        assert!(!roots.coverage(producer).unwrap().covers_current_revision());
    }
    assert_eq!(
        roots
            .boundary()
            .classify(&nested.join("theme-font.woff2"))
            .producers(),
        ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::Fonts)
    );
    assert_eq!(
        roots
            .boundary()
            .classify(&content.join("sibling.typ"))
            .producers(),
        ProducerSet::one(ProducerKind::Content)
    );
}

#[test]
fn parent_coverage_replacement() {
    let (_dir, root) = canonical_dir();
    let file = root.join("one.typ");
    let mut roots = root_set(vec![
        exact(file.clone(), ProducerKind::TypstReads).optional(),
    ]);
    let selected = roots.initial_selection();
    install(&mut roots, selected, 1);
    std::fs::write(&file, "first").unwrap();
    let created = roots.current_selection();
    assert!(
        roots
            .directory_update(&created, false)
            .retains_directories()
    );
    let replacement = root.join("replacement");
    std::fs::write(&replacement, "second").unwrap();
    std::fs::rename(&replacement, &file).unwrap();
    let replaced = roots.current_selection();
    assert!(
        roots
            .directory_update(&replaced, false)
            .retains_directories()
    );
    let ev = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(file.clone());
    assert!(roots.boundary().invalidated_by(&ev).is_empty());
    assert!(accepts(&roots.boundary(), &file));
    assert!(!accepts(&roots.boundary(), &root.join("unrelated.typ")));
}

#[test]
fn shrink_retains_directories() {
    let (_dir, root) = canonical_dir();
    let a = root.join("a");
    let b = root.join("b");
    let c = root.join("c");
    for path in [&a, &b, &c] {
        std::fs::create_dir_all(path).unwrap();
    }
    let first = recursive(a.clone(), ProducerKind::Content);
    let second = recursive(b.clone(), ProducerKind::Content);
    let third = recursive(c.clone(), ProducerKind::Content);
    let mut roots = root_set(vec![first.clone()]);
    let selected = roots.initial_selection();
    install(&mut roots, selected, 1);
    install(&mut roots, selection_for(vec![first, second.clone()]), 2);
    let installed = roots.installed.clone();
    let published = selection_for(vec![second.clone()]);
    assert!(!roots.extends_observation(&published));
    assert!(
        roots
            .directory_update(&published, false)
            .retains_directories()
    );
    roots.commit_logical(published, 3);
    assert_eq!(roots.installed, installed);
    assert!(!accepts(&roots.boundary(), &a.join("left.typ")));
    let next = selection_for(vec![second, third]);
    let update = roots.directory_update(&next, false);
    assert!(!update.retains_directories());
    roots.commit(next, update, 4);
    assert!(roots.installed.iter().any(|dir| dir.path == b));
    assert!(roots.installed.iter().any(|dir| dir.path == c));
    assert!(!roots.installed.iter().any(|dir| dir.path == a));
}

#[test]
fn retained_directory_invalidation() {
    let (_dir, root) = canonical_dir();
    let a = root.join("a");
    let b = root.join("b");
    for path in [&a, &b] {
        std::fs::create_dir_all(path).unwrap();
    }
    let mut roots = root_set(vec![
        recursive(a.clone(), ProducerKind::Content),
        recursive(b.clone(), ProducerKind::Content),
    ]);
    let selected = roots.initial_selection();
    install(&mut roots, selected, 1);
    roots.commit_logical(selection_for(vec![recursive(b, ProducerKind::Content)]), 2);
    assert!(!accepts(&roots.boundary(), &a));
    let child = notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
        .add_path(a.join("unused.typ"));
    assert!(roots.boundary().invalidated_by(&child).is_empty());
    let removed = notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
        .add_path(a);
    assert_eq!(roots.boundary().invalidated_by(&removed), ProducerSet::ALL);
}

#[test]
fn missing_paths_nearest_parent() {
    let (_dir, root) = canonical_dir();
    let target = root.join("generated/fonts");
    let mut roots = root_set(vec![
        recursive(target.clone(), ProducerKind::Fonts).optional(),
    ]);
    let missing = roots.initial_selection();
    assert_eq!(
        missing.directories,
        vec![DirectoryWatch {
            path: root.clone(),
            mode: RecursiveMode::NonRecursive
        }]
    );
    assert!(accepts(&missing.boundary(), &root.join("generated")));
    assert!(!accepts(&missing.boundary(), &root.join("unrelated")));
    install(&mut roots, missing, 1);
    std::fs::create_dir(root.join("generated")).unwrap();
    let intermediate = roots.current_selection();
    assert!(watches(
        &intermediate,
        &root.join("generated"),
        RecursiveMode::NonRecursive
    ));
    install(&mut roots, intermediate, 2);
    std::fs::create_dir(&target).unwrap();
    let existing = roots.current_selection();
    assert!(watches(&existing, &target, RecursiveMode::Recursive));
    assert!(
        !roots
            .directory_update(&existing, false)
            .retains_directories()
    );
}

#[cfg(unix)]
#[test]
fn asset_symlink_retarget() {
    use std::os::unix::fs::symlink;

    let (_dir, base) = canonical_dir();
    let site = base.join("site");
    let external = base.join("external");
    let first_tree = external.join("tree-a");
    let second_tree = external.join("tree-b");
    let first_file = external.join("file-a.bin");
    let second_file = external.join("file-b.bin");
    for dir in [
        site.join("content"),
        first_tree.clone(),
        second_tree.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(first_tree.join("member.bin"), b"first").unwrap();
    std::fs::write(second_tree.join("member.bin"), b"second").unwrap();
    std::fs::write(&first_file, b"first").unwrap();
    std::fs::write(&second_file, b"second").unwrap();
    let tree_link = site.join("tree-link");
    let file_link = site.join("file-link.bin");
    symlink(&first_tree, &tree_link).unwrap();
    symlink(&first_file, &file_link).unwrap();

    let schema = tola_build::config::SiteConfigSchema::parse(
        &site.join("tola.toml"),
        "[assets]\n\
         trees = [{ source = \"tree-link\", url-prefix = \"/assets\", include = [\"tree-link/*.bin\"] }]\n\
         files = [{ source = \"file-link.bin\", url = \"/download.bin\" }]\n",
        tola_input::InputScope::Online,
    )
    .unwrap();
    let cfg = resolve_schema(&site, schema, local_packages());
    std::fs::write(&cfg.build().entry, "#document(\"index.html\")[Site]").unwrap();
    let first =
        tola_build::build::build_site(&cfg, tola_build::build::BuildMode::Production).unwrap();
    let first_tree_physical = std::fs::canonicalize(&first_tree).unwrap();
    let first_file_physical = std::fs::canonicalize(&first_file).unwrap();
    let dynamic = crate::dev::watch::requirements::watch_requirements_from_observation(
        &first.input_observation(),
    );
    let sel = selection_for(collect_roots(&cfg, &dynamic).unwrap());

    assert!(has_assets_requirement(
        &sel.desired,
        &tree_link,
        WatchScope::Recursive
    ));
    assert!(has_assets_requirement(
        &sel.desired,
        &file_link,
        WatchScope::Exact
    ));
    assert!(has_assets_requirement(
        &sel.desired,
        &first_tree_physical,
        WatchScope::Recursive,
    ));
    assert!(has_assets_requirement(
        &sel.desired,
        &first_file_physical,
        WatchScope::Exact,
    ));
    assert!(classifies(
        &sel,
        &first_tree_physical.join("member.bin"),
        ProducerKind::SiteAssets
    ));
    let unrelated = sel.boundary().classify_change(
        &first_tree_physical.join("notes.txt"),
        notify::EventKind::Create(notify::event::CreateKind::File),
    );
    assert!(!unrelated.producers().contains(ProducerKind::SiteAssets));
    assert!(classifies(
        &sel,
        &first_file_physical,
        ProducerKind::SiteAssets
    ));
    assert!(
        sel.boundary()
            .classify(&external.join("unowned.bin"))
            .is_empty()
    );

    std::fs::remove_dir_all(&first_tree).unwrap();
    assert!(
        sel.boundary()
            .classify(&first_tree_physical)
            .producers()
            .contains(ProducerKind::SiteAssets)
    );
    let missing_sel = selection_for(collect_roots(&cfg, &dynamic).unwrap());
    assert!(has_assets_requirement(
        &missing_sel.desired,
        &first_tree_physical,
        WatchScope::Recursive,
    ));

    std::fs::remove_file(&tree_link).unwrap();
    std::fs::remove_file(&file_link).unwrap();
    symlink(&second_tree, &tree_link).unwrap();
    symlink(&second_file, &file_link).unwrap();
    let retargeted =
        tola_build::build::build_site(&cfg, tola_build::build::BuildMode::Production).unwrap();
    let retargeted_dynamic = crate::dev::watch::requirements::watch_requirements_from_observation(
        &retargeted.input_observation(),
    );
    let retargeted_sel = selection_for(collect_roots(&cfg, &retargeted_dynamic).unwrap());
    assert!(has_assets_requirement(
        &retargeted_sel.desired,
        &std::fs::canonicalize(&second_tree).unwrap(),
        WatchScope::Recursive,
    ));
    assert!(has_assets_requirement(
        &retargeted_sel.desired,
        &std::fs::canonicalize(&second_file).unwrap(),
        WatchScope::Exact,
    ));
    assert!(
        retargeted_sel
            .boundary()
            .classify(&first_tree_physical.join("member.bin"))
            .is_empty()
    );
}

#[test]
fn missing_file_siblings() {
    let dir = TempDir::new().unwrap();
    let cfg = configured_site(
        dir.path(),
        "[assets]\ntrees = []\nfiles = [{ source = \"assets/CNAME\", url = \"/CNAME\" }]\n",
    );
    let root = cfg.root().to_path_buf();
    let assets = root.join("assets");
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(&assets).unwrap();
    let target = assets.join("CNAME");

    let sel = selection_for(collect_roots(&cfg, &WatchRequirements::default()).unwrap());
    assert!(has_requirement(
        &sel.desired,
        &target,
        WatchScope::Exact,
        ProducerKind::SiteAssets,
        false,
    ));
    assert!(watches(
        &sel,
        &assets.canonicalize().unwrap(),
        RecursiveMode::NonRecursive
    ));
    let boundary = sel.boundary();
    assert!(accepts(&boundary, &target));
    assert!(!accepts(&boundary, &assets.join("unrelated.txt")));
    assert!(!accepts(&boundary, &assets.join("other/deep.txt")));
}

#[test]
fn hook_rerun_literals() {
    let (_dir, root) = canonical_dir();
    let src = root.join(" scripts");
    let input = src.join("search.py");
    for dir in [root.join("content"), root.join("public"), src] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(&input, "# search index generator").unwrap();

    let cfg = configured_site(
        &root,
        "[[hooks.generate-outputs]]\nname = \"search\"\nargv = [\"python3\", \" scripts/search.py\"]\n\
         rerun-on = { include = [\" scripts/search.py\"] }\noutputs = [{ file = \"search.json\" }]\n",
    );
    let mut logical = Vec::new();
    collect_hook_requirements(&cfg, &mut logical).unwrap();
    assert!(has_requirement(
        &logical,
        &input,
        WatchScope::Recursive,
        ProducerKind::Hooks,
        false,
    ));

    let sel = selection_for(collect_roots(&cfg, &WatchRequirements::default()).unwrap());
    assert!(classifies(&sel, &input, ProducerKind::Hooks));
    assert!(sel.directories.iter().any(|dir| {
        dir.path == input.parent().unwrap().canonicalize().unwrap()
            || (dir.mode == RecursiveMode::Recursive && input.starts_with(&dir.path))
    }));
}

#[test]
fn recovery_both_configs() {
    let (_dir, root, published) = default_site();
    let published_content = root.join("content");
    std::fs::write(
        &published.build().entry,
        "#document(\"index.html\")[Published]",
    )
    .unwrap();
    let mut attempted_schema = tola_build::config::SiteConfigSchema::default();
    let attempted_content = root.join("candidate/content");
    let attempted_entry = root.join("candidate/site.typ");
    attempted_schema.build.content_dir = PathBuf::from("candidate/content");
    attempted_schema.build.entry = "candidate/site.typ".into();
    let attempted = resolve_schema(&root, attempted_schema, local_packages());
    let mut roots = RootSet::from_config(&published, &WatchRequirements::default()).unwrap();
    let initial = roots.initial_selection();
    install(&mut roots, initial, 1);
    let (recovery, _) = roots
        .configs_selection([&published, &attempted], &WatchRequirements::default())
        .unwrap();
    assert!(watches(&recovery, &root, RecursiveMode::NonRecursive));
    assert!(classifies(
        &recovery,
        &attempted_content,
        ProducerKind::Content
    ));
    assert!(classifies(
        &recovery,
        &attempted_entry,
        ProducerKind::TypstReads
    ));
    install(&mut roots, recovery, 2);
    std::fs::create_dir_all(&attempted_content).unwrap();
    std::fs::write(&attempted_entry, "#document(\"index.html\")[Recovered]").unwrap();
    let recovered = roots.current_selection();
    for content in [&published_content, &attempted_content] {
        assert!(watches(&recovered, content, RecursiveMode::Recursive));
    }
    assert!(accepts(&recovered.boundary(), &published.build().entry));
    assert!(accepts(&recovered.boundary(), &attempted_entry));
}
