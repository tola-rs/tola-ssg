//! The resolved configuration and the paths it selects, as one JSON document for
//! `tola inspect config`.
//!
//! Preparing modules resolves their declared paths without executing a driver, so the document
//! states what a build would use while running none of the site's code.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use tola_typst_syntax::style::ProseWrap;

use crate::cli::BuildOverrideArgs;

#[derive(Serialize)]
struct ModuleDocument<'a> {
    root: &'a Path,
    manifest: &'a Path,
    library: Option<PathBuf>,
    build_entry: Option<PathBuf>,
}

#[derive(Serialize)]
struct MinifyDocument {
    html: bool,
    css: bool,
    javascript: bool,
}

#[derive(Serialize)]
struct SiteDocument<'a> {
    origin: Option<&'a str>,
    base_path: String,
    url: Option<String>,
}

#[derive(Serialize)]
struct FormatterDocument<'a> {
    indent: usize,
    print_width: usize,
    prose_wrap: &'a ProseWrap,
}

#[derive(Serialize)]
struct ServerDocument {
    interface: String,
    port: u16,
}

#[derive(Serialize)]
struct DevDocument {
    watch: bool,
}

#[derive(Serialize)]
struct ConfigDocument<'a> {
    config: &'a Path,
    root: &'a Path,
    content: &'a Path,
    entry: &'a Path,
    entry_module: Option<&'a str>,
    modules: BTreeMap<String, ModuleDocument<'a>>,
    output: &'a Path,
    minify: MinifyDocument,
    site: SiteDocument<'a>,
    formatter: FormatterDocument<'a>,
    package_path: Option<&'a Path>,
    package_cache_path: Option<&'a Path>,
    vendored_package_path: Option<PathBuf>,
    icon_cache_directory: PathBuf,
    system_fonts: bool,
    server: ServerDocument,
    dev: DevDocument,
}

pub(super) fn value(
    request: &super::SiteRequest<'_>,
    build: &BuildOverrideArgs,
) -> Result<serde_json::Value> {
    let overrides = crate::cli::config::config_overrides(build);
    let loaded = request.load_config(&overrides)?;
    let prepared = tola_build::modules::prepare_site(
        loaded.config(),
        tola_input::InputMode::new(request.scope(), true),
        &request.cancellation.token(),
    )?;
    let config = &prepared;
    let modules = config
        .loaded_modules()
        .map(|(alias, module)| {
            (
                alias.to_owned(),
                ModuleDocument {
                    root: module.root.as_path(),
                    manifest: module.manifest.as_path(),
                    library: module
                        .typst
                        .as_ref()
                        .map(|entry| module.root.join(&entry.entry)),
                    build_entry: module
                        .build
                        .as_ref()
                        .map(|entry| module.root.join(&entry.entry)),
                },
            )
        })
        .collect();
    let document = ConfigDocument {
        config: config.config_path(),
        root: config.get_root(),
        content: config.build().content_dir.as_path(),
        entry: config.build().entry.as_path(),
        entry_module: config.module_entry(),
        modules,
        output: config.build().publish_dir.as_path(),
        minify: MinifyDocument {
            html: config.build().minify.html,
            css: config.build().minify.css,
            javascript: config.build().minify.javascript,
        },
        site: SiteDocument {
            origin: config.site().origin.as_deref(),
            base_path: config.site().base_path.base_path(),
            url: config.site_url(),
        },
        formatter: FormatterDocument {
            indent: config.formatter().indent,
            print_width: config.formatter().print_width,
            prose_wrap: &config.formatter().prose_wrap,
        },
        package_path: config
            .package_locations()
            .data()
            .map(|location| location.root()),
        package_cache_path: config
            .package_locations()
            .cache()
            .map(|location| location.root()),
        vendored_package_path: config.vendor().typst_packages(),
        icon_cache_directory: tola_build::BuildResources::icon_cache_directory(config.get_root()),
        system_fonts: config.system_fonts_allowed(&request.resources),
        server: ServerDocument {
            interface: loaded.server().interface.to_string(),
            port: loaded.server().port,
        },
        dev: DevDocument {
            watch: loaded.dev().watch,
        },
    };
    Ok(serde_json::to_value(document)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::Cancellation;
    use crate::cli::{ConfigFileArgs, TypstPackageArgs};
    use tola_input::InputScope;

    #[test]
    fn theme_resolved_without_executing_driver() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let theme = directory.path().join("theme");
        std::fs::create_dir_all(project.join("content")).unwrap();
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("lib.typ"), "").unwrap();
        std::fs::write(theme.join("site.typ"), "#panic(\"must not run\")").unwrap();
        std::fs::write(
            theme.join("tola-module.toml"),
            "\n[typst]\nentry='lib.typ'\n[build]\nentry='site.typ'\n",
        )
        .unwrap();
        std::fs::write(
            project.join("tola.toml"),
            "[modules.notes]\npath='../theme'\n[build]\nentry={module='notes'}\n",
        )
        .unwrap();
        let output = super::super::testing::buffered_output();
        let cancellation = Cancellation::default();
        let request = super::super::SiteRequest::new(
            crate::cli::SiteArgs {
                config: ConfigFileArgs {
                    path: Some(project.join("tola.toml")),
                },
                packages: TypstPackageArgs::default(),
            },
            crate::cli::BuildArgs::default(),
            tola_input::InputMode::new(InputScope::Offline, false),
            &output,
            &cancellation,
        )
        .unwrap();
        let shown = value(&request, &BuildOverrideArgs::default()).unwrap();
        let module = &shown["modules"]["notes"];
        assert_eq!(shown["entry_module"], "notes");
        assert_eq!(
            shown["entry"],
            serde_json::json!(theme.canonicalize().unwrap().join("site.typ"))
        );
        assert_eq!(
            module["library"],
            serde_json::json!(theme.canonicalize().unwrap().join("lib.typ"))
        );
        assert_eq!(module["build_entry"], shown["entry"]);
        assert_eq!(
            shown["content"],
            serde_json::json!(project.canonicalize().unwrap().join("content"))
        );
        assert!(!project.join("site.typ").exists());
        assert!(!project.join("public").exists());
    }
}
