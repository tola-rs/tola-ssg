//! One module per command, and the request every site command receives.
//!
//! A command resolves intent, calls the library that owns the work, and reports the outcome
//! through `output::CommandOutput`; build semantics stay in `tola-build`, and policy only a
//! command can decide stays here.
pub(super) mod build;
pub(super) mod check;
pub(super) mod completions;
pub(super) mod config;
pub(super) mod dev;
pub(super) mod diagnostic;
pub(super) mod doctor;
pub(super) mod edit;
pub(super) mod editor;
pub(super) mod extensions;
pub(super) mod fmt;
pub(super) mod fonts;
pub(super) mod help;
pub(super) mod init;
pub(super) mod inspect;
pub(super) mod lsp;
pub(super) mod manpage;
pub(super) mod nu;
pub(super) mod preview;
pub(super) mod query;
pub(super) mod read;
pub(super) mod report;
pub(super) mod skill;
pub(super) mod vendor;

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;

/// Whether any diagnostic fails the command that reported it.
pub(super) fn holds_error(diagnostics: &[tola_diagnostic::Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == tola_diagnostic::Severity::Error)
}

/// What every site command receives from the command line: the site it acts on, and the run's
/// borrowed context. Bundling them keeps each command's signature to what it adds on top.
pub(in crate::cli) struct SiteRequest<'a> {
    pub config_file: crate::cli::ConfigFileArgs,
    pub packages: crate::cli::TypstPackageArgs,
    pub scope: tola_build::InputScope,
    pub build: crate::cli::BuildOverrideArgs,
    pub resources: tola_build::BuildResources,
    pub output: &'a CommandOutput,
    pub cancellation: &'a Cancellation,
}

impl<'a> SiteRequest<'a> {
    pub(super) fn new(
        config_file: crate::cli::ConfigFileArgs,
        packages: crate::cli::TypstPackageArgs,
        scope: tola_build::InputScope,
        build: crate::cli::BuildOverrideArgs,
        fonts: crate::cli::CompilerFontArgs,
        output: &'a CommandOutput,
        cancellation: &'a Cancellation,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config_file,
            packages,
            scope,
            build,
            resources: crate::cli::config::build_resources_with_fonts(scope, &fonts)?,
            output,
            cancellation,
        })
    }

    pub(super) fn load_config(
        &self,
        overrides: &crate::config::ConfigOverrides,
    ) -> anyhow::Result<crate::config::LoadedConfig> {
        crate::cli::config::load_site(
            &self.config_file,
            &self.packages,
            self.scope,
            overrides,
            self.output,
            self.cancellation,
        )
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use crate::cli::ConfigFileArgs;
    use crate::cli::output::CommandOutput;

    pub(crate) fn site(sources: &[(&str, &str)]) -> (tempfile::TempDir, ConfigFileArgs) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), "= Home\n").unwrap();
        std::fs::write(
            root.join("tola.toml"),
            "[site]\norigin = \"https://example.com\"\n",
        )
        .unwrap();
        for (path, contents) in sources {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let config = ConfigFileArgs {
            path: Some(root.join("tola.toml")),
        };
        (directory, config)
    }

    pub(crate) fn buffered_output() -> CommandOutput {
        CommandOutput::new(
            crate::terminal::Terminal::with_sink(
                crate::terminal::OutputSink::buffered().0,
                false,
                false,
            ),
            None,
        )
    }
}
