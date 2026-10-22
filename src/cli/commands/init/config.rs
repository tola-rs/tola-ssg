//! Configuration for the single built-in starter.

use crate::writes::FileWrites;
use std::path::{Path, PathBuf};
use tola_build::config::SiteConfigSchema;
use tola_build::config::section::build::hooks::HooksConfig;
use tola_build::config::section::{
    AssetTreeDeclaration, AssetUrlPrefix, AssetsConfig, BuildSectionConfig, SiteSectionConfig,
    VendorConfig,
};

pub(super) const CONFIG_PATH: &str = "tola.toml";

pub(super) fn schema() -> SiteConfigSchema {
    SiteConfigSchema {
        assets: AssetsConfig {
            trees: vec![AssetTreeDeclaration::new(
                super::files::WEB_ASSETS_DIR,
                AssetUrlPrefix::parse("/assets").expect("valid starter asset prefix"),
            )],
            ..Default::default()
        },
        vendor: VendorConfig {
            path: Some(PathBuf::from("vendor")),
        },
        ..Default::default()
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
    out.push_str(
        &SiteSectionConfig::try_template_with_header_from(&schema.site)
            .expect("initial site configuration must be serializable"),
    );
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
    format!("# tola help \"[{section}]\"\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_config_has_no_optional_sections() {
        let config = config_file(&schema());
        for omitted in [
            "[build.minify]",
            "[build.references]",
            "[typst",
            "[icons]",
            "[diagnostics]",
            "[[build.hooks",
            "deno",
            "tailwind",
        ] {
            assert!(
                !config.contains(omitted),
                "starter must omit {omitted}: {config}"
            );
        }
    }

    #[test]
    fn starter_help_targets_resolve() {
        use crate::help::pages::{page_of, request};
        use crate::i18n::HelpLanguage;
        let config = config_file(&schema());
        let commands = config
            .lines()
            .filter_map(|line| line.strip_prefix("# tola help "))
            .collect::<Vec<_>>();
        assert!(!commands.is_empty());
        for command in commands {
            let arguments = command
                .split_whitespace()
                .map(|argument| argument.trim_matches('"').to_owned())
                .collect::<Vec<_>>();
            let target = request(&arguments).unwrap();
            page_of(target.page().unwrap(), HelpLanguage::English).unwrap();
        }
    }
}
