//! Formatting the site's Typst sources.

use std::path::Path;

use anyhow::Result;
use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_typst_syntax::format::{self, Formatting, Options};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::run::REPORT_URL;
use crate::cli::{ConfigFileArgs, TypstPackageArgs};
use tola_build::InputScope;

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    check: bool,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let loaded = crate::cli::config::load_site_or_defaults(
        &config_file,
        &packages,
        scope,
        output,
        cancellation,
    )?;
    let config = loaded.config();
    let root = config.get_root();
    let formatter = config.formatter();
    let options = Options {
        tab_spaces: formatter.indent,
        max_width: formatter.print_width,
        prose_wrap: formatter.prose_wrap,
    };
    // The sources are formatted in place: every write below carries the bytes this run read, so a
    // file an editor changed meanwhile is never overwritten.
    let files = tola_build::filesystem::owned_files(config, &cancellation.token())?;
    let mut writes = crate::writes::FileWrites::new(root)?;
    let mut diagnostics = Vec::new();
    let mut examined = 0;
    let mut reformatted = 0;
    for source in files.iter().filter(|path| is_typst_source(path)) {
        cancellation.token().ensure_active()?;
        let relative = source
            .strip_prefix(root)
            .expect("the site walk stays inside the site root");
        let Some(original) = writes.read_file(relative)? else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&original) else {
            diagnostics.push(unreadable(source, root));
            continue;
        };
        examined += 1;
        match format::format(text, &options) {
            Formatting::Stands => {}
            Formatting::Reformatted(edit) => {
                let mut formatted = text.to_owned();
                formatted.replace_range(edit.range, &edit.text);
                reformatted += 1;
                if check {
                    diagnostics.push(unformatted(source, root, text, &formatted));
                } else {
                    writes.replace_file_if_unchanged(relative, Some(original), formatted)?;
                }
            }
            Formatting::Unparsable => diagnostics.push(unparsable(source, root)),
            Formatting::Unrenderable => diagnostics.push(unrenderable(source, root)),
        }
    }
    if !check && reformatted > 0 {
        writes.check()?;
        writes.apply(&cancellation.token())?;
    }
    output.summary(run_summary(check, examined, reformatted))?;
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(DiagnosticError::new("formatting did not complete", diagnostics).into());
    }
    Ok(())
}

fn is_typst_source(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "typ")
}

fn unreadable(path: &Path, root: &Path) -> Diagnostic {
    let source = tola_build::filesystem::display_path(path, root);
    Diagnostic::at_path(
        crate::codes::fmt::UNREADABLE,
        Severity::Error,
        source.clone(),
        format!("Tola could not read `{source}` as UTF-8 text"),
    )
    .with_help("Save the file as UTF-8, then run `tola fmt` again")
}

fn unparsable(path: &Path, root: &Path) -> Diagnostic {
    let source = tola_build::filesystem::display_path(path, root);
    Diagnostic::at_path(
        crate::codes::fmt::UNFORMATTABLE,
        Severity::Error,
        source.clone(),
        format!("Tola could not format `{source}`"),
    )
    .with_note("Tola left it unchanged")
    .with_help("Fix the syntax, then run `tola fmt` again")
}

fn unrenderable(path: &Path, root: &Path) -> Diagnostic {
    let source = tola_build::filesystem::display_path(path, root);
    Diagnostic::at_path(
        crate::codes::fmt::UNFORMATTABLE,
        Severity::Error,
        source.clone(),
        format!("Tola could not format `{source}`"),
    )
    .with_note("Tola left it unchanged")
    .with_help(format!("Report this at {REPORT_URL} with the file"))
}

fn unformatted(path: &Path, root: &Path, before: &str, after: &str) -> Diagnostic {
    let source = tola_build::filesystem::display_path(path, root);
    let diagnostic = Diagnostic::at_path(
        crate::codes::fmt::UNFORMATTED,
        Severity::Error,
        source.clone(),
        format!("`{source}` is not formatted"),
    )
    .with_help("Run `tola fmt` to format this source");
    match first_difference(before, after) {
        Some((line, column)) => diagnostic.with_position(line, column),
        None => diagnostic,
    }
}

/// The one-based position the formatted text first differs at, when the two texts differ.
fn first_difference(before: &str, after: &str) -> Option<(usize, usize)> {
    let mut before = before.chars();
    let mut after = after.chars();
    let mut line = 1;
    let mut column = 1;
    loop {
        match (before.next(), after.next()) {
            (Some(left), Some(right)) if left == right => {
                if left == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            (None, None) => return None,
            _ => return Some((line, column)),
        }
    }
}

fn run_summary(check: bool, examined: usize, reformatted: usize) -> String {
    match (check, examined, reformatted) {
        (_, 0, _) => "No sources to format".to_owned(),
        (true, _, 0) => format!("All {} are formatted", describe_sources(examined)),
        (true, _, _) => format!("{} are not formatted", describe_sources(reformatted)),
        (false, _, 0) => "No source needed formatting".to_owned(),
        (false, _, _) => format!("Formatted {} of {examined}", describe_sources(reformatted)),
    }
}

fn describe_sources(count: usize) -> String {
    format!("{count} {}", if count == 1 { "source" } else { "sources" })
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

    fn source_text(root: &Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative)).unwrap()
    }

    #[test]
    fn formats_sources_across_the_site() {
        let (directory, config) = site(&[
            ("content/index.typ", "#let  x =  1\n"),
            ("templates/page.typ", "#let  name =  \"page\"\n"),
        ]);
        let root = directory.path();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            false,
            &output,
            &Cancellation::default(),
        )
        .unwrap();

        assert_eq!(source_text(root, "content/index.typ"), "#let x = 1\n");
        assert_eq!(
            source_text(root, "templates/page.typ"),
            "#let name = \"page\"\n"
        );
        assert_eq!(source_text(root, "site.typ"), "= Home\n");
    }

    #[test]
    fn formatted_sources_are_left_alone() {
        let (directory, config) = site(&[("templates/page.typ", "#let name = \"page\"\n")]);
        let root = directory.path();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            false,
            &output,
            &Cancellation::default(),
        )
        .unwrap();

        assert_eq!(
            source_text(root, "templates/page.typ"),
            "#let name = \"page\"\n"
        );
    }

    #[test]
    fn check_reports_without_writing() {
        let (directory, config) = site(&[("content/index.typ", "#let  x =  1\n")]);
        let root = directory.path();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        let error = run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            true,
            &output,
            &Cancellation::default(),
        )
        .unwrap_err();

        assert_eq!(source_text(root, "content/index.typ"), "#let  x =  1\n");
        let diagnostics =
            tola_build::diagnostic::attached(&error).expect("the check names sources");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, crate::codes::fmt::UNFORMATTED);
        let location = diagnostics[0].location.as_ref().unwrap();
        assert_eq!(location.path, "content/index.typ");
    }

    #[test]
    fn formatted_sources_pass_check() {
        let (directory, config) = site(&[("content/index.typ", "#let x = 1\n")]);
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            true,
            &output,
            &Cancellation::default(),
        )
        .unwrap();

        assert_eq!(
            source_text(directory.path(), "content/index.typ"),
            "#let x = 1\n"
        );
    }

    #[test]
    fn vendor_and_published_output_are_left_alone() {
        let (directory, config) = site(&[
            ("vendor/package/lib.typ", "#let  vendored =  1"),
            ("public/index.typ", "#let  published =  1"),
            (".tola/state.typ", "#let  internal =  1"),
        ]);
        let root = directory.path();
        std::fs::write(
            root.join("tola.toml"),
            "[site]\norigin = \"https://example.com\"\n[vendor]\npath = \"vendor\"\n",
        )
        .unwrap();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            false,
            &output,
            &Cancellation::default(),
        )
        .unwrap();

        assert_eq!(
            source_text(root, "vendor/package/lib.typ"),
            "#let  vendored =  1"
        );
        assert_eq!(
            source_text(root, "public/index.typ"),
            "#let  published =  1"
        );
        assert_eq!(source_text(root, ".tola/state.typ"), "#let  internal =  1");
    }

    #[test]
    fn unparsable_source_is_reported_and_others_are_formatted() {
        let (directory, config) = site(&[
            ("content/broken.typ", "#let = "),
            ("content/index.typ", "#let  y =  2\n"),
        ]);
        let root = directory.path();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        let error = run(
            config,
            TypstPackageArgs::default(),
            InputScope::Online,
            false,
            &output,
            &Cancellation::default(),
        )
        .unwrap_err();

        let diagnostics = tola_build::diagnostic::attached(&error).expect("the failure is stated");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, crate::codes::fmt::UNFORMATTABLE);
        assert!(diagnostics[0].message.contains("content/broken.typ"));
        assert_eq!(source_text(root, "content/broken.typ"), "#let = ");
        assert_eq!(source_text(root, "content/index.typ"), "#let y = 2\n");
    }

    #[test]
    fn differing_position_names_the_line_and_column() {
        assert_eq!(first_difference("a\nb\n", "a\nc\n"), Some((2, 1)));
        assert_eq!(first_difference("a\nb", "a\nb\n"), Some((2, 2)));
        assert_eq!(first_difference("same\n", "same\n"), None);
    }
}
