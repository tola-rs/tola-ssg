use std::sync::Arc;

use tempfile::TempDir;

use crate::package::SiteBindings;

use super::IconFiles;

struct Site {
    dir: TempDir,
    cfg: crate::config::SiteConfig,
    lib: tola_packages::library::SiteLibrary,
}

fn site(files: &[(&str, &str)], tree: Option<(&str, &str)>, settings: &str) -> Site {
    let dir = TempDir::new().unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), settings);
    let mut decls = Vec::new();
    for (name, url) in files {
        let src = dir.path().join(name);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, format!("/* {name} */")).unwrap();
        decls.push(crate::config::section::AssetFileDeclaration::new(
            &src,
            crate::config::section::AssetUrl::parse(url).unwrap(),
        ));
    }
    if let Some((src, prefix)) = tree {
        let src = dir.path().join(src);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("member.js"), "/* member */").unwrap();
        cfg.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            &src,
            crate::config::section::AssetUrlPrefix::parse(prefix).unwrap(),
        )];
    }
    cfg.build.publish_dir = dir.path().join("public");
    cfg.assets.files = decls;
    let lib = library_for(&cfg);
    Site { dir, cfg, lib }
}

fn configured_inventory(cfg: &crate::config::SiteConfig) -> crate::asset::SiteAssets {
    crate::asset::build_configured_assets(
        cfg,
        crate::modules::site::AppliedAssets::project_only(cfg),
        &crate::resources::source_boundary(cfg, tola_input::InputScope::Online),
        &crate::cancellation::BuildCancellation::default(),
        None,
        &[],
    )
    .unwrap()
}

fn library_for(cfg: &crate::config::SiteConfig) -> tola_packages::library::SiteLibrary {
    let urls = configured_inventory(cfg).asset_urls(cfg).unwrap();
    SiteBindings::from_config(cfg, urls, None).library(Default::default())
}

fn published_identity(cfg: &crate::config::SiteConfig, name: &str) -> String {
    let inv = configured_inventory(cfg);
    let (_, bytes) = inv
        .entries()
        .find(|(out, _)| out.output.as_str() == name)
        .unwrap_or_else(|| panic!("{name} is published"));
    tola_typst::ContentDigest::of(bytes).to_hex()
}

fn published_names(cfg: &crate::config::SiteConfig) -> Vec<String> {
    let mut names: Vec<_> = configured_inventory(cfg)
        .entries()
        .map(|(out, _)| out.output.as_str().to_owned())
        .collect();
    names.sort();
    names
}

impl Site {
    fn compile(&self, body: &str) -> Result<tola_typst::BundleCompilation, String> {
        let main = self.dir.path().join("site.typ");
        std::fs::write(
            &main,
            format!("#import \"@tola/address:0.0.0\": asset-url\n{body}"),
        )
        .unwrap();
        let files = tola_typst::FileStore::new().with_provider(IconFiles::default());
        let world = tola_typst::TypstWorld::builder(&main, self.dir.path())
            .with_files(Arc::new(files))
            .with_shared_library(self.lib.shared())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid test world");
        tola_typst::compile_bundle_world(&world, &Default::default()).map_err(|err| {
            err.diagnostics()
                .expect("Typst failure has diagnostics")
                .errors()
                .map(|diag| {
                    let hints: Vec<_> = diag
                        .hints
                        .iter()
                        .map(|h| format!("hint: {}", h.message))
                        .collect();
                    format!("{}\n{}", diag.message, hints.join("\n"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn resolve(&self, declared: &str) -> Result<String, String> {
        let compilation = self.compile(&format!(
            "#document(\"index.html\")[#metadata(asset-url(\"{declared}\")) <resolved>]"
        ))?;
        let path = typst::syntax::VirtualPath::new("index.html").unwrap();
        let val = compilation
            .document(&path)
            .expect("test document compiled")
            .metadata_unique("resolved")
            .expect("one resolved URL")
            .expect("document declares resolved URL");
        Ok(val.cast::<typst::foundations::Str>().unwrap().to_string())
    }
}

#[test]
fn cache_busting_names_bytes() {
    let mut s = site(
        &[("assets/app.js", "/app.js")],
        Some(("assets", "/assets")),
        "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
    );
    std::fs::write(s.dir.path().join("assets/app.js"), "window.answer = 42;").unwrap();
    s.lib = library_for(&s.cfg);
    let first = s.resolve("/app.js").unwrap();
    let first_id = published_identity(&s.cfg, "app.js");
    assert_eq!(first, format!("/blog/app.js?h={first_id}"));
    let member = published_identity(&s.cfg, "assets/member.js");
    assert_eq!(
        s.resolve("/assets/member.js").unwrap(),
        format!("/blog/assets/member.js?h={member}")
    );

    std::fs::write(s.dir.path().join("assets/app.js"), "window.answer = 43;").unwrap();
    s.lib = library_for(&s.cfg);
    let changed_id = published_identity(&s.cfg, "app.js");
    assert_ne!(changed_id, first_id);
    assert_eq!(
        s.resolve("/app.js").unwrap(),
        format!("/blog/app.js?h={changed_id}")
    );
    assert_eq!(
        s.resolve("/assets/member.js").unwrap(),
        format!("/blog/assets/member.js?h={member}")
    );
    assert_eq!(published_names(&s.cfg), ["app.js", "assets/member.js"]);
}

#[test]
fn undeclared_urls_rejected() {
    let s = site(
        &[("assets/app.js", "/app.js")],
        None,
        "[assets]\ncache-busting = true",
    );
    let err = s.resolve("/missing.js").unwrap_err();
    assert!(err.contains("/missing.js"), "{err}");
    assert!(err.contains("assets.files"), "{err}");
    assert!(err.contains("site configuration"), "{err}");
    assert!(!err.contains("tola.toml"), "{err}");
    assert!(!err.contains("/app.js"), "{err}");
}
