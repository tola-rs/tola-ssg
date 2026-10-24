//! The resolved configuration and the paths it selects, as one JSON document for
//! `tola inspect config`.
//!
//! Preparing modules resolves their declared paths without executing a driver, so the document
//! states what a build would use while running none of the site's code.
use anyhow::Result;

use crate::cli::BuildOverrideArgs;

pub(super) fn value(
    request: &super::SiteRequest<'_>,
    build: &BuildOverrideArgs,
) -> Result<serde_json::Value> {
    let overrides = crate::cli::config::config_overrides(build);
    let loaded = request.load_config(&overrides)?;
    let prepared = tola_build::modules::prepare(loaded.config(), request.scope)?;
    let config = &prepared;
    let modules = config
        .loaded_modules()
        .map(|(alias, module)| {
            (
                alias.to_owned(),
                serde_json::json!({
                    "root": module.root,
                    "manifest": module.manifest,
                    "package": module.package_name,
                    "version": module.version.map(|version| version.to_string()),
                    "library": module.typst.as_ref().map(|entry| module.root.join(&entry.entry)),
                    "site_entry": module.site.as_ref().map(|entry| module.root.join(&entry.entry)),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let value = serde_json::json!({
        "config": config.config_path(),
        "root": config.get_root(),
        "content": config.build().content_dir,
        "entry": config.build().entry,
        "theme": config.theme().module,
        "modules": modules,
        "output": config.build().publish_dir,
        "minify": {
            "html": config.build().minify.html,
            "css": config.build().minify.css,
            "javascript": config.build().minify.javascript,
        },
        "site": {
            "origin": config.site().origin,
            "base_path": config.site().base_path.base_path(),
            "url": config.site_url(),
        },
        "formatter": {
            "indent": config.formatter().indent,
            "print_width": config.formatter().print_width,
            "prose_wrap": config.formatter().prose_wrap,
        },
        "package_path": config.package_locations().data().map(|location| location.root()),
        "package_cache_path": config.package_locations().cache().map(|location| location.root()),
        "vendored_package_path": config.vendor().typst_packages(),
        "icon_cache_directory": tola_build::BuildResources::icon_cache_directory(config.get_root()),
        "system_fonts": config.system_fonts_allowed(&request.resources),
        "server": {
            "interface": loaded.server().interface,
            "port": loaded.server().port,
        },
        "dev": {
            "watch": loaded.dev().watch,
        },
    });
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::Cancellation;
    use crate::cli::{ConfigFileArgs, TypstPackageArgs};
    use tola_build::InputScope;

    #[test]
    fn theme_resolved_without_executing_driver() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let theme = directory.path().join("theme");
        std::fs::create_dir_all(project.join("content")).unwrap();
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("lib.typ"), "").unwrap();
        std::fs::write(theme.join("site.typ"), "#panic(\"must not run\")").unwrap();
        std::fs::write(theme.join("tola-module.toml"),
            "schema-version=1\nname='notebook'\nversion='1.2.3'\n[typst]\nentry='lib.typ'\n[site]\nentry='site.typ'\n"
        ).unwrap();
        std::fs::write(
            project.join("tola.toml"),
            "[modules.notes]\npath='../theme'\n[theme]\nmodule='notes'\n",
        )
        .unwrap();
        let output = super::super::testing::buffered_output();
        let cancellation = Cancellation::default();
        let request = super::super::SiteRequest::new(
            ConfigFileArgs {
                path: Some(project.join("tola.toml")),
            },
            TypstPackageArgs::default(),
            InputScope::Offline,
            BuildOverrideArgs::default(),
            crate::cli::CompilerFontArgs::default(),
            &output,
            &cancellation,
        )
        .unwrap();
        let shown = value(&request, &BuildOverrideArgs::default()).unwrap();
        let module = &shown["modules"]["notes"];
        assert_eq!(shown["theme"], "notes");
        assert_eq!(module["package"], "notebook");
        assert_eq!(module["version"], "1.2.3");
        assert_eq!(
            shown["entry"],
            serde_json::json!(theme.canonicalize().unwrap().join("site.typ"))
        );
        assert_eq!(
            module["library"],
            serde_json::json!(theme.canonicalize().unwrap().join("lib.typ"))
        );
        assert_eq!(
            shown["content"],
            serde_json::json!(project.canonicalize().unwrap().join("content"))
        );
        assert!(!project.join("site.typ").exists());
        assert!(!project.join("public").exists());
    }

    #[test]
    fn build_overrides_reach_config_json() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), "").unwrap();
        std::fs::write(
            root.join("tola.toml"),
            "[site]\norigin = \"https://example.com\"\nbase-path = \"/docs/\"\n",
        )
        .unwrap();
        let output = super::super::testing::buffered_output();
        let cancellation = Cancellation::default();
        let request = super::super::SiteRequest::new(
            ConfigFileArgs {
                path: Some(root.join("tola.toml")),
            },
            TypstPackageArgs::default(),
            InputScope::Online,
            BuildOverrideArgs::default(),
            crate::cli::CompilerFontArgs::default(),
            &output,
            &cancellation,
        )
        .unwrap();
        let overrides = BuildOverrideArgs {
            minify: Some(false),
            origin: Some("https://override.example".to_owned()),
            base_path: Some("/preview/".to_owned()),
            ..BuildOverrideArgs::default()
        };

        let shown = value(&request, &overrides).unwrap();

        assert_eq!(shown["site"]["origin"], "https://override.example");
        assert_eq!(shown["site"]["base_path"], "/preview/");
        assert_eq!(shown["site"]["url"], "https://override.example/preview/");
        assert_eq!(shown["minify"]["html"], false);
        assert_eq!(shown["formatter"]["indent"], 2);
        assert_eq!(shown["formatter"]["print_width"], 120);
        assert_eq!(shown["formatter"]["prose_wrap"], "none");
        assert_eq!(shown["server"]["interface"], "127.0.0.1");
        assert_eq!(shown["server"]["port"], 5277);
        assert_eq!(shown["dev"]["watch"], true);
    }
}
