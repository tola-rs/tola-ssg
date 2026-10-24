//! Command-line arguments and parsing.

use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tola_build::InputScope;

use crate::editor::Editor;
use crate::i18n::HelpLanguage;

#[derive(Parser, Debug, Clone)]
#[command(version = version_text(), about, long_about = None, arg_required_else_help = true, disable_help_subcommand = true)]
pub struct Cli {
    /// Choose when to use colored output.
    #[arg(long, global = true, default_value = "auto", display_order = 100)]
    pub color: ColorChoice,

    /// Show debug events; repeat for trace events.
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count, display_order = 101)]
    pub verbose: u8,

    /// Hide progress, but keep warnings and errors.
    #[arg(short = 'q', long, global = true, display_order = 102)]
    pub quiet: bool,

    /// Append detailed logs to a file
    #[arg(long, global = true, value_hint = clap::ValueHint::FilePath, display_order = 103)]
    pub log_file: Option<PathBuf>,

    /// Do not record a detailed session log.
    #[arg(long, global = true, conflicts_with = "log_file", display_order = 104)]
    pub no_log_file: bool,

    /// Use no network; packages, icons, and fonts already on this machine still apply.
    #[arg(long, global = true, display_order = 105)]
    pub offline: bool,

    /// Build only from the site's own sources and its `[vendor]` inputs.
    ///
    /// Also refuses host package roots, host caches, system fonts, and source files outside the
    /// site. This is the check CI runs: freeze the site's external inputs with `tola vendor` on a
    /// machine with network access, commit them, then `tola build --pure` there.
    #[arg(long, global = true, display_order = 106)]
    pub pure: bool,

    /// Write `tola help` documentation in `en` (default) or `zh`.
    #[arg(long, global = true, value_name = "LANGUAGE", display_order = 107, value_parser = parse_help_language)]
    pub lang: Option<HelpLanguage>,

    /// Do not page `tola help` documentation.
    #[arg(long, global = true, display_order = 108)]
    pub no_pager: bool,

    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    pub fn input_scope(&self) -> InputScope {
        if self.pure {
            InputScope::Pure
        } else if self.offline {
            InputScope::Offline
        } else {
            InputScope::Online
        }
    }
}

/// The `--version` text: the Tola release that wrote this executable and the Typst release it
/// compiles with.
fn version_text() -> String {
    format!(
        "{}\ntypst {}",
        env!("CARGO_PKG_VERSION"),
        typst::utils::version().raw()
    )
}

/// The language one `--lang` value names.
fn parse_help_language(value: &str) -> Result<HelpLanguage, String> {
    HelpLanguage::parse(value).ok_or_else(|| {
        format!("`{value}` is not a language Tola writes `tola help` in; use `en` or `zh`")
    })
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Fetch remote modules declared by the site.
    Module {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(subcommand)]
        command: ModuleCommand,
    },

    /// Create a new Tola site.
    Init(InitArgs),

    /// Build the site and write its output files.
    Build {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        build: BuildOverrideArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        /// Print this build's result and diagnostics as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Build and serve the site, rebuilding when files change.
    Dev {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        build: BuildOverrideArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        development: DevelopmentArgs,
    },

    /// Build once and serve a local preview.
    ///
    /// Uses production build settings without running after-publish commands.
    Preview {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        build: BuildOverrideArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        server: ServerArgs,
    },

    /// Build and check the complete site without committing output files.
    ///
    /// Runs before-build and generate-outputs hooks. These trusted
    /// hook scripts may generate source files. Does not run after-publish commands.
    Check {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        build: BuildOverrideArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        /// Analyze the site's own sources without building anything.
        ///
        /// Every `.typ` file the site owns is parsed, and the sources that do not parse are
        /// reported. It compiles nothing, so it proves only that the sources parse — not imports,
        /// evaluation, routes, or output.
        #[arg(long)]
        no_build: bool,
        /// Skip build hooks when examining a candidate site.
        #[arg(long, conflicts_with = "no_build")]
        no_hooks: bool,
        /// Print this check's result and diagnostics as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Format the site's Typst sources.
    Fmt {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        /// Report the sources that are not formatted, without writing them.
        #[arg(long)]
        check: bool,
    },

    /// Inspect source files, resources, and resolved configuration.
    Inspect {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Evaluate a model query declared by a module or an explicit Typst file.
    Query {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        query: QueryArgs,
    },

    /// Read project text and check or apply changes with explicit input digests.
    Edit {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(subcommand)]
        command: super::commands::edit::EditCommand,
    },

    /// Freeze the packages this site's build used into its own vendor directory.
    Vendor {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        vendor: VendorArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
    },

    /// Check site configuration, source paths, and local tools.
    Doctor(DoctorArgs),

    /// Configure or display editor integration.
    Editor {
        #[command(subcommand)]
        command: EditorCommand,
    },

    /// Provide Tola source diagnostics and language queries over stdio.
    ///
    /// A workspace that holds no `tola.toml` is served as the documents it holds.
    Lsp {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
    },

    /// Print or export the Tola site authoring skill.
    Skill(SkillArgs),

    /// Show resolved site paths and selected settings as JSON.
    Config {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        build: BuildOverrideArgs,
    },

    /// Write a shell completion script to stdout.
    Completions(CompletionsArgs),

    /// Generate a roff manual page.
    Manpage(ManpageArgs),

    /// Read configuration and package documentation.
    Help(HelpArgs),
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct HelpArgs {
    /// `"[TABLE]"`, `"@tola/NAME" [EXPORT...]`, or a `tola-help://` address.
    #[arg(value_name = "TARGET", num_args = 0..)]
    pub targets: Vec<String>,

    /// Browse documentation and source files in the terminal.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Leave mouse selection to the terminal; use the keyboard to navigate. The reader's `c`
    /// key hands the pointer over and back.
    #[arg(long)]
    pub no_mouse: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    #[value(name = "powershell")]
    PowerShell,
    Zsh,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ManpageArgs {
    /// Write the generated roff document to a file instead of stdout.
    #[arg(short, long, value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SkillArgs {
    /// Topic followed by heading names, such as `math rendering svg`.
    #[arg(value_name = "SECTION", num_args = 0..)]
    pub sections: Vec<String>,

    /// List the selected topic and its heading paths without loading their bodies.
    #[arg(long, conflicts_with = "all")]
    pub list: bool,

    /// Read the selected topic or section with all its subsections.
    #[arg(long)]
    pub all: bool,

    /// Write the selected Markdown to `FILE` instead of stdout.
    ///
    /// Creates missing directories and never overwrites existing files.
    /// No site configuration or agent client setup is required.
    #[arg(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct InitArgs {
    /// Site directory; omit to initialize the current directory.
    #[arg(value_hint = clap::ValueHint::DirPath)]
    pub path: Option<PathBuf>,

    /// Preview the site files and configuration without creating them.
    #[arg(long)]
    pub dry_run: bool,

    /// Allow a non-empty directory; existing files are never overwritten.
    #[arg(long)]
    pub force: bool,

    /// Use the initializer declared by a local module.
    #[arg(long, value_name = "MODULE", value_hint = clap::ValueHint::DirPath)]
    pub from: Option<PathBuf>,

    /// Name the module in the new site; defaults to its manifest name.
    #[arg(long, value_name = "NAME", requires = "from")]
    pub alias: Option<String>,

    /// Confirm the starter or enable the module's interactive setup.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Arguments for the module initializer, following `--`.
    #[arg(last = true, value_name = "ARGS", requires = "from")]
    pub args: Vec<OsString>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ConfigFileArgs {
    /// Site configuration file.
    ///
    /// Without this option, commands search the current and parent directories for `tola.toml`.
    /// The Typst language server instead checks only the client's workspace root.
    #[arg(
        short = 'C',
        long = "config",
        global = true,
        value_hint = clap::ValueHint::FilePath
    )]
    pub path: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct TypstPackageArgs {
    /// Local Typst package directory on this host, searched after the site's vendored packages.
    ///
    /// It cannot replace what `{vendor}/typst-packages` already carries; `tola vendor --refresh`
    /// re-selects without reading that copy.
    #[arg(long, global = true, value_hint = clap::ValueHint::DirPath)]
    pub package_path: Option<PathBuf>,

    /// Typst package cache directory on this host, searched after the local package directory.
    #[arg(long, global = true, value_hint = clap::ValueHint::DirPath)]
    pub package_cache_path: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildOverrideArgs {
    /// Override the configured published directory for this build.
    #[arg(short = 'o', long, value_hint = clap::ValueHint::DirPath)]
    pub publish_dir: Option<PathBuf>,

    /// Enable or disable HTML, CSS, and JavaScript minification; omit the value for true.
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub minify: Option<bool>,

    /// Override the site origin, such as <https://example.com> (no path).
    #[arg(long, value_hint = clap::ValueHint::Url)]
    pub origin: Option<String>,

    /// Override the deployment URL path, such as /docs/.
    #[arg(long = "base-path")]
    pub base_path: Option<String>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ServerArgs {
    /// IP address to bind, such as 127.0.0.1 or 0.0.0.0.
    #[arg(short, long)]
    pub interface: Option<std::net::IpAddr>,

    /// HTTP port; use 0 to let the operating system choose.
    #[arg(short, long)]
    pub port: Option<u16>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DevelopmentArgs {
    #[command(flatten)]
    pub server: ServerArgs,

    /// Enable or disable automatic rebuilds; omit the value for true.
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub watch: Option<bool>,

    /// Run every reusable generator on each rebuild; disable `dev = "reuse"` for this session.
    #[arg(long)]
    pub no_reuse: bool,

    /// Print development events as JSON lines.
    ///
    /// Writes one event per line to stdout; logs and hook output go to stderr.
    /// Events report server readiness, builds, after-publish hooks, and shutdown.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum InspectCommand {
    /// List discovered content source files without evaluating their metadata.
    Sources(SourceInspectArgs),
    /// List configured icon collections or the names in one collection.
    Icons(IconInspectArgs),
    /// List the fonts available to the Typst compiler, independently of browser CSS fonts.
    Fonts(JsonOutputArgs),
    /// Show resolved configuration, module choices, and site paths.
    Config(InspectConfigArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct QueryArgs {
    /// Module-qualified query name or an explicit ./path.typ with --label.
    #[arg(value_name = "TARGET")]
    pub target: String,
    /// Metadata label to read from an explicit Typst query file.
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    #[command(flatten)]
    pub options: super::commands::query::QueryOptions,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct JsonOutputArgs {
    /// Indent the JSON output.
    #[arg(long)]
    pub pretty: bool,
    /// Write the ordinary JSON result without opening an interactive browser.
    #[arg(long, value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
    /// Browse the result in a terminal; Enter returns the selected value, q closes.
    #[arg(short = 'i', long)]
    pub interactive: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct InspectConfigArgs {
    #[command(flatten)]
    pub build: BuildOverrideArgs,
    #[command(flatten)]
    pub display: JsonOutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct IconInspectArgs {
    /// Configured namespace whose icon names should be listed.
    #[arg(value_name = "NAMESPACE", conflicts_with = "presets")]
    pub namespace: Option<String>,
    /// List supported preset collections without loading a site.
    #[arg(long)]
    pub presets: bool,
    #[command(flatten)]
    pub display: JsonOutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SourceInspectArgs {
    /// Content source files or directories; use - to read paths from stdin.
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub display: JsonOutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub config: ConfigFileArgs,

    #[command(flatten)]
    pub packages: TypstPackageArgs,

    /// Print environment and diagnostics as JSON for issue reports.
    #[arg(long)]
    pub json: bool,
}

/// The compiler font inputs one invocation overrides.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct CompilerFontArgs {
    /// Extra compiler font directories for this invocation, searched before `[typst.fonts] paths`.
    ///
    /// Repeat the flag for more than one directory. A relative path resolves against the
    /// invocation directory. `--pure` refuses these.
    #[arg(
        id = "font_paths",
        long = "font-path",
        global = true,
        value_name = "DIR",
        value_hint = clap::ValueHint::DirPath
    )]
    pub paths: Vec<PathBuf>,

    /// Do not use system fonts, even when `[typst.fonts] system` is on.
    #[arg(long = "ignore-system-fonts", global = true)]
    pub ignore_system: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum EditorCommand {
    /// Regenerate editor packages and merge workspace settings.
    Setup {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        /// Editors to configure; omit to choose interactively.
        #[arg(value_enum)]
        editors: Vec<Editor>,
        /// Print the editors Tola configures and the settings any other LSP client needs, then stop.
        #[arg(long)]
        list: bool,
        /// Report the editor files, package inputs, and removals this setup would apply without writing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate editor packages without changing editor settings.
    Packages {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
    },
    /// Print one editor's configuration and where it belongs, without writing files.
    Template {
        /// Editor configuration to print.
        #[arg(value_enum)]
        editor: Editor,
    },
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct VendorArgs {
    /// Resolve without the existing vendor copy, then replace it after validation.
    #[arg(long)]
    pub refresh: bool,

    /// Prepare and validate without replacing vendor; may fetch, cache, and acquire the site lock.
    ///
    /// A replacement interrupted before it finished refuses this dry run and leaves the vendor
    /// trees and the working files unchanged; run `tola vendor` to complete the replacement.
    /// Vendoring runs no hooks and does not publish site output.
    #[arg(long)]
    pub dry_run: bool,
}

impl Cli {
    pub fn parse_for_process() -> Self {
        let arguments = std::env::args_os().collect::<Vec<_>>();
        let color = requested_color(&arguments);
        let stdout_color =
            if crate::terminal::color::enabled(color, crate::terminal::color::Stream::Stdout) {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            };
        let mut command = Self::command().color(stdout_color);
        let matches = command
            .try_get_matches_from_mut(arguments)
            .unwrap_or_else(|error| {
                if error.use_stderr() {
                    let stderr_color = if crate::terminal::color::enabled(
                        color,
                        crate::terminal::color::Stream::Stderr,
                    ) {
                        ColorChoice::Always
                    } else {
                        ColorChoice::Never
                    };
                    error.with_cmd(&command.color(stderr_color)).exit();
                }
                error.exit()
            });
        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
    }
}

impl Commands {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Module { .. } => "module",
            Self::Init(_) => "init",
            Self::Build { .. } => "build",
            Self::Dev { .. } => "dev",
            Self::Preview { .. } => "preview",
            Self::Check { .. } => "check",
            Self::Fmt { .. } => "fmt",
            Self::Inspect { .. } => "inspect",
            Self::Vendor { .. } => "vendor",
            Self::Doctor(_) => "doctor",
            Self::Fonts(_) => "fonts",
            Self::Editor { .. } => "editor",
            Self::Lsp { .. } => "lsp",
            Self::Skill(_) => "skill",
            Self::Config { .. } => "config",
            Self::Completions(_) => "completions",
            Self::Manpage(_) => "manpage",
            Self::Help(_) => "help",
        }
    }

    /// Site configuration this command loads, when it loads one.
    pub(crate) fn config_file(&self) -> Option<&ConfigFileArgs> {
        match self {
            Self::Build { config, .. }
            | Self::Check { config, .. }
            | Self::Fmt { config, .. }
            | Self::Vendor { config, .. }
            | Self::Dev { config, .. }
            | Self::Preview { config, .. }
            | Self::Inspect { config, .. }
            | Self::Lsp { config, .. }
            | Self::Config { config, .. }
            | Self::Editor {
                command: EditorCommand::Setup { config, .. },
            } => Some(config),
            Self::Doctor(args) => Some(&args.config),
            Self::Fonts(args) => Some(&args.config),
            _ => None,
        }
    }

    /// File this command writes to a caller-chosen path, when it writes one.
    pub(crate) fn written_output(&self) -> Option<&Path> {
        match self {
            Self::Manpage(args) => args.output.as_deref(),
            Self::Skill(args) => args.output.as_deref(),
            Self::Inspect { command, .. } => match command {
                InspectCommand::Sources(args) => args.display.output.as_deref(),
                InspectCommand::Icons(args) => args.display.output.as_deref(),
                InspectCommand::Fonts(args) => args.output.as_deref(),
                InspectCommand::Config(args) => args.display.output.as_deref(),
            },
            Self::Query { query, .. } => query.options.display.output.as_deref(),
            _ => None,
        }
    }
}

fn requested_color(arguments: &[OsString]) -> ColorChoice {
    let mut color = ColorChoice::Auto;
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if argument == "--" {
            break;
        }
        let value = if argument == "--color" {
            index += 1;
            arguments.get(index).map(|value| value.to_string_lossy())
        } else {
            argument.strip_prefix("--color=").map(Into::into)
        };
        if let Some(value) = value {
            color = match value.as_ref() {
                "always" => ColorChoice::Always,
                "never" => ColorChoice::Never,
                "auto" => ColorChoice::Auto,
                _ => color,
            };
        }
        index += 1;
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_selection_font_directories_independent() {
        for arguments in [
            &["tola", "inspect", "sources", "content/QHXS.typ"][..],
            &[
                "tola",
                "inspect",
                "--font-path",
                "fonts",
                "sources",
                "content/QHXS.typ",
            ][..],
            &[
                "tola",
                "inspect",
                "sources",
                "content/QHXS.typ",
                "--font-path",
                "fonts",
            ][..],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            let Commands::Inspect {
                command: InspectCommand::Sources(sources),
                fonts,
                ..
            } = cli.command
            else {
                panic!("expected source inspection");
            };
            assert_eq!(sources.paths, [PathBuf::from("content/QHXS.typ")]);
            let expected_fonts = if arguments.contains(&"--font-path") {
                vec![PathBuf::from("fonts")]
            } else {
                Vec::new()
            };
            assert_eq!(fonts.paths, expected_fonts);
        }
    }

    #[test]
    fn extension_arguments_opaque_builtin_parsing() {
        let cli = Cli::try_parse_from([
            "tola",
            "forest",
            "edit",
            "a title with spaces",
            "--",
            "--quiet",
            "$(literal)",
        ])
        .unwrap();
        let Commands::Extension(arguments) = cli.command else {
            panic!("expected declared extension dispatch");
        };
        assert_eq!(
            arguments,
            [
                "forest",
                "edit",
                "a title with spaces",
                "--",
                "--quiet",
                "$(literal)"
            ]
            .map(OsString::from),
        );
        assert!(!cli.quiet);
        for arguments in [
            vec!["tola", "build", "--not-a-build-option"],
            vec!["tola", "inspect", "not-a-projection"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn help_language_follows_explicit_flag() {
        for (arguments, expected) in [
            (vec!["tola", "help"], None),
            (
                vec!["tola", "help", "--lang", "en"],
                Some(HelpLanguage::English),
            ),
            (
                vec!["tola", "--lang", "zh", "help"],
                Some(HelpLanguage::SimplifiedChinese),
            ),
            (
                vec!["tola", "help", "--lang", "zh-Hans"],
                Some(HelpLanguage::SimplifiedChinese),
            ),
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert_eq!(cli.lang, expected);
        }
    }

    fn path_text(path: &Option<PathBuf>) -> String {
        path.as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    /// The three states of a command-line flag as the invocation wrote it.
    fn flag_state(value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "true",
            Some(false) => "false",
            None => "unset",
        }
    }

    #[test]
    fn parsed_lines_reach_their_options() {
        type Shown = fn(&Cli) -> String;
        let cases: &[(&[&str], Shown, &str)] = &[
            (
                &[
                    "tola",
                    "build",
                    "--config",
                    "custom.toml",
                    "--package-path",
                    "typst-packages",
                ],
                |cli| match &cli.command {
                    Commands::Build {
                        config, packages, ..
                    } => format!(
                        "{}/{}",
                        path_text(&config.path),
                        path_text(&packages.package_path)
                    ),
                    _ => panic!("expected build command"),
                },
                "custom.toml/typst-packages",
            ),
            (
                &["tola", "completions", "zsh"],
                |cli| match &cli.command {
                    Commands::Completions(args) => args
                        .shell
                        .to_possible_value()
                        .expect("completion shells have a value name")
                        .get_name()
                        .to_owned(),
                    _ => panic!("expected completions command"),
                },
                "zsh",
            ),
            (
                &["tola", "build", "--minify=false"],
                |cli| match &cli.command {
                    Commands::Build { build, .. } => flag_state(build.minify).to_owned(),
                    _ => panic!("expected build command"),
                },
                "false",
            ),
            (
                &["tola", "build"],
                |cli| match &cli.command {
                    Commands::Build { build, .. } => flag_state(build.minify).to_owned(),
                    _ => panic!("expected build command"),
                },
                "unset",
            ),
            (
                &["tola", "dev", "--minify", "--watch"],
                |cli| match &cli.command {
                    Commands::Dev {
                        build, development, ..
                    } => format!(
                        "{}/{}",
                        flag_state(build.minify),
                        flag_state(development.watch)
                    ),
                    _ => panic!("expected development command"),
                },
                "true/true",
            ),
            (
                &["tola", "manpage", "--output", "tola.1"],
                |cli| match &cli.command {
                    Commands::Manpage(args) => path_text(&args.output),
                    _ => panic!("expected manpage command"),
                },
                "tola.1",
            ),
            (
                &[
                    "tola",
                    "preview",
                    "--interface",
                    "127.0.0.1",
                    "--port",
                    "8080",
                ],
                |cli| match &cli.command {
                    Commands::Preview { server, .. } => format!(
                        "{}/{}",
                        server.interface.expect("preview interface"),
                        server.port.expect("preview port")
                    ),
                    _ => panic!("expected preview command"),
                },
                "127.0.0.1/8080",
            ),
            (
                &["tola", "dev", "--port", "8080", "--watch=false"],
                |cli| match &cli.command {
                    Commands::Dev { development, .. } => format!(
                        "{}/{}",
                        development.server.port.expect("development port"),
                        flag_state(development.watch)
                    ),
                    _ => panic!("expected development command"),
                },
                "8080/false",
            ),
            (
                &[
                    "tola",
                    "check",
                    "--origin",
                    "https://example.com",
                    "--base-path",
                    "/docs/",
                ],
                |cli| match &cli.command {
                    Commands::Check { build, .. } => format!(
                        "{}/{}",
                        build.origin.as_deref().unwrap_or_default(),
                        build.base_path.as_deref().unwrap_or_default()
                    ),
                    _ => panic!("expected check command"),
                },
                "https://example.com//docs/",
            ),
            (
                &[
                    "tola",
                    "inspect",
                    "config",
                    "--publish-dir",
                    "dist",
                    "--minify=false",
                ],
                |cli| match &cli.command {
                    Commands::Inspect {
                        command: InspectCommand::Config(args),
                        ..
                    } => {
                        let build = &args.build;
                        format!(
                            "{}/{}",
                            path_text(&build.publish_dir),
                            flag_state(build.minify)
                        )
                    }
                    _ => panic!("expected config command"),
                },
                "dist/false",
            ),
            (
                &[
                    "tola",
                    "editor",
                    "setup",
                    "--package-path",
                    "typst-packages",
                    "--package-cache-path",
                    "downloaded-packages",
                    "--dry-run",
                    "vscode",
                ],
                |cli| match &cli.command {
                    Commands::Editor {
                        command:
                            EditorCommand::Setup {
                                packages, dry_run, ..
                            },
                    } => format!(
                        "{}/{}/{}",
                        path_text(&packages.package_path),
                        path_text(&packages.package_cache_path),
                        dry_run
                    ),
                    _ => panic!("expected editor setup"),
                },
                "typst-packages/downloaded-packages/true",
            ),
            (
                &["tola", "editor", "template", "neovim"],
                |cli| match &cli.command {
                    Commands::Editor {
                        command: EditorCommand::Template { editor },
                    } => editor
                        .to_possible_value()
                        .expect("editors have a value name")
                        .get_name()
                        .to_owned(),
                    _ => panic!("expected editor template"),
                },
                "neovim",
            ),
            (
                &["tola", "-vv", "build"],
                |cli| cli.verbose.to_string(),
                "2",
            ),
            (
                &["tola", "check", "--offline", "--pure"],
                |cli| format!("{}/{}", cli.offline, cli.pure),
                "true/true",
            ),
            (
                &["tola", "dev", "--log-file", "session.jsonl"],
                |cli| {
                    format!(
                        "{}/{}/{}",
                        path_text(&cli.log_file),
                        cli.verbose,
                        cli.command.name()
                    )
                },
                "session.jsonl/0/dev",
            ),
            (
                &[
                    "tola",
                    "init",
                    "author site",
                    "--from",
                    "theme",
                    "--alias",
                    "forest",
                ],
                |cli| match &cli.command {
                    Commands::Init(args) => format!(
                        "{}/{}/{}",
                        path_text(&args.path),
                        path_text(&args.from),
                        args.alias.as_deref().unwrap_or("")
                    ),
                    _ => panic!("expected init command"),
                },
                "author site/theme/forest",
            ),
        ];

        for (arguments, shown, expected) in cases {
            let cli = Cli::try_parse_from(arguments.iter().copied())
                .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
            assert_eq!(shown(&cli), *expected, "{arguments:?}");
        }
    }

    /// Site options reach the commands that declare them, and no other.
    ///
    /// The commands whose decode the table above does not cover, plus the scope refusals: the
    /// shared option structs are flattened per command, so a missing flatten (or a stray
    /// `global`) changes which command accepts which option.
    #[test]
    fn site_options_stay_scoped_commands() {
        fn parses(argv: &[&str]) -> bool {
            Cli::try_parse_from(argv.iter().copied()).is_ok()
        }
        for argv in [
            &["tola", "fmt", "--config", "x.toml", "--check"][..],
            &["tola", "query", "--package-path", "p", "site.index"][..],
            &["tola", "edit", "--config", "x.toml", "read", "a.typ"][..],
            &["tola", "vendor", "--refresh", "--font-path", "f"][..],
            &["tola", "doctor", "--package-path", "p", "--json"][..],
            &["tola", "lsp", "--config", "x.toml", "--font-path", "f"][..],
            &["tola", "editor", "setup", "--package-path", "p", "vscode"][..],
            &["tola", "nu", "--config", "x.toml", "script.nu"][..],
        ] {
            assert!(parses(argv), "{argv:?} must parse");
        }
        for argv in [
            &["tola", "preview", "--watch=false"][..],
            &["tola", "fmt", "--font-path", "f"][..],
            &["tola", "lsp", "--watch=false"][..],
            &["tola", "edit", "--package-path", "p"][..],
            &["tola", "build", "--interface", "127.0.0.1"][..],
        ] {
            assert!(!parses(argv), "{argv:?} must be refused");
        }
    }

    #[test]
    fn init_arguments_only_after_separator() {
        let cli = Cli::try_parse_from([
            "tola",
            "init",
            "site",
            "--from",
            "theme",
            "--interactive",
            "--",
            "--title",
            "A title",
            "--force",
        ])
        .unwrap();
        let Commands::Init(args) = cli.command else {
            panic!("expected init");
        };
        assert!(args.interactive);
        assert!(!args.force);
        assert_eq!(
            args.args,
            ["--title", "A title", "--force"].map(OsString::from)
        );
        assert!(Cli::try_parse_from(["tola", "init", "--alias", "forest"]).is_err());
        assert!(Cli::try_parse_from(["tola", "init", "--", "author-argument"]).is_err());
    }

    #[test]
    fn log_file_flags_conflict() {
        assert!(
            Cli::try_parse_from([
                "tola",
                "build",
                "--log-file",
                "session.jsonl",
                "--no-log-file",
            ])
            .is_err()
        );
    }

    #[test]
    fn color_read_before_clap_runs() {
        let arguments = ["tola", "dev", "--color=always"]
            .map(OsString::from)
            .to_vec();
        assert_eq!(requested_color(&arguments), ColorChoice::Always);
    }
}
