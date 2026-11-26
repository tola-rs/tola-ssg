use super::{AfterPublishHookConfig, BeforeBuildHookConfig, HooksConfig, OutputCommandConfig};

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
two stages and skips `after-publish`. Set `dev = "reuse"` to opt into reuse for
[`before-build`](tola-help://config/hooks/before-build) or
[`generate-outputs`](tola-help://config/hooks/generate-outputs).

An error before publication leaves the previous site in place. An `after-publish` failure
is reported after the new files are already on disk.

### Report issues and find paths

In a Nu hook, import `use tola diagnostic` and pass the issues to `diagnostic submit`.
Submit one list per run. Pass `[]` when there are no issues. Omit `location` in a regular
hook report. Warnings appear in the terminal and errors fail the hook. An external
command can write its report to `reportPath`. See [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic)
for a complete example. If `tola dev` reuses a hook's files, it also reuses that hook's report.

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
Name a file symlink explicitly to track edits to its regular target within the declaring
root. For a directory symlink, list the target directory. A selected link whose contents
cannot be tracked prevents a before-build or generate-outputs cache hit.
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

impl BeforeBuildHookConfig {
    /// What `tola help "[[hooks.before-build]]"` adds under its table.
    pub const HELP: &'static str = r#"
### Generate other inputs

Write templates, data files, or assets to their source paths. For example, generate
`static/site.css` and declare it under `[assets]` to publish it. Declare the files or directories
in `generates`. Tola checks that they exist after the command runs and watches them in dev mode.
Keep generated inputs outside `build.publish-dir` and `.tola`.

Use `hook.cacheDir` for files needed across runs and `hook.tempDir` for this run's
temporary files. This stage runs before there is a built site to read, so write
input files directly in the site.

### Reuse generated inputs during development

Set `dev = "reuse"` and list every generated file or directory in `generates`.
This works in site and module hooks. Within one `tola dev` session, Tola can skip
the command and keep its previous diagnostics when the command, settings, watched
inputs, and generated files still match a successful run. If a declared input
appears, disappears, or changes, or if a generated file is missing or changed,
Tola runs the command again.

Declare every additional file the command reads in `rerun-on`, including imported
scripts, tool files and configuration, and files generated by earlier hooks.
Tola already tracks the Nu entry script. Omit `rerun-on` if the command reads no
additional files. Use `dev = "run"` if the result depends on time, the network,
environment variables, system tools or other state that Tola cannot track, or if
the command has side effects needed on every build.

Use `tola dev --no-reuse` to force these commands to run during development.
Production builds always run enabled hooks.

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
dev = "reuse"
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

impl OutputCommandConfig {
    /// What `tola help "[[hooks.generate-outputs]]"` adds under its table.
    pub const HELP: &'static str = r#"
### Read pages and write files

- `hook.inputDir`: Read compiled pages and `[assets]` files here.
- `hook.outputDir`: Write every path listed in `outputs`. This directory starts empty.
  Tola publishes the files at the same relative paths.
- `hook.outputs`: Read this hook's publish-relative `file` and `tree` declarations.
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

impl AfterPublishHookConfig {
    /// What `tola help "[[hooks.after-publish]]"` adds under its table.
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
