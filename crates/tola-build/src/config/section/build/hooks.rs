//! Finite commands around candidate construction and publication.
//!
//! The environment a hook command receives, and what a superseded build cancels, is written
//! for the site author in the section's and each stage's `HELP`, which `tola help "[hooks]"`
//! and its stage pages render.

use crate::config::{ConfigDiagnostics, FieldPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tola_config::Config;
use tola_module::Action;
use tola_module::hooks::RerunSelection;
pub use tola_module::hooks::{
    CommandOutput, DevParticipation, HookStage, ModuleHookOrigin, RerunOn,
};
use tola_module::options::OptionPath;

/// Run scripts before compilation, while generating files, or after publication.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks")]
pub struct HooksConfig {
    #[serde(rename = "before-build")]
    #[config(sub, collection = array_table)]
    pub before_build: Vec<BeforeBuildHookConfig>,
    #[serde(rename = "generate-outputs")]
    #[config(sub, collection = array_table)]
    pub generate_outputs: Vec<OutputCommandConfig>,
    #[serde(rename = "after-publish")]
    #[config(sub, collection = array_table)]
    pub after_publish: Vec<AfterPublishHookConfig>,
}

impl HooksConfig {
    /// What `tola help "[hooks]"` adds under its child tables.
    pub const HELP: &'static str = r#"
| Stage | When to use it |
| --- | --- |
| `before-build` | Generate Typst pages, templates, data files, or assets before source discovery. |
| `generate-outputs` | Add files after compilation, such as a search index. |
| `after-publish` | Read the published files, for example to upload the site. |

### Choose how to run a hook

Set `nu` to a script for Tola’s built-in Nushell. Put script arguments in `args`.
For an external program and its arguments, use `argv`. Site hooks run from the site root.

### Set the order

Within a stage, each site and module runs its own hooks in ascending `order`.
The default is 0. Hooks at the same order may run together. Use
[`include`](tola-help://config/include) to keep hook declarations in separate files.
Tola sorts those site hooks together by `order`.

Modules run in alias order. Site hooks run first in `before-build` and last in the
other two stages. A failed group stops later groups in that stage. All `generate-outputs`
hooks read the same compiled files. Put steps that share generated files in one script.

Under `[build]`, use `hook-jobs` to cap concurrency or set `par-hooks = false` to run one
hook at a time.

### See which hooks will run

From the site root, run `tola inspect config --mode dev --json --pretty`. Under
`hooks.stages`, `declarations` lists hooks, where they were declared, and why any are skipped.
`batches` lists the run groups in order. Use `--mode prod` for production. Inspection
only reads the configuration.

### When hooks run

Hooks are enabled by default. Set `enable = false` to turn one off. Set
`enabled-by = "images.enable"` to run a hook only when that option is true. Quote keys
that contain a dot, as in `enabled-by = '"images.enable"'`. A missing key, non-table parent,
or non-boolean value is an error. With `enable = false`, Tola skips the lookup.

| Command | Build mode | before-build | generate-outputs | after-publish |
| --- | --- | --- | --- | --- |
| `tola build` | `prod` | run | run | run |
| `tola check` | `prod` | run | run | skip |
| `tola preview` | `prod` | run | run | skip |
| `tola dev` | `dev` | per `dev` | per `dev` | per `dev` |
| `tola inspect` | — | skip | skip | skip |
| `tola vendor` | `prod` | skip | skip | skip |

Use `tola check --no-hooks` to skip both build stages. By default, `tola dev` runs the first
two stages and skips `after-publish`. See `[[hooks.generate-outputs]]` for `dev = "reuse"`.

An error before publication leaves the previous site in place. An `after-publish` failure
is reported after the new files are already on disk.

### Report issues and find paths

In a Nu hook, import `use tola diagnostic` and pass the issues to `diagnostic submit`.
Submit one list per run. Pass `[]` when there are no issues. Omit `location` in a regular
hook report. Warnings appear in the terminal and errors fail the hook. An external
command can write its report to `reportPath`. See [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic)
for a complete example. If `tola dev` reuses files from a `generate-outputs` hook, it also
reuses that hook's report.

In a Nu script, import `use tola get-context` and run `get-context | get hook.stage` to
read the stage. From an external command, run `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json`.
See [`@tola-scripts/context`](tola-help://scripts/context) for fields and examples.

Your script manages its own cache files. Each hook run gets a fresh temporary directory.
`TMPDIR`, `TMP`, and `TEMP` point to `hook.tempDir`.

### Watch files the script reads

Tola already watches the entry script named by `nu`. List any extra data, templates, or
configuration files in `rerun-on`. Patterns start at the site root, and matching a
directory also matches its contents. `*` matches within one directory and `**` can
cross directories. Exclusions take priority. A pattern may start with no matches.
A file created later can still trigger a rebuild.
`include` is required when you write `rerun-on`; `exclude` defaults to an empty list.
In a module hook, string paths start at the module root. For files in the consuming
site, use entries such as `{ project = "content/**/*.typ" }`.

Hooks run with your permissions. The `--offline` and `--pure` flags restrict Tola's input
reads. Scripts can still access files and the network. A cancelled development build stops
its pre-publication hooks. Files those hooks already wrote remain on disk.

Examples:

- [Run hooks in order across configuration files](tola-help://config/hooks/before-build)
- [Add a page list or search index](tola-help://config/hooks/generate-outputs)
- [Upload the site or check its home page](tola-help://config/hooks/after-publish)"#;
}

/// Chain one projection over the three hook stages in lifecycle order.
macro_rules! hook_stages {
    ($config:expr, keep $keep:expr, map $projection:expr) => {
        $config
            .before_build
            .iter()
            .filter($keep)
            .map($projection)
            .chain(
                $config
                    .generate_outputs
                    .iter()
                    .filter($keep)
                    .map($projection),
            )
            .chain($config.after_publish.iter().filter($keep).map($projection))
    };
}

impl HooksConfig {
    /// Enabled command arguments in lifecycle order, borrowed without copying.
    pub fn enabled_commands(&self) -> impl Iterator<Item = &[String]> {
        hook_stages!(
            self,
            keep | hook | hook.enable,
            map | hook | hook.argv.as_deref()
        )
        .flatten()
    }

    /// Additional input selections of the hooks that participate in development.
    pub fn development_rerun_selections(&self, root: &Path) -> anyhow::Result<Vec<RerunSelection>> {
        hook_stages!(
            self,
            keep | hook | hook.enable
                && hook.dev.participates_in_development()
                && !hook.rerun_on.include.is_empty(),
            map | hook | hook.rerun_on.compile(root, hook.origin.as_ref())
        )
        .collect()
    }

    /// Absolute Nu script paths of the hooks that participate in development.
    pub fn development_hook_scripts<'a>(
        &'a self,
        root: &'a Path,
    ) -> impl Iterator<Item = PathBuf> + 'a {
        hook_stages!(
            self,
            keep | hook | hook.enable && hook.dev.participates_in_development(),
            map move | hook | hook.nu.as_ref().map(|path| {
                hook.origin
                    .as_ref()
                    .map_or(root, |origin| origin.root.as_path())
                    .join(path)
            })
        )
        .flatten()
    }

    /// Select site hooks using the declaring site's boolean options.
    pub(crate) fn apply_site_options(&mut self, options: &toml::Table) -> anyhow::Result<()> {
        fn select(
            enable: &mut bool,
            key: Option<&OptionPath>,
            options: &toml::Table,
            stage: HookStage,
            name: &str,
        ) -> anyhow::Result<()> {
            *enable = tola_module::options::enabled_by(*enable, key, options)
                .map_err(|message| anyhow::anyhow!("{}: {message}", hook_identity(stage, name)))?;
            Ok(())
        }
        for hook in &mut self.before_build {
            select(
                &mut hook.enable,
                hook.enabled_by.as_ref(),
                options,
                HookStage::BeforeBuild,
                &hook.name,
            )?;
        }
        for hook in &mut self.generate_outputs {
            select(
                &mut hook.enable,
                hook.enabled_by.as_ref(),
                options,
                HookStage::GenerateOutputs,
                &hook.name,
            )?;
        }
        for hook in &mut self.after_publish {
            select(
                &mut hook.enable,
                hook.enabled_by.as_ref(),
                options,
                HookStage::AfterPublish,
                &hook.name,
            )?;
        }
        Ok(())
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        let mut named = Vec::new();
        for (index, hook) in self.before_build.iter().enumerate() {
            let fields = HookEntryFields::BEFORE_BUILD;
            diag.with_array_element(Self::FIELDS.before_build.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    hook.action(),
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_dev(hook.dev, false, index, fields.dev, diag);
                validate_name_is_unique(&mut named, &hook.name, index, fields.name, diag);
                for (entry_index, generated) in hook.generates.iter().enumerate() {
                    validate_site_relative(
                        generated,
                        index,
                        entry_index,
                        BeforeBuildHookConfig::FIELDS.generates,
                        diag,
                    );
                }
            });
        }

        let outputs = OutputCommandConfig::FIELDS.outputs;
        let mut named = Vec::new();
        for (index, command) in self.generate_outputs.iter().enumerate() {
            let fields = HookEntryFields::GENERATE_OUTPUTS;
            diag.with_array_element(Self::FIELDS.generate_outputs.as_str(), index, |diag| {
                validate_command(
                    &command.name,
                    command.action(),
                    &command.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_name_is_unique(&mut named, &command.name, index, fields.name, diag);
                if command.outputs.is_empty() {
                    diag.error_with_help(
                        outputs,
                        format!("`{}` is empty", message_key(outputs, index)),
                        "declare each output with `{ file = … }` or `{ tree = … }`",
                    );
                }
                let mut paths = std::collections::BTreeSet::new();
                for output in &command.outputs {
                    let path = output.path();
                    if path.is_reserved_for_non_system_output() {
                        diag.error_with_help(
                            outputs,
                            format!("`{path}` is reserved"),
                            "choose a path outside `_tola`",
                        );
                    }
                    if !paths.insert(path) {
                        diag.error_with_help(
                            outputs,
                            format!("`{}` declares `{path}` twice", message_key(outputs, index)),
                            "keep one entry",
                        );
                    }
                }
            });
        }

        let mut named = Vec::new();
        for (index, hook) in self.after_publish.iter().enumerate() {
            let fields = HookEntryFields::AFTER_PUBLISH;
            diag.with_array_element(Self::FIELDS.after_publish.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    hook.action(),
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_dev(hook.dev, false, index, fields.dev, diag);
                validate_name_is_unique(&mut named, &hook.name, index, fields.name, diag);
            });
        }
    }
}

/// The declared field paths of one entry, per hook stage.
///
/// A path names a key inside an entry, so a diagnostic about it points at the entry the author
/// must edit; [`message_key`] spells the same field with the entry's index.
#[derive(Clone, Copy)]
struct HookEntryFields {
    name: FieldPath,
    argv: FieldPath,
    dev: FieldPath,
    rerun_on: FieldPath,
}

impl HookEntryFields {
    const BEFORE_BUILD: Self = Self {
        name: BeforeBuildHookConfig::FIELDS.name,
        argv: BeforeBuildHookConfig::FIELDS.argv,
        dev: BeforeBuildHookConfig::FIELDS.dev,
        rerun_on: BeforeBuildHookConfig::FIELDS.rerun_on,
    };

    const GENERATE_OUTPUTS: Self = Self {
        name: OutputCommandConfig::FIELDS.name,
        argv: OutputCommandConfig::FIELDS.argv,
        dev: OutputCommandConfig::FIELDS.dev,
        rerun_on: OutputCommandConfig::FIELDS.rerun_on,
    };

    const AFTER_PUBLISH: Self = Self {
        name: AfterPublishHookConfig::FIELDS.name,
        argv: AfterPublishHookConfig::FIELDS.argv,
        dev: AfterPublishHookConfig::FIELDS.dev,
        rerun_on: AfterPublishHookConfig::FIELDS.rerun_on,
    };
}

/// The key one entry's field writes, as messages spell it: `hooks.before-build[1].argv`.
///
/// A declared field path names the field after the array's own path, so the entry's index joins
/// them where the document writes it.
fn message_key(field: FieldPath, index: usize) -> String {
    match field.as_str().rsplit_once('.') {
        Some((array, name)) => format!("{array}[{index}].{name}"),
        None => field.as_str().to_owned(),
    }
}

/// Runs before source discovery to write the files the site then reads: Typst's inputs, and the
/// files `[assets]` declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.before-build", collection = array_table)]
pub struct BeforeBuildHookConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Command and arguments to run from the site root. Use `argv` or `nu`. To run a shell
    /// script, name the shell explicitly. Write files where the site will read them.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"run"`, `tola dev` runs the hook on each build. Use `"skip"`
    /// to skip it during development.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns.
    #[serde(rename = "rerun-on")]
    pub rerun_on: RerunOn,
    /// Files or directories the hook's command generates, relative to the site root; they must not
    /// be inside `build.publish-dir` or `.tola`. After the hook's command, Tola checks that each
    /// declared path exists: the hook declares what it wrote, and Tola does not infer it. Declaring
    /// it does not publish it: publish it through `[assets]` or a Bundle `asset(...)`, or use it to
    /// produce a document or another output.
    #[config(collection = inline)]
    pub generates: Vec<PathBuf>,
}

impl BeforeBuildHookConfig {
    /// What `tola help "[hooks.before-build]"` adds under its table.
    pub const HELP: &'static str = r#"
### Generate other inputs

Write templates, data files, or assets to their source paths. For example, generate
`static/site.css` and declare it under `[assets]` to publish it. Declare the files or directories
in `generates`. Tola checks that they exist after the command runs and watches them in dev mode.
Keep generated inputs outside `build.publish-dir` and `.tola`.

The command runs once per build. Use `hook.cacheDir` for files needed across runs and
`hook.tempDir` for this run's temporary files. This stage runs before there is a built
site to read, so write input files directly in the site.

Example: Generate a Typst page:

In a site created with `tola init`, save this as `write-release-notes.nu` in the site root.
The script uses the configured content directory.

```nu
use tola get-context

let content = get-context | get site.contentRoot
"= Release notes\n" | save --force ($content | path join "releases.typ")
```

Add the hook to `tola.toml`.

```toml
[[hooks.before-build]]
name = "release-notes"
nu = "write-release-notes.nu"
generates = ["content/releases.typ"]
```

Run `tola build` from the site root. The hook writes `content/releases.typ` before source
discovery. The default `site.typ` publishes it as `public/releases/index.html`.
If you changed `build.content-dir`, update the path in `generates` as well.

Example: Run two hooks in order across config files:

In a site created with `tola init`, create `config/`, `data/`, and `tools/`. Save
this as `data/release.json`:

```json
{"title": "Atlas 2.0"}
```

Save this as `tools/read-release.nu`:

```nu
mkdir generated
open data/release.json | get title | save --force generated/release.txt
```

Save this as `tools/write-upcoming.nu`:

```nu
let title = open --raw generated/release.txt
$"= ($title)\n" | save --force content/upcoming.typ
```

Add this line at the top of `tola.toml`, before the first table header:

```toml
include = ["config/release.toml"]
```

Save this as `config/release.toml`:

```toml
[[hooks.before-build]]
name = "upcoming-page"
order = 10
nu = "tools/write-upcoming.nu"
generates = ["content/upcoming.typ"]
```

Add the data hook to `tola.toml`:

```toml
[[hooks.before-build]]
name = "release-data"
order = 0
nu = "tools/read-release.nu"
rerun-on = { include = ["data/release.json"] }
generates = ["generated/release.txt"]
```

Run `tola build` from the site root. `release-data` runs before `upcoming-page` even
though `config/release.toml` was loaded first. With the default `site.typ` and publish
directory, `public/upcoming/index.html` shows “Atlas 2.0”. If you run `tola dev` instead,
editing `data/release.json` rebuilds the page. Paths such as `tools/write-upcoming.nu`
start at the site root, even when declared in `config/release.toml`."#;
}

/// Generate files after Typst compiles. Use `before-build` for files Typst needs to read.
/// Tola checks links to generated output before publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.generate-outputs", collection = array_table)]
pub struct OutputCommandConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Command and arguments to run from the site root. Use `argv` or `nu`. Read compiled pages
    /// and assets from `hook.inputDir`. Write files to `hook.outputDir` using
    /// paths relative to the publish directory.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"run"`, `tola dev` runs the hook on each build. Use `"reuse"`
    /// to use the last successful outputs when the inputs are unchanged, or `"skip"`
    /// to skip the hook during development.
    #[config(values = DevParticipation::REUSE_VALUES)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns. For
    /// `dev = "reuse"`, the selected inputs are also part of the hook's fingerprint.
    #[serde(rename = "rerun-on")]
    pub rerun_on: RerunOn,
    /// Required output paths relative to `build.publish-dir`, for example
    /// `{ file = "build.txt" }` or `{ tree = "assets/search" }`. Written files must match
    /// these declarations. Keep them outside `_tola` and use unique output paths.
    #[config(collection = inline)]
    pub outputs: Vec<CommandOutput>,
}

impl OutputCommandConfig {
    /// What `tola help "[hooks.generate-outputs]"` adds under its table.
    pub const HELP: &'static str = r#"
### Read pages and write files

- `hook.inputDir`: Read compiled pages and `[assets]` files here.
- `hook.outputDir`: Write every path listed in `outputs`. This directory starts empty.
  Tola publishes the files at the same relative paths.
- `hook.cacheDir`: Keep files for later runs. Copy the finished files to `hook.outputDir`
  even if you reuse the cache.
- `hook.tempDir`: Store scratch files for this run. Tola removes it when the command exits.

Hooks in this stage read the same files and write their own outputs. If one step needs a file
created by another, put both steps in one script.

To link the generated file from Typst, import `output-url` from `@tola/address:0.0.0` and use
`#link(output-url("build.txt"))[Build info]`. Tola checks the target after the hook runs.
If the files need minification or a hash in the URL, add that step to the hook.

### Reuse files during development

With `dev = "reuse"`, a development build uses the last successful outputs when the
site, script and settings, watched inputs, and output declarations are unchanged.

List additional inputs in `rerun-on`. Use `dev = "run"` if the script depends on the
network, time, environment variables, or files outside the site. Production builds
always run enabled hooks. Use `tola dev -v` to see which outputs changed.


For a complete Nu example that writes `public/build.txt`, see [`@tola-scripts/context`](tola-help://scripts/context).

Example: List the built pages as JSON:

Save this as `write-page-list.nu` next to `tola.toml`. It uses the Nushell bundled
with Tola.

```nu
use tola get-context

let hook = get-context | get hook
glob ($hook.inputDir | path join "**/*.html")
| each {|file| $file | path relative-to $hook.inputDir }
| sort
| to json
| save ($hook.outputDir | path join "pages.json")
```

Add the hook to `tola.toml`:

```toml
[[hooks.generate-outputs]]
name = "page-list"
nu = "write-page-list.nu"
outputs = [{ file = "pages.json" }]
```

In a site created with `tola init`, write a heading in `content/index.typ`, then run
`tola build` from the site root. With the default publish directory, open
`public/pages.json`. It lists HTML filenames relative to that directory, such as
`index.html`.

Example: Pagefind search:

To use another search tool, keep the same input and output directories and declare
its files in `outputs`.

With Node.js and npm available, save this as `index-search.nu` in the site root.
Tola runs this script with its built-in Nushell.

```nu
use tola get-context

let hook = get-context | get hook
let index_dir = $hook.outputDir | path join "pagefind"
^npx -y pagefind --site $hook.inputDir --output-path $index_dir
```

Add the hook to `tola.toml`.

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "index-search.nu"
outputs = [{ tree = "pagefind" }]
```

In a site created with `tola init`, add this to `content/index.typ`. It loads the
[Pagefind search components](https://pagefind.app/docs/) from the hook's output.

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/site:0.0.0": site
#html.link(rel: "stylesheet", href: output-url("pagefind/pagefind-component-ui.css"))
#html.elem("script", attrs: (
  type: "module",
  src: output-url("pagefind/pagefind-component-ui.js"),
))[]
#html.elem("pagefind-config", attrs: ("base-url": site.base-path))[]
#html.elem("pagefind-modal-trigger", attrs: (placeholder: "Search"))[]
#html.elem("pagefind-modal")[]
```

Run `tola build` from the site root. The default output includes the search bundle under
`public/pagefind/`. To try it locally, run `tola preview` and open the printed URL.
Click Search on the home page and enter a word from one of your pages."#;
}

/// Run a command with a read-only copy of the published site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.after-publish", collection = array_table)]
pub struct AfterPublishHookConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Command and arguments to run from the site root. Use `argv` or `nu`.
    /// Read the published files from `hook.inputDir`. If the command fails, the site is
    /// already published.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"skip"`, `tola dev` skips the hook. Use `"run"` to run it
    /// after a successful development build.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns.
    #[serde(rename = "rerun-on")]
    pub rerun_on: RerunOn,
}

impl AfterPublishHookConfig {
    /// What `tola help "[hooks.after-publish]"` adds under its table.
    pub const HELP: &'static str = r#"
### Read the published files

Read the published files for this run from the read-only `hook.inputDir`. Keep files
across runs in `hook.cacheDir`, and use `hook.tempDir` for scratch files. To add files
to the publication, use `generate-outputs`.

### When the hook runs in development

`tola dev` skips this stage by default. Set `dev = "run"` to run it after the browser
can load the published files. If several builds finish while the hook is busy, the
hook takes the latest one next.

If the site, command, and `rerun-on` inputs are unchanged, Tola keeps the previous
result, even a failure. Stopping `tola dev` cancels running and pending work.

Example: Copy the published site:

With `rsync` available, save this as `upload.nu` in the site root. Replace the example
destination with a directory you can write to. The trailing slash on the source tells
`rsync` to copy the directory's contents.

```nu
use tola get-context

let input_dir = get-context | get hook.inputDir
^rsync -a -- ($input_dir + "/") "user@example.com:/srv/site/"
```

Add this to `tola.toml`.

```toml
[[hooks.after-publish]]
name = "upload"
nu = "upload.nu"
```

Run `tola build` from the site root. Tola publishes the site, then the hook uploads
the files from `hook.inputDir`. If the upload fails, the command exits with an error.
The published files are still on disk.

Example: Flag a draft marker on the published home page:

In a site created with `tola init`, write `DRAFT-MARKER` on a line in `content/index.typ`.
Save this as `check-home.nu` next to `tola.toml`:

```nu
use tola [get-context diagnostic]

let home = get-context | get hook.inputDir | path join "index.html"

let issues = if (open --raw $home | str contains "DRAFT-MARKER") {
  [
    (
      diagnostic make "Remove DRAFT-MARKER from the home page." --code draft-marker
    )
  ]
} else {
  []
}
$issues | diagnostic submit
```

Add this to `tola.toml`:

```toml
[[hooks.after-publish]]
name = "check-home"
nu = "check-home.nu"
dev = "run"
```

Run `tola build` from the site root. The new home page is published and Tola prints a
warning about the marker. Remove the marker and build again to clear the warning.
With `dev = "run"`, this check also runs after a development build publishes."#;
}

impl Default for BeforeBuildHookConfig {
    fn default() -> Self {
        Self {
            origin: None,
            enable: true,
            name: String::new(),
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Run,
            rerun_on: RerunOn::default(),
            generates: Vec::new(),
        }
    }
}

impl Default for OutputCommandConfig {
    fn default() -> Self {
        Self {
            origin: None,
            enable: true,
            name: String::new(),
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Run,
            rerun_on: RerunOn::default(),
            outputs: Vec::new(),
        }
    }
}

impl Default for AfterPublishHookConfig {
    fn default() -> Self {
        Self {
            origin: None,
            enable: true,
            name: String::new(),
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Skip,
            rerun_on: RerunOn::default(),
        }
    }
}

macro_rules! hook_actions {
    ($($entry:ty),+ $(,)?) => {
        $(impl $entry {
            /// Validate and return this hook's executable action.
            pub fn action(&self) -> Result<Action, String> {
                Action::from_fields(self.nu.clone(), self.argv.clone(), self.args.clone())
            }

            /// Set the already validated module action on a build hook.
            pub(crate) fn set_action(&mut self, action: Action) {
                match action {
                    Action::Nu { path, args } => {
                        self.argv = None;
                        self.nu = Some(path);
                        self.args = Some(args);
                    }
                    Action::Argv(argv) => {
                        self.argv = Some(argv);
                        self.nu = None;
                        self.args = None;
                    }
                }
            }
        })+
    };
}

hook_actions!(
    BeforeBuildHookConfig,
    OutputCommandConfig,
    AfterPublishHookConfig
);

fn validate_command(
    name: &str,
    action: Result<Action, String>,
    rerun_on: &RerunOn,
    index: usize,
    fields: HookEntryFields,
    diag: &mut ConfigDiagnostics,
) {
    if let Err(reason) = action {
        let key = message_key(fields.argv, index);
        diag.error_with_help(
            fields.argv,
            format!("`{key}`: {reason}"),
            "provide a `nu` script or an `argv` array",
        );
    }
    validate_name(name, index, fields.name, diag);
    for (list, entry, error) in rerun_on.validate(false) {
        let field = fields.rerun_on;
        let key = message_key(field, index);
        diag.error_with_help(
            field,
            format!("`{key}.{list}[{entry}]`: {error}"),
            "write a path or glob relative to the site root",
        );
    }
}

pub(crate) fn hook_identity(stage: HookStage, name: &str) -> String {
    format!("the `{name}` hook in `hooks.{}`", stage.as_str())
}

fn validate_dev(
    dev: DevParticipation,
    reuse_allowed: bool,
    index: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    if dev == DevParticipation::Reuse && !reuse_allowed {
        let key = message_key(field, index);
        diag.error_with_help(
            field,
            format!("`{key}` is `\"reuse\"`, which only `generate-outputs` accepts"),
            "Use `\"run\"` or `\"skip\"` for this stage",
        );
    }
}

fn validate_name(name: &str, index: usize, field: FieldPath, diag: &mut ConfigDiagnostics) {
    let key = message_key(field, index);
    if name.trim().is_empty() {
        diag.error_with_help(
            field,
            format!("`{key}` is blank"),
            "name the command, such as `name = \"css-build\"`",
        );
    } else if name.chars().any(char::is_whitespace) {
        diag.error_with_help(
            field,
            format!("`{key}` is not one word"),
            "Remove the whitespace, as in `css-build`",
        );
    } else if name.starts_with("@module/") {
        diag.error_with_help(
            field,
            format!("`{key}` starts with reserved `@module/`"),
            "choose a name for the site hook",
        );
    }
}

fn validate_name_is_unique<'a>(
    named: &mut Vec<(&'a str, usize)>,
    name: &'a str,
    index: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    if name.trim().is_empty() {
        return;
    }
    if let Some((_, first)) = named.iter().find(|(seen, _)| *seen == name) {
        diag.error_with_help(
            field,
            format!(
                "`{}` repeats `{}`",
                message_key(field, index),
                message_key(field, *first)
            ),
            "give each entry its own name",
        );
        return;
    }
    named.push((name, index));
}

fn validate_site_relative(
    path: &Path,
    hook_index: usize,
    entry_index: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    let key = format!("{}[{entry_index}]", message_key(field, hook_index));
    // A hook names a file or directory: the site root itself is not one.
    if path == Path::new(".") {
        diag.error_with_help(
            field,
            format!("`{key}` is `.`"),
            "name a file or directory inside the site root",
        );
        return;
    }
    crate::config::section::path::validate_site_relative_path(path, &key, field, diag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::diagnostic_mentioning;

    #[test]
    fn reuse_is_only_for_generate_outputs() {
        let config: HooksConfig = toml::from_str(
            r#"
[[generate-outputs]]
name = "search"
argv = ["search"]
dev = "reuse"
outputs = [{ file = "search.json" }]
"#,
        )
        .unwrap();
        assert_eq!(config.generate_outputs[0].dev, DevParticipation::Reuse);
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        assert!(diagnostics.into_result().is_ok());

        for source in [
            r#"
[[before-build]]
name = "prepare"
argv = ["prepare"]
dev = "reuse"
"#,
            r#"
[[after-publish]]
name = "deploy"
argv = ["deploy"]
dev = "reuse"
"#,
        ] {
            let config: HooksConfig = toml::from_str(source).unwrap();
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate(&mut diagnostics);
            let diagnostics = diagnostics.into_result().unwrap_err();
            assert!(
                diagnostics.errors().iter().any(|diagnostic| diagnostic
                    .message
                    .contains("only `generate-outputs` accepts")),
                "{diagnostics}"
            );
        }
    }

    #[test]
    fn development_inputs_match_participation() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
name = "prepare"
argv = ["prepare"]
rerun-on = { include = [" before "] }

[[before-build]]
name = "production-prepare"
argv = ["production-prepare"]
dev = "skip"
rerun-on = { include = ["production-input"] }

[[before-build]]
name = "disabled-prepare"
argv = ["disabled-prepare"]
enable = false
rerun-on = { include = ["disabled-input"] }

[[generate-outputs]]
name = "generate"
argv = ["generate"]
rerun-on = { include = ["generated/*.json"] }
outputs = [{ file = "search.json" }]

[[generate-outputs]]
name = "disabled-generate"
argv = ["disabled-generate"]
enable = false
rerun-on = { include = ["disabled-generated"] }

[[after-publish]]
name = "production-consumer"
argv = ["production-consumer"]
rerun-on = { include = ["production-consumer-input"] }

[[after-publish]]
name = "development-consumer"
argv = ["development-consumer"]
dev = "run"
rerun-on = { include = ["consumer"] }

[[after-publish]]
name = "disabled-consumer"
argv = ["disabled-consumer"]
enable = false
dev = "run"
rerun-on = { include = ["disabled-consumer-input"] }
"#,
        )
        .unwrap();
        let root = std::path::absolute("test-site").unwrap();
        let selections = config.development_rerun_selections(&root).unwrap();
        assert_eq!(
            selections
                .iter()
                .flat_map(RerunSelection::roots)
                .collect::<Vec<_>>(),
            [" before ", "generated", "consumer"].map(|path| root.join(path))
        );
        assert!(selections[1].matches(&root.join("generated/search.json")));
        assert!(!selections[1].matches(&root.join("generated/search.txt")));
    }

    #[test]
    fn disabled_hook_declarations_are_validated() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
enable = false
argv = [" "]
generates = ["../source"]

[[generate-outputs]]
enable = false
name = " "
rerun-on = { include = ["/outside"] }

[[after-publish]]
enable = false
rerun-on = { include = ["../committed"] }
"#,
        )
        .unwrap();
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        crate::config::loading::validate_hook_output_boundaries(
            &config,
            Path::new("/site"),
            Path::new("/site/tola.toml"),
            Path::new("/site/public"),
            None,
            None,
            &mut diagnostics,
        );
        for field in [
            BeforeBuildHookConfig::FIELDS.argv,
            BeforeBuildHookConfig::FIELDS.generates,
            OutputCommandConfig::FIELDS.name,
            OutputCommandConfig::FIELDS.rerun_on,
            AfterPublishHookConfig::FIELDS.rerun_on,
        ] {
            assert!(
                diagnostics
                    .errors()
                    .iter()
                    .any(|error| error.field == field)
            );
        }
    }

    fn input_generator(name: &str, output: &str) -> BeforeBuildHookConfig {
        BeforeBuildHookConfig {
            name: name.into(),
            argv: Some(vec!["generator".into()]),
            generates: vec![output.into()],
            ..BeforeBuildHookConfig::default()
        }
    }

    #[test]
    fn stages_require_their_output_shape() {
        for source in [
            "[[before-build]]\ngenerates = [{ file = \"index.json\" }]\n",
            "[[generate-outputs]]\noutputs = [\"index.json\"]\n",
            "[[after-publish]]\noutputs = [\"index.json\"]\n",
        ] {
            assert!(toml::from_str::<HooksConfig>(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rerun_paths_stay_inside_the_site() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
name = "echo-bad"
argv = ["echo", "bad"]
rerun-on = { include = ["/src"], exclude = [{ project = "../templates" }] }
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate(&mut diag);

        assert_eq!(diag.errors().len(), 2);
        assert!(
            diag.errors()
                .iter()
                .all(|error| error.field == BeforeBuildHookConfig::FIELDS.rerun_on),
            "{:?}",
            diag.errors()
        );
    }

    /// One enabled hook in each stage, every one named `name` and running `command`.
    fn one_hook_per_stage(name: &str, command: &str) -> HooksConfig {
        HooksConfig {
            before_build: vec![BeforeBuildHookConfig {
                name: name.into(),
                argv: Some(vec![command.into()]),
                generates: vec!["generated".into()],
                ..BeforeBuildHookConfig::default()
            }],
            generate_outputs: vec![OutputCommandConfig {
                name: name.into(),
                argv: Some(vec![command.into()]),
                outputs: vec![CommandOutput::File(
                    tola_address::OutputPath::parse("search.json").unwrap(),
                )],
                ..OutputCommandConfig::default()
            }],
            after_publish: vec![AfterPublishHookConfig {
                name: name.into(),
                argv: Some(vec![command.into()]),
                ..AfterPublishHookConfig::default()
            }],
        }
    }

    #[test]
    fn enabled_names_require_one_word() {
        for name in ["", " ", "build stylesheet", "build\tstylesheet"] {
            let hooks = one_hook_per_stage(name, "generator");
            let mut diagnostics = ConfigDiagnostics::new();

            hooks.validate(&mut diagnostics);

            assert_eq!(
                diagnostics
                    .errors()
                    .iter()
                    .map(|error| error.field)
                    .collect::<Vec<_>>(),
                [
                    BeforeBuildHookConfig::FIELDS.name,
                    OutputCommandConfig::FIELDS.name,
                    AfterPublishHookConfig::FIELDS.name,
                ],
                "{name:?}",
            );
        }
    }

    #[test]
    fn names_are_unique_within_stage() {
        let mut hooks = one_hook_per_stage("styles", "generator");
        let mut diagnostics = ConfigDiagnostics::new();
        hooks.validate(&mut diagnostics);
        assert!(diagnostics.errors().is_empty());

        hooks.before_build.push(hooks.before_build[0].clone());
        hooks
            .generate_outputs
            .push(hooks.generate_outputs[0].clone());
        hooks.after_publish.push(hooks.after_publish[0].clone());
        let mut diagnostics = ConfigDiagnostics::new();
        hooks.validate(&mut diagnostics);
        assert_eq!(
            diagnostics
                .errors()
                .iter()
                .map(|error| error.field)
                .collect::<Vec<_>>(),
            [
                BeforeBuildHookConfig::FIELDS.name,
                OutputCommandConfig::FIELDS.name,
                AfterPublishHookConfig::FIELDS.name,
            ],
        );
    }

    /// Assert that a load of one schema whose single before-build hook declares `declared`
    /// reports it as a refused hook output.
    fn assert_hook_output_refused(root: &Path, declared: &str) {
        let mut schema = crate::config::SiteConfigSchema::default();
        schema.vendor.path = Some("vendor".into());
        schema.hooks.before_build = vec![input_generator("generator", declared)];

        let error = schema
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap_err();

        let diagnostic = diagnostic_mentioning(&error, declared);
        assert_eq!(diagnostic.code, tola_diagnostic::codes::config::INVALID);
        assert_eq!(
            diagnostic.notes,
            [BeforeBuildHookConfig::FIELDS.generates.as_str()]
        );
    }

    #[test]
    fn hook_outputs_cannot_reach_owned_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for declared in [
            "public/assets",
            ".tola/cache",
            ".vendor-swap",
            ".vendor-swap/marker.txt",
        ] {
            assert_hook_output_refused(root, declared);
        }

        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let workspace = root.join(".vendor-swap");
            std::fs::create_dir(&workspace).unwrap();
            std::os::unix::fs::symlink(&workspace, root.join("vendored")).unwrap();
            assert_hook_output_refused(root, "vendored/marker.txt");
        }
    }

    #[test]
    fn one_hook_owns_each_output_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let hooks = HooksConfig {
            before_build: vec![
                input_generator("directory", "generated"),
                input_generator("file", "generated/site.css"),
            ],
            ..HooksConfig::default()
        };
        let mut diag = ConfigDiagnostics::new();

        crate::config::loading::validate_hook_output_boundaries(
            &hooks,
            root,
            &root.join("tola.toml"),
            &root.join("public"),
            None,
            None,
            &mut diag,
        );

        assert_eq!(diag.errors().len(), 1);
        assert_eq!(
            diag.errors()[0].field,
            BeforeBuildHookConfig::FIELDS.generates
        );
    }
}
