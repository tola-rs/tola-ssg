//! Composition of the generated `tola.toml`.

use std::path::{Path, PathBuf};

use crate::writes::FileWrites;
use tola_build::config::SiteConfigSchema;
use tola_build::config::section::build::hooks::{
    BeforeBuildHookConfig, CommandOutput, HooksConfig, OutputCommandConfig,
};
use tola_build::config::section::{
    AssetTreeDeclaration, AssetUrlPrefix, AssetsConfig, BuildSectionConfig, IconsConfig,
    SiteSectionConfig, TypstSectionConfig, VendorConfig,
};

use super::features::{Effects, Hook, HookStage};

pub(super) const CONFIG_PATH: &str = "tola.toml";

/// The configuration the scaffold's effects require: default site settings, the browser-delivered
/// asset subtree mapped, and vendoring pointed at `vendor`.
pub(super) fn schema(effects: &Effects) -> SiteConfigSchema {
    // Only the browser-delivered subtree is mapped: compiler fonts and processing inputs
    // elsewhere under `static` stay unpublished unless another producer selects them.
    let assets = AssetsConfig {
        trees: vec![AssetTreeDeclaration::new(
            super::files::WEB_ASSETS_DIR,
            AssetUrlPrefix::parse("/assets").expect("the scaffold asset prefix is valid"),
        )],
        cache_busting: effects.cache_busting,
        ..AssetsConfig::default()
    };
    let build = BuildSectionConfig {
        hooks: HooksConfig {
            before_build: before_build_hooks(effects),
            generate_outputs: output_commands(effects),
            ..HooksConfig::default()
        },
        ..BuildSectionConfig::default()
    };
    SiteConfigSchema {
        site: SiteSectionConfig::default(),
        build,
        assets,
        typst: TypstSectionConfig::default(),
        icons: IconsConfig::default(),
        vendor: VendorConfig {
            path: Some(PathBuf::from("vendor")),
        },
    }
}

/// The `before-build` configuration entries the effects declare.
fn before_build_hooks(effects: &Effects) -> Vec<BeforeBuildHookConfig> {
    effects
        .hooks
        .iter()
        .filter(|hook| hook.stage == HookStage::BeforeBuild)
        .map(before_build_hook)
        .collect()
}

/// The `generate-outputs` configuration entries the effects declare.
fn output_commands(effects: &Effects) -> Vec<OutputCommandConfig> {
    effects
        .hooks
        .iter()
        .filter(|hook| hook.stage == HookStage::GenerateOutputs)
        .map(output_command)
        .collect()
}

/// The configuration entry one `before-build` hook declares.
fn before_build_hook(hook: &Hook) -> BeforeBuildHookConfig {
    BeforeBuildHookConfig {
        name: hook.name.to_owned(),
        command: hook.command.iter().map(|word| (*word).to_owned()).collect(),
        rerun_on: hook.rerun_on.iter().map(PathBuf::from).collect(),
        generates: hook.outputs.iter().map(PathBuf::from).collect(),
        ..BeforeBuildHookConfig::default()
    }
}

/// The configuration entry one `generate-outputs` hook declares: the trees it owns.
fn output_command(hook: &Hook) -> OutputCommandConfig {
    OutputCommandConfig {
        name: hook.name.to_owned(),
        command: hook.command.iter().map(|word| (*word).to_owned()).collect(),
        rerun_on: hook.rerun_on.iter().map(PathBuf::from).collect(),
        outputs: hook
            .outputs
            .iter()
            .map(|tree| {
                CommandOutput::Tree(
                    tola_address::OutputPath::parse(tree)
                        .expect("the scaffold output tree is a valid path"),
                )
            })
            .collect(),
        ..OutputCommandConfig::default()
    }
}

/// The configuration text inside `writes`.
pub(super) fn config_source(writes: &FileWrites) -> &str {
    let bytes = writes
        .file_contents(Path::new(CONFIG_PATH))
        .expect("initial files always contain tola.toml");
    std::str::from_utf8(bytes).expect("generated configuration is UTF-8")
}

/// Generate the initial `tola.toml` file: the sections a new site sets, each value paired with the
/// one line that points at what its keys mean. Omitted sections keep their documented defaults.
pub(super) fn config_file(schema: &SiteConfigSchema) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# Tola configuration (v{})\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str("# https://github.com/tola-rs/tola-ssg\n\n");

    out.push_str(&help_line(SiteSectionConfig::TEMPLATE_SECTION));
    out.push_str(&site_template(&schema.site));
    out.push('\n');

    out.push_str(&help_line(BuildSectionConfig::TEMPLATE_SECTION));
    out.push_str(&build_section(&schema.build));
    out.push('\n');

    out.push_str(&help_line(AssetsConfig::TEMPLATE_SECTION));
    out.push_str(
        &AssetsConfig::try_template_with_header_from(&schema.assets)
            .expect("the scaffold asset configuration must be serializable"),
    );
    out.push('\n');

    out.push_str(&help_line(VendorConfig::TEMPLATE_SECTION));
    out.push_str(
        &VendorConfig::try_template_with_header_from(&schema.vendor)
            .expect("the scaffold vendor configuration must be serializable"),
    );
    out.push('\n');

    out
}

/// The line that points one scaffold section at its own documentation.
fn help_line(section: &str) -> String {
    format!("# tola help config {section}\n")
}

/// `[build]` with the fields every scaffold sets; every other build section keeps its documented
/// default, and one line points at the hook reference.
fn build_section(build: &BuildSectionConfig) -> String {
    let mut out = String::new();
    out.push_str("[build]\n");
    for (name, value) in [
        ("entry", &build.entry),
        ("content-dir", &build.content_dir),
        ("publish-dir", &build.publish_dir),
    ] {
        let value = tola_config::serialize_toml_value(value)
            .expect("scaffold build fields must be serializable");
        out.push_str(&format!("{name} = {value}\n"));
    }
    out.push('\n');
    out.push_str(&help_line(HooksConfig::TEMPLATE_SECTION));
    let hooks = HooksConfig::try_template_from(&build.hooks)
        .expect("the scaffold hook configuration must be serializable");
    if !hooks.is_empty() {
        out.push('\n');
        out.push_str(hooks.trim_start_matches('\n'));
    }
    out
}

fn site_template(site: &SiteSectionConfig) -> String {
    let mut out = SiteSectionConfig::try_template_with_header_from(site)
        .expect("initial site configuration must be serializable");
    if !site.extra.is_empty() {
        let extra = toml::Value::try_from(&site.extra)
            .expect("initial site extensions must be serializable");
        let site = toml::Value::Table(toml::map::Map::from_iter([("extra".into(), extra)]));
        let extensions = toml::map::Map::from_iter([("site".into(), site)]);
        out.push('\n');
        out.push_str(
            &toml::to_string(&extensions).expect("initial site extensions must be serializable"),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::init::features;
    use crate::cli::commands::init::files;

    /// The writes one preset stages at `root`.
    fn staged_writes(root: &Path, name: &str) -> FileWrites {
        let effects = features::Effects::combine(&features::features(name));
        files::file_writes(root, &[], &Default::default(), &effects).unwrap()
    }

    #[test]
    fn configuration_has_only_the_starter_sections() {
        for name in ["minimal", "medium", "rich"] {
            let effects = features::Effects::combine(&features::features(name));
            let config = config_file(&schema(&effects));
            for omitted in [
                "[build.minify]",
                "[build.references]",
                "[typst",
                "[icons]",
                "[diagnostics]",
                "# [[build.hooks",
                "# To use a hook",
            ] {
                assert!(
                    !config.contains(omitted),
                    "{name} must omit {omitted}:\n{config}"
                );
            }
            assert_eq!(
                config.contains("[[build.hooks.before-build]]"),
                name == "rich",
                "{name} hook presence:\n{config}"
            );
        }
    }

    #[test]
    fn starter_help_targets_resolve() {
        use crate::help::pages::{CrossRefs, page_of, request};
        use crate::i18n::HelpLanguage;

        let effects = features::Effects::combine(&features::features("minimal"));
        let config = config_file(&schema(&effects));
        let commands = config
            .lines()
            .filter_map(|line| line.strip_prefix("# tola help "))
            .collect::<Vec<_>>();
        assert!(!commands.is_empty());
        for command in commands {
            let arguments = command
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let target = request(&arguments).unwrap();
            page_of(
                target.page().unwrap(),
                HelpLanguage::English,
                CrossRefs::Emit,
            )
            .unwrap();
        }
    }

    #[test]
    fn rich_configuration_wires_the_tailwind_build() {
        let directory = tempfile::tempdir().unwrap();
        let writes = staged_writes(directory.path(), "rich");
        let schema =
            super::super::validate_configuration(&writes, tola_build::InputScope::Online).unwrap();

        let hook = &schema.build.hooks.before_build[0];
        assert_eq!(hook.name, "tailwind");
        assert_eq!(hook.command, ["just", "css"]);
        assert_eq!(
            hook.generates,
            [PathBuf::from("static/web-assets/tailwind-output/site.css")]
        );
        assert!(schema.assets.cache_busting);
        assert!(schema.assets.files.is_empty());
    }

    #[test]
    fn generated_configuration_validates_for_every_preset() {
        for name in ["minimal", "medium", "rich"] {
            let directory = tempfile::tempdir().unwrap();
            let writes = staged_writes(directory.path(), name);
            super::super::validate_configuration(&writes, tola_build::InputScope::Online)
                .unwrap_or_else(|error| panic!("{name} must validate: {error}"));
        }
    }
}
