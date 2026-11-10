//! Staging of the files and directories `tola init` creates.

use std::path::Path;

use anyhow::Result;
use tola_build::config::section::BuildSectionConfig;
use tola_build::filesystem::swap_workspace;

use crate::writes::FileWrites;

use super::config::{self, CONFIG_PATH};

/// The directory a new site starts with; its sources live below it.
const CONTENT_DIR: &str = "content";

/// The program every new site builds from.
const SITE_PATH: &str = "site.typ";
const SITE_SOURCE: &str = include_str!("templates/site.typ");

/// Every file and directory `tola init` creates at `root`.
pub(super) fn file_writes(root: &Path) -> Result<FileWrites> {
    let mut writes = FileWrites::new(root)?;
    writes.add_directory(CONTENT_DIR)?;
    writes.create_file(CONFIG_PATH, config::config_file(SITE_PATH))?;
    writes.create_file(SITE_PATH, SITE_SOURCE)?;
    let ignore = ignore_file(&BuildSectionConfig::default().publish_dir);
    writes.create_file(".gitignore", ignore.clone())?;
    writes.create_file(".ignore", ignore)?;
    Ok(writes)
}

fn ignore_file(output_dir: &Path) -> String {
    let mut content = String::new();
    let mut append_directory = |path: &Path| {
        content.push('/');
        content.extend(
            path.to_string_lossy()
                .trim_matches(['/', '\\'])
                .chars()
                .map(|character| if character == '\\' { '/' } else { character }),
        );
        content.push_str("/\n");
    };
    append_directory(output_dir);
    if let Some(workspace) = swap_workspace(output_dir) {
        append_directory(&workspace);
    }
    content.push_str("/.tola/\n/");
    content.push_str(tola_build::filesystem::SITE_BUILD_LOCK_FILE);
    content.push_str("\n.DS_Store\n");
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_site_holds_only_the_minimal_paths() {
        let directory = tempfile::tempdir().unwrap();
        let writes = file_writes(directory.path()).unwrap();
        let mut files = writes
            .file_paths()
            .map(|path| writes.relative_path(path).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files, [".gitignore", ".ignore", "site.typ", "tola.toml"]);
        let directories = writes
            .directory_paths()
            .map(|path| writes.relative_path(path).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(directories, [CONTENT_DIR]);
    }
}
