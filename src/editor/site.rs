//! Resolved compiler inputs supplied to the editor's Typst language services.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use tola_build::config::ResolvedSiteConfig;

/// The executable every editor runs, named as the PATH entries an author's terminal already has.
pub(super) const SERVER_COMMAND: &str = "tola";

/// The one command every editor starts: Tola's language server, plus the arguments that select
/// this directory's configuration and its explicit package roots.
#[derive(Debug)]
pub(super) struct ServerCommand {
    pub(super) command: &'static str,
    pub(super) arguments: Vec<String>,
}

/// One argument as a shell word, quoted only where a shell would split or expand it.
pub(super) fn shell_word(argument: &str) -> String {
    if argument
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._/:=@,+-".contains(character))
    {
        return argument.to_owned();
    }
    format!("'{}'", argument.replace('\'', "'\\''"))
}

/// The directory one editor session is configured for, and the configuration file it names.
///
/// A directory that holds no site configuration is still configured: its settings name none, so
/// the language server discovers a `tola.toml` added later and until then answers for each Typst
/// document on its own.
#[derive(Debug)]
pub(crate) struct EditorDirectory {
    root: PathBuf,
    configuration_file: Option<PathBuf>,
    entry: Option<PathBuf>,
    font_directories: Vec<PathBuf>,
    packages: tola_typst::PackageLocations,
}

impl EditorDirectory {
    /// The directory of a site that exists: its resolved configuration selects the site's inputs.
    pub(crate) fn from_config(config: &ResolvedSiteConfig) -> Result<Self> {
        Self {
            root: config.get_root().to_path_buf(),
            configuration_file: Some(config.config_path().to_path_buf()),
            entry: Some(config.build().entry.clone()),
            font_directories: config.fonts().paths.clone(),
            packages: config.package_locations().clone(),
        }
        .validate_paths()
    }

    /// The directory `init` is creating a site in: settings name the configuration it writes.
    pub(super) fn initial(root: &Path, packages: &tola_typst::PackageLocations) -> Result<Self> {
        Self {
            root: root.to_path_buf(),
            configuration_file: Some(root.join("tola.toml")),
            entry: Some(root.join("site.typ")),
            font_directories: vec![root.join("static/typst-fonts")],
            packages: packages.clone(),
        }
        .validate_paths()
    }

    /// The directory of a workspace that holds no site configuration.
    pub(crate) fn workspace(root: &Path, packages: &tola_typst::PackageLocations) -> Result<Self> {
        Self {
            root: root.to_path_buf(),
            configuration_file: None,
            entry: None,
            font_directories: Vec::new(),
            packages: packages.clone(),
        }
        .validate_paths()
    }

    fn validate_paths(self) -> Result<Self> {
        let declared = [
            ("the site root", Some(self.root.as_path())),
            ("`tola.configPath`", self.configuration_file.as_deref()),
            ("`build.entry`", self.entry.as_deref()),
        ];
        for (label, path) in declared
            .into_iter()
            .filter_map(|(label, path)| Some((label, path?)))
            .chain(
                self.font_directories
                    .iter()
                    .map(|path| ("`typst.fonts.paths`", path.as_path())),
            )
            .chain(
                self.packages
                    .data()
                    .map(|directory| ("`--package-path`", directory.root())),
            )
            .chain(
                self.packages
                    .cache()
                    .map(|directory| ("`--package-cache-path`", directory.root())),
            )
        {
            if path.to_str().is_none() {
                anyhow::bail!(
                    "{label} is not valid UTF-8; move the site or that directory to a UTF-8 path"
                );
            }
        }
        Ok(self)
    }

    /// Whether the settings this directory publishes name a configuration file.
    pub(super) fn names_configuration_file(&self) -> bool {
        self.configuration_file.is_some()
    }

    /// The configuration file this directory's settings name, when it names one.
    pub(super) fn configuration_file(&self) -> Option<&Path> {
        self.configuration_file.as_deref()
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The one command every editor is configured with: Tola's language server, started on stdio
    /// with the arguments that select this directory's configuration and its package roots.
    ///
    /// A root a bare `tola lsp` discovers on its own stays out of generated entries and settings:
    /// naming it again would outlive a host that moved its packages, while a root the author
    /// declared is what the entry has to name.
    pub(super) fn server_command(&self) -> ServerCommand {
        let mut arguments = vec!["lsp".to_owned()];
        if let Some(configuration_file) = &self.configuration_file {
            arguments.push("--config".to_owned());
            arguments.push(
                configuration_file
                    .to_str()
                    .expect("validated editor configuration path")
                    .to_owned(),
            );
        }
        let discovered = discovered_packages();
        for (flag, directory, discovered) in [
            (
                "--package-path",
                self.packages.data(),
                discovered.as_ref().and_then(|locations| locations.data()),
            ),
            (
                "--package-cache-path",
                self.packages.cache(),
                discovered.as_ref().and_then(|locations| locations.cache()),
            ),
        ] {
            let Some(directory) = directory else {
                continue;
            };
            if restates_discovered_root(discovered, directory) {
                continue;
            }
            arguments.push(flag.to_owned());
            arguments.push(
                directory
                    .root()
                    .to_str()
                    .expect("validated editor package path")
                    .to_owned(),
            );
        }
        ServerCommand {
            command: SERVER_COMMAND,
            arguments,
        }
    }

    pub(super) fn tola_settings(&self) -> Value {
        let mut settings = serde_json::Map::new();
        if let Some(configuration_file) = &self.configuration_file {
            settings.insert("configPath".into(), json!(configuration_file));
        }
        let discovered = discovered_packages();
        for (key, directory, discovered) in [
            (
                "packagePath",
                self.packages.data(),
                discovered.as_ref().and_then(|locations| locations.data()),
            ),
            (
                "packageCachePath",
                self.packages.cache(),
                discovered.as_ref().and_then(|locations| locations.cache()),
            ),
        ] {
            let Some(directory) = directory else {
                continue;
            };
            if restates_discovered_root(discovered, directory) {
                continue;
            }
            settings.insert(key.into(), json!(directory.root()));
        }
        Value::Object(settings)
    }
}

/// The package roots a bare `tola lsp` discovers on this host.
fn discovered_packages() -> Option<tola_typst::PackageLocations> {
    tola_typst::PackageLocations::discover(None, None).ok()
}

/// Whether a package root only restates what a bare invocation discovers.
fn restates_discovered_root(
    discovered: Option<&tola_typst::PackageLocation>,
    directory: &tola_typst::PackageLocation,
) -> bool {
    discovered.is_some_and(|discovered| discovered.root() == directory.root())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn non_utf8_paths_are_rejected() {
        use std::os::unix::ffi::OsStringExt;

        let root = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(EditorDirectory::initial(&root, &Default::default()).is_err());
    }

    #[test]
    fn workspace_settings_name_no_configuration() {
        let directory = EditorDirectory::workspace(Path::new("."), &Default::default()).unwrap();

        assert_eq!(directory.server_command().arguments, ["lsp"]);
        assert!(directory.tola_settings().get("configPath").is_none());
    }

    #[test]
    fn site_settings_name_the_configuration() {
        let directory = EditorDirectory::initial(Path::new("."), &Default::default()).unwrap();

        assert_eq!(
            directory.server_command().arguments[..2],
            ["lsp".to_owned(), "--config".to_owned()]
        );
        assert_eq!(
            directory.tola_settings()["configPath"],
            serde_json::json!("./tola.toml")
        );
    }

    /// A site whose package roots are the ones a bare invocation discovers writes no package
    /// flags: those roots are what the editor's server resolves by itself.
    #[test]
    fn discovered_package_roots_stay_out_of_the_entry() {
        let discovered =
            tola_typst::PackageLocations::discover(None, None).expect("host package roots");
        let directory = EditorDirectory::initial(Path::new("."), &discovered).unwrap();

        assert_eq!(
            directory.server_command().arguments,
            [
                "lsp".to_owned(),
                "--config".to_owned(),
                "./tola.toml".to_owned()
            ]
        );
    }

    /// A package root this process resolved differently is what the entry has to name.
    #[test]
    fn declared_package_roots_reach_the_entry() {
        let directory = tempfile::tempdir().unwrap();
        let packages = tola_typst::PackageLocations::from_absolute_roots(
            Some(directory.path().to_path_buf()),
            None,
        )
        .unwrap();
        let settings = EditorDirectory::initial(Path::new("."), &packages).unwrap();

        assert_eq!(
            settings.server_command().arguments,
            [
                "lsp".to_owned(),
                "--config".to_owned(),
                "./tola.toml".to_owned(),
                "--package-path".to_owned(),
                directory.path().to_str().unwrap().to_owned(),
            ]
        );
    }

    /// Settings, like the entry, name no package root a bare invocation discovers.
    #[test]
    fn discovered_package_roots_stay_out_of_settings() {
        let discovered =
            tola_typst::PackageLocations::discover(None, None).expect("host package roots");
        let directory = EditorDirectory::initial(Path::new("."), &discovered).unwrap();

        let settings = directory.tola_settings();
        assert!(settings.get("packagePath").is_none(), "{settings}");
        assert!(settings.get("packageCachePath").is_none(), "{settings}");
    }

    /// A package root this process resolved differently is what the settings have to name.
    #[test]
    fn declared_package_roots_reach_the_settings() {
        let root = tempfile::tempdir().unwrap();
        let packages = tola_typst::PackageLocations::from_absolute_roots(
            Some(root.path().to_path_buf()),
            None,
        )
        .unwrap();
        let settings = EditorDirectory::initial(Path::new("."), &packages)
            .unwrap()
            .tola_settings();

        assert_eq!(settings["packagePath"], serde_json::json!(root.path()));
        assert!(settings.get("packageCachePath").is_none(), "{settings}");
    }
}
