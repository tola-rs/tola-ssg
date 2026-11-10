//! The configuration written into a new site.

use std::path::Path;

use crate::writes::FileWrites;
use tola_build::config::section::BuildSectionConfig;

pub(super) const CONFIG_PATH: &str = "tola.toml";

/// The configuration text inside `writes`.
pub(super) fn config_source(writes: &FileWrites) -> &str {
    let bytes = writes
        .file_contents(Path::new(CONFIG_PATH))
        .expect("initial files always contain tola.toml");
    std::str::from_utf8(bytes).expect("generated configuration is UTF-8")
}

/// Generate `tola.toml`: the build's three paths, paired with the line that points at what the
/// section means. Omitted sections keep their documented defaults.
pub(super) fn config_file() -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Tola configuration (v{})\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str("# https://github.com/tola-rs/tola-ssg\n\n");
    out.push_str(&help_line(BuildSectionConfig::TEMPLATE_SECTION));
    out.push_str(&build_section(&BuildSectionConfig::default()));
    out
}

/// The line that points one configuration section at its own documentation.
fn help_line(section: &str) -> String {
    format!("# tola help \"[{section}]\"\n")
}

/// `[build]` with the paths a new site starts from.
fn build_section(build: &BuildSectionConfig) -> String {
    let mut out = String::from("[build]\n");
    let entry = tola_config::serialize_toml_value(&build.entry)
        .expect("initial build entry must be serializable");
    out.push_str(&format!("entry = {entry}\n"));
    for (name, value) in [
        ("content-dir", &build.content_dir),
        ("publish-dir", &build.publish_dir),
    ] {
        let value = tola_config::serialize_toml_value(value)
            .expect("initial build paths must be serializable");
        out.push_str(&format!("{name} = {value}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_help_targets_resolve() {
        use crate::help::pages::{page_of, request};
        use crate::i18n::HelpLanguage;
        let config = config_file();
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
