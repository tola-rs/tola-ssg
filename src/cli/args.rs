//! Command-line arguments and parsing.

use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;
use tola_input::{InputMode, InputScope};

use crate::editor::Editor;
use crate::i18n::HelpLanguage;

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ProcessArgs {
    /// Hide progress. Keep warnings and errors.
    #[arg(short, long, help_heading = "Output", display_order = 80)]
    pub quiet: bool,

    /// Show debug logs with `-v` or trace logs with `-vv`.
    #[arg(short, long, action = clap::ArgAction::Count, help_heading = "Output", display_order = 81)]
    pub verbose: u8,

    /// Use `always` or `never` to force color on or off. `auto` follows the terminal.
    #[arg(
        long,
        default_value = "auto",
        value_name = "WHEN",
        help_heading = "Output",
        display_order = 82
    )]
    pub color: ColorChoice,

    /// Append this run's log to `FILE`.
    #[arg(long, value_name = "FILE", value_hint = clap::ValueHint::FilePath, help_heading = "Output", display_order = 83)]
    pub log_file: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct OfflineArgs {
    /// Use local files and cached dependencies. Without this flag, Tola can download missing dependencies.
    #[arg(long, display_order = 40)]
    pub offline: bool,
}

impl OfflineArgs {
    pub(super) fn scope(self) -> InputScope {
        if self.offline {
            InputScope::Offline
        } else {
            InputScope::Online
        }
    }
}

#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct ScopeArgs {
    #[command(flatten)]
    pub offline: OfflineArgs,

    /// Use the site's files, vendored dependencies, and Tola's built-ins.
    ///
    /// Requires matching entries in `tola.lock`. Tola skips external package directories,
    /// caches, and system fonts. Hooks still run with their usual access.
    #[arg(long, display_order = 41)]
    pub pure: bool,
}

impl ScopeArgs {
    pub(super) fn scope(self) -> InputScope {
        if self.pure {
            InputScope::Pure
        } else {
            self.offline.scope()
        }
    }
}

#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct InputArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,

    /// Require `tola.lock` to match the configuration. Leave the lock file unchanged.
    ///
    /// Tola can still download missing dependencies at their locked versions. `--pure` also
    /// requires a matching lock file.
    #[arg(long, help_heading = "Build", display_order = 42)]
    pub locked: bool,
}

impl InputArgs {
    pub(super) fn mode(self) -> InputMode {
        InputMode::new(self.scope.scope(), self.locked)
    }
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
#[command(version = version_text(), about, long_about = None, disable_help_subcommand = true,
    after_help = "Run 'tola <command> --help' for options and examples. Use 'tola help -i' to browse the reference."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
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
        format!("unknown help language `{value}`; use `en` or `zh`")
    })
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModuleCommand {
    /// Check the manifest and declared files without running any code.
    #[command(after_long_help = "Run tola module check ./theme from the parent directory, or tola module check\nfrom the directory containing tola-module.toml. Tola checks the manifest and its declared\npaths, such as library entries, scripts, assets, and font directories.\nTo compile the module and run its build hooks, use it in a site and run tola check there.")]
    Check {
        /// Directory containing `tola-module.toml`. If omitted, use the current directory.
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        directory: PathBuf,
    },
    /// Run `before-pack` hooks and write the files to ship.
    #[command(after_long_help = "In theme/tola-module.toml, declare the files to ship. For a module with a lib.typ file:\n  [pack]\n  include = [\"lib.typ\"]\n\nFrom the directory containing theme/, run:\n  mkdir -p dist\n  tola module pack theme --output dist/theme\n\nTola checks the release before replacing dist/theme. It contains lib.typ and\ntola-module.toml. For an example that generates a file before packing, see tola help \"[modules]\".")]
    Pack {
        /// Directory containing `tola-module.toml`. If omitted, use the current directory.
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        source: PathBuf,

        /// Write the release files to `DIR`. Its parent directory must exist.
        /// A successful pack replaces the previous release there.
        #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
        output: PathBuf,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Create a new Tola site.
    #[command(after_long_help = "From the directory that should contain the new site, run tola init my-site.\nTola creates my-site/tola.toml, site.typ, ignore files, and an empty content/ directory.\nUse tola init my-site --dry-run to see the planned files first.\n\nTo initialize from a module in ./theme, run:\n  tola init my-site --from ./theme --alias notes\nThe module must declare an initializer. Pass its own arguments after --.\nRun tola help \"[modules]\" for a complete initializer example.")]
    Init {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        args: InitArgs,
    },

    /// Serve the site locally and rebuild when files change.
    #[command(
        after_long_help = "From the site root, run tola dev --port 3000 and open the URL it prints.\nWith file watching enabled, saving a change to a page rebuilds the site and refreshes the browser.\nPress Ctrl+C to stop."
    )]
    Dev {
        #[command(flatten)]
        development: DevelopmentArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        process: ProcessArgs,

        /// Stream development events as JSON lines on stdout.
        ///
        /// Writes one event per line to stdout; logs and hook output go to stderr.
        /// Events report server readiness, builds, after-publish hooks, and shutdown.
        #[arg(long, help_heading = "Output", display_order = 79)]
        json: bool,

        /// Skip the session log that `tola dev` normally saves in `.tola/state/logs/`.
        #[arg(
            long,
            conflicts_with = "log_file",
            help_heading = "Output",
            display_order = 84
        )]
        no_log_file: bool,
    },

    /// Build the site and write it to the publish directory.
    ///
    /// A successful build replaces the contents of the publish directory (public/ by default).
    #[command(
        after_long_help = "From the site root, run tola build --publish-dir dist.\nWith the default starter site and a content/index.typ file, the build writes dist/index.html."
    )]
    Build {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        /// Print this build's result and diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Indent the JSON result for reading. Requires `--json`. Without it, JSON is compact.
        #[arg(long, requires = "json")]
        pretty: bool,
    },

    /// Build and check the site without writing the publish directory.
    ///
    /// Runs `before-build` and `generate-outputs` hooks. If a hook writes a Typst source,
    /// the check compiles it. Use `--no-hooks` to skip both hook stages.
    #[command(
        after_long_help = "From the site root, run tola check --json --pretty to print the build result and diagnostics as JSON.\nUse tola check --no-build to check only the syntax of the site's Typst sources."
    )]
    Check {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,

        /// Parse the site's Typst sources to check their syntax.
        ///
        /// Run `tola check` without this flag to compile the site and check its output.
        #[arg(long, conflicts_with_all = ["locked", "font_paths", "ignore_system", "minify", "origin", "base_path"])]
        no_build: bool,
        /// Skip before-build and generate-outputs hooks.
        #[arg(long, conflicts_with = "no_build")]
        no_hooks: bool,
        /// Print this check's result and diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Indent the JSON result for reading. Requires `--json`. Without it, JSON is compact.
        #[arg(long, requires = "json")]
        pretty: bool,
    },

    /// Build once with production settings and serve the result locally.
    ///
    /// The preview stays in memory. Run the command again to preview later edits.
    /// Runs before-build and generate-outputs hooks.
    #[command(
        after_long_help = "From the site root, run tola preview --port 0 and open the printed URL.\nTola picks a free port and serves one production build. Press Ctrl+C to stop."
    )]
    Preview {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        build: BuildArgs,
        #[command(flatten)]
        server: ServerArgs,
    },

    /// Format the site's Typst sources.
    #[command(after_long_help = "From the site root, run tola fmt --check to see which .typ files need formatting.\nRun tola fmt to apply the changes. With no site configuration, Tola formats .typ files\nunder the current directory with its default settings.")]
    Fmt {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: ScopeArgs,
        #[command(flatten)]
        site: SiteArgs,
        /// Report files that need formatting and exit with an error if any need changes. Omit this flag to write the formatted files.
        #[arg(long)]
        check: bool,
    },

    /// Check site configuration, source paths, and local tools.
    #[command(after_long_help = "From the site root, run tola doctor --json --pretty to print the environment and diagnostic report as JSON.")]
    Doctor(DoctorArgs),

    /// Refresh locked dependencies in `tola.lock`.
    #[command(after_long_help = "From the site root, run tola update --dry-run to see what would change in tola.lock.\nTo update just a configured collection named lucide, run tola update lucide.")]
    Update {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: OfflineArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        update: UpdateArgs,
    },

    /// Copy locked remote modules, Typst packages, and remote icon collections into the site.
    #[command(
        after_long_help = "In tola.toml, set path = \"vendor\" under [vendor]. Run tola vendor from the site root.\nIt prepares vendor/modules/, vendor/typst-packages/, and vendor/icons/ for tola build --pure.\nConfiguration examples: tola help \"[vendor]\" or tola help --lang zh \"[vendor]\"."
    )]
    Vendor {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        vendor: VendorArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
    },

    /// Check or package a standalone module directory.
    #[command(after_long_help = "From a directory containing theme/tola-module.toml, run tola module check theme\nto check the manifest and declared files. See tola module pack --help for a release example.")]
    Module {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(subcommand)]
        command: ModuleCommand,
    },

    /// Inspect site inputs and the settings Tola uses.
    #[command(after_long_help = "From the site root, run tola inspect config to read the settings Tola will use,\nor tola inspect sources to list the content files it finds.")]
    Inspect {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        config: ConfigFileArgs,
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Run a module query or read a labeled metadata value from a Typst file.
    #[command(after_long_help = "In the site root, create query.typ containing this line:\n  #metadata((title: \"Field notes\")) <result>\n\nRun tola query ./query.typ --label result --json from the same directory.\nIt prints {\"title\":\"Field notes\"}.")]
    Query {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        query: QueryArgs,
    },

    /// Run a Nushell script or start the Nushell language server.
    #[command(
        override_usage = "tola nu [OPTIONS] <ENTRY> [ARGS]...\n       tola nu [OPTIONS] --lsp",
        after_help = "Everything after <ENTRY> goes to the script, including --help.",
        after_long_help = "Everything after <ENTRY> goes to the script, including --help.\n\nFrom a directory containing photo.png, run:\n  tola nu @tola-scripts/image metadata photo.png\nThis prints JSON with the image dimensions and format.\n\nFrom the site root, run this to see the options for tools/report.nu:\n  tola nu tools/report.nu --help"
    )]
    Nu(NuArgs),

    /// Configure or display editor integration.
    #[command(after_long_help = "From the site root, run tola editor setup vscode --dry-run to see the files\nTola would write. Run it without --dry-run to apply those changes.")]
    Editor {
        #[command(subcommand)]
        command: EditorCommand,
    },

    /// Provide Tola source diagnostics and language queries over stdio.
    ///
    /// If the workspace has no `tola.toml`, the server uses the default settings.
    #[command(after_long_help = "Start tola lsp as a stdio language server in your editor's site workspace.\nFor example, run tola editor template helix to print the client settings for Helix.")]
    Lsp {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        site: SiteArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
    },

    /// Read configuration, package, and built-in script documentation.
    #[command(after_long_help = "You can read the built-in reference from any directory.\nRun tola help \"[assets]\" for static-file settings, or\n  tola help \"@tola/image\" resize-image\nfor the function's signature and examples.")]
    Help(HelpArgs),

    /// Print or export the Tola site authoring skill.
    #[command(after_long_help = "Run these commands from any directory:\n  tola skill --list\n  tola skill site assets files\n  tola skill site assets --all -o assets-guide.md\nThe first lists sections, the second reads one section, and the third exports a section\nwith its subsections to assets-guide.md in the current directory.\nFor a module's own guide command, see tola help \"[modules]\".")]
    Skill {
        #[command(flatten)]
        process: ProcessArgs,
        #[command(flatten)]
        args: SkillArgs,
    },

    /// Write a shell completion script to stdout.
    #[command(after_long_help = "From any directory, run tola completions zsh > _tola to write the Zsh completion script\nto _tola in the current directory.")]
    Completions(CompletionsArgs),

    /// Generate a roff manual page.
    #[command(after_long_help = "From any directory, run tola manpage --output tola.1 to write a manual page in the current directory.\nRun man ./tola.1 to read it.")]
    Manpage(ManpageArgs),

    /// Print the Tola, Typst, and embedded Nu versions.
    #[command(after_long_help = "From any directory, run tola version --json --pretty to print the three version numbers as indented JSON.")]
    Version(VersionArgs),

    /// Run a command declared by one of this site’s modules.
    #[command(external_subcommand)]
    Extension(Vec<OsString>),
}

/// Options for refreshing the inputs `tola.lock` records.
#[derive(clap::Args, Debug, Clone)]
pub struct UpdateArgs {
    /// Module aliases or icon collection names to update. Omit the names to check all configured remote modules and icon collections.
    pub names: Vec<String>,

    /// Show what would change without writing `tola.lock`. May download inputs to the local cache.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct NuArgs {
    #[command(flatten)]
    pub input: ScopeArgs,

    #[command(flatten)]
    pub config: ConfigFileArgs,

    /// Start the Nushell language server over stdio.
    #[arg(long, display_order = 43, group = "nu_lsp", conflicts_with_all = ["stdin", "invocation", "lsp_worker", "lsp_snapshot"])]
    pub lsp: bool,

    /// Make Tola's private native module visible while editing built-in Nushell sources.
    #[arg(long, display_order = 44, requires = "nu_lsp")]
    pub builtin_dev: bool,

    #[arg(long, hide = true, group = "nu_lsp", requires = "lsp_snapshot", conflicts_with_all = ["stdin", "invocation", "lsp"])]
    pub lsp_worker: bool,

    #[arg(long, hide = true, requires = "lsp_worker", value_name = "FILE")]
    pub lsp_snapshot: Option<PathBuf>,

    #[arg(long, hide = true, value_name = "FILE", conflicts_with_all = ["stdin", "invocation", "lsp", "lsp_worker", "no_site", "path"])]
    pub hook_worker: Option<PathBuf>,

    #[arg(long, hide = true, conflicts_with_all = ["path", "lsp", "lsp_worker", "lsp_snapshot"])]
    pub no_site: bool,

    /// Pass stdin to the script. Put this flag before the script name.
    #[arg(long, display_order = 39)]
    pub stdin: bool,

    /// A .nu file, @tola-scripts/<NAME>, or @tola-modules/<ALIAS>, followed by script arguments.
    #[arg(value_name = "ENTRY", num_args = 1.., trailing_var_arg = true, required_unless_present_any = ["lsp", "lsp_worker", "hook_worker"])]
    pub invocation: Vec<OsString>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct HelpArgs {
    #[command(flatten)]
    pub process: ProcessArgs,

    #[command(flatten)]
    pub input: ScopeArgs,

    /// A config table, `"@tola/NAME" [EXPORT...]`, `"@tola-scripts/NAME" [COMMAND...]`,
    /// or a `tola-help://` address. Omit the target to open the help overview.
    #[arg(value_name = "TARGET", num_args = 0..)]
    pub targets: Vec<String>,

    /// Browse configuration, packages, and built-in scripts in the terminal.
    #[arg(short = 'i', long, conflicts_with = "targets")]
    pub interactive: bool,

    /// Print the help page as terminal text, Markdown, or JSON.
    #[arg(
        long,
        value_enum,
        default_value = "text",
        conflicts_with = "interactive"
    )]
    pub format: HelpFormat,

    /// Indent the JSON output. Requires --format json.
    #[arg(long, conflicts_with = "interactive")]
    pub pretty: bool,

    /// Write `tola help` documentation in `en` (default) or `zh`.
    #[arg(long, value_name = "LANGUAGE", value_parser = parse_help_language)]
    pub lang: Option<HelpLanguage>,

    /// Print directly instead of opening the pager.
    #[arg(long)]
    pub no_pager: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct VersionArgs {
    /// Print the versions as JSON.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output for reading. Requires `--json`. Without it, JSON is compact.
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
    #[command(flatten)]
    pub process: ProcessArgs,

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
    #[command(flatten)]
    pub process: ProcessArgs,

    /// Write the generated roff document to a file instead of stdout.
    #[arg(short, long, value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SkillArgs {
    /// Topic followed by heading names, such as `math rendering svg`.
    #[arg(value_name = "SECTION", num_args = 0..)]
    pub sections: Vec<String>,

    /// List the section names in a topic.
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

    /// Directory to create or initialize. If omitted, use the current directory.
    #[arg(value_hint = clap::ValueHint::DirPath)]
    pub path: Option<PathBuf>,

    /// Show the files and configuration that would be written. With `--from`, Tola runs the initializer to prepare this preview.
    #[arg(long)]
    pub dry_run: bool,

    /// Allow a non-empty directory. Tola keeps existing files and rejects name conflicts.
    #[arg(long)]
    pub force: bool,

    /// Use an initializer from a local directory, Git URL, or archive (`.tar.gz`, `.tar.zst`, `.zip`). Supply `--alias NAME`. If omitted, use Tola's starter files.
    #[arg(long, value_name = "SOURCE")]
    pub from: Option<OsString>,

    /// Use the initializer from this Git branch.
    #[arg(long, requires = "from", conflicts_with_all = ["tag", "rev"])]
    pub branch: Option<String>,

    /// Use the initializer at this Git tag.
    #[arg(long, requires = "from", conflicts_with_all = ["branch", "rev"])]
    pub tag: Option<String>,

    /// Use the initializer at this full Git commit hash.
    #[arg(long, requires = "from", conflicts_with_all = ["branch", "tag"])]
    pub rev: Option<String>,

    /// Module directory inside the Git repository. If omitted, look at the repository root.
    #[arg(long, requires = "from")]
    pub subdir: Option<PathBuf>,

    /// Module alias to write in the new site. Required when you use `--from`.
    #[arg(long, value_name = "NAME", requires = "from")]
    pub alias: Option<String>,

    /// Review the starter files before writing. With `--from`, let the initializer prompt for input. Omit this flag to run without prompts.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Arguments for the module initializer, following `--`.
    #[arg(last = true, value_name = "ARGS", requires = "from")]
    pub args: Vec<OsString>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ConfigFileArgs {
    /// Path to the site's TOML configuration.
    ///
    /// Without this option, commands search the current and parent directories for `tola.toml`.
    /// The Typst language server instead checks only the client's workspace root.
    #[arg(
        short = 'C',
        long = "config",
        value_name = "FILE",
        value_hint = clap::ValueHint::FilePath
    )]
    pub path: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct TypstPackageArgs {
    /// Search this local Typst package directory. Omit it to use Typst's normal local directory.
    ///
    /// Packages in `{vendor}/typst-packages` take priority. Use `tola vendor --refresh`
    /// to refresh them from their sources.
    #[arg(long, help_heading = "Build", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub package_path: Option<PathBuf>,

    /// Use this Typst package cache. If omitted, use Typst's configured or system cache directory.
    ///
    /// Tola searches the local package directory first.
    #[arg(long, help_heading = "Build", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub package_cache_path: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildOverrideArgs {
    /// Set where Tola writes the built site.
    ///
    /// Relative paths start at the site root. Omit this flag to use `build.publish-dir` (`public/`
    /// by default). Use an empty directory or one where Tola has published this site before.
    #[arg(short = 'o', long, help_heading = "Build", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub publish_dir: Option<PathBuf>,

    /// Use `--minify=false` to turn off HTML, CSS, and JavaScript minification for this run.
    /// Pass `--minify` alone to turn all three on. Omit it to use `[build.minify]`.
    #[arg(short, long, help_heading = "Build", value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub minify: Option<bool>,

    /// Use an origin such as `https://example.com` for this run. Omit it to use `site.origin`.
    #[arg(long, help_heading = "Build", value_name = "URL", value_hint = clap::ValueHint::Url)]
    pub origin: Option<String>,

    /// Use a deployment path such as `/docs/` for this run. Omit it to use `site.base-path` (`/` by default).
    #[arg(long = "base-path", help_heading = "Build")]
    pub base_path: Option<String>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ServerArgs {
    /// Listen on this IP address. If omitted, use `[server] interface` (`127.0.0.1` by default). Use `0.0.0.0` to accept connections from other machines.
    #[arg(short, long, value_name = "IP", help_heading = "Server and rebuilds")]
    pub interface: Option<std::net::IpAddr>,

    /// Listen on this port. Use `0` to pick a free port. If omitted, use `[server] port` (`5277` by default).
    #[arg(short, long, help_heading = "Server and rebuilds")]
    pub port: Option<u16>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DevelopmentArgs {
    #[command(flatten)]
    pub server: ServerArgs,

    /// Use `--watch=false` to turn off file watching for this run. `--watch` turns it on. If omitted, use `[dev] watch` (on by default).
    #[arg(short, long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", help_heading = "Server and rebuilds")]
    pub watch: Option<bool>,

    /// Run `generate-outputs` hooks on every rebuild even if they use `dev = "reuse"`.
    #[arg(long, help_heading = "Server and rebuilds")]
    pub no_reuse: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum InspectCommand {
    /// List the site's content source files.
    #[command(override_usage = "tola inspect [-C <FILE>] sources [OPTIONS] [PATH]...")]
    #[command(after_long_help = "From the site root, run tola inspect sources content/guide.typ to check whether that file\nis selected as a content source. Omit the path to list all sources.")]
    Sources(SourceInspectArgs),
    /// List configured icon collections or the names in one collection.
    #[command(after_long_help = "From the site root, run tola inspect icons brand to list icons in the site's brand collection.\nUse tola inspect icons brand --module notes to read the collection from the notes module.")]
    Icons(IconInspectArgs),
    /// List the fonts available when Typst compiles the site.
    #[command(after_long_help = "From the site root, run tola inspect fonts --json to list the fonts available to the Typst compiler.")]
    Fonts {
        #[command(flatten)]
        input: ScopeArgs,
        #[command(flatten)]
        packages: TypstPackageArgs,
        #[command(flatten)]
        fonts: CompilerFontArgs,
        #[command(flatten)]
        display: OutputArgs,
    },
    /// Show the settings, modules, and paths Tola uses for this site.
    #[command(after_long_help = "From the site root, run tola inspect config --mode dev --json --pretty.\nLook in hooks.stages for the hook order and skip reasons. If you use include,\nconfig_sources lists the loaded files and field_origins shows where each setting came from.\nInspection never runs hooks.")]
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
    /// Label to read from a Typst query file. For `<result>`, pass `--label result`. Module queries already declare their label.
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    #[command(flatten)]
    pub options: super::commands::query::QueryOptions,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct OutputArgs {
    /// Print the result as JSON.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output for reading. Requires `--json`. Without it, JSON is compact.
    #[arg(long, requires = "json")]
    pub pretty: bool,
    /// Write JSON to `FILE`. Requires `--json`. If omitted, JSON goes to stdout.
    #[arg(long, requires = "json", value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
    /// Browse and select results in the terminal.
    ///
    /// Press Enter to return the selected value, or q to close.
    #[arg(short = 'i', long, conflicts_with = "json")]
    pub interactive: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct InspectConfigArgs {
    #[command(flatten)]
    pub input: ScopeArgs,
    #[command(flatten)]
    pub packages: TypstPackageArgs,
    #[command(flatten)]
    pub build: BuildOverrideArgs,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct IconInspectArgs {
    #[command(flatten)]
    pub input: InputArgs,
    /// Collection name from `icons.collections`. Omit it to list configured collections.
    #[arg(value_name = "NAMESPACE")]
    pub namespace: Option<String>,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct SourceInspectArgs {
    /// Require content sources to stay inside the site.
    #[arg(long)]
    pub pure: bool,

    /// Files or directories. Use '-' to read paths from stdin.
    ///
    /// Omit paths to list all sources. Pass '-' alone to read one path per line.
    #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub display: OutputArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub process: ProcessArgs,

    #[command(flatten)]
    pub input: ScopeArgs,

    #[command(flatten)]
    pub site: SiteArgs,

    /// Print environment and diagnostics as JSON for issue reports.
    #[arg(long)]
    pub json: bool,
    /// Indent the JSON output for reading. Requires `--json`. Without it, JSON is compact.
    #[arg(long, requires = "json")]
    pub pretty: bool,
}

/// The compiler font inputs one invocation overrides.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct CompilerFontArgs {
    /// Add a directory to search for Typst fonts. Repeat for each directory.
    ///
    /// Tola searches these directories before `[typst.fonts] paths`. Relative paths
    /// start in the directory where you run the command. Unavailable with `--pure`.
    #[arg(
        id = "font_paths",
        long = "font-path",
        help_heading = "Build",
        value_name = "DIR",
        value_hint = clap::ValueHint::DirPath
    )]
    pub paths: Vec<PathBuf>,

    /// Skip system fonts for this run. If omitted, follow `[typst.fonts] system`.
    #[arg(long = "ignore-system-fonts", help_heading = "Build")]
    pub ignore_system: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum EditorCommand {
    /// Regenerate editor packages and merge workspace settings.
    #[command(after_long_help = "From the site root, run tola editor setup helix --dry-run to preview the Helix setup.\nOmit --dry-run to write the files.")]
    Setup {
        #[command(flatten)]
        process: ProcessArgs,

        #[command(flatten)]
        input: ScopeArgs,
        #[command(flatten)]
        site: SiteArgs,

        /// Editors to configure; omit to choose interactively.
        #[arg(value_enum)]
        editors: Vec<Editor>,
        /// Use Tola for Nushell. Adds a launcher for the VS Code Nushell extension (macOS/Linux), or configures Helix directly.
        #[arg(long, conflicts_with = "list")]
        nu: bool,
        /// Print the editors Tola configures and the settings any other LSP client needs, then stop.
        #[arg(long)]
        list: bool,
        /// List the files this setup would write or remove. Omit this flag to apply the changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate editor packages without changing editor settings.
    #[command(after_long_help = "From the site root, run tola editor packages after updating a module to refresh the Typst packages used by your editor.")]
    Packages {
        #[command(flatten)]
        process: ProcessArgs,

        #[command(flatten)]
        input: ScopeArgs,
        #[command(flatten)]
        site: SiteArgs,
    },
    /// Print one editor's configuration and where it belongs, without writing files.
    #[command(after_long_help = "From any directory, run tola editor template helix to print the configuration for .helix/languages.toml.")]
    Template {
        #[command(flatten)]
        process: ProcessArgs,

        /// Editor whose settings to print.
        #[arg(value_enum)]
        editor: Editor,
    },
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct VendorArgs {
    /// Select dependencies again from their configured sources. If omitted, Tola can use the existing vendor copy.
    #[arg(long)]
    pub refresh: bool,

    /// Check what would go into vendor without replacing it.
    ///
    /// This may fetch dependencies and update the cache. If a previous replacement was
    /// interrupted, run `tola vendor` to finish it before using `--dry-run`.
    #[arg(long)]
    pub dry_run: bool,
}

impl Commands {
    pub(super) fn process(&self) -> ProcessArgs {
        match self {
            Self::Init { process, .. }
            | Self::Skill { process, .. }
            | Self::Update { process, .. }
            | Self::Module { process, .. }
            | Self::Build { process, .. }
            | Self::Dev { process, .. }
            | Self::Preview { process, .. }
            | Self::Check { process, .. }
            | Self::Fmt { process, .. }
            | Self::Inspect { process, .. }
            | Self::Query { process, .. }
            | Self::Vendor { process, .. }
            | Self::Lsp { process, .. }
            | Self::Editor {
                command:
                    EditorCommand::Setup { process, .. }
                    | EditorCommand::Packages { process, .. }
                    | EditorCommand::Template { process, .. },
            } => process.clone(),
            Self::Help(args) => args.process.clone(),
            Self::Doctor(args) => args.process.clone(),
            Self::Manpage(args) => args.process.clone(),
            Self::Completions(args) => args.process.clone(),
            Self::Nu(_) | Self::Version(_) | Self::Extension(_) => ProcessArgs::default(),
        }
    }

    pub(super) fn input_mode(&self) -> InputMode {
        let scope = match self {
            Self::Build { input, .. }
            | Self::Dev { input, .. }
            | Self::Preview { input, .. }
            | Self::Check { input, .. }
            | Self::Query { input, .. }
            | Self::Vendor { input, .. }
            | Self::Lsp { input, .. } => return input.mode(),
            Self::Init { args, .. } => return args.input.mode(),
            Self::Update { input, .. } => input.scope(),
            Self::Fmt { input, .. }
            | Self::Editor {
                command: EditorCommand::Setup { input, .. } | EditorCommand::Packages { input, .. },
            } => input.scope(),
            Self::Inspect { command, .. } => match command {
                InspectCommand::Icons(args) => return args.input.mode(),
                InspectCommand::Config(args) => args.input.scope(),
                InspectCommand::Fonts { input, .. } => input.scope(),
                InspectCommand::Sources(args) if args.pure => InputScope::Pure,
                InspectCommand::Sources(_) => InputScope::Online,
            },
            Self::Doctor(args) => args.input.scope(),
            Self::Nu(args) => args.input.scope(),
            Self::Help(args) => args.input.scope(),
            _ => InputScope::Online,
        };
        InputMode::new(scope, false)
    }

    pub(super) fn config_file(&self) -> Option<&ConfigFileArgs> {
        match self {
            Self::Update { site, .. }
            | Self::Build { site, .. }
            | Self::Check { site, .. }
            | Self::Fmt { site, .. }
            | Self::Vendor { site, .. }
            | Self::Dev { site, .. }
            | Self::Preview { site, .. }
            | Self::Lsp { site, .. }
            | Self::Query { site, .. }
            | Self::Editor {
                command: EditorCommand::Setup { site, .. } | EditorCommand::Packages { site, .. },
            } => Some(&site.config),
            Self::Inspect { config, .. } => Some(config),
            Self::Doctor(args) => Some(&args.site.config),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn font_options_belong_to_font_inspection() {
        let cli =
            Cli::try_parse_from(["tola", "inspect", "fonts", "--font-path", "fonts"]).unwrap();
        let Commands::Inspect {
            command: InspectCommand::Fonts { fonts, .. },
            ..
        } = cli.command
        else {
            panic!("expected font inspection");
        };
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
        let cli = Cli::try_parse_from(["tola", "build", "-vv"]).unwrap();
        assert_eq!(cli.command.process().verbose, 2);
    }

    #[test]
    fn init_forwards_arguments_after_separator() {
        let cli = Cli::try_parse_from(["tola", "init", "site", "--from", "theme", "--", "--force"])
            .unwrap();
        let Commands::Init { args, .. } = cli.command else {
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
        assert!(cli.command.input_mode().locked());
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
            &["tola", "inspect", "sources", "--font-path", "fonts"][..],
            &["tola", "check", "--no-build", "--font-path", "fonts"][..],
            &["tola", "update", "--locked"][..],
            &["tola", "update", "--pure"][..],
            &["tola", "nu", "--log-file", "trace", "script.nu"][..],
            &["tola", "build", "--no-log-file"][..],
            &["tola", "-v", "build"][..],
            &["tola", "build", "--interface", "127.0.0.1"][..],
        ] {
            assert!(!parses(argv), "{argv:?} must be refused");
        }
    }
}
