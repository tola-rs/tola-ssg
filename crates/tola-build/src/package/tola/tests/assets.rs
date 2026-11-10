use std::sync::Arc;

use super::IconFiles;
use tempfile::TempDir;

use crate::package::SiteBindings;

struct Site {
    directory: TempDir,
    config: crate::config::ResolvedSiteConfig,
    library: tola_packages::library::SiteLibrary,
}

/// One site with the given `assets` declarations on disk.
///
/// `files` are `(source, url)` declarations; `tree` is an optional
/// `(source directory, url prefix)` declaration. `settings` is the whole
/// site configuration, so a caller asks for cache busting with
/// `[assets] cache-busting = true` there.
fn site(files: &[(&str, &str)], tree: Option<(&str, &str)>, settings: &str) -> Site {
    let directory = TempDir::new().unwrap();
    let mut config = crate::config::tests::load_test_config(directory.path(), settings);
    let mut declarations = Vec::new();
    for (name, url) in files {
        let source = directory.path().join(name);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, format!("/* {name} */")).unwrap();
        declarations.push(crate::config::section::AssetFileDeclaration::new(
            &source,
            crate::config::section::AssetUrl::parse(url).unwrap(),
        ));
    }
    if let Some((source, prefix)) = tree {
        let source = directory.path().join(source);
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("member.js"), "/* member */").unwrap();
        config.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            &source,
            crate::config::section::AssetUrlPrefix::parse(prefix).unwrap(),
        )];
    }
    config.build.publish_dir = directory.path().join("public");
    config.assets.files = declarations;
    let library = library_for(&config);
    Site {
        directory,
        config,
        library,
    }
}

/// The inventory of one site's configured assets, rendered as a build renders it.
fn configured_inventory(
    config: &crate::config::ResolvedSiteConfig,
) -> crate::asset::ConfiguredAssets {
    crate::asset::build_configured_assets(
        config,
        crate::modules::prepared::AppliedAssets::project_only(config),
        &crate::resources::source_boundary(config, tola_input::InputScope::Online),
        &crate::cancellation::BuildCancellation::default(),
        None,
        &[],
    )
    .unwrap()
}

/// Bind one site's configured asset URLs the way a build does: from the
/// rendered inventory, so the addresses are the ones this build publishes.
fn library_for(config: &crate::config::ResolvedSiteConfig) -> tola_packages::library::SiteLibrary {
    let urls = configured_inventory(config).asset_urls(config).unwrap();
    let bindings = SiteBindings::from_config(config, urls);
    bindings.library(Default::default())
}

/// The identity of the published bytes of the one file at `name`.
fn published_identity(config: &crate::config::ResolvedSiteConfig, name: &str) -> String {
    let inventory = configured_inventory(config);
    let (_, bytes) = inventory
        .entries()
        .find(|(output, _)| output.output.as_str() == name)
        .unwrap_or_else(|| panic!("{name} is published"));
    tola_typst::ContentDigest::of(bytes).to_hex()
}

/// Every output path this site's declarations published.
fn published_names(config: &crate::config::ResolvedSiteConfig) -> Vec<String> {
    let mut names = configured_inventory(config)
        .entries()
        .map(|(output, _)| output.output.as_str().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

impl Site {
    fn compile(&self, body: &str) -> Result<tola_typst::BundleCompilation, String> {
        let main = self.directory.path().join("site.typ");
        std::fs::write(
            &main,
            format!("#import \"@tola/address:0.0.0\": asset-url\n{body}"),
        )
        .unwrap();
        let files = tola_typst::FileResolver::new().with_provider(IconFiles::default());
        let world = tola_typst::TypstWorld::builder(&main, self.directory.path())
            .with_files(Arc::new(files))
            .with_shared_library(self.library.shared())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid test world");
        tola_typst::compile_bundle_world(&world, &Default::default()).map_err(|error| {
            error
                .diagnostics()
                .expect("Typst failure has diagnostics")
                .errors()
                .map(|diagnostic| {
                    // A diagnostic tells the author what failed and what to do about it,
                    // so the rendered failure has its hints too.
                    let hints = diagnostic
                        .hints
                        .iter()
                        .map(|hint| format!("hint: {}", hint.message))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("{}\n{hints}", diagnostic.message)
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// The value `asset-url(declared)` produced in a real Bundle compilation.
    fn resolve(&self, declared: &str) -> Result<String, String> {
        let compilation = self.compile(&format!(
            "#document(\"index.html\")[#metadata(asset-url(\"{declared}\")) <resolved>]"
        ))?;
        let path = typst::syntax::VirtualPath::new("index.html").unwrap();
        let value = compilation
            .document(&path)
            .expect("the test document was compiled")
            .metadata_unique("resolved")
            .expect("one resolved URL")
            .expect("the document declares the resolved URL");
        Ok(value
            .cast::<typst::foundations::Str>()
            .expect("asset-url returns a string")
            .to_string())
    }
}

#[test]
fn cache_busting_names_bytes() {
    let mut site = site(
        &[("assets/app.js", "/app.js")],
        Some(("assets", "/assets")),
        "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
    );
    std::fs::write(
        site.directory.path().join("assets/app.js"),
        "window.answer = 42;",
    )
    .unwrap();
    site.library = library_for(&site.config);
    let first = site.resolve("/app.js").unwrap();
    let first_identity = published_identity(&site.config, "app.js");
    assert_eq!(first, format!("/blog/app.js?h={first_identity}"));
    let member = published_identity(&site.config, "assets/member.js");
    assert_eq!(
        site.resolve("/assets/member.js").unwrap(),
        format!("/blog/assets/member.js?h={member}")
    );

    std::fs::write(
        site.directory.path().join("assets/app.js"),
        "window.answer = 43;",
    )
    .unwrap();
    site.library = library_for(&site.config);
    let changed_identity = published_identity(&site.config, "app.js");
    assert_ne!(changed_identity, first_identity);
    assert_eq!(
        site.resolve("/app.js").unwrap(),
        format!("/blog/app.js?h={changed_identity}")
    );
    assert_eq!(
        site.resolve("/assets/member.js").unwrap(),
        format!("/blog/assets/member.js?h={member}")
    );
    assert_eq!(
        published_names(&site.config),
        ["app.js", "assets/member.js"]
    );
}

#[test]
fn undeclared_urls_are_errors() {
    let site = site(
        &[("assets/app.js", "/app.js")],
        None,
        "[assets]\ncache-busting = true",
    );

    let error = site.resolve("/missing.js").unwrap_err();

    assert!(error.contains("/missing.js"), "{error}");
    assert!(error.contains("assets.files"), "{error}");
    assert!(error.contains("site configuration"), "{error}");
    assert!(!error.contains("tola.toml"), "{error}");
    assert!(
        !error.contains("/app.js"),
        "an undefined URL must not resolve to another declaration: {error}"
    );
}
