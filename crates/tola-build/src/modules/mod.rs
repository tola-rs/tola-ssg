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

pub use manifest::{find_initializer, read_declarations, read_nu_modules};
pub use prepared::{PreparedSite, prepare_site};
pub use watch::{PreparedHookWatch, prepare_hook_watch};

pub const HELP: &str = r#"A module namespace selects a directory containing `tola-module.toml`:

```toml
[modules.notes]
path = "theme"
```

Paths are relative to the site root unless absolute. Manifests are read when a module is used,
under the invocation's input scope. `--pure` requires its physical directory inside the site.
Module directories are build inputs. A site before-build hook can generate files in a
subdirectory of a site-owned local module. Publication cannot replace module inputs.
Tola loads the modules the site declares.

A prepared module can also come from an HTTPS download or a local archive:

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

`path`, `git`, and `tarball` are mutually exclusive. A `tarball` table has exactly one
`url` or file `path`, plus optional `sha256` as 64 hexadecimal digits. It accepts tar.gz,
tar.zst, and ZIP (stored/deflate). The archive must contain one outer directory with
`tola-module.toml` directly inside it. That outer layer is removed; no `subdir` or
manifest search is performed. Acquisition never runs author hooks. HTTPS downloads
must not embed credentials; private Release authentication is not provided.

For an `https` or `ssh` Git repository, replace `path` with a remote source:

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

The `git` table holds `url`, optional `subdir`,
and at most one of `tag`, `branch`, or `rev`. `git.rev` is a full commit ID; `git.subdir`
is relative to the repository root. `git.url` accepts `https://`, `ssh://`, or `git@host:path`.
Omitting all three selectors selects the repository's default branch.

A private `https` repository uses your Git credential helper. An `ssh` address, including
`git@host:path`, uses the system `ssh` with its agent, `~/.ssh/config`, and `known_hosts`.
`tola.lock` records addresses, selectors, commits, and digests, never credentials.

`tola.lock` pins Git commits and module trees, or archive files and their expanded trees.
Every existing locked input is verified. `--locked` forbids changing the lock but can
fetch its missing bytes; `--offline` forbids network access but can record local inputs.
`--pure` requires locked inputs from the site and vendor, without host caches or network.
Project-local tarballs can be expanded afresh in pure mode. Run `tola update notes` after
changing a source or to refresh it, then `tola vendor --refresh` to replace frozen copies.
An ordinary checksum failure never refreshes the lock automatically.
Remote initialization is not implemented; initialization still takes a local module directory.

Select a module as the site entry with `[build] entry = { module = "notes" }`.
Its public library is imported as `@tola-modules/notes:0.0.0`. The manifest declares
its site entry, public exports, assets, and option defaults:

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

Manifest paths are relative to the module. Project `[modules.notes.options]` values
replace the defaults, with recursive merging when both values are tables.

Module assets publish under `/_tola-modules/notes/`, with the site's deployment prefix.
`enable = false` under `[modules.notes.assets]` disables the instance's static assets.
A replacement keeps the public resource name and reads from a project-relative file:

```toml
[modules.notes.assets.replace]
"style.css" = "static/custom.css"
```

Within a module, import `get-module` from `@tola/module:0.0.0` to read its alias, options,
and assets. `assets.files` contains native input paths and `assets.urls` contains their
final URLs. Package absolute paths remain relative to the package root. Use `project-path`
from `@tola/site:0.0.0` to construct a project input path.

Modules may declare build hooks:

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "tools/search.nu"
enabled-by = "search"
rerun-on = { include = [{ project = "content/**/*.typ" }] }
outputs = [{ tree = "search" }]
```

`enabled-by` names a top-level boolean option. A string in `rerun-on.include` or
`rerun-on.exclude` is relative to the module root. A `{ project = "..." }` entry is
relative to the project root. Both forms accept globs and substitute `{name}` with
the module alias. The Nu script is observed automatically. Module hooks run in
alias and declaration order. Site hooks run first in before-build and last in the
other stages. External hooks read effective options as JSON from the file named by
`TOLA_MODULE_OPTIONS`. Embedded Nu hooks read `(get-module).options`.

Queries and commands are named in the manifest:

```toml
[queries.nodes]
entry = "model.typ"
label = "notebook-nodes"

[commands.list]
help = "List notebook entries."
query = "nodes"
projection = ["id", "name"]

[commands.edit]
help = "Open a notebook entry."
argv = ["{module}/tools/edit"]
```

Run `tola notes --help`, `tola notes list --help`, or `tola query notes.nodes`.
Built-in command names are reserved; different aliases may select the same module. Help reads
declarations without evaluating a query or running its tool. Unknown manifest fields are errors.
A Nu command uses `nu = "tools/new.nu"` and may declare `args = ["--release"]`.
Define arguments, types, defaults, and a description in the script's `main` signature
and comments. Help reads the signature without executing the script. An explicit
`help` overrides its description. External commands use the declared `help`.

A query entry is relative to its module, including a declared module outside the site root.
It uses the site's source discovery and packages. Its label selects exactly one metadata value.
Ordinary queries accept JSON-compatible scalars, arrays, and dictionaries. Native Typst paths
are converted to records with `root` and `path`; project content and functions need an explicit
projection. Private query helpers can stay outside the public Typst library.

Declare references and definitions under their query:

```toml
[[queries.nodes.references]]
export = "local"
parameter = { positional = 0 }

[[queries.nodes.definitions]]
export = "subtree"
parameter = { named = "address" }
```

Each entry names a function exported by `[typst].entry` and one of its parameters. Positional
indices start at zero. Dot-separated exports traverse public members, such as `content.local`.
Multiple entries let several functions or parameters use the same query. Unknown exports,
missing parameters, and conflicting assignments are diagnosed when the editor reads them.

A query with references or definitions automatically requires an array of records with unique,
nonempty string `id`, string `name`, optional string `description` and `url`, and optional native
Typst `source`. Both CLI and LSP validate this shape. IDs are scoped to the module alias and
query. The metadata `label` selects the whole result; each record's `name` is display text.
An ordinary query has its own fields and types. Remove the old `returns`, `semantics`, and
`role` declarations when migrating; rename record `label` and `detail` to `name` and `description`.

Queries compile their selected entry independently of the site's page template. Prepare
required generated inputs before calling them. Module input declarations list possible
producers regardless of their `enabled-by` condition. Queries sharing an entry also share
its calculations and failures. The LSP retains query sessions across edits and shares one
entry compilation across its labels within a source revision.

Queries print a human-readable table by default. `--json` returns compact JSON,
`--json --pretty` indents it, and `--json --output FILE` writes it to a file. `-i`
browses a value in a terminal. A command's fixed `projection` is applied after selection, so
interactive previews keep the original fields. Use Nu `where` and `select` for further filtering.

An argv command runs only when invoked, from the site root, with inherited standard streams.
Its arguments receive literal `{module}`, `{project}`, and `{tola}` path substitutions; trailing user arguments
are forwarded unchanged. No shell interpolation runs. The tool receives `TOLA_PROJECT_ROOT`,
`TOLA_MODULE_ROOT`, `TOLA_MODULE_NAME`, `TOLA_CONTENT_ROOT`, `TOLA_CONFIG_PATH`, `TOLA_EXECUTABLE`, and `TOLA_INPUT_SCOPE`. The last two let a
tool call the same Tola executable with the caller's input policy; they do not sandbox the tool.
Tola forwards exit status and cancellation and restores a controlling terminal after an interactive
command. Shell job suspension with Ctrl-Z is not managed by this interface.

Declare `nu = "tools/new.nu"` to run a module script with the embedded Nushell runtime.
The script entry is relative to its declaring module; the working directory remains the project
root. A direct `tola nu file.nu` invocation resolves the entry from the caller's directory.
No separate Nushell installation is needed. `tola nu --lsp` serves Nu files; `tola lsp` serves Typst.

A module argv command or hook can submit an unlocated diagnostic batch through `TOLA_REPORT_PATH`,
for example with `use tola diagnostic` and `diagnostic submit`. Warnings do not fail a successful
command; errors do. `after-publish` errors cannot undo publication. A reused development
`generate-outputs` result retains its report. A missing batch differs from a submitted empty batch.
Independent checks and unsaved editor snapshots are not supported by this report interface.

A module may also export a reusable Nu library:

```toml
[nu]
entry = "tools/mod.nu"
```

Use `use tola-modules notes` in Nu for the module selected as `notes`. Public definitions use
ordinary Nu `export`; imports do not choose a current module configuration. Pass site options
explicitly to reusable functions. The module's own resources can use Nu `path self`, while user
paths keep the invocation's working directory.

`tola nu @tola-modules/notes` runs that public entry and requires `export def main`.
`tola nu @tola/image` runs the built-in image entry. These are explicit Tola locators, not Nu
file paths or download requests. Unknown locators never fall back to files; a file beginning
with `@` can be written as `./@file.nu`. Nu uses the native roots `tola` and `tola-modules`,
such as `use tola image`, rather than a special path-prefix import mechanism.
`tola nu --help` lists built-in and configured Nu libraries without evaluating them.

The root command `query` wraps the existing site query: `use tola` provides `tola query`,
while `use tola query` imports `query` directly. `use tola cli` provides `cli capture` for
finite commands, returning `stdout`, `stderr`, and `exit_code`; pass `--config` in the
argument list when needed. For continuous development events, run `tola dev --json` as a
native external command and consume its JSON lines, rather than capturing the process.

`use tola image` provides byte operations and file operations. For example:

```nu
use tola image
let source = (image metadata photo.png)
image resize photo.png --output preview.webp --width 640 --op fit-width --format webp
```

`image resize --output` returns metadata and requires a new destination in an existing directory.

A module can also declare its site initializer:

```toml
[init]
nu = "tools/init.nu"
```

Run `tola init notes --from path/to/module --alias forest -- --title "My notes"` before a site exists.
An existing site can invoke the same operation as `tola forest init notes`.
`[init]` and `[commands.init]` cannot both be declared.

The initializer runs in an empty temporary directory and writes a complete candidate site there.
`TOLA_INIT_OUTPUT_DIR` names that directory; `TOLA_INIT_TARGET_DIR` names the final directory.
`TOLA_INIT_INTERACTIVE` is `true` only when the caller requests interactive setup. The ordinary
module and executable context remains available; project, content and config variables are absent
because the new site does not yet exist. Module paths written into `tola.toml` must resolve from
the final target, and the selected module must be registered under `TOLA_MODULE_NAME`.

Tola validates the candidate configuration and module options before creating target files.
`--dry-run` runs generation and validation but writes no target files. `--force` permits an
occupied target without overwriting existing files. Initializers write their proposed files
to the temporary generation directory. Nu initializers read it through `get-init`.

For another configuration, use `tola notes --config other.toml COMMAND ...`; after COMMAND,
arguments belong to that command. A declared tool never silently falls back to a global plugin.
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
    module_set::load_modules_with_roots(site_root, declarations, &roots, mode)
}

/// Read every declared module's manifest through a resolved root.
pub fn load_modules_with_roots(
    site_root: &Path,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
) -> std::result::Result<BTreeMap<String, LoadedModule>, module_set::ModuleLoadError> {
    module_set::load_modules_with_roots(site_root, declarations, roots, mode)
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
