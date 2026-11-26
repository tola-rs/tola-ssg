pub const HELP: &str = r#"Add each module instance as `[modules.<alias>]` in the site’s `tola.toml`. Several aliases can
select the same module. The module root must contain `tola-module.toml`.

### Choose a source and input scope

Use `path` for a local module. Relative paths start at the site root. With `--pure`,
the module must be inside the site. A site `before-build` hook can generate inputs in
such a module. Keep the module directory separate from the publish directory.

Choose one of `path`, `git`, `tarball`, or `builtin`. This executable supplies
`builtin = "resize-images"` and `builtin = "tailwind"`.

A `tarball` table has exactly one
HTTPS `url` or local file `path`, plus optional `sha256` as 64 hexadecimal digits. Relative
archive paths start at the site root. Tola accepts tar.gz, tar.zst, and ZIP (stored/deflate).
Put `tola-module.toml` directly inside the archive's one outer directory. For a private
release attachment, download the archive first and use `tarball.path`.

The `git` table holds `url`, optional `subdir`,
and at most one of `tag`, `branch`, or `rev`. `git.rev` is a full commit ID; `git.subdir`
is relative to the repository root. `git.url` accepts `https://`, `ssh://`, or `git@host:path`.
Omitting all three selectors selects the repository's default branch.

A private `https` repository uses your Git credential helper. An `ssh` address, including
`git@host:path`, uses the system `ssh` with its agent, `~/.ssh/config`, and `known_hosts`.
`tola.lock` records addresses, selectors, commits, and digests, never credentials.

Run `tola update notes` after changing the module source or to select a new version.
If you use vendor, run `tola vendor --refresh` to update its copy. Builds verify
locked Git commits and archives against `tola.lock`.

- `--locked`: Keep the lock file as written. Tola can download a locked dependency that is missing.
- `--offline`: Use local files and cached inputs. Tola can record local dependencies in the lock.
- `--pure`: Use locked inputs from the site and vendor. Tola can also expand a locked archive stored inside the site.

### Choose the site entry, options, and assets

Select a module as the site entry with `[build] entry = { module = "notes" }`.
Import its public Typst library as `@tola-modules/notes:0.0.0`.

Manifest paths are relative to the module. Project `[modules.notes.options]` values
replace the defaults, with recursive merging when both values are tables.
Use [`include`](tola-help://config/include) to keep a site's module source and overrides in separate files.

In `tola-module.toml`, write `dev = { option = "preview" }` to read a hook's `dev`
setting from the instance's `preview` option. The referenced value replaces the
whole field and must pass the same validation as a literal value. The module's
`[options]` supplies the defaults for `tola module check`.

References also work in build hooks' `order`, `args`, `rerun-on`, `generates`, and
`outputs`, module commands' `args` and `projection`, and static assets' `minify`,
tree `include`, and tree `exclude`.

Module assets publish under `/_tola-modules/notes/`, with the site's deployment prefix.
`enable = false` under `[modules.notes.assets]` disables the instance's static assets.
A replacement keeps the public resource name and reads from a project-relative file.

Within a module, import `get-module` from `@tola/module:0.0.0` to read its alias, options,
and assets. Use `assets.files` to read files and `assets.urls` to link them from pages.
Inside a package, use `project-path` from `@tola/site:0.0.0` to read a file from the site root.

### Run module hooks

A module hook's `enabled-by` reads the boolean setting for that alias. Use
`get-context | get module.options` in Nu to read the settings after site overrides.
In `rerun-on`, strings start at the module root. Use `{ project = "..." }` for site
files. Both accept globs. Tola expands `{name}` to the alias in `generates`, `outputs`,
and `rerun-on`.

Modules run in alias order. Within a stage, `order` sorts hooks from the same module.
The site's `par-hooks` and `hook-jobs` apply to all modules. See [`[hooks]`](tola-help://config/hooks)
for defaults and an ordered example.

### Resize a directory of photos

Publish `photos/` at `/gallery/` and choose which images to resize by their source paths.

```toml
[assets]
trees = [{ source = "photos", url-prefix = "/gallery" }]

[modules.resize-images]
builtin = "resize-images"

[modules.resize-images.options]
include = ["photos/**"]
widths = [480, 960]
format = "webp"

[modules.resize-images.options.overrides."photos/cover.jpg"]
widths = [1200]
```

The module skips widths larger than the original. Use `output-url` to link to the
result, for example `images/resize-images/photos/cover.jpg/w1200.webp` for a cover
at least 1200 pixels wide.

### Generate Tailwind CSS

```toml
[modules.tailwind]
builtin = "tailwind"

[modules.tailwind.options]
minify = true
# runner = ["deno", "task", "tailwind"]
```

The module finds an installed Tailwind CSS v4 CLI in the site's `node_modules/.bin`
or on `PATH`. Set `runner` to run a project task or wrapper. It receives the module's
Tailwind arguments and exits after one compilation. The module scans rendered HTML
and publishes `_tola-modules/tailwind/tailwind.css`. Link it with `output-url`.
Add `styles/tailwind.css` to set Tailwind options or scan additional sources. List
the extra inputs in `watch.include` so editing them triggers a rebuild.

### Declare and run queries

Set `entry` relative to the module directory. The query uses the site's sources and packages.
`label` must identify exactly one metadata value. Return values that can be written as JSON.
A Typst `path` becomes a record with `root` and `path`. For content or functions, return the
fields your caller needs.

Declare `references` and `definitions` under their query. Each entry names a function exported
by `[typst].entry` and one of its parameters. Positional indices start at zero. Dot-separated
exports traverse public members, such as `content.local`.
Multiple entries let several functions or parameters use the same query. Unknown exports,
missing parameters, and conflicting assignments are diagnosed when the editor reads them.

For editor references or definitions, return an array of records. Each record needs a
nonempty string `id` and a string `name`. Keep IDs unique within the query for that module
alias. `description` and `url` are optional strings, and `source` is an optional Typst `path`.
The CLI and LSP both validate these fields. The metadata `label` selects the whole result.
`name` is the text shown for an individual entry.

When updating an older manifest, remove `returns`, `semantics`, and `role`. In returned
records, rename `label` to `name` and `detail` to `description`.

A query runs its own Typst entry. Generate any files it needs before running it, since queries
skip build hooks. Editor queries can use unsaved changes.

Queries print a human-readable table by default. `--json` returns compact JSON,
`--json --pretty` indents it, and `--json --output FILE` writes it to a file. `-i`
browses a value in a terminal. Set `projection` to choose the output fields. In interactive mode, you can see the complete
record before selecting it. Use Nu `where` and `select` for further filtering.

### Run module commands and import Nu libraries

Built-in command names are reserved. Help reads declarations without evaluating a query
or running its tool. Unknown manifest fields are errors.

Set `nu` to the script path relative to the module root. Use `args` for default script
arguments. The script runs from the site root with Tola's built-in Nushell.
Define arguments, types, defaults, and a description in the script's `main` signature
and comments. Help reads the signature without executing the script. An explicit
`help` overrides its description. External commands use the declared `help`.

Use `argv` to run an external program when the author calls the command. In declared arguments,
`{module}` expands to the module directory, `{project}` to the site directory, and `{tola}`
to the Tola executable. Arguments from the user are passed as written. From that program,
run `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json` to read the site, module options, and
input scope for the command.

In a module Nu command, use `diagnostic submit` to report warnings or errors to Tola.
Submit one list per invocation, even when it is empty. An error fails the command.
An external program can write its report to `reportPath` from the context.
See [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic) for a Nu example.

Use `use tola-modules notes` in Nu for the module selected as `notes`. Public definitions use
ordinary Nu `export`; imports do not choose a current module configuration. Pass site options
explicitly to reusable functions. The module's own resources can use Nu `path self`, while user
paths keep the invocation's working directory.

`tola nu @tola-modules/notes` calls the public entry's `export def main`.
`tola nu @tola-scripts/image` runs Tola's built-in image script. To import its functions
inside a Nu script, write `use tola image`. To run a local file, give its path. For a
filename starting with `@`, write `./@file.nu`. Run `tola nu --help` to list available libraries.

The imported `query` returns Nu values, so you can filter rows with `where` and read fields with
`get`. To read a metadata label from a separate Typst file, use
`query ./tools/model.typ --label model`.

For other Tola commands, run `$env.TOLA_EXECUTABLE` as an external program and use Nu's
`complete` to capture `stdout`, `stderr`, and `exit_code`. Pass `--config` if the child
needs a specific site. For commands using an input scope, pass `--offline` or `--pure`
to match `$env.TOLA_INPUT_SCOPE`. Read the output of `tola dev --json` one JSON line at a time.

For another configuration, use `tola notes --config other.toml COMMAND ...`; after COMMAND,
arguments belong to that command. A declared tool never silently falls back to a global plugin.

See [the built-in image script](tola-help://scripts/image) for file and byte operations.

### Read module guides

The Markdown path starts at the module root. Export paths start in the directory where
you run the command. Tola reads the guide as text.

`tola skill` reads Tola's own authoring guide from any directory. A module guide uses the
same section selection and export flags through its declared command.

### Initialize a site

Write new files under `init.outputDir`. If the generated configuration contains relative
paths, calculate them from `init.targetDir`, where the site will live. Read `module.name`
if the generated configuration needs the chosen alias, and prompt only when `init.interactive`
is true. The source may also be a Git URL or an archive. For Git, use `--branch`, `--tag`,
or `--rev` to select a revision and `--subdir` to select the module directory.
From a site that already uses the module as `forest`, run `tola forest init new-site`.
Declare either `[init]` or `[commands.init]`.

Tola checks the new configuration and module options before writing to the target directory.
`--dry-run` runs those checks without writing to the target. `--force` allows an occupied
directory and preserves existing files.

### Pack a module

Use `hook.inputDir` in `before-pack` to find the module root. There is no site context
at this stage.

`before-pack` is a module author hook. Enabled entries run in declaration order
when the author runs `tola module pack`. Use `name`, `enable`, and either `nu` with
optional `args` or `argv`. The `order` and `rerun-on` settings are for build hooks.

`[pack] include` selects the files to ship after those hooks run. It is required and
its paths start at the module root. A directory includes its contents. Use
`exclude = ["assets/**/*.map"]` to omit matching files. Exclusions take priority.
Tola always ships `tola-module.toml`, and the release must contain every local file
or directory it still declares.

`tola module check ./dist/labels-theme` checks the release manifest and the paths it
declares. Pack already checks the release before replacing the destination. The
separate command is useful for a directory you downloaded or unpacked yourself.
To test compilation and runtime hooks, use the release in a site and run `tola check`.

Example: Import a module’s Typst library:

In a site created with `tola init`, add this to the site’s `tola.toml`.

```toml
[modules.notes]
path = "theme"
```

Create `theme/tola-module.toml`.

```toml
[typst]
entry = "lib.typ"

[options]
title = "Field notes"
```

Save this as `theme/lib.typ`.

```typst
#import "@tola/module:0.0.0": get-module
#let note-title = get-module().options.title
```

Use it in `content/index.typ`.

```typst
#import "@tola-modules/notes:0.0.0": note-title
= #note-title
```

Run `tola build` from the site root. The heading in `public/index.html` reads “Field notes”.
Set `title = "Lab notes"` under `[modules.notes.options]` in `tola.toml` and build again
to change it.

Example: Load a module from an archive or Git:

Load a module from a release archive.

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

Load a module from a Git repository.

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

Example: Declare entries, default options, and assets:

Declare the public library, site entry, assets, and default options in the module manifest.

```toml
[typst]
entry = "lib.typ"
[build]
entry = "site.typ"
[assets]
trees = [{ source = "assets", url-prefix = "/" }]
[options]
search = false
```

Replace the module’s `style.css` in the site configuration.

```toml
[modules.notes.assets.replace]
"style.css" = "static/custom.css"
```

Example: Add a search hook to a module:

This example wires up a search script. Write `theme/tools/search.nu` and set
`search = true` under `[modules.notes.options]` in the site's `tola.toml`. Put
`{name}` in the output path to give each alias its own files.

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "tools/search.nu"
enabled-by = "search"
rerun-on = { include = [{ project = "content/**/*.typ" }] }
outputs = [{ tree = "_tola-modules/{name}/search" }]
```

Example: Add a query and a command:

For the local `notes` module above, put these declarations in `theme/tola-module.toml`.

```toml
[queries.nodes]
entry = "model.typ"
label = "notebook-nodes"

[commands.list]
help = "List notebook entries."
query = "nodes"
projection = ["id", "name"]
```

Save this as `theme/model.typ`.

```typst
#metadata(((id: "welcome", name: "Welcome"),)) <notebook-nodes>
```

From the site root, run `tola query notes.nodes --json` to see the record. Run `tola notes list` to see its `id` and `name`.

Use `tola notes list --help` to see its options.

Example: Declare references and definitions for a query:

Declare references and definitions under their query:

```toml
[[queries.nodes.references]]
export = "local"
parameter = { positional = 0 }

[[queries.nodes.definitions]]
export = "subtree"
parameter = { named = "address" }
```

Example: Run a Nu command from a module:

Keep the local `notes` module from the example above. Add this to `theme/tola-module.toml`:

```toml
[commands.title]
nu = "tools/title.nu"
```

Create `theme/tools/` and save this as `theme/tools/title.nu`:

```nu
use tola get-context

def main [] {
  get-context | get module.options.title
}
```

Run `tola notes title` from the site root. It prints the title for this module alias,
including any override in `[modules.notes.options]`.

Example: Export a Nu library:

A module may also export a reusable Nu library:

```toml
[nu]
entry = "tools/mod.nu"
```

Add that `[nu]` table to `theme/tola-module.toml` and save this as `theme/tools/mod.nu`.

```nu
export def note-title [] { "Field notes" }
export def main [] { note-title }
```

From the site root, run `tola nu @tola-modules/notes` to print `Field notes`.

Example: Use a Typst query from Nu:

After adding the `notes.nodes` query above, save this as `list-titles.nu` next to
`tola.toml`:

```nu
use tola query

query notes.nodes | get name | to text
```

Run `tola nu list-titles.nu` from the site root. It prints `Welcome`.

Example: Add a guide command:

Add this to `theme/tola-module.toml`:

```toml
[commands.guide]
skill = "GUIDE.md"
help = "Read the theme guide."
```

Save this as `theme/GUIDE.md`:

```md
# Notes theme

## Writing

Put posts in content/posts/.

### Drafts

Set draft to true in a post's metadata to keep it out of the published site.
```

From a site that uses this module as `notes`, run `tola notes guide --list` to see the
available sections, then `tola notes guide writing drafts` to read the draft instructions.
Use `tola notes guide writing --all -o writing.md` to export that section and its subsections.

Example: Create a site from a module:

Add this to `theme/tola-module.toml` to give the local module a site initializer:

```toml
[init]
nu = "tools/init.nu"
```

Save this as `theme/tools/init.nu`:

```nu
use tola get-context

def main [--title: string = "Field notes"] {
  let out = get-context | get init.outputDir
  {
    site: {title: $title}
  } | to toml | save ($out | path join "tola.toml")
  mkdir ($out | path join "content")
  [
    '#import "@tola/site:0.0.0": site'
    '#document("index.html", format: "html")[#site.title]'
  ] | str join (char newline) | save ($out | path join "site.typ")
}
```

From the directory containing `theme/`, run:

```sh
tola init new-site --from ./theme --alias forest --dry-run -- --title "My notes"
tola init new-site --from ./theme --alias forest -- --title "My notes"
cd new-site
tola build
```

Open `public/index.html` to see “My notes”. Tola adds the source to `[modules.forest]`
in the new `tola.toml`. The first `--` separates Tola's options from the initializer's
arguments. Omit `--title` to use the script's default, “Field notes”.

Example: Generate a file before packing a module:

Create a module directory `labels-theme/`. Save the following as
`labels-theme/tola-module.toml`:

```toml
[pack]
include = ["assets"]

[assets]
files = [{ source = "assets/labels.json", url = "/labels.json" }]

[[hooks.before-pack]]
name = "build-labels"
nu = "prepare-labels.nu"
```

In `labels-theme/labels.csv`, put:

```csv
id,label
draft,Draft
published,Published
```

Save this as `labels-theme/prepare-labels.nu`:

```nu
use tola get-context

let source = get-context | get hook.inputDir
let assets = $source | path join "assets"
mkdir $assets
open ($source | path join "labels.csv")
| to json
| save --force ($assets | path join "labels.json")
```

From the directory containing `labels-theme/`, run:

```sh
mkdir -p dist
tola module pack ./labels-theme --output ./dist/labels-theme
tola module check ./dist/labels-theme
```

Open `dist/labels-theme/assets/labels.json` to see the two labels. The hook writes into
the module's source directory before pack selects the files. The shipped directory
contains `assets/` and `tola-module.toml`. Its manifest omits `[pack]` and
`[[hooks.before-pack]]`, so sites using this release can read the prepared JSON directly.
"#;
