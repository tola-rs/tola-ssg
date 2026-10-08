//! Command-line arguments and parsing.

use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tola_build::InputScope;

use crate::cli::commands::init::{feature_values, preset_values};
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

    /// Write `tola help` documentation in this language.
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

/// Parses `--preset`: the preset names the scaffold model declares, each carrying its terse
/// composition as the value's help.
#[derive(Clone)]
struct PresetValueParser;

impl clap::builder::TypedValueParser for PresetValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        clap::builder::PossibleValuesParser::new(preset_values().map(|(name, _)| name))
            .parse_ref(cmd, arg, value)
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        Some(Box::new(preset_values().map(|(name, composition)| {
            clap::builder::PossibleValue::new(name).help(composition)
        })))
    }
}

/// Parses `--features`: the feature names the scaffold model declares, each carrying its summary
/// as the value's help.
#[derive(Clone)]
struct FeatureValueParser;

impl clap::builder::TypedValueParser for FeatureValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        clap::builder::PossibleValuesParser::new(feature_values().map(|(name, _)| name))
            .parse_ref(cmd, arg, value)
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        Some(Box::new(feature_values().map(|(name, summary)| {
            clap::builder::PossibleValue::new(name).help(summary)
        })))
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
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
    },

    /// Inspect site sources and build results.
    Inspect {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Freeze the packages this site's build used into its own vendor directory.
    Vendor {
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        vendor: VendorArgs,
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

    /// Read configuration, package documentation, and runnable demos.
    Help(HelpArgs),
}

#[derive(clap::Args, Debug, Clone, Default)]
#[command(group(clap::ArgGroup::new("editor_action").args(["edit", "interactive"]).multiple(true)))]
pub struct HelpArgs {
    /// `config TABLE`, `package NAME [EXPORT...]`, `demo NAME [FILE]`, or a `tola-help://` address.
    #[arg(value_name = "TARGET", num_args = 0..)]
    pub targets: Vec<String>,

    /// Browse documentation, source files, and demo actions in the terminal.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Leave mouse selection to the terminal; use the keyboard to navigate. The reader's `c`
    /// key hands the pointer over and back.
    #[arg(long)]
    pub no_mouse: bool,

    /// Build and serve the selected demo locally until stopped.
    #[arg(long, conflicts_with = "export")]
    pub preview: bool,

    /// Export the demo's complete source to a directory that does not yet exist.
    /// Its parent directory must already exist; existing empty directories are refused too.
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub export: Option<PathBuf>,

    /// Open the exported site in your editor; requires --export.
    #[arg(long, requires = "export")]
    pub edit: bool,

    /// Editor command for --edit or interactive exports; otherwise use TOLA_EDITOR, VISUAL, then EDITOR.
    #[arg(long, requires = "editor_action", value_name = "COMMAND")]
    pub editor: Option<String>,
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
    /// Write the skill to `DIR` instead of stdout.
    ///
    /// Creates missing directories and never overwrites existing files.
    /// No site configuration or agent client setup is required.
    #[arg(short, long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
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

    /// Scaffold preset: minimal, medium, or rich.
    ///
    /// Defaults to minimal; `--interactive` recommends rich.
    #[arg(long, value_name = "NAME", value_parser = PresetValueParser)]
    pub preset: Option<String>,

    /// Select individual features instead of a preset; repeat or separate with commas.
    ///
    /// With `--interactive`, the screen selects these features and their dependencies.
    #[arg(
        long,
        value_name = "NAMES",
        value_delimiter = ',',
        value_parser = FeatureValueParser,
        conflicts_with = "preset"
    )]
    pub features: Vec<String>,

    /// Choose the site directory, features, and editors in a terminal.
    #[arg(long)]
    pub interactive: bool,

    /// Write editor settings (Neovim prints instructions); repeat or separate with commas.
    #[arg(long, value_enum, value_delimiter = ',')]
    pub editor: Vec<Editor>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ConfigFileArgs {
    /// Configuration file; omit to search the current and parent directories for `tola.toml`.
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
}

#[derive(Subcommand, Debug, Clone)]
pub enum InspectCommand {
    /// Show declared source metadata as JSON without building the site.
    ///
    /// Sources without metadata are omitted, so a selection with none prints an empty array.
    Sources(SourceInspectArgs),
    /// Show configured icon namespaces as JSON, and one namespace's icon names.
    ///
    /// Remote collections are downloaded and cached exactly as a build downloads them.
    Icons(IconInspectArgs),
    /// Build without publishing and show HTML document paths and properties as JSON.
    Documents(InspectProjectionArgs),
    /// Build without publishing and show document and asset URLs as JSON.
    Routes(InspectProjectionArgs),
    /// Build without publishing and show every output and its producer as JSON.
    Outputs(InspectProjectionArgs),
    /// Build without publishing and show link and resource resolution as JSON.
    References(InspectProjectionArgs),
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct InspectProjectionArgs {
    #[command(flatten)]
    pub build: BuildOverrideArgs,

    /// Browse the rows in a terminal table with filtering, row detail, and export.
    #[arg(long)]
    pub interactive: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct IconInspectArgs {
    /// Icon namespace to list icon names for; omit to list every configured namespace.
    #[arg(value_name = "NAMESPACE")]
    pub namespace: Option<String>,

    /// Browse the rows in a terminal table with filtering, row detail, and export.
    #[arg(long)]
    pub interactive: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SourceInspectArgs {
    /// Source files or directories; omit for all sources.
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub paths: Vec<PathBuf>,

    /// Keep serialized Typst content instead of converting it to plain text.
    #[arg(long)]
    pub raw: bool,

    /// Pretty-print JSON output.
    #[arg(long)]
    pub pretty: bool,

    /// Omit metadata fields containing null, an empty string, or an empty array.
    #[arg(short = 'E', long)]
    pub filter_empty: bool,

    /// Select metadata fields, separated by commas or spaces; the source path is always included.
    #[arg(short, long, value_delimiter = ',', num_args = 1..)]
    pub fields: Option<Vec<String>>,

    /// Write JSON to a file instead of stdout.
    #[arg(short, long, value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,

    /// Browse the rows in a terminal table with filtering, row detail, and export.
    #[arg(long)]
    pub interactive: bool,
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
    /// An installation interrupted before it committed refuses this dry run and leaves vendor and
    /// its recovery material unchanged; run `tola vendor` to restore the previous inputs. Vendoring
    /// runs no hooks and does not publish site output.
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
            Self::Init(_) => "init",
            Self::Build { .. } => "build",
            Self::Dev { .. } => "dev",
            Self::Preview { .. } => "preview",
            Self::Check { .. } => "check",
            Self::Inspect { .. } => "inspect",
            Self::Vendor { .. } => "vendor",
            Self::Doctor(_) => "doctor",
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
            _ => None,
        }
    }

    /// File this command writes to a caller-chosen path, when it writes one.
    pub(crate) fn written_output(&self) -> Option<&Path> {
        match self {
            Self::Manpage(args) => args.output.as_deref(),
            Self::Inspect {
                command: InspectCommand::Sources(args),
                ..
            } => args.output.as_deref(),
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
                &[
                    "tola",
                    "lsp",
                    "--config",
                    "site/tola.toml",
                    "--package-path",
                    "packages",
                ],
                |cli| match &cli.command {
                    Commands::Lsp { config, packages } => format!(
                        "{}/{}",
                        path_text(&config.path),
                        path_text(&packages.package_path)
                    ),
                    _ => panic!("expected language server command"),
                },
                "site/tola.toml/packages",
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
                &["tola", "config", "--publish-dir", "dist", "--minify=false"],
                |cli| match &cli.command {
                    Commands::Config { build, .. } => {
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
                    "inspect",
                    "outputs",
                    "--config",
                    "site/tola.toml",
                    "--package-path",
                    "packages",
                    "--minify=false",
                ],
                |cli| match &cli.command {
                    Commands::Inspect {
                        config,
                        packages,
                        command: InspectCommand::Outputs(args),
                    } => format!(
                        "{}/{}/{}",
                        path_text(&config.path),
                        path_text(&packages.package_path),
                        flag_state(args.build.minify)
                    ),
                    _ => panic!("expected output inspection"),
                },
                "site/tola.toml/packages/false",
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
                    "doctor",
                    "--package-path",
                    "host-packages",
                    "--json",
                ],
                |cli| match &cli.command {
                    Commands::Doctor(args) => {
                        format!("{}/{}", path_text(&args.packages.package_path), args.json)
                    }
                    _ => panic!("expected doctor command"),
                },
                "host-packages/true",
            ),
            (
                &["tola", "init", "--preset", "rich"],
                |cli| match &cli.command {
                    Commands::Init(args) => args.preset.clone().unwrap_or_default(),
                    _ => panic!("expected init command"),
                },
                "rich",
            ),
            (
                &["tola", "init", "--features", "feed,sitemap"],
                |cli| match &cli.command {
                    Commands::Init(args) => args.features.join(","),
                    _ => panic!("expected init command"),
                },
                "feed,sitemap",
            ),
            (
                &[
                    "tola",
                    "init",
                    "--features",
                    "feed",
                    "--features",
                    "sitemap",
                ],
                |cli| match &cli.command {
                    Commands::Init(args) => args.features.join(","),
                    _ => panic!("expected init command"),
                },
                "feed,sitemap",
            ),
        ];

        for (arguments, shown, expected) in cases {
            let cli = Cli::try_parse_from(arguments.iter().copied())
                .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
            assert_eq!(shown(&cli), *expected, "{arguments:?}");
        }
        assert!(Cli::try_parse_from(["tola", "preview", "--watch=false"]).is_err());
    }

    #[test]
    fn unknown_preset_names_are_refused() {
        let error = Cli::try_parse_from(["tola", "init", "--preset", "bogus"]).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("--preset <NAME>"), "{rendered}");
        assert!(
            rendered.contains("[possible values: minimal, medium, rich]"),
            "{rendered}"
        );
    }

    #[test]
    fn preset_help_names_each_preset_with_its_composition() {
        let mut command = Cli::command();
        let init = command
            .find_subcommand_mut("init")
            .expect("init is a subcommand")
            .render_long_help()
            .to_string();
        for (name, composition) in preset_values() {
            assert!(init.contains(name), "{name} missing:\n{init}");
            assert!(init.contains(composition), "{composition} missing:\n{init}");
        }
    }

    #[test]
    fn unknown_feature_names_are_refused() {
        let error = Cli::try_parse_from(["tola", "init", "--features", "feeds"]).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("--features <NAMES>"), "{rendered}");
        assert!(
            rendered.contains(
                "[possible values: starter-stylesheet, canonical, feed, sitemap, open-graph, \
                 twitter-card, pagefind, deno-toolchain, tailwind-css]"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn feature_selection_conflicts_with_the_preset() {
        assert!(
            Cli::try_parse_from(["tola", "init", "--features", "feed", "--preset", "medium"])
                .is_err()
        );
    }

    #[test]
    fn feature_selection_combines_with_the_interactive_screen() {
        assert!(
            Cli::try_parse_from(["tola", "init", "--features", "feed", "--interactive"]).is_ok()
        );
    }

    #[test]
    fn feature_help_names_each_feature_with_its_summary() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("init")
            .expect("init is a subcommand")
            .render_long_help()
            .to_string();
        // The renderer wraps long lines; collapse its layout before matching a summary.
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for (name, summary) in feature_values() {
            assert!(help.contains(name), "{name} missing:\n{help}");
            assert!(help.contains(summary), "{summary} missing:\n{help}");
        }
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
    fn color_flag_is_read_before_clap_runs() {
        let arguments = ["tola", "dev", "--color=always"]
            .map(OsString::from)
            .to_vec();
        assert_eq!(requested_color(&arguments), ColorChoice::Always);
    }
}
