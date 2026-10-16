use crate::cancellation::Cancellation;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tola_build::diagnostic::{DiagnosticError, Severity};
use tola_build::has_typ_extension;

use crate::cli::output::CommandOutput;
use crate::cli::{BuildOverrideArgs, ConfigFileArgs, TypstPackageArgs};
use crate::config::ConfigOverrides;
use tola_build::InputScope;

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    build: BuildOverrideArgs,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        ..ConfigOverrides::default()
    };
    let started = Instant::now();
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let config = Arc::new(loaded.into_config());
    output.status("Checking site…")?;
    output.hook_overview(
        &config.build().hooks,
        tola_build::build::BuildMode::Production,
        crate::terminal::PRE_PUBLICATION_STAGES,
    )?;
    let locked = super::build_candidate(
        config,
        cancellation,
        tola_build::build::HookExecution::Run,
        resources,
        output,
    )?;
    locked.ensure_fresh(&cancellation.token())?;
    let site = locked.release();
    output.diagnostics(site.diagnostics())?;
    let counts = site.graph().counts();
    output.summary(crate::terminal::site_summary(
        "Checked",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    ))?;
    Ok(())
}

/// Parse every source the site owns, compiling nothing.
///
/// The walk is [`tola_build::filesystem::owned_files`], so this analysis reaches exactly the files
/// `tola fmt` would rewrite. What a source *means* — values, routes, references, output — is the
/// build's to establish, not this.
pub(in crate::cli) fn sources(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    build: BuildOverrideArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        ..ConfigOverrides::default()
    };
    let started = Instant::now();
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let config = loaded.into_config();
    output.status("Reading site sources…")?;
    let root = config.get_root();
    let files = tola_build::filesystem::owned_files(&config, &cancellation.token())?;
    let mut diagnostics = Vec::new();
    let mut examined = 0;
    for source in files.iter().filter(|path| has_typ_extension(path)) {
        cancellation.token().ensure_active()?;
        let Ok(bytes) = std::fs::read(source) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            diagnostics.push(
                tola_build::diagnostic::unreadable_source(
                    crate::codes::check::UNREADABLE,
                    &tola_build::filesystem::display_path(source, root),
                )
                .with_help("Save the file as UTF-8, then run `tola check` again"),
            );
            continue;
        };
        examined += 1;
        if let Some(range) = tola_typst_syntax::errors::first_error_range(text) {
            diagnostics.push(tola_build::diagnostic::unparsable_source(
                crate::codes::check::SYNTAX,
                &tola_build::filesystem::display_path(source, root),
                text,
                Some(range.start),
            ));
        }
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(DiagnosticError::new("source analysis failed", diagnostics).into());
    }
    output.summary(crate::terminal::site_summary(
        "Checked",
        started.elapsed(),
        &crate::terminal::plural_count(examined, "source"),
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(sources: &[(&str, &str)]) -> (tempfile::TempDir, ConfigFileArgs) {
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

    fn buffered_output() -> (CommandOutput, crate::terminal::BufferedOutput) {
        let (sink, captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        (output, captured)
    }

    fn analyse(config_file: ConfigFileArgs, output: &CommandOutput) -> Result<()> {
        sources(
            config_file,
            TypstPackageArgs::default(),
            InputScope::Online,
            BuildOverrideArgs::default(),
            output,
            &Cancellation::default(),
        )
    }

    #[test]
    fn broken_source_is_reported_with_its_place() {
        let (directory, config) = site(&[("content/broken.typ", "= Title\n#let = \n")]);
        let (output, _captured) = buffered_output();

        let error = analyse(config, &output).unwrap_err();

        let diagnostics = tola_build::diagnostic::attached(&error).expect("the failure is stated");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, crate::codes::check::SYNTAX);
        let location = diagnostics[0].location.as_ref().expect("a location");
        assert_eq!(location.path, "content/broken.typ");
        assert!(
            location.line.is_some(),
            "the failure is placed: {location:?}"
        );
        assert!(
            !directory.path().join("public").exists(),
            "nothing is built"
        );
    }

    #[test]
    fn readable_sources_pass_and_write_nothing() {
        let (directory, config) = site(&[("content/index.typ", "#let  x =  1\n")]);
        let (output, _captured) = buffered_output();

        analyse(config, &output).unwrap();

        assert_eq!(
            std::fs::read_to_string(directory.path().join("content/index.typ")).unwrap(),
            "#let  x =  1\n"
        );
        assert!(
            !directory.path().join("public").exists(),
            "nothing is built"
        );
    }

    #[test]
    fn state_outside_the_site_sources_is_not_analysed() {
        let (_directory, config) = site(&[
            ("public/broken.typ", "#let = \n"),
            (".tola/broken.typ", "#let = \n"),
        ]);
        let (output, _captured) = buffered_output();

        analyse(config, &output).unwrap();
    }

    #[test]
    fn non_utf8_source_is_reported() {
        let (directory, config) = site(&[]);
        std::fs::write(directory.path().join("content/binary.typ"), [0xff, 0xfe]).unwrap();
        let (output, _captured) = buffered_output();

        let error = analyse(config, &output).unwrap_err();

        let diagnostics = tola_build::diagnostic::attached(&error).expect("the failure is stated");
        assert_eq!(diagnostics[0].code, crate::codes::check::UNREADABLE);
        let location = diagnostics[0].location.as_ref().expect("a location");
        assert_eq!(location.path, "content/binary.typ");
    }
}
