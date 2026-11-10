//! Command-line arguments and parsing.

use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;
use tola_input::{InputMode, InputScope};

use crate::editor::Editor;
use crate::i18n::HelpLanguage;

// Headings are set per argument: `next_help_heading` on a flattened struct is a command
// setting, so it leaks into arguments the caller adds after the flatten (clap 4.0, #4222).
#[derive(clap::Args, Debug, Clone)]
pub struct ProcessArgs {
    /// Choose when to use colored output.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        default_value = "auto",
        display_order = 100
    )]
    pub color: ColorChoice,

    /// Show debug events; repeat for trace events.
    #[arg(short = 'v', long, global = true, help_heading = "Global options", action = clap::ArgAction::Count, display_order = 101)]
    pub verbose: u8,

    /// Hide progress, but keep warnings and errors.
    #[arg(
        short = 'q',
        long,
        global = true,
        help_heading = "Global options",
        display_order = 102
    )]
    pub quiet: bool,

    /// Append detailed logs to a file
    #[arg(long, global = true, help_heading = "Global options", value_hint = clap::ValueHint::FilePath, display_order = 103)]
    pub log_file: Option<PathBuf>,

    /// Do not record a detailed session log.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        conflicts_with = "log_file",
        display_order = 104
    )]
    pub no_log_file: bool,
}

/// The input scope and lock policy one command runs under.
#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct InputArgs {
    /// Use no network; packages, icons, and fonts already on this machine still apply.
    #[arg(long, help_heading = "Input options", display_order = 105)]
    pub offline: bool,

    /// Build only from the site's own sources and its `[vendor]` inputs.
    ///
    /// Also refuses host package roots, host caches, system fonts, and source files outside the
    /// site. This is the check CI runs: freeze the site's external inputs with `tola vendor` on a
    /// machine with network access, commit them, then `tola build --pure` there.
    #[arg(long, help_heading = "Input options", display_order = 106)]
    pub pure: bool,

    /// Refuse to write `tola.lock`; fail when a declaration no longer matches it.
    ///
    /// `--pure` implies this.
    #[arg(long, help_heading = "Input options", display_order = 107)]
    pub locked: bool,
}

/// The site a command reads: its configuration and the host package roots that override it.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct SiteArgs {
    #[command(flatten)]
    pub config: ConfigFileArgs,

    #[command(flatten)]
    pub packages: TypstPackageArgs,
}

/// The build overrides and compiler font inputs a build accepts.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildArgs {
    #[command(flatten)]
    pub overrides: BuildOverrideArgs,

    #[command(flatten)]
    pub fonts: CompilerFontArgs,
}

impl BuildArgs {
    pub(super) fn from_fonts(fonts: CompilerFontArgs) -> Self {
        Self {
            fonts,
            ..Self::default()
        }
    }
}

#[derive(Parser, Debug, Clone)]
#[command(version = version_text(), about, long_about = None, disable_help_subcommand = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: ProcessArgs,

    #[command(subcommand)]
    pub command: Commands,
}

impl InputArgs {
    pub(super) fn scope(self) -> InputScope {
        if self.pure {
            InputScope::Pure
        } else if self.offline {
            InputScope::Offline
        } else {
            InputScope::Online
        }
    }

    /// What this invocation may write, fetch, and read for its inputs.
    pub(super) fn mode(self) -> InputMode {
        InputMode::new(self.scope(), self.locked)
    }
}

/// The `--version` text: the Tola release that wrote this executable, the Typst
/// release it compiles with, and the embedded Nu release.
fn version_text() -> String {
    format!(
        "{}\ntypst {}\nnu {}",
        env!("CARGO_PKG_VERSION"),
        typst::utils::version().raw(),
        tola_nu::NU_VERSION
    )
}

/// The language one `--lang` value names.
fn parse_help_language(value: &str) -> Result<HelpLanguage, String> {
    HelpLanguage::parse(value).ok_or_else(|| {
        format!("`{value}` is not a language Tola writes `tola help` in; use `en` or `zh`")
    })
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModuleCommand {
    /// Check the manifest and declared files without running any code.
    Check {
        /// Module directory.
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        directory: PathBuf,
    },
    /// Run hooks.before-pack and write a checked release directory.
    Pack {
        /// Source module directory.
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        source: PathBuf,

        /// Replace this directory with the complete release.
        #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
        output: PathBuf,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Refresh the inputs `tola.lock` records.
    Update {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        update: UpdateArgs,
    },

    /// Create a new Tola site.
    Init(InitArgs),

    /// Check or package a standalone module directory.
    Module {
        #[command(subcommand)]
        command: ModuleCommand,
    },

    /// Build the site and write its output files.
    Build {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        /// Print this build's result and diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Indent the JSON result. Requires --json.
        #[arg(long, requires = "json")]
        pretty: bool,
    },

    /// Build and serve the site, rebuilding when files change.
    Dev {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        #[command(flatten)]
        development: DevelopmentArgs,
    },

    /// Build once and serve a local preview.
    ///
    /// Uses production build settings without running after-publish commands.
    Preview {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        #[command(flatten)]
        server: ServerArgs,
    },

    /// Build and check the complete site without committing output files.
    ///
    /// Runs hooks.before-build and hooks.generate-outputs. These trusted
    /// hook scripts may generate source files. Does not run after-publish commands.
    Check {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,

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
        /// Indent the JSON result. Requires --json.
        #[arg(long, requires = "json")]
        pretty: bool,
    },

    /// Format the site's Typst sources.
    Fmt {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        /// Report the sources that are not formatted, without writing them.
        #[arg(long)]
        check: bool,
    },

    /// Inspect source files, resources, and resolved configuration.
    Inspect {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Evaluate a model query declared by a module or an explicit Typst file.
    Query {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        query: QueryArgs,
    },

    /// Read project text and check or apply changes with explicit input digests.
    Edit {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(subcommand)]
        command: super::commands::edit::EditCommand,
    },

    /// Freeze the packages this site's build used into its own vendor directory.
    Vendor {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
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
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
    },

    /// Print or export the Tola site authoring skill.
    Skill(SkillArgs),

    /// Write a shell completion script to stdout.
    Completions(CompletionsArgs),

    /// Generate a roff manual page.
    Manpage(ManpageArgs),

    /// Run the embedded Nushell runtime or its language server.
    #[command(
        override_usage = "tola nu [OPTIONS] <ENTRY> [ARGS]...\n       tola nu --lsp",
        after_help = "`nu` runs the script directly: `-q`, `-v`, `--color`, `--log-file`, `--no-log-file`, `--lang` and `--no-pager` do not apply. `--pure` and `--offline` still set the script's input scope.",
        after_long_help = "Examples:\n  tola nu tools/build.nu\n  tola nu @tola-scripts/image metadata photo.png\n  tola nu @tola-scripts/image resize photo.png --output photo.webp --format webp\n  tola nu @tola-modules/catalog --help\n\nIn Nu, `use tola` provides `tola query`; `use tola query` imports\nthe unqualified `query`. Import `use tola cli` for finite `cli capture`\nand `use tola image` for `image metadata` and `image resize`.\nUse `tola dev --json` as a native external command for continuous JSON lines.\nSite libraries use `use tola-modules catalog` and declare `[nu]` with\n`entry = \"tools/mod.nu\"` in tola-module.toml. Direct module invocations\nrun that entry's exported main command."
    )]
    Nu(NuArgs),

    /// Read configuration and package documentation.
    Help(HelpArgs),

    /// Print the Tola, Typst, and embedded Nu versions.
    Version(VersionArgs),

    /// Display diagnostics from an independent check.
    Diagnostic {
        #[command(subcommand)]
        command: DiagnosticCommand,
    },

    /// Run a command declared by one of this site’s modules.
    #[command(external_subcommand)]
    Extension(Vec<OsString>),
}

#[derive(Subcommand, Debug, Clone)]
pub enum DiagnosticCommand {
    /// Read diagnostic records from stdin and print them. Locations use file paths.
    Render,
}

/// Options for refreshing the inputs `tola.lock` records.
#[derive(clap::Args, Debug, Clone)]
pub struct UpdateArgs {
    /// Module aliases or icon collection names to refresh; all remote inputs when omitted.
    pub names: Vec<String>,

    /// Report what would change without writing `tola.lock`.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct NuArgs {
    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub config: ConfigFileArgs,

    /// Serve the Nushell language server over standard input and output.
    #[arg(long, conflicts_with_all = ["stdin", "invocation", "lsp_worker", "lsp_snapshot"])]
    pub lsp: bool,

    #[arg(long, hide = true, requires = "lsp_snapshot", conflicts_with_all = ["stdin", "invocation", "lsp"])]
    pub lsp_worker: bool,

    #[arg(long, hide = true, requires = "lsp_worker", value_name = "FILE")]
    pub lsp_snapshot: Option<PathBuf>,

    #[arg(long, hide = true, conflicts_with_all = ["path", "lsp", "lsp_worker", "lsp_snapshot"])]
    pub no_site: bool,

    /// Pass standard input to the script.
    #[arg(long)]
    pub stdin: bool,

    /// Script file or @tola-scripts/NAME / @tola-modules/ALIAS, followed by its arguments.
    #[arg(value_name = "ENTRY", num_args = 1.., trailing_var_arg = true, required_unless_present_any = ["lsp", "lsp_worker"])]
    pub invocation: Vec<OsString>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct HelpArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// `"[TABLE]"`, `"@tola-scripts/NAME" [EXPORT...]`, or a `tola-help://` address.
    #[arg(value_name = "TARGET", num_args = 0..)]
    pub targets: Vec<String>,

    /// Browse documentation and source files in the terminal.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Output a readable page, its original Markdown, or structured JSON.
    #[arg(
        long,
        value_enum,
        default_value = "text",
        conflicts_with = "interactive"
    )]
    pub format: HelpFormat,

    /// Write `tola help` documentation in `en` (default) or `zh`.
    #[arg(long, value_name = "LANGUAGE", value_parser = parse_help_language)]
    pub lang: Option<HelpLanguage>,

    /// Do not page `tola help` documentation.
    #[arg(long)]
    pub no_pager: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct VersionArgs {
    /// Print the versions as JSON.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output. Requires --json.
    #[arg(long, requires = "json")]
    pub pretty: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum HelpFormat {
    #[default]
    Text,
    Markdown,
    Json,
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
    #[command(flatten)]
    pub input: InputArgs,

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

    /// Choose the module alias in the new site.
    #[arg(long, value_name = "NAME", requires = "from")]
    pub alias: Option<String>,

    /// Confirm the files to write or enable the module's interactive setup.
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
        help_heading = "Site options",
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
    #[arg(long, help_heading = "Site options", value_hint = clap::ValueHint::DirPath)]
    pub package_path: Option<PathBuf>,

    /// Typst package cache directory on this host, searched after the local package directory.
    #[arg(long, help_heading = "Site options", value_hint = clap::ValueHint::DirPath)]
    pub package_cache_path: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildOverrideArgs {
    /// Override the configured published directory for this build.
    ///
    /// A relative path starts at the site root. Choose a directory inside the site that is empty
    /// or already owned by Tola for this site.
    #[arg(short = 'o', long, help_heading = "Build options", value_hint = clap::ValueHint::DirPath)]
    pub publish_dir: Option<PathBuf>,

    /// Enable or disable HTML, CSS, and JavaScript minification; omit the value for true.
    #[arg(short, long, help_heading = "Build options", num_args = 0..=1, default_missing_value = "true")]
    pub minify: Option<bool>,

    /// Override the site origin, such as <https://example.com> (no path).
    #[arg(long, help_heading = "Build options", value_hint = clap::ValueHint::Url)]
    pub origin: Option<String>,

    /// Override the deployment URL path, such as /docs/.
    #[arg(long = "base-path", help_heading = "Build options")]
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
    #[arg(short, long, num_args = 0..=1, default_missing_value = "true")]
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
    Fonts(OutputArgs),
    /// Show resolved configuration, module choices, and site paths.
    Config(InspectConfigArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct QueryArgs {
    /// Module query such as notes.nodes, or a Typst file with --label.
    ///
    /// Start a relative file path with ./ or ../, or use an absolute path. The file must be inside
    /// the site. For example, `tola query ./query.typ --label result`.
    #[arg(value_name = "TARGET")]
    pub target: String,
    /// Metadata label to read from an explicit Typst query file.
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    #[command(flatten)]
    pub options: super::commands::query::QueryOptions,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct OutputArgs {
    /// Write the machine-readable result as compact JSON.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output. Requires --json.
    #[arg(long, requires = "json")]
    pub pretty: bool,
    /// Write the machine-readable result to this file. Requires --json.
    #[arg(long, requires = "json", value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
    /// Browse the result in a terminal; Enter returns the selected value, q closes.
    #[arg(short = 'i', long, conflicts_with_all = ["json", "output"])]
    pub interactive: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct InspectConfigArgs {
    #[command(flatten)]
    pub build: BuildOverrideArgs,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct IconInspectArgs {
    /// Configured namespace whose icon names should be listed.
    #[arg(value_name = "NAMESPACE")]
    pub namespace: Option<String>,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SourceInspectArgs {
    /// Content source files or directories; use - to read paths from stdin.
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub site: SiteArgs,

    /// Print environment and diagnostics as JSON for issue reports.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output. Requires --json.
    #[arg(long, requires = "json")]
    pub pretty: bool,
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
        help_heading = "Font options",
        value_name = "DIR",
        value_hint = clap::ValueHint::DirPath
    )]
    pub paths: Vec<PathBuf>,

    /// Do not use system fonts, even when `[typst.fonts] system` is on.
    #[arg(long = "ignore-system-fonts", help_heading = "Font options")]
    pub ignore_system: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum EditorCommand {
    /// Regenerate editor packages and merge workspace settings.
    Setup {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,

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
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
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

impl Commands {
    /// Input options this command accepts, when it resolves inputs.
    pub(crate) fn input(&self) -> Option<&InputArgs> {
        match self {
            Self::Update { input, .. }
            | Self::Build { input, .. }
            | Self::Dev { input, .. }
            | Self::Preview { input, .. }
            | Self::Check { input, .. }
            | Self::Fmt { input, .. }
            | Self::Inspect { input, .. }
            | Self::Query { input, .. }
            | Self::Edit { input, .. }
            | Self::Vendor { input, .. }
            | Self::Lsp { input, .. } => Some(input),
            Self::Init(args) => Some(&args.input),
            Self::Doctor(args) => Some(&args.input),
            Self::Nu(args) => Some(&args.input),
            Self::Help(args) => Some(&args.input),
            Self::Editor { command } => match command {
                EditorCommand::Setup { input, .. } | EditorCommand::Packages { input, .. } => {
                    Some(input)
                }
                EditorCommand::Template { .. } => None,
            },
            Self::Module { .. }
            | Self::Skill(_)
            | Self::Version(_)
            | Self::Completions(_)
            | Self::Manpage(_)
            | Self::Diagnostic { .. } => None,
            Self::Extension(_) => None,
        }
    }

    /// Site inputs this command reads, when it reads a site.
    pub(crate) fn site(&self) -> Option<&SiteArgs> {
        match self {
            Self::Update { site, .. }
            | Self::Build { site, .. }
            | Self::Check { site, .. }
            | Self::Fmt { site, .. }
            | Self::Vendor { site, .. }
            | Self::Dev { site, .. }
            | Self::Preview { site, .. }
            | Self::Inspect { site, .. }
            | Self::Edit { site, .. }
            | Self::Lsp { site, .. }
            | Self::Query { site, .. }
            | Self::Editor {
                command: EditorCommand::Setup { site, .. } | EditorCommand::Packages { site, .. },
            } => Some(site),
            Self::Doctor(args) => Some(&args.site),
            _ => None,
        }
    }

    /// Site configuration this command loads, when it loads one.
    pub(crate) fn config_file(&self) -> Option<&ConfigFileArgs> {
        self.site().map(|site| &site.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn font_options_never_become_source_paths() {
        let cli = Cli::try_parse_from([
            "tola",
            "inspect",
            "--font-path",
            "fonts",
            "sources",
            "content/QHXS.typ",
        ])
        .unwrap();
        let Commands::Inspect {
            command: InspectCommand::Sources(sources),
            fonts,
            ..
        } = cli.command
        else {
            panic!("expected source inspection");
        };
        assert_eq!(sources.paths, [PathBuf::from("content/QHXS.typ")]);
        assert_eq!(fonts.paths, [PathBuf::from("fonts")]);
    }

    fn build_override(argv: &[&str]) -> BuildOverrideArgs {
        let cli = Cli::try_parse_from(argv.iter().copied()).unwrap();
        let Commands::Build { build, .. } = cli.command else {
            panic!("expected build");
        };
        build.overrides
    }

    #[test]
    fn minify_reads_an_optional_value() {
        assert_eq!(
            build_override(&["tola", "build", "--minify=false"]).minify,
            Some(false)
        );
        assert_eq!(build_override(&["tola", "build"]).minify, None);
    }

    #[test]
    fn verbosity_counts_repeated_flags() {
        let cli = Cli::try_parse_from(["tola", "-vv", "build"]).unwrap();
        assert_eq!(cli.global.verbose, 2);
    }

    #[test]
    fn init_forwards_arguments_after_separator() {
        let cli = Cli::try_parse_from(["tola", "init", "site", "--from", "theme", "--", "--force"])
            .unwrap();
        let Commands::Init(args) = cli.command else {
            panic!("expected init");
        };
        assert_eq!(args.args, [OsString::from("--force")]);
    }

    #[test]
    fn help_lists_only_the_options_a_command_runs() {
        let command = Cli::command();
        let completions = command.find_subcommand("completions").unwrap();
        for long in ["pure", "lang", "no-pager"] {
            assert!(
                !completions
                    .get_arguments()
                    .any(|argument| argument.get_long() == Some(long)),
                "completions: {long}"
            );
        }
        let help = command.find_subcommand("help").unwrap();
        assert!(
            help.get_arguments()
                .any(|argument| argument.get_long() == Some("lang")),
            "help carries --lang"
        );
    }

    #[test]
    fn input_options_live_on_their_command() {
        let cli = Cli::try_parse_from(["tola", "build", "--locked"]).unwrap();
        assert!(cli.command.input().unwrap().mode().locked());
        assert!(Cli::try_parse_from(["tola", "--pure", "build"]).is_err());
        assert!(Cli::try_parse_from(["tola", "completions", "--pure", "zsh"]).is_err());
    }

    /// Site options reach the commands that declare them, and no other.
    ///
    /// The shared option structs are flattened per command, so a missing flatten (or a stray
    /// `global`) changes which command accepts which option.
    #[test]
    fn site_options_stay_scoped_commands() {
        fn parses(argv: &[&str]) -> bool {
            Cli::try_parse_from(argv.iter().copied()).is_ok()
        }
        for argv in [
            &["tola", "fmt", "--config", "x.toml", "--check"][..],
            &["tola", "inspect", "-C", "x.toml", "sources"][..],
            &["tola", "editor", "setup", "--package-path", "p", "vscode"][..],
        ] {
            assert!(parses(argv), "{argv:?} must parse");
        }
        for argv in [
            &["tola", "fmt", "--font-path", "f"][..],
            &["tola", "inspect", "sources", "-C", "x.toml"][..],
            &["tola", "build", "--interface", "127.0.0.1"][..],
        ] {
            assert!(!parses(argv), "{argv:?} must be refused");
        }
    }
}
