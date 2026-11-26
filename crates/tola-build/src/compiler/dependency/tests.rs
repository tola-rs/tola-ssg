use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_typst::{ReadLocator, ReadRecord};

use crate::compiler::tests::compiler_host;

use super::reads::{DepRead, DepReader, package_check_is_fresh, physical_watch_paths};
use super::*;

fn retained_deps(
    root: &Path,
    srcs: impl IntoIterator<Item = PathBuf>,
    entries: impl IntoIterator<Item = PathBuf>,
    reader_evidence: BTreeMap<TypstDependencyReader, DepReader>,
) -> CompilationDependencies {
    let root = tola_input::normalize_path(root);
    CompilationDependencies(Arc::new(DependencyReads {
        path_index: DependencyPathIndex::build(&root, &reader_evidence, &[]),
        content_sources: srcs
            .into_iter()
            .map(|p| tola_input::normalize_path(&p))
            .collect(),
        bundle_entries: entries
            .into_iter()
            .map(|p| tola_input::normalize_path(&p))
            .collect(),
        reader_evidence,
        package_checks: Vec::new(),
    }))
}

fn evidence(path: &str) -> ReadRecord {
    ReadRecord::new(ReadLocator::Root(path.into()), path.as_bytes())
}

fn physical_evidence(logical: PathBuf, canonical: PathBuf, ev: ReadRecord) -> DepRead {
    DepRead::Physical {
        watch_paths: physical_watch_paths(&logical, &canonical),
        logical_path: logical,
        canonical_target: canonical,
        evidence: ev,
    }
}

fn producer_read(root: &Path, ev: ReadRecord) -> DepRead {
    if let ReadLocator::Root(rel) = ev.locator() {
        let logical = logical_read_path(&root.join(rel));
        if let Ok(canonical) = std::fs::canonicalize(&logical) {
            return physical_evidence(logical, canonical, ev);
        }
    }
    DepRead::Virtual {
        process_stable: false,
        evidence: ev,
    }
}

fn physical_read(path: &Path, ev: ReadRecord) -> DepRead {
    let logical = logical_read_path(path);
    let canonical = std::fs::canonicalize(&logical).unwrap();
    physical_evidence(logical, canonical, ev)
}

fn logical_read_path(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap();
    let Some(name) = abs.file_name() else {
        return abs;
    };
    abs.parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .map_or(abs.clone(), |parent| parent.join(name))
}

fn deps_from(
    root: &Path,
    srcs: impl IntoIterator<Item = PathBuf>,
    program: &Path,
    reader_evidence: impl IntoIterator<Item = (TypstDependencyReader, Vec<ReadRecord>)>,
) -> CompilationDependencies {
    let root = tola_input::normalize_path(root);
    let reader_evidence = reader_evidence
        .into_iter()
        .map(|(reader, reads)| {
            (
                reader,
                DepReader {
                    reads: reads
                        .into_iter()
                        .map(|read| producer_read(&root, read))
                        .collect(),
                    package_checks: Vec::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    retained_deps(&root, srcs, [program.to_path_buf()], reader_evidence)
}

fn reader_path(path: &str) -> PathBuf {
    tola_input::normalize_path(Path::new(path))
}

fn captured_deps(
    root: &Path,
    srcs: impl IntoIterator<Item = PathBuf>,
    program: &Path,
    src_ev: Vec<ReadRecord>,
    site_ev: Vec<ReadRecord>,
) -> CompilationDependencies {
    let srcs: Vec<_> = srcs.into_iter().collect();
    let src = srcs
        .first()
        .cloned()
        .unwrap_or_else(|| root.join("content/source.typ"));
    deps_from(
        root,
        srcs,
        program,
        [
            (TypstDependencyReader::ContentSource(src), src_ev),
            (TypstDependencyReader::SiteProgram, site_ev),
        ],
    )
}

#[test]
fn paths_select_readers() {
    let root = Path::new("/site");
    let ev = captured_deps(
        root,
        [PathBuf::from("/site/content/post.typ")],
        Path::new("/site/site.typ"),
        vec![
            evidence("content/post.typ"),
            evidence("templates/document.typ"),
        ],
        vec![evidence("site.typ"), evidence("templates/document.typ")],
    );

    let shared = || {
        vec![
            TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
            TypstDependencyReader::SiteProgram,
        ]
    };
    for (changed, readers) in [
        ("/site/templates/document.typ", shared()),
        ("/site/site.typ", vec![TypstDependencyReader::SiteProgram]),
        ("/site/assets/unread.svg", Vec::new()),
        ("/site/templates", shared()),
        ("/site/templates-old", Vec::new()),
    ] {
        assert_eq!(
            ev.readers_of_paths(&[PathBuf::from(changed)]),
            readers,
            "{changed}"
        );
    }
}

#[test]
fn paths_rebuild_priority() {
    let src = reader_path("/site/content/post.typ");
    let ev = captured_deps(
        Path::new("/site"),
        [src.clone()],
        Path::new("/site/program.typ"),
        vec![evidence("templates/document.typ")],
        vec![evidence("site.typ")],
    );

    for (changed, src_prio, site_prio, affected) in [
        (
            "/site/content/post.typ",
            RebuildPriority::Direct,
            RebuildPriority::Affected,
            vec![TypstDependencyReader::SiteProgram],
        ),
        (
            "/site/templates/document.typ",
            RebuildPriority::Affected,
            RebuildPriority::Affected,
            vec![
                TypstDependencyReader::ContentSource(src.clone()),
                TypstDependencyReader::SiteProgram,
            ],
        ),
    ] {
        let decision = RebuildDecision::for_paths(Some(&ev), &[PathBuf::from(changed)], false);
        assert_eq!(decision.source_analysis, src_prio, "{changed}");
        assert_eq!(decision.site_program, site_prio, "{changed}");
        assert_eq!(decision.affected_readers, affected, "{changed}");
    }

    let parent = RebuildDecision::for_paths(Some(&ev), &[PathBuf::from("/site/content")], false);
    assert_eq!(parent.source_analysis, RebuildPriority::Direct);
    assert_eq!(
        parent.direct_readers,
        vec![TypstDependencyReader::ContentSource(src)]
    );

    let inventory =
        RebuildDecision::for_paths(Some(&ev), &[PathBuf::from("/site/content/added.typ")], true);
    assert_eq!(inventory.source_analysis, RebuildPriority::Direct);
    assert!(inventory.direct_readers.is_empty());
}

#[test]
fn physical_reads_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let tpl = root.join("template.typ");
    std::fs::write(&tpl, "#let helper = 1").unwrap();
    let ev = captured_deps(
        root,
        std::iter::empty(),
        &root.join("site.typ"),
        Vec::new(),
        vec![ReadRecord::new(
            ReadLocator::Root("template.typ".into()),
            b"#let helper = 1",
        )],
    );
    let boundary = tola_typst::SourceBoundary::default();
    let cancel = BuildCancellation::default();
    let fresh = || ev.physical_reads_are_fresh(&boundary, &cancel).unwrap();

    assert!(fresh());
    assert!(RebuildDecision::for_paths(Some(&ev), &[], false).reuses_site_program());

    std::fs::write(&tpl, "#let helper = 2").unwrap();
    assert!(!fresh());

    std::fs::remove_file(&tpl).unwrap();
    assert!(!fresh());
}

#[cfg(unix)]
#[test]
fn retargeted_read_stale() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let first = root.join("first.typ");
    let second = root.join("second.typ");
    let logical = root.join("template.typ");
    std::fs::write(&first, b"same bytes").unwrap();
    std::fs::write(&second, b"same bytes").unwrap();
    symlink(&first, &logical).unwrap();
    let ev = captured_deps(
        root,
        std::iter::empty(),
        &root.join("site.typ"),
        Vec::new(),
        vec![ReadRecord::new(
            ReadLocator::Root("template.typ".into()),
            b"same bytes",
        )],
    );
    let boundary = tola_typst::SourceBoundary::default();
    let cancel = BuildCancellation::default();

    assert!(ev.physical_reads_are_fresh(&boundary, &cancel).unwrap());
    let watched = ev.physical_read_paths();
    assert!(watched.contains(&tola_input::normalize_path(root).join("template.typ")));
    assert!(watched.contains(&std::fs::canonicalize(&first).unwrap()));

    std::fs::remove_file(&logical).unwrap();
    symlink(&second, &logical).unwrap();
    assert!(!ev.physical_reads_are_fresh(&boundary, &cancel).unwrap());
}

#[test]
fn conflicting_reads_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("template.typ"), "first").unwrap();
    let boundary = tola_typst::SourceBoundary::default();
    let cancel = BuildCancellation::default();

    let conflicting = captured_deps(
        root,
        [root.join("content/post.typ")],
        &root.join("site.typ"),
        vec![ReadRecord::new(
            ReadLocator::Root("template.typ".into()),
            b"first",
        )],
        vec![ReadRecord::new(
            ReadLocator::Root("template.typ".into()),
            b"second",
        )],
    );
    assert!(
        !conflicting
            .physical_reads_are_fresh(&boundary, &cancel)
            .unwrap()
    );
}

#[test]
fn shared_read_deduped() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let tpl = root.join("template.typ");
    std::fs::write(&tpl, "first").unwrap();
    let boundary = tola_typst::SourceBoundary::default();
    let cancel = BuildCancellation::default();

    let read = || ReadRecord::new(ReadLocator::Root("template.typ".into()), b"first");
    let shared = captured_deps(
        root,
        std::iter::empty(),
        &root.join("site.typ"),
        vec![read()],
        vec![read()],
    );
    assert_eq!(
        shared.physical_read_paths(),
        vec![tola_input::normalize_path(&tpl)]
    );
    assert!(shared.physical_reads_are_fresh(&boundary, &cancel).unwrap());
}

#[test]
fn package_read_watched() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let pkg_root = root.join("packages");
    let pkg_file = pkg_root.join("preview/demo/1.2.3/lib.typ");
    std::fs::create_dir_all(pkg_file.parent().unwrap()).unwrap();
    std::fs::write(&pkg_file, "#let helper = 1").unwrap();

    let pkg_read = ReadRecord::new(
        ReadLocator::Package {
            package: "@preview/demo:1.2.3".parse().unwrap(),
            path: PathBuf::from("lib.typ"),
        },
        b"#let helper = 1",
    );
    let reader = TypstDependencyReader::ContentSource(root.join("content/source.typ"));
    let ev = retained_deps(
        root,
        std::iter::empty::<PathBuf>(),
        [root.join("site.typ")],
        BTreeMap::from([(
            reader.clone(),
            DepReader {
                reads: vec![physical_read(&pkg_file, pkg_read)],
                package_checks: Vec::new(),
            },
        )]),
    );

    assert_eq!(
        ev.readers_of_paths(&[pkg_root.join("preview/demo")]),
        vec![reader]
    );
    assert_eq!(
        RebuildDecision::for_paths(Some(&ev), &[pkg_file], false).source_analysis,
        RebuildPriority::Affected
    );
}

#[test]
fn same_bytes_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let file = root.join("shared.typ");
    std::fs::write(&file, "Shared").unwrap();
    let boundary = tola_typst::SourceBoundary::default();
    let cancel = BuildCancellation::default();

    let reads = [
        ReadLocator::Root("shared.typ".into()),
        ReadLocator::ProvidedRoot("mapped.typ".into()),
    ]
    .into_iter()
    .map(|loc| physical_read(&file, ReadRecord::new(loc, b"Shared")))
    .collect();
    let shared = retained_deps(
        root,
        [],
        [],
        BTreeMap::from([(
            TypstDependencyReader::SiteProgram,
            DepReader {
                reads,
                package_checks: Vec::new(),
            },
        )]),
    );
    assert!(shared.physical_reads_are_fresh(&boundary, &cancel).unwrap());
}

#[test]
fn asset_read_skips_analysis() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    for path in [
        "templates/document.typ",
        "site.typ",
        "assets/a.svg",
        "content/a.typ",
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, path).unwrap();
    }
    let ev = deps_from(
        root,
        [root.join("content/a.typ")],
        &root.join("site.typ"),
        [
            (
                TypstDependencyReader::ContentSource(root.join("content/a.typ")),
                vec![evidence("templates/document.typ")],
            ),
            (
                TypstDependencyReader::SiteProgram,
                vec![evidence("site.typ"), evidence("assets/a.svg")],
            ),
        ],
    );
    let decision = RebuildDecision::for_paths(Some(&ev), &[root.join("assets/a.svg")], false);
    assert_eq!(decision.source_analysis, RebuildPriority::Unchanged);
    assert_eq!(decision.site_program, RebuildPriority::Affected);
    assert!(decision.direct_readers.is_empty());
    assert_eq!(
        decision.affected_readers,
        vec![TypstDependencyReader::SiteProgram]
    );
}

#[test]
fn package_checks_selection() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let data = root.join("package-data");
    let cache = root.join("package-cache");
    std::fs::create_dir_all(cache.join("preview/demo/1.0.0")).unwrap();
    let locs =
        tola_typst::PackageLocations::from_absolute_roots(Some(data.clone()), Some(cache)).unwrap();
    let store = tola_typst::PackageStore::new(locs, tola_typst::PackageFetchMode::LocalOnly);
    let pkg = "@preview/demo:1.0.0".parse().unwrap();
    let checks = store.prepare(&pkg).unwrap().checks().to_vec();

    assert!(checks.iter().all(package_check_is_fresh));
    std::fs::create_dir_all(data.join("preview/demo/1.0.0")).unwrap();
    assert!(!checks.iter().all(package_check_is_fresh));
}

#[test]
fn virtual_reads_ownership() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let host = compiler_host(&cfg).unwrap();
    let ev = |loc: ReadLocator| {
        let process_stable = host.read_is_process_stable(&loc);
        let reader_evidence = [(
            TypstDependencyReader::SiteProgram,
            DepReader {
                reads: vec![DepRead::Virtual {
                    evidence: ReadRecord::new(loc, b"virtual"),
                    process_stable,
                }],
                package_checks: Vec::new(),
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        retained_deps(
            cfg.root(),
            std::iter::empty::<PathBuf>(),
            [cfg.root().join("site.typ")],
            reader_evidence,
        )
    };
    let prio =
        |loc: ReadLocator| RebuildDecision::for_paths(Some(&ev(loc)), &[], false).site_program;

    for (loc, expected) in [
        (
            ReadLocator::ProvidedPackage {
                package: "@tola/document:0.0.0".parse().unwrap(),
                path: "lib.typ".into(),
            },
            RebuildPriority::Unchanged,
        ),
        (
            ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: ".tola-capability".into(),
            },
            RebuildPriority::Unchanged,
        ),
        (
            ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: ".tola-source/content/document.typ".into(),
            },
            RebuildPriority::Unchanged,
        ),
        (
            ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Document.spec(),
                path: ".tola-source/content/document.typ".into(),
            },
            RebuildPriority::Affected,
        ),
        (
            ReadLocator::ProvidedRoot("internal.typ".into()),
            RebuildPriority::Affected,
        ),
        (
            ReadLocator::NonPersistent("<generated>".into()),
            RebuildPriority::Affected,
        ),
    ] {
        assert_eq!(prio(loc.clone()), expected, "{loc:?}");
    }
}

#[test]
fn icon_reads_bytes() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let host = compiler_host(&cfg).unwrap();
    let collections = |mark: &str, unused: &str| {
        let wrap = |paint: &str| {
            format!("<svg viewBox='0 0 1 1'><path fill='{paint}' d='M0 0h1v1H0z'/></svg>")
        };
        crate::compiler::tests::brand_icons(&wrap(mark), &wrap(unused))
    };
    let original = collections("red", "blue");
    let orig_host = host.with_icons(Arc::clone(&original));
    let read = ReadRecord::new(
        ReadLocator::ProvidedPackage {
            package: tola_packages::TolaPackage::Icon.spec(),
            path: ".tola-icon/brand/mark.svg".into(),
        },
        original.get("brand", "mark").unwrap().svg().as_bytes(),
    );
    assert!(!orig_host.read_is_process_stable(read.locator()));
    let deps = captured_deps(
        cfg.root(),
        std::iter::empty(),
        &cfg.build.entry,
        Vec::new(),
        vec![read],
    );
    let cancel = BuildCancellation::default();
    let matches = |h: &crate::compiler::SiteHost| {
        deps.virtual_reads_match(|ev| h.virtual_read_matches(ev), &cancel)
            .unwrap()
    };
    assert!(matches(&orig_host));
    assert!(matches(&host.with_icons(collections("red", "green"))));
    assert!(!matches(&host.with_icons(collections("tan", "blue"))));
    assert!(!matches(&host.with_icons(Arc::default())));

    let named = |names: &[&str]| {
        let mut col = tola_icons::IconCollection::new();
        for name in names {
            col.insert_svg(name, "<svg viewBox='0 0 1 1'><path/></svg>")
                .unwrap();
        }
        let mut cols = tola_icons::IconCollections::new();
        cols.mount("brand", col).unwrap();
        Arc::new(cols)
    };
    let pkg = tola_packages::TolaPackage::Icon.spec();
    let path = Path::new(".tola-icon/brand/.names");
    let bytes = tola_packages::icon_collection_names_bytes(&original, &pkg, path).unwrap();
    let deps = captured_deps(
        cfg.root(),
        std::iter::empty(),
        &cfg.build.entry,
        Vec::new(),
        vec![ReadRecord::new(
            ReadLocator::ProvidedPackage {
                package: pkg,
                path: path.into(),
            },
            &bytes,
        )],
    );
    let matches = |cols| {
        let cur = host.with_icons(cols);
        deps.virtual_reads_match(|ev| cur.virtual_read_matches(ev), &cancel)
            .unwrap()
    };
    assert!(matches(Arc::clone(&original)));
    assert!(!matches(named(&["palette", "paints"])));
    assert!(!matches(named(&[])));
}
