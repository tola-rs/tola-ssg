use crate::cancellation::Cancellation;
use anyhow::Result;

use crate::cli::output::CommandOutput;
use crate::cli::{BuildOverrideArgs, ConfigFileArgs, TypstPackageArgs};
use crate::config::ConfigOverrides;
use tola_build::InputScope;

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    build: BuildOverrideArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        ..ConfigOverrides::default()
    };
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let config = loaded.config();
    let resources = crate::cli::config::build_resources(scope);
    let value = serde_json::json!({
        "config": config.config_path(),
        "root": config.get_root(),
        "output": config.build().publish_dir,
        "minify": {
            "html": config.build().minify.html,
            "css": config.build().minify.css,
            "javascript": config.build().minify.javascript,
        },
        "site": {
            "origin": config.site().origin,
            "base_path": config.site().base_path,
            "url": config.site_url(),
        },
        "package_path": config.package_locations().data().map(|location| location.root()),
        "package_cache_path": config.package_locations().cache().map(|location| location.root()),
        "vendored_package_path": config.vendor.typst_packages(),
        "icon_cache_directory": tola_build::BuildResources::icon_cache_directory(config.get_root()),
        "system_fonts": config.system_fonts_allowed(&resources),
        "server": {
            "interface": loaded.server().interface,
            "port": loaded.server().port,
        },
        "dev": {
            "watch": loaded.dev().watch,
        },
    });
    output.write_stdout_line(serde_json::to_string_pretty(&value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (sink, captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        run(
            ConfigFileArgs {
                path: Some(root.join("tola.toml")),
            },
            TypstPackageArgs::default(),
            InputScope::Online,
            BuildOverrideArgs {
                minify: Some(false),
                origin: Some("https://override.example".to_owned()),
                base_path: Some("/preview/".to_owned()),
                ..BuildOverrideArgs::default()
            },
            &output,
            &Cancellation::default(),
        )
        .unwrap();

        let shown: serde_json::Value = serde_json::from_slice(&captured.bytes()).unwrap();
        assert_eq!(shown["site"]["origin"], "https://override.example");
        assert_eq!(shown["site"]["base_path"], "/preview/");
        assert_eq!(shown["site"]["url"], "https://override.example/preview/");
        assert_eq!(shown["minify"]["html"], false);
        assert_eq!(shown["server"]["interface"], "127.0.0.1");
        assert_eq!(shown["server"]["port"], 5277);
        assert_eq!(shown["dev"]["watch"], true);
    }
}
