//! Finite commands around candidate construction and publication.
//!
//! The environment a hook command receives, and what a superseded build cancels, is written
//! for the site author in the section's and each stage's `HELP`, which `tola help config build.hooks`
//! and its stage pages render.

use crate::config::{ConfigDiagnostics, FieldPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tola_config::Config;

/// External commands Tola runs around a build, grouped by stage. Each stage waits for its
/// commands to exit before it continues.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "build.hooks")]
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
    /// What `tola help config build.hooks` adds under its child tables.
    pub const HELP: &'static str = "\
Choose the stage by what the command produces: `before-build` writes site inputs before source
discovery; `generate-outputs` adds final files after the Bundle compiles; `after-publish` consumes
the committed site. Bundle, configured assets, and generated outputs pass the same conflict and
reference checks before publication.

The commands differ in which stages they run. Every entry below also requires `enable = true`:

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

Commands run from the site root as finite argv arrays, without an implicit shell. Within a stage,
entries run in configuration order; failure stops the remaining entries. Put dependent steps or
conditions in the site's own runner, for example `command = [\"just\", \"css\"]`. Tola does not
require `just`. Output generators each read the same upstream snapshot and write to a private
directory: a later generator cannot read an earlier generator's new outputs.

Every invocation receives these environment variables:

| Variable | Purpose | Stages |
| --- | --- | --- |
| `TOLA_HOOK_STAGE` | the stage: `before-build`, `generate-outputs`, or `after-publish` | every stage |
| `TOLA_BUILD_MODE` | the build mode: `prod` or `dev` | every stage |
| `TOLA_HOOK_CACHE_DIR` | `.tola/hook-cache/<stage>/<entry name>`, persisting across runs, such as `.tola/hook-cache/generate-outputs/search` | every stage |
| `TOLA_HOOK_TEMP_DIR` | this invocation's own temporary directory, removed afterwards | every stage |
| `TOLA_HOOK_INPUT_DIR` | the command's input: a read-only snapshot of everything built upstream, laid out like the published directory | `generate-outputs`, `after-publish` |
| `TOLA_HOOK_OUTPUT_DIR` | empty on every run; a relative path written there is that path under `build.publish-dir` | `generate-outputs` |

The command owns what it puts in the cache directory; Tola only creates it, never cleans it, and
never reuses what it holds. The temporary and output directories are new on every run. `TMPDIR`,
`TMP`, and `TEMP` name `TOLA_HOOK_TEMP_DIR`. A variable a stage does not define is absent, never
inherited from the caller.

`rerun-on` adds literal site-relative paths, not globs; directories are watched recursively.
Their edits trigger a whole development build, not just one command. Add paths a tool reads but
Tola does not otherwise observe, such as its input directory, runner, manifest, or lockfile.
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
            map | hook | hook.command.as_slice()
        )
    }

    /// Additional literal paths whose edits rerun the commands the development session runs.
    pub fn development_rerun_paths(&self) -> impl Iterator<Item = &Path> {
        hook_stages!(
            self,
            keep | hook | hook.enable && hook.dev.participates_in_development(),
            map | hook | hook.rerun_on.iter()
        )
        .flatten()
        .map(PathBuf::as_path)
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        let mut named = Vec::new();
        for (index, hook) in self.before_build.iter().enumerate() {
            if !hook.enable {
                continue;
            }
            let fields = HookEntryFields::BEFORE_BUILD;
            diag.with_array_element(Self::FIELDS.before_build.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    &hook.command,
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
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
            if !command.enable {
                continue;
            }
            let fields = HookEntryFields::GENERATE_OUTPUTS;
            diag.with_array_element(Self::FIELDS.generate_outputs.as_str(), index, |diag| {
                validate_command(
                    &command.name,
                    &command.command,
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
            if !hook.enable {
                continue;
            }
            let fields = HookEntryFields::AFTER_PUBLISH;
            diag.with_array_element(Self::FIELDS.after_publish.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    &hook.command,
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
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
    command: FieldPath,
    rerun_on: FieldPath,
}

impl HookEntryFields {
    const BEFORE_BUILD: Self = Self {
        name: BeforeBuildHookConfig::FIELDS.name,
        command: BeforeBuildHookConfig::FIELDS.command,
        rerun_on: BeforeBuildHookConfig::FIELDS.rerun_on,
    };

    const GENERATE_OUTPUTS: Self = Self {
        name: OutputCommandConfig::FIELDS.name,
        command: OutputCommandConfig::FIELDS.command,
        rerun_on: OutputCommandConfig::FIELDS.rerun_on,
    };

    const AFTER_PUBLISH: Self = Self {
        name: AfterPublishHookConfig::FIELDS.name,
        command: AfterPublishHookConfig::FIELDS.command,
        rerun_on: AfterPublishHookConfig::FIELDS.rerun_on,
    };
}

/// The key one entry's field writes, as messages spell it: `build.hooks.before-build[1].command`.
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
#[config(section = "build.hooks.before-build", collection = array_table)]
pub struct BeforeBuildHookConfig {
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell. This stage sets
    /// no `TOLA_HOOK_INPUT_DIR` or `TOLA_HOOK_OUTPUT_DIR` (`generate-outputs` does): the command
    /// writes the site inputs itself, with no build directory to read.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this entry: `"run"` (the default) or `"skip"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Paths this hook watches, relative to the site root, whose edits trigger a `tola dev` build,
    /// not only the hook's own command.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
    /// Files or directories the hook's command generates, relative to the site root; they must not
    /// be inside `build.publish-dir` or `.tola`. After the hook's command, Tola checks that each
    /// declared path exists: the hook declares what it wrote, and Tola does not infer it. Declaring
    /// it does not publish it: publish it through `[assets]` or a Bundle `asset(...)`, or use it to
    /// produce a document or another output.
    #[config(collection = inline)]
    pub generates: Vec<PathBuf>,
}

impl BeforeBuildHookConfig {
    /// What `tola help config build.hooks.before-build` adds under its table.
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
[[build.hooks.before-build]]
name = \"tailwind\"
command = [\"just\", \"css\"]
generates = [\"static/web-assets/tailwind-output/site.css\"]
rerun-on = [\"static/tailwind-sources\", \"deno.json\", \"deno.lock\", \"justfile\"]
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

Every page links it in its head (`site/page.typ` and `site/not-found.typ`):

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
- `TOLA_HOOK_CACHE_DIR`: persists across runs below `.tola/hook-cache/before-build/<entry name>`;
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
#[config(section = "build.hooks.generate-outputs", collection = array_table)]
pub struct OutputCommandConfig {
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell. It reads the
    /// upstream snapshot below `TOLA_HOOK_INPUT_DIR` and writes declared outputs below
    /// `TOLA_HOOK_OUTPUT_DIR`; written paths are relative to the final site output.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this hook: `"run"` (the default) or `"skip"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Paths this hook watches, relative to the site root, whose edits trigger a `tola dev` build,
    /// not only the hook's own command.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
    /// Final site output paths this hook adds to the candidate, written relative to
    /// `build.publish-dir` (default `public`), as `{ file = "manifest.json" }` or
    /// `{ tree = "assets/thumbnails" }`. Required: the written files must match these
    /// declarations, every path stays outside `_tola`, and a path another producer owns
    /// fails the build instead of replacing that output.
    #[config(collection = inline)]
    pub outputs: Vec<CommandOutput>,
}

impl OutputCommandConfig {
    /// What `tola help config build.hooks.generate-outputs` adds under its table.
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
[[build.hooks.generate-outputs]]
name = \"search\"
command = [\"just\", \"search\"]
outputs = [{ tree = \"assets/pagefind-search\" }]
rerun-on = [\"deno.json\", \"deno.lock\", \"justfile\"]
```

Every generator reads the same compiled site. Put transformations that depend on one another in the
same command.

The declared output needs no `[assets]` entry. `asset-url` resolves only `[assets]` declarations,
and errors unless the file was published. Link a known output path with
`output-to-url(\"assets/pagefind-search/pagefind-ui.css\")`. These files do not exist during the
preceding Typst compilation, so Tola cannot compute their asset URLs then. Tola does not minify or
cache-bust a hook's products; the producer's command handles all of that itself: Pagefind, for
example, gives its index and fragment files content-hashed names.

A page uses it like this: the template head links the generated CSS and JavaScript directly:

```typst
#import \"@tola/address:0.0.0\": output-to-url

#html.link(rel: \"stylesheet\", href: output-to-url(\"assets/pagefind-search/pagefind-ui.css\"))
#html.elem(\"script\", attrs: (src: output-to-url(\"assets/pagefind-search/pagefind-ui.js\")))
```

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
#[config(section = "build.hooks.after-publish", collection = array_table)]
pub struct AfterPublishHookConfig {
    /// Whether this hook runs.
    pub enable: bool,
    /// The hook's name, so status lines and diagnostics can point at it: one word without
    /// whitespace, unique within its stage. A blank, spaced, or repeated name is an error.
    pub name: String,
    /// The command to run, executed from the site root without an implicit shell.
    /// `TOLA_HOOK_INPUT_DIR` is a read-only view of the published site, so a failure here cannot
    /// undo publication.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this hook: `"skip"` (the default) or `"run"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Paths this hook watches, relative to the site root, whose edits trigger a `tola dev` build,
    /// not only the hook's own command.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
}

impl AfterPublishHookConfig {
    /// What `tola help config build.hooks.after-publish` adds under its table.
    pub const HELP: &'static str = "\
This stage consumes a successfully committed site, for example to sync it to a host:

`justfile`:

```just
rsync:
    rsync -a --delete \"$TOLA_HOOK_INPUT_DIR/\" user@example.com:/srv/site/
```

`tola.toml`:

```toml
[[build.hooks.after-publish]]
name = \"rsync\"
command = [\"just\", \"rsync\"]
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
kept, so intermediate revisions may receive no consumer call. Stopping `tola dev` cancels the
running command and discards waiting work. Use this mode for consumers that can handle the latest
site, rather than for an audit of every revision.

The command environment for this stage:
- `TOLA_HOOK_INPUT_DIR`: the command's input — a read-only snapshot of the site just committed,
  laid out like the published directory, so the whole directory is what a consumer uploads.
- `TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE`, `TOLA_HOOK_CACHE_DIR`, and `TOLA_HOOK_TEMP_DIR`: as in
  every stage — the stage name, the mode (`prod` or `dev`), a cross-run cache directory, and this
  invocation's temporary directory.

There is no `TOLA_HOOK_OUTPUT_DIR`: this stage adds no output.";
}

/// One declared generated file or exclusive generated directory tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOutput {
    /// One exact output file.
    File(tola_address::OutputPath),
    /// An exclusively owned directory whose files may vary between invocations.
    Tree(tola_address::OutputPath),
}

impl CommandOutput {
    /// Logical path of the declared file or directory.
    pub fn path(&self) -> &tola_address::OutputPath {
        match self {
            Self::File(path) | Self::Tree(path) => path,
        }
    }
}

impl Default for BeforeBuildHookConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Run,
            rerun_on: Vec::new(),
            generates: Vec::new(),
        }
    }
}

impl Default for OutputCommandConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Run,
            rerun_on: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

impl Default for AfterPublishHookConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Skip,
            rerun_on: Vec::new(),
        }
    }
}

fn validate_command(
    name: &str,
    command: &[String],
    rerun_on: &[PathBuf],
    index: usize,
    fields: HookEntryFields,
    diag: &mut ConfigDiagnostics,
) {
    let key = message_key(fields.command, index);
    if command.is_empty() {
        diag.error_with_help(
            fields.command,
            format!("`{key}` is empty"),
            "name the executable first, then its arguments",
        );
    } else if command[0].trim().is_empty() {
        diag.error_with_help(
            fields.command,
            format!("`{key}[0]` is blank"),
            "name the executable first, then its arguments",
        );
    }
    validate_name(name, index, fields.name, diag);
    for (entry_index, path) in rerun_on.iter().enumerate() {
        validate_site_relative(path, index, entry_index, fields.rerun_on, diag);
    }
}

pub(crate) fn hook_identity(stage: HookStage, name: &str) -> String {
    format!("the `{name}` hook in `build.hooks.{}`", stage.as_str())
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

/// Point in the build lifecycle at which a hook executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStage {
    BeforeBuild,
    GenerateOutputs,
    AfterPublish,
}

impl HookStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeBuild => "before-build",
            Self::GenerateOutputs => "generate-outputs",
            Self::AfterPublish => "after-publish",
        }
    }
}

/// Whether an enabled finite command also runs in development.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DevParticipation {
    /// Run the command on every build that reaches its stage.
    #[default]
    Run,
    /// Leave the command out of the development session.
    ///
    /// Production builds still run it.
    Skip,
}

impl DevParticipation {
    const VALUES: &'static [&'static str] = &["\"run\"", "\"skip\""];

    /// Whether a development session runs this command at all.
    pub const fn participates_in_development(self) -> bool {
        !matches!(self, Self::Skip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{attached_diagnostics, diagnostic_mentioning};

    #[test]
    fn hook_template_preserves_stage_entries() {
        let config: crate::config::SiteConfigSchema = toml::from_str(
            r#"
[[build.hooks.before-build]]
name = "icons"
command = ["./scripts/icons.sh", "generated/icons"]
rerun-on = ["src/icons"]
generates = ["generated/icons"]

[[build.hooks.generate-outputs]]
name = "search"
command = ["node", "scripts/search.mjs"]
dev = "skip"
rerun-on = ["scripts/search.mjs"]
outputs = [{ file = "index.json" }, { tree = "search" }]

[[build.hooks.after-publish]]
name = "deploy"
command = ["./scripts/deploy.sh"]
dev = "run"
rerun-on = ["scripts/deploy.sh"]
"#,
        )
        .unwrap();
        let template =
            super::super::BuildSectionConfig::try_template_with_header_from(&config.build).unwrap();
        let decoded: crate::config::SiteConfigSchema = toml::from_str(&template).unwrap();

        assert_eq!(decoded.build.hooks, config.build.hooks);
    }

    #[test]
    fn development_inputs_match_participation() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
name = "prepare"
command = ["prepare"]
rerun-on = [" before "]

[[before-build]]
name = "production-prepare"
command = ["production-prepare"]
dev = "skip"
rerun-on = ["production-input"]

[[before-build]]
name = "disabled-prepare"
command = ["disabled-prepare"]
enable = false
rerun-on = ["disabled-input"]

[[generate-outputs]]
name = "generate"
command = ["generate"]
rerun-on = ["generated/*.json"]
outputs = [{ file = "search.json" }]

[[generate-outputs]]
name = "disabled-generate"
command = ["disabled-generate"]
enable = false
rerun-on = ["disabled-generated"]

[[after-publish]]
name = "production-consumer"
command = ["production-consumer"]
rerun-on = ["production-consumer-input"]

[[after-publish]]
name = "development-consumer"
command = ["development-consumer"]
dev = "run"
rerun-on = ["consumer"]

[[after-publish]]
name = "disabled-consumer"
command = ["disabled-consumer"]
enable = false
dev = "run"
rerun-on = ["disabled-consumer-input"]
"#,
        )
        .unwrap();
        assert_eq!(
            config.development_rerun_paths().collect::<Vec<_>>(),
            [" before ", "generated/*.json", "consumer"].map(Path::new)
        );
    }

    #[test]
    fn disabled_commands_are_not_validated() {
        let mut config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
enable = false
command = [" "]
generates = ["../source"]

[[generate-outputs]]
enable = false
name = " "
rerun-on = ["/outside"]

[[after-publish]]
enable = false
rerun-on = ["../committed"]
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
            &mut diagnostics,
        );
        assert!(diagnostics.into_result().is_ok());

        config.before_build[0].enable = true;
        config.generate_outputs[0].enable = true;
        config.after_publish[0].enable = true;
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        for field in [
            BeforeBuildHookConfig::FIELDS.command,
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
            command: vec!["generator".into()],
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
command = ["echo", "bad"]
rerun-on = ["/src", "../templates"]
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

    #[test]
    fn enabled_commands_need_executable() {
        for command in [Vec::new(), vec!["  ".into()]] {
            let hooks = HooksConfig {
                before_build: vec![BeforeBuildHookConfig {
                    name: "generator".into(),
                    command,
                    ..BeforeBuildHookConfig::default()
                }],
                ..HooksConfig::default()
            };
            let mut diagnostics = ConfigDiagnostics::new();

            hooks.validate(&mut diagnostics);

            assert_eq!(diagnostics.errors().len(), 1);
            assert_eq!(
                diagnostics.errors()[0].field,
                BeforeBuildHookConfig::FIELDS.command
            );
        }
    }

    /// One enabled hook in each stage, every one named `name` and running `command`.
    fn one_hook_per_stage(name: &str, command: &str) -> HooksConfig {
        HooksConfig {
            before_build: vec![BeforeBuildHookConfig {
                name: name.into(),
                command: vec![command.into()],
                generates: vec!["generated".into()],
                ..BeforeBuildHookConfig::default()
            }],
            generate_outputs: vec![OutputCommandConfig {
                name: name.into(),
                command: vec![command.into()],
                outputs: vec![CommandOutput::File(
                    tola_address::OutputPath::parse("search.json").unwrap(),
                )],
                ..OutputCommandConfig::default()
            }],
            after_publish: vec![AfterPublishHookConfig {
                name: name.into(),
                command: vec![command.into()],
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

    /// The 1-based line of the last line `source` writes as `key`.
    fn last_written_line(source: &str, key: &str) -> usize {
        let lines = source.lines().collect::<Vec<_>>();
        lines.iter().rposition(|line| *line == key).unwrap() + 1
    }

    /// The location of the single diagnostic settings validation reports for `source`.
    fn reported_location(source: &str) -> (usize, String) {
        let parsed = crate::config::ConfigSource::parse(
            Path::new("/site/tola.toml"),
            source,
            crate::resources::InputScope::Online,
        )
        .unwrap()
        .decode_site()
        .unwrap();
        let error = parsed.validate_settings().unwrap_err();
        let diagnostics = attached_diagnostics(&error);
        let [diagnostic] = diagnostics else {
            panic!("expected one diagnostic for {source}: {diagnostics:#?}");
        };
        let location = diagnostic.location.as_ref().unwrap();
        (
            location.line.unwrap(),
            location.source_lines[0].text.clone(),
        )
    }

    #[test]
    fn later_entry_error_names_its_own_entry() {
        // The last case writes no `name` at all: a field the entry does not write points at the
        // entry's own header, never at the first entry of its array.
        for (source, key) in [
            (
                r#"
[[build.hooks.before-build]]
name = "first"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
name = "second"
command = []
generates = ["generated/second"]
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.generate-outputs]]
name = "first"
command = ["first.sh"]
outputs = [{ file = "first.json" }]

[[build.hooks.generate-outputs]]
name = "second"
command = []
outputs = [{ file = "second.json" }]
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.after-publish]]
name = "first"
command = ["first.sh"]

[[build.hooks.after-publish]]
name = "second"
command = []
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.before-build]]
name = "styles"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
name = "styles"
command = ["second.sh"]
generates = ["generated/second"]
"#,
                r#"name = "styles""#,
            ),
            (
                r#"
[[build.hooks.before-build]]
name = "first"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
command = ["second.sh"]
generates = ["generated/second"]
"#,
                "[[build.hooks.before-build]]",
            ),
        ] {
            assert_eq!(
                reported_location(source),
                (last_written_line(source, key), key.to_owned()),
                "{source}"
            );
        }
    }

    /// Assert that a load of one schema whose single before-build hook declares `declared`
    /// reports it as a refused hook output.
    fn assert_hook_output_refused(root: &Path, declared: &str) {
        let mut schema = crate::config::SiteConfigSchema::default();
        schema.vendor.path = Some("vendor".into());
        schema.build.hooks.before_build = vec![input_generator("generator", declared)];

        let error = schema
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap_err();

        let diagnostic = diagnostic_mentioning(&error, declared);
        assert_eq!(diagnostic.code, crate::codes::config::INVALID);
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
            ".vendor-vendor",
            ".vendor-vendor/marker.txt",
        ] {
            assert_hook_output_refused(root, declared);
        }

        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let workspace = root.join(".vendor-vendor");
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
            &mut diag,
        );

        assert_eq!(diag.errors().len(), 1);
        assert_eq!(
            diag.errors()[0].field,
            BeforeBuildHookConfig::FIELDS.generates
        );
    }
}
