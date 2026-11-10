//! Independent Tola language service with application-owned configuration and I/O.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::{ConfigSource, ResolvedSiteConfig};
use tola_diagnostic::{DiagnosticError, Location};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{CompilerFontArgs, ConfigFileArgs, TypstPackageArgs};
use tola_input::{InputMode, InputScope};

use tola_lsp::ServedWorkspace;

pub(in crate::cli) fn run(
    config: ConfigFileArgs,
    packages: TypstPackageArgs,
    fonts: CompilerFontArgs,
    mode: InputMode,
    cancellation: &Cancellation,
    output: &CommandOutput,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let resources = crate::cli::config::build_resources_with_fonts(mode, &fonts)?;
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
    tola_lsp::serve(tola_lsp::ServeRequest {
        reader: input,
        writer,
        resources,
        cancellation: token,
        load_configuration: move |root, sources| {
            configuration.load(root, sources, &loader_output, &loader_cancel)
        },
        record_diagnostics: move |diagnostics| {
            diagnostics_output.record_diagnostics(None, diagnostics)
        },
        named_configuration,
        host_sections: crate::config::HOST_SECTIONS,
    })
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
        let file = self.configuration_file(root);
        let report_path = file
            .clone()
            .unwrap_or_else(|| root.join(tola_build::config::loading::CONFIG_FILE_NAME));
        self.serve_workspace(root, file, sources, output, cancellation)
            .map_err(|error| configuration_failure(error, output, root, &report_path, cancellation))
    }

    /// The client's workspace bounds automatic discovery; an explicit --config may name another
    /// site, which CLI commands resolve through.
    fn configuration_file(&self, root: &Path) -> Option<PathBuf> {
        self.named.clone().or_else(|| {
            let path = root.join(tola_build::config::loading::CONFIG_FILE_NAME);
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                _ => Some(path),
            }
        })
    }

    fn serve_workspace(
        &mut self,
        root: &Path,
        file: Option<PathBuf>,
        sources: &[(PathBuf, Arc<str>)],
        output: &CommandOutput,
        cancellation: &BuildCancellation,
    ) -> Result<ServedWorkspace> {
        cancellation.ensure_active()?;
        if self
            .loaded
            .as_ref()
            .is_none_or(|loaded| loaded.file != file)
        {
            self.open(root, file, sources, output, cancellation)?;
        }
        self.adopt_changed_settings(sources, output)
    }

    /// Resolve the configuration source this workspace holds, replacing any earlier one.
    fn open(
        &mut self,
        root: &Path,
        file: Option<PathBuf>,
        sources: &[(PathBuf, Arc<str>)],
        output: &CommandOutput,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        let source = match file.as_deref() {
            Some(path) => match configuration_text(path, sources)? {
                Some(text) => ConfigSource::parse(path, text, self.scope)?,
                None => ConfigSource::load(Some(path), self.scope)?,
            },
            // A workspace that holds no site configuration is served as the documents it holds:
            // the schema's defaults, rooted at its own directory.
            None => ConfigSource::defaults(root, self.scope)?,
        };
        // The check that reads the packages is what resolves them, so a refusal is a diagnostic on
        // a session that keeps answering rather than an exit.
        let packages = crate::cli::config::package_locations(&self.packages, self.scope)?;
        let resolved =
            crate::config::resolve(source, packages, &crate::config::ConfigOverrides::default())?;
        output.apply_diagnostic_limits(resolved.diagnostics());
        crate::cli::log::destination::start_site(output, resolved.config(), cancellation)?;
        let (configuration, _, _, loader) = resolved.into_parts();
        self.loaded = Some(LoadedConfiguration {
            file,
            configuration,
            loader,
        });
        Ok(())
    }

    /// Take the settings the author changed since the session last read the document.
    fn adopt_changed_settings(
        &mut self,
        sources: &[(PathBuf, Arc<str>)],
        output: &CommandOutput,
    ) -> Result<ServedWorkspace> {
        let loaded = self.loaded.as_mut().expect("configuration is loaded");
        let site_root = tola_input::normalize_existing_prefix(
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
        // A workspace that holds no configuration document has no file to re-read: its defaults
        // stand until its author writes one.
        let candidate = match loaded.file.as_deref() {
            Some(path) => match configuration_text(path, sources)? {
                Some(text) => loaded.loader.load_text_candidate(text)?,
                None => loaded.loader.load_candidate()?,
            },
            None => None,
        };
        if let Some(candidate) = candidate {
            if !candidate.has_same_effective_settings(&loaded.configuration)? {
                loaded.configuration = candidate.config();
            }
            output.apply_diagnostic_limits(candidate.diagnostics());
            loaded.loader.acknowledge(&candidate);
        }
        Ok(match loaded.file {
            Some(_) => ServedWorkspace::Site(Arc::clone(&loaded.configuration)),
            None => ServedWorkspace::Documents(Arc::clone(&loaded.configuration)),
        })
    }
}

/// Report a configuration failure as the diagnostics the author acts on.
///
/// Configuration diagnostics name a file relative to the configuration's own directory, which the
/// language service would resolve against its workspace root.
fn configuration_failure(
    error: anyhow::Error,
    output: &CommandOutput,
    root: &Path,
    report_path: &Path,
    cancellation: &BuildCancellation,
) -> anyhow::Error {
    crate::cli::log::destination::start_after_error(output, root, cancellation);
    let diagnostics = match tola_diagnostic::attached(&error) {
        Some(attached) => {
            let mut diagnostics = attached.to_vec();
            for diagnostic in &mut diagnostics {
                establish_configuration_path(&mut diagnostic.location, report_path);
            }
            diagnostics
        }
        None => {
            let diagnostic = tola_diagnostic::fallback(crate::codes::editor::CONFIGURATION, &error)
                .with_path(absolute_configuration_path(report_path));
            vec![diagnostic]
        }
    };
    DiagnosticError::attach(error, diagnostics).into()
}

fn absolute_configuration_path(path: &Path) -> String {
    crate::terminal::display_path_as_given(&tola_input::normalize_existing_prefix(path))
}

fn establish_configuration_path(location: &mut Option<Location>, config_path: &Path) {
    let Some(location) = location else {
        return;
    };
    if Path::new(&location.path).is_absolute() {
        return;
    }
    let base = config_path.parent().unwrap_or(config_path);
    location.path = crate::terminal::display_path_as_given(&tola_input::normalize_existing_prefix(
        &base.join(&location.path),
    ));
}

fn configuration_text<'a>(
    path: &Path,
    sources: &'a [(PathBuf, Arc<str>)],
) -> Result<Option<&'a str>> {
    let physical = tola_input::normalize_existing_prefix(path);
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
            tola_input::normalize_path(directory.path())
        );
    }

    #[test]
    fn conflicting_buffers_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = tola_input::normalize_existing_prefix(&directory.path().join("tola.toml"));
        let sources = vec![
            (path.clone(), Arc::from("first")),
            (path.clone(), Arc::from("second")),
        ];
        assert!(configuration_text(&path, &sources).is_err());
    }
}
