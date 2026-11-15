//! Build-side module facade: project configuration, resolution wrappers, and help text.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tola_input::InputMode;

use tola_module::module_set::{self, LoadedModule};

pub mod hooks;
pub mod manifest;
pub(crate) mod prepared;
pub mod query;
mod watch;

pub use manifest::{
    find_initializer, nu_font_options, read_available_declarations, read_nu_modules,
};
pub use prepared::apply_module_icons;
pub use prepared::{PreparedSite, prepare_site};
pub use watch::{PreparedHookWatch, prepare_hook_watch};

pub const HELP: &str = r#"Add each module instance as `[modules.<alias>]` in the site’s `tola.toml`. Several aliases can
select the same module. The module root must contain `tola-module.toml`.

### Choose a source and input scope

Use `path` for a local module. Relative paths start at the site root. With `--pure`,
the module must be inside the site. A site `before-build` hook can generate inputs in
such a module. Keep the module directory separate from the publish directory.

`path`, `git`, and `tarball` are mutually exclusive. A `tarball` table has exactly one
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

/// Resolve the declared modules' roots and read their manifests.
pub fn load_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, LoadedModule>, module_set::ModuleLoadError> {
    let bundle = cancellation.bundle_cancellation();
    let roots = tola_module::inputs::module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        &bundle,
    )?;
    module_set::load_modules_with_roots(site_root, declarations, &roots, mode, &[])
}

/// Read every declared module's manifest through a resolved root.
pub fn load_modules_with_roots(
    site_root: &Path,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
) -> std::result::Result<BTreeMap<String, LoadedModule>, module_set::ModuleLoadError> {
    module_set::load_modules_with_roots(site_root, declarations, roots, mode, &[])
}

/// Command declarations remain available when unrelated build inputs need repair.
pub fn select_command_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<
    BTreeMap<String, module_set::ModuleCommands>,
    tola_module::manifest::ManifestError,
> {
    let bundle = cancellation.bundle_cancellation();
    module_set::select_command_modules(site_root, vendor, declarations, mode, &bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tola_input::InputScope;
    use tola_module::declaration::*;
    use tola_module::module_set::{LoadedModule, ModuleLoadError};

    fn loaded_modules(
        site: &Path,
        declarations: &BTreeMap<String, ModuleDeclaration>,
        mode: InputMode,
    ) -> std::result::Result<BTreeMap<String, LoadedModule>, ModuleLoadError> {
        let roots =
            crate::inputs::module_roots(site, mode, None, declarations, &Default::default())?;
        load_modules_with_roots(site, declarations, &roots, mode)
    }

    fn theme_site(manifest: &str) -> (tempfile::TempDir, BTreeMap<String, ModuleDeclaration>) {
        let site = tempfile::tempdir().unwrap();
        let module = site.path().join("theme");
        fs::create_dir(&module).unwrap();
        fs::write(module.join("model.typ"), "#metadata(())<forest-nodes>").unwrap();
        fs::write(module.join("tola-module.toml"), manifest).unwrap();
        (
            site,
            BTreeMap::from([(
                "forest".into(),
                ModuleDeclaration {
                    source: ModuleSource::Local {
                        path: "theme".into(),
                    },
                    ..Default::default()
                },
            )]),
        )
    }

    const VALID: &str = r#"
[queries.nodes]
entry = "model.typ"
label = "forest-nodes"
[commands.list]
help = "List nodes"
query = "nodes"
projection = ["id", "name"]
[commands.edit]
help = "Edit a node"
argv = ["editor", "--wait"]
"#;

    #[test]
    fn a_prepared_tarball_can_supply_the_site_program() {
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        let gzip = flate2::write::GzEncoder::new(
            fs::File::create(root.join("theme.tar.gz")).unwrap(),
            flate2::Compression::fast(),
        );
        let mut archive = tar::Builder::new(gzip);
        for (name, text) in [
            ("theme/tola-module.toml", "[build]\nentry = 'site.typ'\n"),
            (
                "theme/site.typ",
                "#document(\"index.html\", format: \"html\")[A packaged site]",
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(text.len() as u64);
            archive
                .append_data(&mut header, name, text.as_bytes())
                .unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
        fs::write(root.join("tola.toml"), "[build]\nentry = { module = 'theme' }\n[modules.theme]\ntarball = { path = 'theme.tar.gz' }\n[typst.fonts]\nsystem = false\n").unwrap();
        let config = load_site_config(
            Some(&root.join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let mut session = BuildSession::with_resources(
            BuildResources::new().with_input_scope(InputScope::Offline),
        );
        session
            .build_and_write(&config, crate::build::BuildRequest::default())
            .unwrap();
        assert!(root.join("public/index.html").is_file());
        assert!(matches!(
            tola_input::InputLock::read(root).unwrap().unwrap().modules["theme"],
            tola_input::LockedModule::Tarball { .. }
        ));
        fs::remove_dir_all(tola_input::layout::modules(root)).unwrap();
        let resources = BuildResources::new().with_input_scope(InputScope::Pure);
        let prepared = prepare_site(&config, resources.input_mode(), &Default::default()).unwrap();
        let boundary = resources.source_boundary(prepared.config());
        assert!(boundary.check(&prepared.build().entry).is_ok());
        assert!(boundary.check(&root.join(".tola/unrelated.typ")).is_err());
        BuildSession::with_resources(resources)
            .build_and_write(&config, crate::build::BuildRequest::default())
            .unwrap();
    }

    #[test]
    fn module_options_merge_with_defaults() {
        let manifest = r#"
[options]
show = true
items = ["default"]
nested = { count = 2, date = 2026-01-01 }
"#;
        let (site, mut declarations) = theme_site(manifest);
        declarations.get_mut("forest").unwrap().options =
            toml::from_str("show = false\nitems = []\nnested = { count = 3 }").unwrap();
        let modules = loaded_modules(
            site.path(),
            &declarations,
            InputMode::new(InputScope::Pure, false),
        )
        .unwrap();
        let options = &modules["forest"].options;
        assert_eq!(options["show"].as_bool(), Some(false));
        assert!(options["items"].as_array().unwrap().is_empty());
        assert_eq!(options["nested"]["count"].as_integer(), Some(3));
        assert_eq!(options["nested"]["date"].as_str(), Some("2026-01-01"));
    }

    #[test]
    fn reads_local_module_without_executing_commands() {
        let (site, declarations) = theme_site(VALID);
        let modules = loaded_modules(
            site.path(),
            &declarations,
            tola_input::InputMode::new(InputScope::Pure, false),
        )
        .unwrap();
        let forest = &modules["forest"];
        assert_eq!(
            forest.root,
            site.path().join("theme").canonicalize().unwrap()
        );
        assert_eq!(forest.queries["nodes"].label, "forest-nodes");
        assert_eq!(
            forest.commands["edit"].action().unwrap(),
            CommandAction::Execute(tola_module::Action::Argv(vec![
                "editor".into(),
                "--wait".into(),
            ]))
        );
    }

    #[test]
    fn rejects_escape_allows_distinct_aliases() {
        let (site, mut declarations) = theme_site(&VALID.replace("model.typ", "../outside.typ"));
        assert!(
            loaded_modules(
                site.path(),
                &declarations,
                tola_input::InputMode::new(InputScope::Online, false)
            )
            .is_err()
        );
        fs::write(site.path().join("theme/tola-module.toml"), VALID).unwrap();
        declarations.insert(
            "other".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: "theme".into(),
                },
                ..Default::default()
            },
        );
        assert_eq!(
            loaded_modules(
                site.path(),
                &declarations,
                tola_input::InputMode::new(InputScope::Online, false)
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn rejects_unknown_command_fields() {
        let (site, declarations) = theme_site(&VALID.replace("argv = [", "typo = true\nargv = ["));
        let error = loaded_modules(
            site.path(),
            &declarations,
            tola_input::InputMode::new(InputScope::Online, false),
        )
        .unwrap_err();
        assert!(format!("{:#}", anyhow::Error::new(error)).contains("unknown field"));
    }

    #[cfg(unix)]
    #[test]
    fn query_entry_symlink_cannot_escape_module() {
        use std::os::unix::fs::symlink;
        let (site, declarations) = theme_site(VALID);
        let path = site.path().join("theme/model.typ");
        fs::remove_file(&path).unwrap();
        fs::write(site.path().join("outside.typ"), "outside").unwrap();
        symlink(site.path().join("outside.typ"), path).unwrap();
        let mode = InputMode::new(InputScope::Online, false);
        assert!(matches!(
            loaded_modules(site.path(), &declarations, mode),
            Err(ModuleLoadError::Roots(
                tola_module::inputs::ModuleRootError::Tree {
                    source: tola_input::TreeHashError::Escapes { .. },
                    ..
                }
            )),
        ));
    }

    #[test]
    fn pure_mode_rejects_external_root() {
        let (external, _) = theme_site(VALID);
        let site = tempfile::tempdir().unwrap();
        let declarations = BTreeMap::from([(
            "forest".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: external.path().join("theme"),
                },
                ..Default::default()
            },
        )]);
        assert!(
            loaded_modules(
                site.path(),
                &declarations,
                tola_input::InputMode::new(InputScope::Online, false)
            )
            .is_ok()
        );
        assert!(
            loaded_modules(
                site.path(),
                &declarations,
                tola_input::InputMode::new(InputScope::Pure, false)
            )
            .is_err()
        );
    }

    mod module_assets {
        use std::fs;
        use std::path::{Path, PathBuf};

        use crate::build::{BuildMode, BuildRequest, BuildReuse, BuildTrigger};
        use crate::config::ResolvedSiteConfig;
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession};
        use tola_input::InputScope;

        fn write(root: &Path, name: &str, value: &str) {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }

        fn rebuild(session: &mut BuildSession, config: &ResolvedSiteConfig, paths: Vec<PathBuf>) {
            let request = BuildRequest {
                trigger: BuildTrigger::Paths(paths.into()),
                reuse: BuildReuse {
                    content_inventory: true,
                    typst_compilation: true,
                    configured_assets: true,
                },
                ..BuildRequest::new(BuildMode::Development)
            };
            session
                .build_and_write(config, request)
                .unwrap_or_else(|error| {
                    panic!(
                        "build failed: {error:#}; diagnostics: {:#?}",
                        tola_diagnostic::attached(&error)
                    )
                });
        }

        struct Project {
            _dir: tempfile::TempDir,
            root: PathBuf,
            module: PathBuf,
        }
        impl Project {
            fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().join("site");
                let module = dir.path().join("module");
                fs::create_dir_all(root.join("content")).unwrap();
                write(
                    &module,
                    "tola-module.toml",
                    "[typst]\nentry=\"lib.typ\"\n[build]\nentry=\"site.typ\"\n[options]\nlegacy=false\nfonts=false\n[assets]\ntrees=[{ source=\"assets\", url-prefix=\"/\" }]\nfiles=[{ source=\"assets/scripts/legacy.js\", url=\"/scripts/legacy.js\", enabled-by=\"legacy\" }, { source=\"assets/fonts/body.woff2\", url=\"/fonts/body.woff2\", enabled-by=\"fonts\" }]\n",
                );
                write(&module, "lib.typ", "#let marker = [Library available]");
                write(&module, "assets/style.css", "module-style");
                write(&module, "assets/scripts/main.js", "main-script");
                write(&module, "assets/scripts/legacy.js", "old-script");
                write(&module, "assets/fonts/body.woff2", "font-data");
                write(
                    &module,
                    "site.typ",
                    r#"
        #import "@tola/module:0.0.0": get-module
        #import "lib.typ": marker
        #let assets = get-module().assets
        #document("index.html", format: "html")[
          #marker
          #for (key, value) in assets.urls { [#key = #value; ] }
          #assert.eq(assets.files.keys().sorted(), assets.urls.keys().sorted())
          #if "style.css" in assets.files { [style-input: #read(assets.files.at("style.css")); ] }
          #if "scripts/main.js" in assets.files { [script-input: #read(assets.files.at("scripts/main.js")); ] }
        ]
        "#,
                );
                write(&root, "content/index.typ", "Content");
                let project = Self {
                    _dir: dir,
                    root,
                    module,
                };
                project.config("");
                project
            }
            fn config(&self, extra: &str) {
                write(
                    &self.root,
                    "tola.toml",
                    &format!(
                        "[site]\nbase-path=\"/notes/\"\n[modules.demo]\npath=\"../module\"\n[build]\nentry={{ module=\"demo\" }}\n[typst.fonts]\nsystem=false\n[build.minify]\nhtml=false\ncss=false\njavascript=false\n{extra}"
                    ),
                );
            }
            fn loaded(&self) -> crate::config::ResolvedSiteConfig {
                load_site_config(
                    Some(&self.root.join("tola.toml")),
                    tola_typst::PackageLocations::default(),
                    &BuildOverrides::default(),
                )
                .unwrap()
                .into_config()
            }
            fn session(&self) -> BuildSession {
                BuildSession::with_resources(
                    BuildResources::new().with_input_scope(InputScope::Offline),
                )
            }
            fn html(&self) -> String {
                fs::read_to_string(self.root.join("public/index.html")).unwrap()
            }
            fn build(&self) {
                self.session()
                    .build_and_write(&self.loaded(), BuildRequest::default())
                    .unwrap();
            }
            fn build_error(&self) -> anyhow::Error {
                self.session()
                    .build_and_write(&self.loaded(), BuildRequest::default())
                    .expect_err("the build must fail")
            }
            fn asset(&self, path: &str) -> String {
                fs::read_to_string(self.root.join("public/_tola-modules/demo").join(path)).unwrap()
            }
            fn develop(&self) -> (ResolvedSiteConfig, BuildSession) {
                let config = self.loaded();
                let mut session = self.session();
                session
                    .build_and_write(&config, BuildRequest::new(BuildMode::Development))
                    .unwrap();
                (config, session)
            }
            fn repoint(&self, path: &str) {
                let config = self.root.join("tola.toml");
                let text = fs::read_to_string(&config)
                    .unwrap()
                    .replace("../module", path);
                fs::write(config, text).unwrap();
            }
        }

        #[test]
        fn disabled_assets_keep_library_available() {
            let project = Project::new();
            project.config("[modules.demo.assets]\nenable=false\n[modules.demo.assets.replace]\n\"style.css\"=\"missing.css\"\n");
            project.build();
            assert!(project.html().contains("Library available"));
            assert!(!project.root.join("public/_tola-modules/demo").exists());
        }

        #[test]
        fn selected_assets_publish_replacements() {
            let project = Project::new();
            write(&project.root, "overrides/custom.css", "project-style");
            project.config("[assets]\ncache-busting=true\n[modules.demo.assets.replace]\n\"style.css\"=\"overrides/custom.css\"\n");
            project.build();
            let output = project.root.join("public/_tola-modules/demo");
            assert_eq!(
                fs::read_to_string(output.join("style.css")).unwrap(),
                "project-style"
            );
            assert_eq!(
                fs::read_to_string(output.join("scripts/main.js")).unwrap(),
                "main-script"
            );
            assert!(!output.join("scripts/legacy.js").exists());
            assert!(!output.join("fonts/body.woff2").exists());
            let html = project.html();
            for text in [
                "style-input: project-style",
                "script-input: main-script",
                "/notes/_tola-modules/demo/style.css?h=",
                "/notes/_tola-modules/demo/scripts/main.js?h=",
            ] {
                assert!(html.contains(text), "{text}: {html}");
            }
        }

        #[test]
        fn disabled_replacement_is_skipped_and_unknown_asset_is_rejected() {
            let project = Project::new();
            project.config("[modules.demo.assets.replace]\n\"scripts/legacy.js\"=\"missing.js\"\n");
            project.build();
            assert!(
                !project
                    .root
                    .join("public/_tola-modules/demo/scripts/legacy.js")
                    .exists()
            );

            project.config("[modules.demo.assets.replace]\n\"missing.css\"=\"custom.css\"\n");
            project.build_error();
        }

        #[test]
        fn moved_module_keeps_alias() {
            let project = Project::new();
            project.build();
            let moved = project.module.with_file_name("relocated");
            fs::rename(&project.module, &moved).unwrap();
            write(&moved, "assets/style.css", "moved-style");
            project.repoint("../relocated");
            project.build();
            assert_eq!(project.asset("style.css"), "moved-style");
            assert!(project.html().contains("style-input: moved-style"));
        }

        #[test]
        fn disabled_exact_file_takes_over_invalid_tree_name() {
            let project = Project::new();
            write(&project.module, "assets/scripts/bad#name.js", "disabled");
            let manifest = project.module.join("tola-module.toml");
            let source = fs::read_to_string(&manifest).unwrap().replace(
                "files=[",
                "files=[{ source=\"assets/scripts/bad#name.js\", url=\"/scripts/ignored.js\", enable=false }, ",
            );
            fs::write(manifest, source).unwrap();
            project.build();
            assert!(
                project
                    .root
                    .join("public/_tola-modules/demo/style.css")
                    .exists()
            );
            assert!(
                !project
                    .root
                    .join("public/_tola-modules/demo/scripts/ignored.js")
                    .exists()
            );
            assert!(
                !project
                    .root
                    .join("public/_tola-modules/demo/scripts/bad#name.js")
                    .exists()
            );
        }

        #[test]
        fn replacement_change_republishes() {
            let project = Project::new();
            write(&project.root, "custom.css", "first replacement");
            project.config("[assets]\ncache-busting=true\n[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n");
            let (config, mut session) = project.develop();
            let first = project.html();
            write(&project.root, "custom.css", "second replacement");
            rebuild(&mut session, &config, vec![project.root.join("custom.css")]);
            let second = project.html();
            assert!(second.contains("style-input: second replacement"));
            assert_ne!(first, second);
            assert_eq!(project.asset("style.css"), "second replacement");
        }

        #[test]
        fn module_replacement_keeps_alias_asset() {
            let project = Project::new();
            fs::rename(&project.module, project.root.join("module")).unwrap();
            project.config("[modules.other]\npath=\"module\"\n[modules.demo.assets.replace]\n\"style.css\"=\"module/assets/style.css\"\n");
            project.repoint("module");
            project.build();
            for alias in ["demo", "other"] {
                assert_eq!(
                    fs::read_to_string(
                        project
                            .root
                            .join(format!("public/_tola-modules/{alias}/style.css"))
                    )
                    .unwrap(),
                    "module-style"
                );
            }
            assert!(project.html().contains("style-input: module-style"));
        }

        #[cfg(unix)]
        #[test]
        fn retargeted_replacement_is_rejected() {
            use std::os::unix::fs::symlink;
            let project = Project::new();
            write(&project.root, "inside.css", "inside");
            write(project._dir.path(), "outside.css", "outside");
            symlink("inside.css", project.root.join("custom.css")).unwrap();
            project.config("[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n");
            let config = project.loaded();
            let candidate = crate::build::build_site(&config, BuildMode::Production).unwrap();
            fs::remove_file(project.root.join("custom.css")).unwrap();
            symlink(
                project._dir.path().join("outside.css"),
                project.root.join("custom.css"),
            )
            .unwrap();
            assert!(crate::build::write_site(&candidate).is_err());
            assert!(!project.root.join("public/index.html").exists());
        }
    }

    mod module_theme {
        use std::fs;
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use crate::build::{BuildMode, BuildRequest, BuildReuse, BuildTrigger};
        use crate::cancellation::BuildCancellation;
        use crate::config::ResolvedSiteConfig;
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession, modules};
        use tola_input::InputScope;

        struct Project {
            _workspace: tempfile::TempDir,
            root: PathBuf,
            module: PathBuf,
        }

        fn write(root: &Path, path: &str, source: &str) {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }

        impl Project {
            fn new() -> Self {
                let workspace = tempfile::tempdir().unwrap();
                let root = workspace.path().join("project");
                let module = workspace.path().join("forester");
                fs::create_dir_all(&root).unwrap();
                fs::create_dir_all(&module).unwrap();
                write(
                    &module,
                    "tola-module.toml",
                    r#"
        [typst]
        entry = "lib.typ"
        [build]
        entry = "site.typ"
        [assets]
        trees = [{ source = "assets", url-prefix = "/" }]
        [options]
        show-footer = true
        labels = ["Default label"]
        [queries.nodes]
        entry = "model.typ"
        label = "theme-acceptance-nodes"
                "#,
                );
                write(
                    &module,
                    "lib.typ",
                    r#"#import "@tola/source:0.0.0": tola-meta
        #import "helper.typ": helper-marker
        #import "@tola/module:0.0.0": get-module
        #let module-settings = get-module()
        #let package-marker = read("/package-root.txt").trim()
        #let note(title) = tola-meta((title: title))
        #let source-text(source) = read(source.file)
        #let source-content(source) = include source.file
        "#,
                );
                write(
                    &module,
                    "helper.typ",
                    r#"#let helper-marker = read("data.txt").trim()"#,
                );
                write(&module, "components/footer.typ", "Default module footer.");
                write(
                    &module,
                    "assets/style.css",
                    "body { color: rebeccapurple; }",
                );
                write(&module, "data.txt", "Relative module data");
                write(&module, "package-root.txt", "Absolute module data");
                write(
                    &module,
                    "content/index.typ",
                    "WRONG MODULE SOURCE: a project path must not be rebased here.",
                );
                write(
                    &module,
                    "site.typ",
                    r#"#import "@tola/source:0.0.0": all-sources
        #import "lib.typ": helper-marker, package-marker, source-content, module-settings
        #document("index.html", format: "html")[
          #html.elem("link", attrs: (rel: "stylesheet", href: module-settings.assets.urls.at("style.css")))
          #helper-marker / #package-marker
          Labels count: #module-settings.options.labels.len()
          #for source in all-sources() { source-content(source) }
          #if module-settings.options.show-footer { include "components/footer.typ" }
        ]
        "#,
                );
                write(
                    &module,
                    "model.typ",
                    r#"#import "@tola/source:0.0.0": all-sources
        #import "lib.typ": helper-marker, package-marker, source-text, module-settings
        #metadata(all-sources().map(source => (
          path: source.path,
          title: source.meta.title,
          text: source-text(source),
          relative: helper-marker,
          absolute: package-marker,
          alias: module-settings.name,
          options: module-settings.options,
        )))<theme-acceptance-nodes>
        "#,
                );
                write(
                    &root,
                    "content/index.typ",
                    r#"#import "@tola-modules/forester:0.0.0": note
        #note("Project title")
        Project body marker.
        "#,
                );
                let project = Self {
                    _workspace: workspace,
                    root,
                    module,
                };
                project.configure("");
                project
            }

            fn configure(&self, extra: &str) {
                write(
                    &self.root,
                    "tola.toml",
                    &format!(
                        r#"[site]
        base-path = "/mounted/"
        [modules.forester]
        path = "../forester"
        [build]
        entry = {{ module = "forester" }}
        [typst.fonts]
        system = false
        {extra}
        "#
                    ),
                );
            }

            fn config(&self) -> anyhow::Result<ResolvedSiteConfig> {
                load_site_config(
                    Some(&self.root.join("tola.toml")),
                    tola_typst::PackageLocations::default(),
                    &BuildOverrides::default(),
                )
                .map(|loaded| loaded.into_config())
            }

            fn session(&self) -> BuildSession {
                BuildSession::with_resources(
                    BuildResources::new().with_input_scope(InputScope::Offline),
                )
            }

            fn html(&self) -> String {
                fs::read_to_string(self.root.join("public/index.html")).unwrap()
            }
            fn build(&self) -> anyhow::Result<()> {
                self.session()
                    .build_and_write(&self.config()?, BuildRequest::default())
                    .map(|_| ())
            }
            fn build_error(&self) -> anyhow::Error {
                self.build().expect_err("the build must fail")
            }
        }

        #[test]
        fn sibling_theme_supplies_site() {
            let project = Project::new();
            let mut initial = fs::read_dir(&project.root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            initial.sort();
            assert_eq!(initial, ["content", "tola.toml"]);
            project.build().unwrap();
            let html = project.html();
            for expected in [
                "Relative module data",
                "Absolute module data",
                "Project body marker.",
                "Default module footer.",
                "Labels count: 1",
                "/mounted/_tola-modules/forester/style.css",
            ] {
                assert!(html.contains(expected), "{expected}: {html}");
            }
            assert!(!html.contains("WRONG MODULE SOURCE"));
            assert!(!project.root.join("site.typ").exists());
            assert!(!project.root.join("theme").exists());
            let stylesheet =
                fs::read_to_string(project.root.join("public/_tola-modules/forester/style.css"))
                    .unwrap();
            assert!(stylesheet.contains("body") && stylesheet.contains("color"));
        }

        #[test]
        fn private_query_keeps_project_paths() {
            let project = Project::new();
            // The query cannot assume before-build has produced the declared asset tree.
            fs::remove_dir_all(project.module.join("assets")).unwrap();
            let model = project.module.join("model.typ");
            let source = fs::read_to_string(&model).unwrap().replace(
                "options: module-settings.options,",
                "options: module-settings.options, assets-ready: module-settings.assets != none,",
            );
            fs::write(model, source).unwrap();
            project.configure(
                r#"[[hooks.before-build]]
        name = "must-not-run-for-a-query"
        argv = ["this-module-test-hook-must-never-be-spawned"]
        "#,
            );
            let config = Arc::new(project.config().unwrap());
            let resolved = super::loaded_modules(
                config.get_root(),
                config.modules(),
                tola_input::InputMode::new(InputScope::Offline, false),
            )
            .unwrap();
            let module = &resolved["forester"];
            let result = modules::query::run(
                Arc::clone(&config),
                module,
                &module.queries["nodes"],
                "forester.nodes",
                BuildResources::new().with_input_scope(InputScope::Offline),
                Vec::new(),
                &BuildCancellation::default(),
            )
            .unwrap();
            assert!(result.success, "{:?}", result.diagnostics);
            let row = &result.data.unwrap()[0];
            assert_eq!(row["path"], "index.typ");
            assert_eq!(row["title"], "Project title");
            assert_eq!(row["relative"], "Relative module data");
            assert_eq!(row["absolute"], "Absolute module data");
            assert_eq!(row["alias"], "forester");
            assert_eq!(row["assets-ready"], false);
            assert_eq!(row["options"]["show-footer"], true);
            assert_eq!(
                row["options"]["labels"],
                serde_json::json!(["Default label"])
            );
            assert!(
                row["text"]
                    .as_str()
                    .unwrap()
                    .contains("Project body marker.")
            );
            assert!(!project.root.join("public").exists());
        }

        #[test]
        fn aliases_share_one_module() {
            let project = Project::new();
            project.configure("[modules.another]\npath = '../forester'\n");
            let config = project.config().unwrap();
            let resolved = super::loaded_modules(
                config.get_root(),
                config.modules(),
                tola_input::InputMode::new(InputScope::Offline, false),
            )
            .unwrap();
            assert_eq!(resolved["forester"].root, resolved["another"].root);
            assert!(
                super::loaded_modules(
                    config.get_root(),
                    config.modules(),
                    tola_input::InputMode::new(InputScope::Pure, false)
                )
                .is_err()
            );
            write(
                &project.root,
                "content/alias.typ",
                r#"#import "@tola-modules/another:0.0.0" as another
        #another.note("Another imported title")
        Imported alias: #another.module-settings.name
        "#,
            );
            project.build().unwrap();
            assert!(project.html().contains("Imported alias: another"));
        }

        #[test]
        fn explicit_options_override_defaults() {
            let project = Project::new();
            project.configure("[modules.forester.options]\nshow-footer = false\nlabels = []\n");
            project.build().unwrap();
            let html = project.html();
            assert!(html.contains("Labels count: 0"), "{html}");
            assert!(!html.contains("Default module footer."), "{html}");
        }

        #[test]
        fn theme_assets_hit_collision_checks() {
            let project = Project::new();
            let entry = project.module.join("site.typ");
            let mut source = fs::read_to_string(&entry).unwrap();
            source.push_str(
                "\n#asset(\"_tola-modules/forester/style.css\", \"Conflicting stylesheet\")\n",
            );
            fs::write(entry, source).unwrap();
            let error = project.build_error();
            assert!(format!("{error:#}").contains("style.css"));
            assert!(!project.root.join("public/index.html").exists());
        }

        #[test]
        fn missing_theme_reports_path() {
            let project = Project::new();
            let config_path = project.root.join("tola.toml");
            let source = fs::read_to_string(&config_path)
                .unwrap()
                .replace("../forester", "../missing-forester");
            fs::write(&config_path, source).unwrap();
            let failure = project.config().and_then(|config| {
                super::loaded_modules(
                    config.get_root(),
                    config.modules(),
                    tola_input::InputMode::new(InputScope::Offline, false),
                )
                .map_err(anyhow::Error::new)
            });
            let error = failure.expect_err("a missing module cannot be resolved");
            assert!(format!("{error:#}").contains("missing-forester"));
        }

        #[test]
        fn build_invalidates_module_reads() {
            let project = Project::new();
            let config = project.config().unwrap();
            let mut session = project.session();
            session
                .build_and_write(&config, BuildRequest::new(BuildMode::Development))
                .unwrap();
            assert!(project.html().contains("Relative module data"));
            let changed = project.module.join("data.txt");
            fs::write(&changed, "Edited external module data").unwrap();
            session
                .build_and_write(
                    &config,
                    BuildRequest {
                        trigger: BuildTrigger::Paths(Arc::from([changed].as_slice())),
                        reuse: BuildReuse {
                            content_inventory: true,
                            typst_compilation: true,
                            configured_assets: true,
                        },
                        ..BuildRequest::new(BuildMode::Development)
                    },
                )
                .unwrap();
            let html = project.html();
            assert!(html.contains("Edited external module data"), "{html}");
            assert!(!html.contains("Relative module data"), "{html}");
        }
    }
}
