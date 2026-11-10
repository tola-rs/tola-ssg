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

/// External commands Tola runs around a build, grouped by stage. Each stage waits for its
/// commands to exit before it continues.
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
    pub const HELP: &'static str = "\
Choose the stage by what the command produces: `before-build` writes site inputs before source
discovery; `generate-outputs` adds final files after the Bundle compiles; `after-publish` consumes
the committed site. Bundle, configured assets, and generated outputs pass the same conflict and
reference checks before publication.

The commands differ in which stages they run. Hooks are enabled by default. Set `enable = false` to disable one, or use `enabled-by`
to select a boolean site option. Declare a `nu` script with optional `args`, or an
`argv` array with an external executable and its arguments.

| Command | Build mode | before-build | generate-outputs | after-publish |
| --- | --- | --- | --- | --- |
| `tola build` | `prod` | run | run | run |
| `tola check` | `prod` | run | run | skip |
| `tola preview` | `prod` | run | run | skip |
| `tola dev` | `dev` | per `dev` | per `dev` | per `dev` |
| `tola inspect` | — | skip | skip | skip |
| `tola vendor` | `prod` | skip | skip | skip |

One `tola build` runs in this order: `before-build` hooks run first → discovery follows
`content-dir`, the root Bundle compiles, and `[assets]` files are collected (that complete result
is the candidate) → `generate-outputs` hooks add their files to the candidate → conflicts and
references are checked → publication writes the whole candidate into `build.publish-dir` →
`after-publish` hooks consume the published site.

Build and publication differ: building produces the candidate and checks it; publication does one
thing — writes the checked result into `build.publish-dir` as a whole (a successful build replaces
the whole directory). The first two stages run before publication, a failure in any of those steps
publishes nothing and leaves the previous output in place; `after-publish` runs after publication,
and a failure there cannot undo it.

`check` and `preview` build from sources with production settings; they do not write
`build.publish-dir`. `dev` serves successful builds in memory. The `dev` field defaults to `run`
for the first two stages and `skip` for after-publish.

`generate-outputs` also accepts `dev = \"reuse\"`. In development, a command whose observed inputs
are unchanged reuses the previous successful revision's outputs instead of running. The command
must then be a pure function of the built site, its `rerun-on` paths, its command, and its declared
outputs. A command that reads time, network, environment state, or files outside the site keeps
`dev = \"run\"`. Production builds always run every hook.

Commands run from the site root as finite argv arrays, without an implicit shell. Within a stage,
entries run in configuration order; failure stops the remaining entries. Put dependent steps or
conditions in the site's own runner, for example `argv = [\"just\", \"css\"]`. Tola does not
require `just`. Output generators each read the same upstream snapshot and write to a private
directory: a later generator cannot read an earlier generator's new outputs.

Any hook can submit an unlocated diagnostic batch through `TOLA_REPORT_PATH`, including from Nu
with `use tola diagnostic` and `diagnostic submit`. Warnings do not fail a successful command;
errors do. An after-publish error cannot undo publication. A reused development `generate-outputs`
result retains its report. No report and an empty batch are distinct. Independent checks and
unsaved editor snapshots are not supported by this report interface.

Every invocation receives these environment variables:

| Variable | Purpose | Stages |
| --- | --- | --- |
| `TOLA_REPORT_PATH` | optional complete diagnostic report | every stage |
| `TOLA_PROJECT_ROOT` | absolute resolved project root | every stage |
| `TOLA_CONFIG_PATH` | absolute resolved path to the configuration file | every stage |
| `TOLA_CONTENT_ROOT` | absolute resolved content directory | every stage |
| `TOLA_EXECUTABLE` | the current Tola executable | every stage |
| `TOLA_INPUT_SCOPE` | the caller's input policy: `online`, `offline`, or `pure` | every stage |
| `TOLA_HOOK_STAGE` | the stage: `before-build`, `generate-outputs`, or `after-publish` | every stage |
| `TOLA_BUILD_MODE` | the build mode: `prod` or `dev` | every stage |
| `TOLA_HOOK_CACHE_DIR` | `.tola/hooks/<stage>/<entry name>`, persisting across runs, such as `.tola/hooks/generate-outputs/search` | every stage |
| `TOLA_HOOK_TEMP_DIR` | this invocation's own temporary directory, removed afterwards | every stage |
| `TOLA_HOOK_INPUT_DIR` | the command's input: a read-only snapshot of everything built upstream, laid out like the published directory | `generate-outputs`, `after-publish` |
| `TOLA_HOOK_OUTPUT_DIR` | empty on every run; a relative path written there is that path under `build.publish-dir` | `generate-outputs` |

The command owns what it puts in the cache directory; Tola only creates it, never cleans it, and
never reuses what it holds. The temporary and output directories are new on every run. `TMPDIR`,
`TMP`, and `TEMP` name `TOLA_HOOK_TEMP_DIR`. A variable a stage does not define is absent, never
inherited from the caller.

`rerun-on` adds extra inputs, for example
`rerun-on = { include = [\"styles/**/*.css\", \"package.json\"], exclude = [\"styles/vendor\"] }`.
Each pattern is relative to the site. A matched directory includes or excludes its descendants.
Changes to selected inputs trigger a whole development build. Add inputs a tool reads that Tola
does not otherwise observe, such as its stylesheet sources, runner, manifest, or lockfile.
The stage pages show a stylesheet producer, a search index, and a publication consumer.

Hooks are trusted scripts with the caller's permissions. `--offline` and `--pure` restrict Tola's
own input reads, not these commands. Superseded development builds cancel their pre-publication
commands, but script side effects are not rolled back. After-publish development scheduling is
explained on that stage's page.";
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
            key: Option<&str>,
            options: &toml::Table,
            stage: HookStage,
            name: &str,
        ) -> anyhow::Result<()> {
            if let (true, Some(key)) = (*enable, key) {
                let identity = hook_identity(stage, name);
                *enable = match options.get(key) {
                    Some(toml::Value::Boolean(value)) => *value,
                    Some(_) => anyhow::bail!("{identity}: option `{key}` must be a boolean"),
                    None => anyhow::bail!("{identity} is enabled by unset option `{key}`"),
                };
            }
            Ok(())
        }
        for hook in &mut self.before_build {
            select(
                &mut hook.enable,
                hook.enabled_by.as_deref(),
                options,
                HookStage::BeforeBuild,
                &hook.name,
            )?;
        }
        for hook in &mut self.generate_outputs {
            select(
                &mut hook.enable,
                hook.enabled_by.as_deref(),
                options,
                HookStage::GenerateOutputs,
                &hook.name,
            )?;
        }
        for hook in &mut self.after_publish {
            select(
                &mut hook.enable,
                hook.enabled_by.as_deref(),
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
                    hook.enabled_by.as_deref(),
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
                    command.enabled_by.as_deref(),
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
                    hook.enabled_by.as_deref(),
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
    enabled_by: FieldPath,
    dev: FieldPath,
    rerun_on: FieldPath,
}

impl HookEntryFields {
    const BEFORE_BUILD: Self = Self {
        name: BeforeBuildHookConfig::FIELDS.name,
        argv: BeforeBuildHookConfig::FIELDS.argv,
        enabled_by: BeforeBuildHookConfig::FIELDS.enabled_by,
        dev: BeforeBuildHookConfig::FIELDS.dev,
        rerun_on: BeforeBuildHookConfig::FIELDS.rerun_on,
    };

    const GENERATE_OUTPUTS: Self = Self {
        name: OutputCommandConfig::FIELDS.name,
        argv: OutputCommandConfig::FIELDS.argv,
        enabled_by: OutputCommandConfig::FIELDS.enabled_by,
        dev: OutputCommandConfig::FIELDS.dev,
        rerun_on: OutputCommandConfig::FIELDS.rerun_on,
    };

    const AFTER_PUBLISH: Self = Self {
        name: AfterPublishHookConfig::FIELDS.name,
        argv: AfterPublishHookConfig::FIELDS.argv,
        enabled_by: AfterPublishHookConfig::FIELDS.enabled_by,
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
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell. This stage sets
    /// no `TOLA_HOOK_INPUT_DIR` or `TOLA_HOOK_OUTPUT_DIR` (`generate-outputs` does): the command
    /// writes the site inputs itself, with no build directory to read.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Embedded Nu entry relative to the site root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Literal arguments to the Nu entry.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Boolean key in the site's options.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<String>,
    /// Whether `tola dev` runs this entry: `"run"` (the default) or `"skip"`.
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
    pub const HELP: &'static str = "\
This stage writes the files Typst and `[assets]` read.

Here is a complete Tailwind CSS example: `just` runs the task, Deno executes `@tailwindcss/cli` to
build the stylesheet, the `before-build` hook runs it before source discovery, and `[assets]` maps
`static/web-assets/tailwind-output` to `/assets/tailwind-output`. Input and product stay apart, and
the paths say which is which: the input is `static/tailwind-sources/site.css` (never published),
the product is `static/web-assets/tailwind-output/site.css` (published with the tree at
`/assets/tailwind-output/site.css`).

`justfile`:

```just
css:
    deno task css
```

`deno.json`:

```json
{
  \"imports\": {
    \"@tailwindcss/cli\": \"npm:@tailwindcss/cli@4.3.3\",
    \"tailwindcss\": \"npm:tailwindcss@4.3.3\"
  },
  \"tasks\": {
    \"css\": \"deno run -A npm:@tailwindcss/cli@4.3.3 -i static/tailwind-sources/site.css -o static/web-assets/tailwind-output/site.css --minify\"
  }
}
```

`tola.toml`:

```toml
[[hooks.before-build]]
name = \"tailwind\"
argv = [\"just\", \"css\"]
generates = [\"static/web-assets/tailwind-output/site.css\"]
rerun-on = { include = [\"static/tailwind-sources\", \"deno.json\", \"deno.lock\", \"justfile\"] }
```

Files reach browsers only once they are published: the `[assets]` below maps the whole
`static/web-assets` tree to `/assets`, so the product is published as
`/assets/tailwind-output/site.css`. Generated data can also stay unpublished and serve Typst alone:
a page imports it to produce a document or another output.

`tola.toml`:

```toml
[assets]
trees = [{ source = \"static/web-assets\", url-prefix = \"/assets\" }]
```

Every page links it in its head (`site.typ` or a page template):

```typst
#html.link(rel: \"stylesheet\", href: asset-url(\"/assets/tailwind-output/site.css\"))
```

Tailwind's input CSS must name the content and template files it scans. Hooks are for commands that
run once: Tola has no plans to support long-running commands such as a tool's watch mode, which
would add great complexity to development, the build flow, the hook mechanism, and analysis. Tola
owns development watching, and the hook mechanism provides only the necessary infrastructure:
incremental builds, caches, and persistence for a command's own products are the producer's own
logic, with environment variables such as `TOLA_HOOK_CACHE_DIR` and `TOLA_HOOK_TEMP_DIR` as Tola's
interface.

`generates` states which files the command writes: Tola checks those paths exist after the command
runs and watches them during development.

They are inputs to the site, not outputs: only after every hook in this stage has run does Tola
build — discovery follows `content-dir`, the root Bundle compiles, `[assets]` files are collected,
then references are checked and the site is published.

The command environment for this stage:
- `TOLA_HOOK_STAGE`: this stage, `before-build`.
- `TOLA_BUILD_MODE`: `prod` (`build`, `check`, `preview`, `vendor`) or `dev` (`tola dev`).
- `TOLA_HOOK_CACHE_DIR`: persists across runs below `.tola/hooks/before-build/<entry name>`;
  the command owns its contents and Tola only creates the directory — keep incremental products
  there, for example `\"$TOLA_HOOK_CACHE_DIR\"/tailwind`.
- `TOLA_HOOK_TEMP_DIR`: this invocation's own temporary directory, removed afterwards; `TMPDIR`,
  `TMP`, and `TEMP` name it too.

There is no `TOLA_HOOK_INPUT_DIR` or `TOLA_HOOK_OUTPUT_DIR`: the command writes the site's input
files directly.";
}

/// Produces declared final output files after the site program compiles, adding them to the
/// candidate output graph before reference checks. They are unavailable to the preceding Typst
/// compilation, but pages may link their known URLs without an `[assets]` declaration, and
/// reference checks validate those links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.generate-outputs", collection = array_table)]
pub struct OutputCommandConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell. It reads the
    /// upstream snapshot below `TOLA_HOOK_INPUT_DIR` and writes declared outputs below
    /// `TOLA_HOOK_OUTPUT_DIR`; written paths are relative to the final site output.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Embedded Nu entry relative to the site root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Literal arguments to the Nu entry.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Boolean key in the site's options.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<String>,
    /// Whether `tola dev` runs this hook: `"run"` (the default), `"reuse"`, or `"skip"`.
    /// `reuse` skips the command when its inputs are unchanged from the previous successful
    /// development revision and reuses that revision's outputs.
    #[config(values = DevParticipation::REUSE_VALUES)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns. For
    /// `dev = "reuse"`, the selected inputs are also part of the hook's fingerprint.
    #[serde(rename = "rerun-on")]
    pub rerun_on: RerunOn,
    /// Final site output paths this hook adds to the candidate, written relative to
    /// `build.publish-dir` (default `public`), as `{ file = "manifest.json" }` or
    /// `{ tree = "assets/thumbnails" }`. Required: the written files must match these
    /// declarations, every path stays outside `_tola`, and a path another producer owns
    /// fails the build instead of replacing that output.
    #[config(collection = inline)]
    pub outputs: Vec<CommandOutput>,
}

impl OutputCommandConfig {
    /// What `tola help "[hooks.generate-outputs]"` adds under its table.
    pub const HELP: &'static str = "\
This stage generates final files from the compiled site.

Here is a complete Pagefind example: it reads the HTML snapshot and writes a search index that
joins the same output set as the pages it indexes.

`justfile`:

```just
search:
    deno task search
```

`deno.json`:

```json
{
  \"imports\": { \"pagefind\": \"npm:pagefind@1.5.2\" },
  \"tasks\": {
    \"search\": \"deno run -A npm:pagefind@1.5.2 --site \\\"$TOLA_HOOK_INPUT_DIR\\\" --output-path \\\"$TOLA_HOOK_OUTPUT_DIR/assets/pagefind-search\\\"\"
  }
}
```

`tola.toml`:

```toml
[[hooks.generate-outputs]]
name = \"search\"
argv = [\"just\", \"search\"]
outputs = [{ tree = \"assets/pagefind-search\" }]
rerun-on = { include = [\"deno.json\", \"deno.lock\", \"justfile\"] }
```

Every generator reads the same compiled site. Put transformations that depend on one another in the
same command.

The declared output needs no `[assets]` entry. `asset-url` resolves only `[assets]` declarations,
and errors unless the file was published. Link a known output path with
`output-url(\"assets/pagefind-search/pagefind-ui.css\")`. These files do not exist during the
preceding Typst compilation, so Tola cannot compute their asset URLs then. Tola does not minify or
cache-bust a hook's products; the producer's command handles all of that itself: Pagefind, for
example, gives its index and fragment files content-hashed names.

A page uses it like this: the template head links the generated CSS and JavaScript directly:

```typst
#import \"@tola/address:0.0.0\": output-url

#html.link(rel: \"stylesheet\", href: output-url(\"assets/pagefind-search/pagefind-ui.css\"))
#html.elem(\"script\", attrs: (src: output-url(\"assets/pagefind-search/pagefind-ui.js\")))
```

In development, `dev = \"reuse\"` records the built site's output identities, the `rerun-on` paths'
contents, the command, and the declared outputs. When all of them match the previous successful
revision, the command does not run and those outputs are reused unchanged. List every site file the
command reads in `rerun-on`; an omitted input can make reuse serve stale output. The value is never
used by production builds. The default remains `dev = \"run\"`. Run `tola dev -v` to list every
changed output path.

Like every hook stage, this stage runs commands once: Tola does not run long-lived commands such as
a watch mode, and Tola owns development watching; incremental builds and caches for the command's
own products are the producer's logic, with `TOLA_HOOK_CACHE_DIR` available.

The command environment for this stage:
- `TOLA_HOOK_INPUT_DIR`: the command's input — a read-only snapshot of what the build produced
  upstream: the compiled pages and `[assets]` files, laid out like the published directory, with
  read-only file modes. It holds nothing another hook wrote in this stage, it is not
  `build.content-dir`, it is not `build.publish-dir`, and it exists only while this command runs.
  The example reads it with `--site \"$TOLA_HOOK_INPUT_DIR\"`.
- `TOLA_HOOK_OUTPUT_DIR`: empty on every run; a relative path written there is that path under
  `build.publish-dir` — writing `\"$TOLA_HOOK_OUTPUT_DIR\"/assets/pagefind-search` publishes at
  `assets/pagefind-search`, served as `/assets/pagefind-search/…`.
- `TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE`, `TOLA_HOOK_CACHE_DIR`, and `TOLA_HOOK_TEMP_DIR`: as in
  every stage — the stage name, the mode (`prod` or `dev`), a cross-run cache directory such as
  `\"$TOLA_HOOK_CACHE_DIR\"/search/index.json`, and this invocation's temporary directory. A cache
  hit still has to write the declared outputs again.";
}

/// Consumes one committed revision through a read-only view, without declaring further site
/// outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.after-publish", collection = array_table)]
pub struct AfterPublishHookConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell.
    /// `TOLA_HOOK_INPUT_DIR` is a read-only view of the published site, so a failure here cannot
    /// undo publication.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Embedded Nu entry relative to the site root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Literal arguments to the Nu entry.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Boolean key in the site's options.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<String>,
    /// Whether `tola dev` runs this hook: `"skip"` (the default) or `"run"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns.
    #[serde(rename = "rerun-on")]
    pub rerun_on: RerunOn,
}

impl AfterPublishHookConfig {
    /// What `tola help "[hooks.after-publish]"` adds under its table.
    pub const HELP: &'static str = "\
This stage consumes a successfully committed site, for example to sync it to a host:

`justfile`:

```just
rsync:
    rsync -a --delete \"$TOLA_HOOK_INPUT_DIR/\" user@example.com:/srv/site/
```

`tola.toml`:

```toml
[[hooks.after-publish]]
name = \"rsync\"
argv = [\"just\", \"rsync\"]
```

The recipe syncs the site just published to a host. after-publish runs after publication:
`TOLA_HOOK_INPUT_DIR` is a read-only snapshot — the site just published — and it does not change
while the command reads. When this stage's hooks run, the preceding build counts as successful: a
command's failure does not undo publication (the built site is already written into
`build.publish-dir`). This stage is the after-publication wrap-up: a hook may work with the site —
upload it, notify something — but files it writes here are not part of this publication and are not
checked. To give the site more files, produce them in `generate-outputs`: those outputs are checked
before publication and published with the site.

`tola build` waits for these commands before exiting; a failure gives a failing command result
even though the output was written. `check`, `preview`, `inspect`, and `vendor` never run them.

Like every hook stage, this one runs commands once: Tola owns development watching, and incremental
builds and caches for the command's own products are the producer's logic
(`TOLA_HOOK_CACHE_DIR` is available).

Development skips this stage by default. With `dev = \"run\"`, consumers run serially without
holding up browser updates. While one revision is running, only the latest waiting revision is
kept, so intermediate revisions may receive no consumer call. A rebuild that changes nothing —
the same site, commands, and `rerun-on` contents as the previous run — keeps that run's result,
failure included, instead of running the commands again. Stopping `tola dev` cancels the running
command and discards waiting work. Use this mode for consumers that can handle the latest site,
rather than for an audit of every revision.

The command environment for this stage:
- `TOLA_HOOK_INPUT_DIR`: the command's input — a read-only snapshot of the site just committed,
  laid out like the published directory, so the whole directory is what a consumer uploads.
- `TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE`, `TOLA_HOOK_CACHE_DIR`, and `TOLA_HOOK_TEMP_DIR`: as in
  every stage — the stage name, the mode (`prod` or `dev`), a cross-run cache directory, and this
  invocation's temporary directory.

There is no `TOLA_HOOK_OUTPUT_DIR`: this stage adds no output.";
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
    enabled_by: Option<&str>,
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
    if enabled_by.is_some_and(|key| key.trim().is_empty()) {
        let field = fields.enabled_by;
        let key = message_key(field, index);
        diag.error_with_help(
            field,
            format!("`{key}` is blank"),
            "name a boolean site option",
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
