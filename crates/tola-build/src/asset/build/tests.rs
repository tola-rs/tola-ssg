use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::asset::AssetSourceError;
use crate::asset::tests::{file_declaration, published_url};
use crate::config::section::assets::{AssetTreeDeclaration, AssetUrlPrefix};

fn check_urls(cfg: &SiteConfig) -> super::super::AssetUrls {
    super::super::AssetUrls::for_check_project(
        cfg,
        &crate::resources::source_boundary(cfg, tola_input::InputScope::Online),
        &crate::cancellation::BuildCancellation::default(),
    )
    .expect("configured declarations resolve before building")
}

fn tree_decl(src: impl Into<PathBuf>, prefix: impl AsRef<str>) -> AssetTreeDeclaration {
    AssetTreeDeclaration::new(src, AssetUrlPrefix::parse(prefix.as_ref()).unwrap())
}

fn build(
    cfg: &SiteConfig,
    prev: Option<&SiteAssets>,
    changed: &[PathBuf],
) -> anyhow::Result<SiteAssets> {
    build_configured_assets(
        cfg,
        crate::modules::site::AppliedAssets::project_only(cfg),
        &crate::resources::source_boundary(cfg, tola_input::InputScope::Online),
        &crate::cancellation::BuildCancellation::default(),
        prev,
        changed,
    )
}

#[cfg(unix)]
#[test]
fn pure_assets_reject_host_aliases() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("site");
    let mut cfg = crate::config::tests::load_test_config(&root, "");
    let outside = dir.path().join("host.css");
    fs::write(&outside, b"body { color: red; }").unwrap();
    let alias = root.join("theme.css");
    std::os::unix::fs::symlink(&outside, &alias).unwrap();
    cfg.assets.files = vec![file_declaration(&alias, "/theme.css")];
    cfg.build.minify.css = false;
    let cancel = crate::cancellation::BuildCancellation::new();
    let prev = built(&cfg);
    let mut entries = prev.entries();
    let (_, bytes) = entries.next().expect("one published asset");
    assert_eq!(entries.count(), 0, "one published asset");
    assert_eq!(bytes.as_ref(), b"body { color: red; }");
    let pure = crate::resources::source_boundary(&cfg, tola_input::InputScope::Pure);
    assert!(
        build_configured_assets(
            &cfg,
            crate::modules::site::AppliedAssets::project_only(&cfg),
            &pure,
            &cancel,
            Some(&prev),
            &[],
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn opted_in_tree_tracks_external_links() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("site");
    let cfg = crate::config::tests::load_test_config(
        &root,
        r#"
[[assets.trees]]
source = "assets"
url-prefix = "/papers"
follow-file-symlinks = true
"#,
    );
    let assets = root.join("assets");
    fs::create_dir(&assets).unwrap();
    let target = dir.path().join("store-paper.pdf");
    fs::write(&target, b"first").unwrap();
    std::os::unix::fs::symlink(&target, assets.join("paper.pdf")).unwrap();

    let prev = built(&cfg);
    let mut entries = prev.entries();
    let (out, bytes) = entries.next().expect("one published file");
    assert!(entries.next().is_none());
    assert_eq!(out.output.as_str(), "papers/paper.pdf");
    assert_eq!(bytes.as_ref(), b"first");

    fs::write(&target, b"second").unwrap();
    let updated = build(&cfg, Some(&prev), &[target.canonicalize().unwrap()]).unwrap();
    assert_eq!(updated.entries().next().unwrap().1.as_ref(), b"second");

    let pure = crate::resources::source_boundary(&cfg, tola_input::InputScope::Pure);
    let applied = crate::modules::site::AppliedAssets::project_only(&cfg);
    let cancel = crate::cancellation::BuildCancellation::default();
    assert!(build_configured_assets(&cfg, applied, &pure, &cancel, None, &[]).is_err());
    assert!(super::super::AssetUrls::for_check_with(&cfg, applied, &pure, &cancel).is_err());
}

#[test]
fn missing_source_fails_discovery() {
    let dir = TempDir::new().unwrap();

    let mut tree_cfg = crate::config::tests::load_test_config(dir.path(), "");
    tree_cfg.assets.trees = vec![tree_decl(dir.path().join("missing-tree"), "/assets")];
    let tree_err = build(&tree_cfg, None, &[]).unwrap_err();
    assert!(matches!(
        tree_err.downcast_ref::<AssetSourceError>(),
        Some(AssetSourceError::SourceMissing { path, key: "assets.trees" })
            if path == "missing-tree"
    ));

    let mut file_cfg = crate::config::tests::load_test_config(dir.path(), "");
    file_cfg.assets.files = vec![file_declaration(
        dir.path().join("missing-file"),
        "/favicon.ico",
    )];
    let file_err = build(&file_cfg, None, &[]).unwrap_err();
    assert!(matches!(
        file_err.downcast_ref::<AssetSourceError>(),
        Some(AssetSourceError::SourceMissing { path, key: "assets.files" })
            if path == "missing-file"
    ));
}

fn exclusive_site_roots(dir: &TempDir) -> [(PathBuf, &'static str); 3] {
    let content = dir.path().join("content");
    let out = dir.path().join("public");
    let internal = dir.path().join(tola_input::INTERNAL_DIR);
    fs::create_dir_all(&content).unwrap();
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&internal).unwrap();
    [
        (content, "content directory"),
        (out, "output directory"),
        (internal, "`.tola` directory"),
    ]
}

fn assert_tree_root_rejected(dir: &TempDir, src: &Path, expected: &str) {
    fs::create_dir_all(src).unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    cfg.assets.trees = vec![tree_decl(src, "/assets")];
    let err = build(&cfg, None, &[]).unwrap_err();
    assert!(err.to_string().contains(expected), "{err:#}");
}

#[test]
fn exclusive_site_roots_rejected() {
    let dir = TempDir::new().unwrap();
    for (root, expected) in exclusive_site_roots(&dir) {
        assert_tree_root_rejected(&dir, &root.join("assets"), expected);
    }
}

#[cfg(unix)]
#[test]
fn aliased_site_roots_rejected() {
    let dir = TempDir::new().unwrap();
    for (idx, (root, expected)) in exclusive_site_roots(&dir).iter().enumerate() {
        let alias = dir.path().join(format!("asset-alias-{idx}"));
        std::os::unix::fs::symlink(root, &alias).unwrap();
        assert_tree_root_rejected(&dir, &alias, expected);
    }
}

#[cfg(unix)]
#[test]
fn retargeted_protected_root_rechecked() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let assets = dir.path().join("assets");
    let initial_out = dir.path().join("initial-output");
    let out_link = dir.path().join("public-link");
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(&initial_out).unwrap();
    fs::write(assets.join("asset.bin"), b"asset").unwrap();
    symlink(&initial_out, &out_link).unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    cfg.build.publish_dir = out_link.clone();
    cfg.assets.trees = vec![tree_decl(&assets, "/assets")];
    let published = built(&cfg);

    fs::remove_file(&out_link).unwrap();
    symlink(&assets, &out_link).unwrap();
    let err = build(&cfg, Some(&published), std::slice::from_ref(&out_link)).unwrap_err();

    assert!(matches!(
        err.downcast_ref::<AssetSourceError>(),
        Some(AssetSourceError::OverlapsProtected { path, .. }) if path == "assets"
    ));
}

#[cfg(unix)]
#[test]
fn unpublishable_tree_members_rejected() {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;

    for (label, reason) in [
        ("live.sock", "not a regular file"),
        ("file-link", "symbolic link"),
        ("directory-link", "points to a directory"),
        ("missing-link", "symbolic link"),
        ("query?name.txt", "cannot be published"),
        ("fragment#name.txt", "cannot be published"),
        ("percent%name.txt", "cannot be published"),
    ] {
        let dir = TempDir::new().unwrap();
        let assets = dir.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        let _listener =
            (label == "live.sock").then(|| UnixListener::bind(assets.join(label)).unwrap());
        match label.strip_suffix("-link") {
            Some(kind) => {
                let target = dir.path().join(format!("{kind}-target"));
                match kind {
                    "file" => fs::write(&target, b"target").unwrap(),
                    "directory" => fs::create_dir(&target).unwrap(),
                    "missing" => {}
                    _ => unreachable!(),
                }
                symlink(&target, assets.join(label)).unwrap();
            }
            None if label.ends_with(".txt") => {
                fs::write(assets.join(label), b"asset").unwrap();
            }
            None => {}
        }
        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        cfg.assets.trees = vec![tree_decl(&assets, "/assets")];

        let msg = build(&cfg, None, &[]).unwrap_err().to_string();

        assert!(msg.contains(label), "{label}: {msg}");
        assert!(msg.contains(reason), "{label}: {msg}");
        if label.ends_with(".txt") {
            let checked_err = super::super::AssetUrls::for_check_project(
                &cfg,
                &crate::resources::source_boundary(&cfg, tola_input::InputScope::Online),
                &crate::cancellation::BuildCancellation::default(),
            )
            .unwrap_err()
            .to_string();
            assert!(checked_err.contains(label), "{checked_err}");
            assert!(checked_err.contains(reason), "{checked_err}");
        }
    }

    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("live.sock");
    let _listener = UnixListener::bind(&sock).unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    cfg.assets.files = vec![file_declaration(&sock, "/live.sock")];
    let msg = build(&cfg, None, &[]).unwrap_err().to_string();
    assert!(msg.contains("not a regular file"), "{msg}");
    assert!(msg.contains("live.sock"), "{msg}");
}

#[test]
fn asset_build_cancellation() {
    let dir = TempDir::new().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let canceller = crate::cancellation::BuildCanceller::new();
    let cancel = canceller.token();
    canceller.cancel();

    let err = build_configured_assets(
        &cfg,
        crate::modules::site::AppliedAssets::project_only(&cfg),
        &crate::resources::source_boundary(&cfg, tola_input::InputScope::Online),
        &cancel,
        None,
        &[],
    )
    .unwrap_err();

    assert!(err.chain().any(|cause| matches!(
        cause.downcast_ref::<crate::cancellation::BuildCancelled>(),
        Some(crate::cancellation::BuildCancelled)
    )));
}

#[test]
fn unminifiable_source_stays_raw() {
    let dir = TempDir::new().unwrap();
    for (name, bytes) in [
        ("invalid.css", b"@media (".as_slice()),
        ("invalid.js", b"function =".as_slice()),
    ] {
        let src = dir.path().join(name);
        fs::write(&src, bytes).unwrap();
        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        cfg.assets.files = vec![file_declaration(&src, format!("/{name}"))];
        cfg.build.minify.css = true;
        cfg.build.minify.javascript = true;
        let assets = built(&cfg);

        let warnings = assets.minify_warnings();
        let [warning] = &warnings[..] else {
            panic!("expected one warning for {name}, got {warnings:?}");
        };
        assert!(
            matches!(warning, AssetMinifyWarning::Syntax { path, .. } if path == name),
            "{warning:?}"
        );
        let published = assets
            .entries()
            .find(|(out, _)| {
                out.logical_source
                    .file_name()
                    .is_some_and(|file| file == std::ffi::OsStr::new(name))
            })
            .map(|(_, bytes)| bytes)
            .expect("the source is published");
        assert_eq!(&**published, bytes, "{name} keeps its own bytes");
    }
}

#[test]
fn one_tree_two_prefixes() {
    let dir = TempDir::new().unwrap();
    let assets = dir.path().join("assets");
    fs::create_dir_all(&assets).unwrap();
    fs::write(assets.join("app.css"), b".a { color: red; }").unwrap();

    let mut cfg = crate::config::tests::load_test_config(
        dir.path(),
        r#"
[assets]
trees = [
  { source = "assets", url-prefix = "/first" },
  { source = "assets", url-prefix = "/second", include = ["asset?"] },
  { source = "assets", url-prefix = "/excluded", exclude = ["assets/*.css"] },
  { source = "assets", url-prefix = "/empty", include = [] },
]
"#,
    );
    cfg.build.minify.css = true;

    let assets = built(&cfg);

    let entries = assets.entries().collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0.output.as_str(), "first/app.css");
    assert_eq!(entries[1].0.output.as_str(), "second/app.css");
    assert!(entries[0].1.len() < b".a { color: red; }".len());
    assert_eq!(
        entries[0].1, entries[1].1,
        "one source publishes one result"
    );
    let urls = assets.asset_urls(&cfg).unwrap();
    let origin = crate::asset::AssetOrigin::TreeMember {
        source: cfg.root().join("assets"),
        member: PathBuf::from("app.css"),
    };
    for declared in ["/first/app.css", "/second/app.css"] {
        assert_eq!(urls.origin(declared), Some(&origin));
    }
}

#[test]
fn declaration_owns_member_url() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("member.bin"), b"tree").unwrap();
    let logo = dir.path().join("logo.svg");
    fs::write(&logo, b"<svg></svg>").unwrap();

    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    cfg.build.publish_dir = dir.path().join("public");
    cfg.assets.trees = vec![tree_decl(&tree, "/assets")];
    cfg.assets.files = vec![file_declaration(&logo, "/assets/logo.svg")];
    let assets = built(&cfg);
    assert_eq!(
        published_outputs(&assets),
        ["assets/logo.svg", "assets/member.bin"]
    );
    let urls = assets.asset_urls(&cfg).unwrap();
    for declared in ["/assets/logo.svg", "/assets/member.bin"] {
        assert_eq!(published_url(&urls, declared).as_deref(), Some(declared));
    }

    let (_dir, mut cfg) = tree_site("[site]\nbase-path = \"/blog/\"");
    cfg.assets.files = vec![
        file_declaration(cfg.root().join("assets/app.js"), "/assets/app.js"),
        file_declaration(cfg.root().join("assets/theme.css"), "/js/theme.css"),
    ];
    let assets = built(&cfg);
    assert_eq!(
        published_outputs(&assets),
        ["assets/app.js", "assets/images/logo.svg", "js/theme.css"]
    );
    let urls = assets.asset_urls(&cfg).unwrap();
    assert_eq!(
        published_url(&urls, "/assets/theme.css"),
        None,
        "the tree no longer publishes the member a declaration took"
    );

    for (tree_prefix, file_url) in [("/downloads/icons", "/downloads"), ("/assets", "/assets")] {
        let mut files = crate::config::tests::load_test_config(dir.path(), "");
        files.assets.trees = vec![tree_decl(&tree, tree_prefix)];
        files.assets.files = vec![file_declaration(&logo, file_url)];
        let err = build(&files, None, &[]).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<AssetSourceError>(),
                Some(AssetSourceError::OutputsOverlap { .. })
            ),
            "{tree_prefix} / {file_url}: {err:#}"
        );
    }
}

fn tree_site(settings: &str) -> (TempDir, SiteConfig) {
    let dir = TempDir::new().unwrap();
    let assets = dir.path().join("assets");
    fs::create_dir_all(assets.join("images")).unwrap();
    fs::write(assets.join("images/logo.svg"), b"<svg></svg>").unwrap();
    fs::write(assets.join("app.js"), "const answer = 40 + 2;").unwrap();
    fs::write(assets.join("theme.css"), "body { color: red; }").unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), settings);
    cfg.build.publish_dir = dir.path().join("public");
    cfg.assets.trees = vec![tree_decl(&assets, "/assets")];
    (dir, cfg)
}

fn built(cfg: &SiteConfig) -> SiteAssets {
    build(cfg, None, &[]).unwrap()
}

fn published_outputs(assets: &SiteAssets) -> Vec<String> {
    let mut outs = assets
        .entries()
        .map(|(out, _)| out.output.as_str().to_owned())
        .collect::<Vec<_>>();
    outs.sort();
    outs
}

#[test]
fn tree_members_use_prefix() {
    let (_dir, cfg) = tree_site("[site]\nbase-path = \"/blog/\"");
    let assets = built(&cfg);

    assert_eq!(
        published_outputs(&assets),
        [
            "assets/app.js",
            "assets/images/logo.svg",
            "assets/theme.css"
        ]
    );
    let urls = assets.asset_urls(&cfg).unwrap();
    assert_eq!(
        published_url(&urls, "/assets/images/logo.svg").as_deref(),
        Some("/blog/assets/images/logo.svg")
    );
    assert_eq!(
        published_url(&urls, "/assets/theme.css").as_deref(),
        Some("/blog/assets/theme.css")
    );
    assert_eq!(published_url(&urls, "/images/logo.svg").as_deref(), None);

    let checked = check_urls(&cfg);
    assert_eq!(
        published_url(&checked, "/assets/images/logo.svg").as_deref(),
        Some("/blog/assets/images/logo.svg")
    );
    assert_eq!(
        published_url(&checked, "/assets/theme.css").as_deref(),
        Some("/blog/assets/theme.css")
    );
    assert!(urls.published().eq(checked.published()));
    assert!(urls.is_prepared());
    assert!(!checked.is_prepared());
}

#[test]
fn declaration_edits_keep_addresses() {
    let (dir, mut cfg) = tree_site("[assets]\ncache-busting = true");
    cfg.assets.files = vec![file_declaration(
        dir.path().join("assets/theme.css"),
        "/site.css",
    )];
    let first = built(&cfg);
    let before = first.asset_urls(&cfg).unwrap();
    let app = published_url(&before, "/assets/app.js")
        .expect("the tree publishes app.js")
        .to_owned();
    assert!(app.contains("?h="), "{app}");

    let added = cfg.root().join("assets/added.svg");
    fs::write(&added, b"<svg></svg>").unwrap();
    let grown = build(&cfg, Some(&first), std::slice::from_ref(&added)).unwrap();
    let grown_urls = grown.asset_urls(&cfg).unwrap();
    assert_eq!(
        published_url(&grown_urls, "/assets/added.svg")
            .as_deref()
            .map(|url| url.split('?').next().unwrap()),
        Some("/assets/added.svg")
    );
    assert_eq!(
        published_url(&grown_urls, "/assets/app.js").as_deref(),
        Some(app.as_str())
    );
    assert!(before.values_match(&grown_urls));

    cfg.assets.files.clear();
    let narrowed = built(&cfg);
    let narrowed_urls = narrowed.asset_urls(&cfg).unwrap();
    assert_eq!(published_url(&narrowed_urls, "/site.css").as_deref(), None);
    assert_eq!(
        published_url(&narrowed_urls, "/assets/app.js").as_deref(),
        Some(app.as_str())
    );
    assert!(before.values_match(&narrowed_urls));
}
