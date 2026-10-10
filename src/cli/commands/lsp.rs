//! Independent Tola language service with application-owned configuration and I/O.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::{ConfigSource, ResolvedSiteConfig};
use tola_build::diagnostic::{DiagnosticError, Location};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs};
use tola_build::InputScope;

use tola_lsp::ServedWorkspace;

pub(in crate::cli) fn run(
    config: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    cancellation: &Cancellation,
    output: &CommandOutput,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let resources = crate::cli::config::build_resources(scope);
    let mut configuration = source_configuration(config, packages, resources.input_scope())?;
    let token = cancellation.token();
    let input = std::io::stdin();
    let writer = std::io::stdout();
    let loader_output = output.clone();
    let loader_cancel = token.clone();
    let diagnostics_output = output.clone();
    // The configuration the author named stays known to the server before a check resolves one, so
    // the editor's own document answers while the site does not compile.
    let named_configuration = configuration.named.clone();
    tola_lsp::serve(
        input,
        writer,
        resources,
        token,
        move |root, sources| configuration.load(root, sources, &loader_output, &loader_cancel),
        move |diagnostics| diagnostics_output.record_diagnostics(None, diagnostics),
        named_configuration,
        crate::config::HOST_SECTIONS,
    )
}

/// The site configuration one language-server session reads its sources through.
///
/// Package inputs are the check's to resolve, not this process's. A `--pure` session that names a
/// package path, or a site whose package was never vendored, must reach the author as a diagnostic
/// in a session that answers, and a process that refuses to start has no way to say anything.
fn source_configuration(
    config: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
) -> Result<SourceConfiguration> {
    Ok(SourceConfiguration {
        named: config.path.map(std::path::absolute).transpose()?,
        scope,
        packages,
        loaded: None,
    })
}

struct SourceConfiguration {
    /// The configuration file the invocation named, which its workspace must resolve through.
    named: Option<PathBuf>,
    scope: InputScope,
    packages: TypstPackageArgs,
    loaded: Option<LoadedConfiguration>,
}

/// What one session resolved its sources through.
struct LoadedConfiguration {
    /// The configuration document the sources resolve from, absent for a workspace that holds no
    /// site configuration: its documents are compiled on their own.
    file: Option<PathBuf>,
    configuration: Arc<ResolvedSiteConfig>,
    loader: crate::config::ConfigLoader,
}

impl SourceConfiguration {
    fn load(
        &mut self,
        root: &Path,
        sources: &[(PathBuf, Arc<str>)],
        output: &CommandOutput,
        cancellation: &BuildCancellation,
    ) -> Result<ServedWorkspace> {
        // The client's workspace bounds automatic discovery; an explicit --config may name
        // another site. CLI commands retain their own ancestor discovery.
        let file = self.named.clone().or_else(|| {
            let path = root.join(tola_build::config::loading::CONFIG_FILE_NAME);
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                _ => Some(path),
            }
        });
        let report_path = file
            .clone()
            .unwrap_or_else(|| root.join(tola_build::config::loading::CONFIG_FILE_NAME));
        let resolved = (|| -> Result<ServedWorkspace> {
            cancellation.ensure_active()?;
            if self
                .loaded
                .as_ref()
                .is_none_or(|loaded| loaded.file != file)
            {
                let source = match file.as_deref() {
                    Some(path) => match configuration_text(path, sources)? {
                        Some(text) => ConfigSource::parse(path, text, self.scope)?,
                        None => ConfigSource::load(Some(path), self.scope)?,
                    },
                    // A workspace that holds no site configuration is served as the documents it
                    // holds: the schema's defaults, rooted at its own directory.
                    None => ConfigSource::defaults(root, self.scope)?,
                };
                // The check that reads the packages is what resolves them, so a refusal is a
                // diagnostic on a session that keeps answering rather than an exit.
                let packages = crate::cli::config::package_locations(&self.packages, self.scope)?;
                let loaded = crate::config::resolve(
                    source,
                    packages,
                    &crate::config::ConfigOverrides::default(),
                )?;
                output.apply_diagnostic_limits(loaded.diagnostics());
                crate::cli::log::destination::start_site(output, loaded.config(), cancellation)?;
                let (config, _, _, loader) = loaded.into_parts();
                self.loaded = Some(LoadedConfiguration {
                    file,
                    configuration: config,
                    loader,
                });
            }
            let loaded = self.loaded.as_mut().expect("configuration is loaded");
            let site_root = tola_build::filesystem::normalize_existing_prefix(
                loaded
                    .loader
                    .path()
                    .parent()
                    .context("configuration source has no parent")?,
            );
            anyhow::ensure!(
                site_root == loaded.configuration.get_root(),
                "the site root changed; restart the Tola language server"
            );
            // A workspace that holds no configuration document has no file to re-read: its
            // defaults stand until its author writes one.
            let text = match loaded.file.as_deref() {
                Some(path) => configuration_text(path, sources)?,
                None => None,
            };
            let candidate = match (loaded.file.as_deref(), text) {
                (Some(_), Some(text)) => loaded.loader.load_text_candidate(text)?,
                (Some(_), None) => loaded.loader.load_candidate()?,
                (None, _) => None,
            };
            if let Some(candidate) = candidate {
                if !candidate.has_same_build_settings(&loaded.configuration)? {
                    loaded.configuration = candidate.config();
                }
                output.apply_diagnostic_limits(candidate.diagnostics());
                loaded.loader.acknowledge(&candidate);
            }
            Ok(match loaded.file {
                Some(_) => ServedWorkspace::Site(Arc::clone(&loaded.configuration)),
                None => ServedWorkspace::Documents(Arc::clone(&loaded.configuration)),
            })
        })();
        resolved.map_err(|error| {
            crate::cli::log::destination::start_after_error(output, root, cancellation);
            // Configuration diagnostics name a file relative to the configuration's own
            // directory, which the language service would resolve against its workspace root.
            let diagnostics = match tola_build::diagnostic::attached(&error) {
                Some(attached) => {
                    let mut diagnostics = attached.to_vec();
                    for diagnostic in &mut diagnostics {
                        establish_configuration_path(&mut diagnostic.location, &report_path);
                    }
                    diagnostics
                }
                None => {
                    let diagnostic = tola_build::diagnostic::fallback(
                        crate::codes::editor::CONFIGURATION,
                        &error,
                    )
                    .with_path(absolute_configuration_path(&report_path));
                    vec![diagnostic]
                }
            };
            DiagnosticError::attach(error, diagnostics).into()
        })
    }
}

fn absolute_configuration_path(path: &Path) -> String {
    crate::terminal::display_path_as_given(&tola_build::filesystem::normalize_existing_prefix(path))
}

fn establish_configuration_path(location: &mut Option<Location>, config_path: &Path) {
    let Some(location) = location else {
        return;
    };
    if Path::new(&location.path).is_absolute() {
        return;
    }
    let base = config_path.parent().unwrap_or(config_path);
    location.path = crate::terminal::display_path_as_given(
        &tola_build::filesystem::normalize_existing_prefix(&base.join(&location.path)),
    );
}

fn configuration_text<'a>(
    path: &Path,
    sources: &'a [(PathBuf, Arc<str>)],
) -> Result<Option<&'a str>> {
    let physical = tola_build::filesystem::normalize_existing_prefix(path);
    let mut candidates = sources.iter().filter(|(path, _)| *path == physical);
    let Some((_, first)) = candidates.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        candidates.all(|(_, text)| text == first),
        "open buffers for `{}` contain different text; reconcile or close the duplicate buffers",
        crate::terminal::display_path(path)
    );
    Ok(Some(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::diagnostic::SourceLine;

    #[test]
    fn workspace_configuration_is_local() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path();
        let root = parent.join("documents");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(parent.join("tola.toml"), "invalid configuration").unwrap();
        let (sink, _) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        let mut configuration = source_configuration(
            ConfigFileArgs::default(),
            TypstPackageArgs::default(),
            InputScope::Offline,
        )
        .unwrap();
        let cancellation = BuildCancellation::new();
        assert!(matches!(
            configuration
                .load(&root, &[], &output, &cancellation)
                .unwrap(),
            ServedWorkspace::Documents(_)
        ));

        std::fs::write(root.join("tola.toml"), "").unwrap();
        assert!(matches!(
            configuration
                .load(&root, &[], &output, &cancellation)
                .unwrap(),
            ServedWorkspace::Site(_)
        ));
        std::fs::write(root.join("tola.toml"), "invalid configuration").unwrap();
        assert!(
            configuration
                .load(&root, &[], &output, &cancellation)
                .is_err()
        );
    }

    #[test]
    fn explicit_configuration_can_select_parent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "").unwrap();
        let root = directory.path().join("documents");
        std::fs::create_dir(&root).unwrap();
        let (sink, _) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        let mut configuration = source_configuration(
            ConfigFileArgs { path: Some(path) },
            TypstPackageArgs::default(),
            InputScope::Offline,
        )
        .unwrap();
        let ServedWorkspace::Site(site) = configuration
            .load(&root, &[], &output, &BuildCancellation::new())
            .unwrap()
        else {
            panic!("explicit configuration selects its site");
        };
        assert_eq!(
            site.get_root(),
            tola_build::filesystem::normalize_path(directory.path())
        );
    }

    #[test]
    fn conflicting_buffers_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path =
            tola_build::filesystem::normalize_existing_prefix(&directory.path().join("tola.toml"));
        let sources = vec![
            (path.clone(), Arc::from("first")),
            (path.clone(), Arc::from("second")),
        ];
        assert!(configuration_text(&path, &sources).is_err());
    }

    #[test]
    fn diagnostics_name_the_configured_file() {
        let directory = tempfile::tempdir().unwrap();
        let configuration = directory.path().join("site/tola.toml");
        std::fs::create_dir_all(configuration.parent().unwrap()).unwrap();
        std::fs::write(&configuration, "version = ??0.8.0\"\n").unwrap();
        let configured = tola_build::filesystem::normalize_existing_prefix(&configuration);
        let expected = crate::terminal::display_path_as_given(&configured);

        let mut location = Some(Location {
            path: "tola.toml".to_owned(),
            line: Some(1),
            column: Some(11),
            range: None,
            source_lines: vec![SourceLine::new(1, "version = ??0.8.0\"", Some((9, 10)))],
        });
        establish_configuration_path(&mut location, &configured);
        let location = location.expect("a location is preserved");
        assert_eq!(location.path, expected);
        assert_eq!(location.line, Some(1));
        assert_eq!(location.column, Some(11));
        assert_eq!(location.source_lines.len(), 1);

        let mut absolute = Some(Location {
            path: expected.clone(),
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
        });
        establish_configuration_path(&mut absolute, &configured);
        assert_eq!(absolute.expect("a location is preserved").path, expected);

        let mut absent: Option<Location> = None;
        establish_configuration_path(&mut absent, &configured);
        assert!(absent.is_none());
        assert_eq!(absolute_configuration_path(&configuration), expected);
    }

    /// A `--pure` session that names a package path is refused by the check that reads the
    /// packages, never by the process: an editor whose server refuses to start sees a closed
    /// connection, while the author needs the reason and the session needs to keep answering.
    #[test]
    fn pure_session_starts_despite_package_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "version = \"0.8.0\"\n").unwrap();
        let configuration = source_configuration(
            ConfigFileArgs { path: Some(path) },
            TypstPackageArgs {
                package_path: Some(directory.path().join("vendor/typst-packages")),
                ..Default::default()
            },
            InputScope::Pure,
        )
        .unwrap();
        assert_eq!(configuration.scope, InputScope::Pure);
        assert!(configuration.loaded.is_none());
    }
}
