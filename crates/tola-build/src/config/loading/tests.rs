use super::*;
use crate::config::tests::{VENDORED_SITE_SOURCE, attached_diagnostics, diagnostic_mentioning};

fn site_source(title: &str) -> String {
    format!(
        "[site]\ntitle = \"{title}\"\n\n[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n"
    )
}

fn write_config(dir: &Path, src: &str) -> PathBuf {
    let path = dir.join("tola.toml");
    std::fs::write(&path, src).unwrap();
    path
}

fn load_error(path: &Path) -> anyhow::Error {
    load_site_config(
        Some(path),
        tola_typst::PackageLocations::default(),
        &BuildOverrides::default(),
    )
    .unwrap_err()
}

#[test]
fn diagnostics_cite_written_line() {
    for (src, field) in [
        (
            "[site]\ntitle = \"Site\"\nlanguage = \"nope\"\n",
            "site.language",
        ),
        (
            "[build]\nentry = \"site.typ\"\nminfy = true\n",
            "build.minfy",
        ),
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_config(dir.path(), src);
        let err = load_error(&path);
        let diag = diagnostic_mentioning(&err, field);
        assert_eq!(diag.code, tola_diagnostic::codes::config::INVALID);
        let loc = diag.location.as_ref().unwrap();
        assert_eq!(loc.path, "tola.toml");
        assert_eq!((loc.line, loc.column), (Some(3), Some(1)));
    }
}

#[test]
fn supplied_text_overrides_disk() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = write_config(dir.path(), &site_source("Disk"));
    let src = site_source("Supplied");
    let parsed = resolve_site_config(
        &path,
        &src,
        tola_typst::PackageLocations::default(),
        &BuildOverrides::default(),
    )
    .unwrap();

    assert_eq!(parsed.config().site().title, "Supplied");
    assert_eq!(
        parsed.config().source().unwrap().sources()[0].hash(),
        blake3::hash(src.as_bytes())
    );
    assert!(parsed.config().warnings().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), site_source("Disk"));
}

#[test]
fn directory_config_path_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let err = resolve_site_config(
        dir.path(),
        &site_source("Site"),
        tola_typst::PackageLocations::default(),
        &BuildOverrides::default(),
    )
    .unwrap_err();

    let diag = attached_diagnostics(&err);
    assert!(diag[0].message.contains("is not a file"), "{diag:?}");
    assert!(
        diag[0].message.contains(&written_path(dir.path())),
        "{diag:?}"
    );
    assert!(
        diag[0].help.iter().any(|h| h.message.contains("--config")),
        "{diag:?}"
    );
    assert!(diag.iter().all(|d| d.location.is_none()));
}

#[test]
fn output_error_cites_config_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = write_config(
        dir.path(),
        &format!("{}publish-dir = \".\"\n", site_source("Site")),
    );

    let err = load_error(&path);
    let diag = attached_diagnostics(&err);

    assert!(!diag.is_empty());
    assert!(diag.iter().all(|d| {
        d.location
            .as_ref()
            .is_some_and(|loc| loc.path == "tola.toml")
    }));
    assert!(err.chain().any(|cause| matches!(
        cause.downcast_ref::<crate::filesystem::OutputBoundaryError>(),
        Some(crate::filesystem::OutputBoundaryError::OutsideSite)
    )));
}

#[cfg(unix)]
#[test]
fn broken_symlink_stops_discovery() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("tola.toml"), "").unwrap();
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let path = nested.join("tola.toml");
    std::os::unix::fs::symlink("missing.toml", &path).unwrap();

    assert_eq!(find_default_config(&nested), Some(path.clone()));
    let err = ConfigSource::load(Some(&path), tola_input::InputScope::Online).unwrap_err();
    assert!(err.paths().contains(&path));
    assert!(
        err.diagnostics()
            .iter()
            .any(|d| d.code == tola_diagnostic::codes::config::IO)
    );
    let err = anyhow::Error::new(err);
    assert!(err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    }));
}

#[test]
fn vendor_packages_precede_host_roots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tola.toml");
    let locs = || {
        tola_typst::PackageLocations::from_absolute_roots(
            Some(dir.path().join("host-packages")),
            Some(dir.path().join("host-cache")),
        )
        .unwrap()
    };
    let mut declared_schema = SiteConfigSchema::default();
    declared_schema.vendor.path = Some("vendor".into());

    let declared = declared_schema
        .resolve(&path, locs(), &BuildOverrides::default())
        .unwrap();

    assert_eq!(
        declared
            .package_locations()
            .declared()
            .first()
            .unwrap()
            .root(),
        declared.root().join("vendor/typst-packages")
    );

    let undeclared = SiteConfigSchema::default()
        .resolve(&path, locs(), &BuildOverrides::default())
        .unwrap();

    assert!(undeclared.package_locations().declared().is_empty());
    assert!(undeclared.package_locations().data().is_some());
}

#[test]
fn schema_enforces_path_boundaries() {
    type SchemaCase<'a> = (fn(&mut SiteConfigSchema), &'a str);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tola.toml");
    let cases: [SchemaCase<'_>; 7] = [
        (|s| s.build.entry = "../site.typ".into(), "build.entry"),
        (|s| s.build.entry = "C:site.typ".into(), "build.entry"),
        (
            |s| s.build.entry = r"\\server\share\site.typ".into(),
            "build.entry",
        ),
        (|s| s.build.publish_dir = ".".into(), "build.publish-dir"),
        (|s| s.build.entry = ".tola/site.typ".into(), "build.entry"),
        (
            |s| s.typst.fonts.paths = vec![".tola/fonts".into()],
            "typst.fonts.paths",
        ),
        (
            |s| {
                s.vendor.path = Some("vendor".into());
                s.build.publish_dir = ".vendor-swap/site".into();
            },
            "build.publish-dir",
        ),
    ];

    for (edit, expected) in cases {
        let mut schema = SiteConfigSchema::default();
        edit(&mut schema);

        let err = schema
            .resolve(
                &path,
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap_err();

        assert!(
            attached_diagnostics(&err)
                .iter()
                .any(|d| { d.notes.iter().any(|n| n == expected) || d.message.contains(expected) })
        );
    }
}

#[cfg(unix)]
#[test]
fn hook_output_cannot_alias_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = write_config(root, "");
    std::os::unix::fs::symlink("tola.toml", root.join("alias.toml")).unwrap();
    std::os::unix::fs::symlink(".", root.join("linked")).unwrap();
    let mut schema = SiteConfigSchema::default();
    schema.hooks.before_build = vec![crate::config::section::build::BeforeBuildHookConfig {
        name: "generator".into(),
        argv: Some(vec!["generator".into()]),
        generates: vec!["tola.toml".into(), "alias.toml".into(), "linked".into()],
        ..crate::config::section::build::BeforeBuildHookConfig::default()
    }];

    let err = schema
        .resolve(
            &path,
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap_err();

    let diag = attached_diagnostics(&err);
    assert_eq!(diag.len(), 3, "{diag:?}");
    for d in diag {
        assert!(d.message.contains("site configuration"), "{d:?}");
    }
}

#[test]
fn hook_output_boundary_cites_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        r#"[[hooks.before-build]]
name = "first"
argv = ["first.sh"]
generates = ["generated/first"]

[[hooks.before-build]]
name = "second"
argv = ["second.sh"]
generates = [".tola/cache"]
"#,
    );

    let err = load_error(&path);
    let diag = diagnostic_mentioning(&err, ".tola/cache");
    assert_eq!(diag.code, tola_diagnostic::codes::config::INVALID);
    let loc = diag.location.as_ref().expect("the refusal is located");
    assert_eq!(loc.line, Some(9));
}

#[test]
fn vendor_candidate_isolates_old_copy() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), VENDORED_SITE_SOURCE);
    let old = cfg
        .vendor
        .typst_packages()
        .unwrap()
        .join("local/demo/1.0.0");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("value.typ"), "old vendor").unwrap();
    let candidate = cfg.root().join(".vendor-swap/candidate");
    std::fs::create_dir_all(&candidate).unwrap();
    let cfg = cfg.with_vendor_root(Some(candidate.clone())).unwrap();
    let pure = crate::BuildResources::new().with_input_scope(tola_input::InputScope::Pure);
    let files = tola_typst::FileStore::from_package_locations(
        pure.package_locations(&cfg),
        tola_typst::PackageFetchMode::LocalOnly,
    )
    .with_source_boundary(pure.source_boundary(&cfg));
    let id = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
        typst::syntax::VirtualRoot::Package("@local/demo:1.0.0".parse().unwrap()),
        typst::syntax::VirtualPath::new("value.typ").unwrap(),
    ));
    assert!(files.read(id, cfg.root()).is_err());
    let pkg = candidate.join("typst-packages/local/demo/1.0.0");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("value.typ"), "candidate vendor").unwrap();
    assert_eq!(files.read(id, cfg.root()).unwrap(), b"candidate vendor");
}

#[test]
fn vendor_refresh_keeps_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), VENDORED_SITE_SOURCE);
    let workspace = cfg
        .vendor
        .path
        .as_deref()
        .and_then(crate::filesystem::swap_workspace)
        .unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("old.typ"), "generated previous source").unwrap();
    let refreshed = cfg.with_vendor_root(None).unwrap();
    let files = tola_typst::FileStore::new()
        .with_source_boundary(crate::BuildResources::new().source_boundary(&refreshed));
    assert!(
        files
            .read(
                tola_typst::file_id(".vendor-swap/old.typ"),
                refreshed.root()
            )
            .is_err()
    );
}
