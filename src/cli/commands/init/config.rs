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

/// Generate `tola.toml` with the new site's Typst entry.
pub(super) fn config_file(entry: &str) -> String {
    let section = BuildSectionConfig::TEMPLATE_SECTION;
    let entry = tola_config::serialize_toml_value(&entry)
        .expect("initial build entry must be serializable");
    format!(
        "# Tola configuration (v{})\n\
         # https://github.com/tola-rs/tola-ssg\n\n\
         # tola help \"[{section}]\"\n\
         [{section}]\n\
         entry = {entry}\n",
        env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_help_targets_resolve() {
        use crate::help::pages::{page_of, request};
        use crate::i18n::HelpLanguage;
        let config = config_file("site.typ");
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
