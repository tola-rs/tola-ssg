//! Module declarations and explicit remote source acquisition.

mod arguments;
mod locked;
mod remote;
pub use locked::declared_root;
pub use locked::fetch_modules;
pub(crate) mod bootstrap;
pub mod entities;
pub mod hooks;
pub(crate) mod options;
pub(crate) mod prepared;
pub mod query;
pub mod semantics;
mod watch;

pub use watch::{HookWatchInputs, PreparedHookWatch, prepare_hook_watch};

pub use arguments::expand_argument;
pub use manifest::{InitializerSource, NuModule, resolve_initializer, resolve_nu_modules};
pub use options::{OptionsFailure, validate_options};
pub use prepared::configure as prepare;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::InputScope;

/// Help for the site's module declarations.
pub const HELP: &str = r#"A module namespace selects a directory containing `tola-module.toml`:

```toml
[modules.notes]
path = "theme"
```

Paths are relative to the site root unless absolute. Manifests are read when a module is used,
under the invocation's input scope. `--pure` requires its physical directory inside the site.
Module directories are inputs: publication and before-build generators cannot overwrite them.
Only explicitly declared modules are loaded; there is no automatic configuration merging.

For an HTTPS Git repository, replace `path` with a remote source:

```toml
[modules.notes]
repository = "https://github.com/example/notebook"
tag = "v1.0.0"
subdir = "theme"
```

Choose exactly one of `tag`, `branch`, or `rev`; `rev` is a full commit ID. `subdir` defaults
to the repository root. Run `tola module fetch notes` to fetch through gix and write `tola.lock`.
No system Git executable is used. Omit the alias to fetch all declared remote modules.
Commit `tola.lock` with the site; fetched sources live under `.tola/modules`.
Builds, queries and help use the locked sources without network access. Fetch explicitly to
refresh a branch or tag; a prepared fixed revision is reused. `--offline` can read prepared
sources, but fetch needs online mode. `--pure` uses local `path` declarations.
Remote initialization is not implemented; initialization still takes a local module directory.

To let a module generate the site, select it with `[theme] module = "notes"`. Do not also
declare `[build].entry`. The module's public library is imported as
`@tola-modules/notes:0.0.0`; `0.0.0` is the host alias version, not the module's release version.
Declare the theme's exports in its manifest:

```toml
schema-version = 1
name = "notebook"
version = "1.0.0"
[typst]
entry = "lib.typ"
[site]
entry = "site.typ"
[options]
schema = "options.typ"
export = "schema"
[assets]
directory = "assets"
[slots]
footer = "components/footer.typ"
```

Manifest paths belong to the module. Project `[modules.notes.options]` values are validated
with the exported `@tola/schema` schema; explicit false and empty arrays retain their values.
`[modules.notes.overrides] footer = "site/footer.typ"` replaces a declared slot with a project
file, preserving its own relative imports. Unknown slots are errors. Module assets publish
under `/_modules/notes/`, with the site's deployment prefix applied normally.

Select literal relative file paths or directory subtrees; these are not glob patterns. An omitted
`include` selects everything, an empty array selects nothing, and `exclude` takes precedence.
`enabled = false` disables the module's static publisher while keeping its library available.
The selection and replacement settings remain configured but inactive; replacement sources need
not exist until the publisher is enabled. An explicit replacement names a selected existing module file and uses a project-relative file:

```toml
[modules.notes.assets]
enabled = true
include = ["style.css", "scripts"]
exclude = ["scripts/legacy.js"]
[modules.notes.assets.replace]
"style.css" = "static/custom.css"
```

`get-module().assets.files` maps selected relative names to their native input paths, including
project-rooted replacements. `assets.urls` has the same keys and already includes the deployment
mount and any configured cache-busting identity. These are the selected static publisher's files,
not arbitrary Bundle or hook outputs. Use the URL directly rather than adding `base-path` again.

Within the package, import `get-module` from `@tola/module:0.0.0` to read its configured options,
native slot paths, version and asset prefix. Package absolute paths remain relative to the package
root. Import `project-path` from `@tola/site:0.0.0` to construct a project input path explicitly.

Modules may declare finite build hooks in their manifest. For example:

```toml
[[hooks]]
stage = "generate-outputs"
name = "search"
command = ["node", "{module}/tools/search.mjs"]
enabled-by = "search"
options = ["search"]
rerun-on = [{ module = "tools/search.mjs" }, { project = "content" }]
outputs = [{ tree = "search" }]
```

`enabled-by` names a validated top-level boolean option; missing or non-boolean values are errors.
The stages are `before-build`, `generate-outputs`, and `after-publish`, with the same `dev`
rules as project hooks. `generates` names project-relative inputs for `before-build`; `outputs`
names published relative files or trees for `generate-outputs`. Every `rerun-on` path explicitly
selects its module or project root. Each stage runs module hooks by alias and manifest order,
then project hooks. Module hooks receive `TOLA_MODULE_NAME` (the alias), `TOLA_MODULE_ROOT`,
and `TOLA_MODULE_MANIFEST` as well as the common hook environment.
In command arguments, `{name}`, `{module}`, `{project}`, and `{tola}` are replaced literally by the
alias, module root, project root, and current executable. In `generates`, `outputs`, and project `rerun-on` paths, `{name}`
is replaced by the alias before validation; `{module}` and `{project}` are only for arguments.
`options` explicitly selects validated top-level options to serialize as JSON. Missing keys and
non-JSON values are errors; unselected options can contain any supported Typst values.
`TOLA_MODULE_OPTIONS` names the resulting JSON file in the invocation's temporary directory;
the file exists only for that invocation, and an empty selection produces `{}`.

The manifest has `schema-version = 1`, an optional `summary`, and named queries and commands:

```toml
schema-version = 1
summary = "Notebook tools"

[queries.nodes]
entry = "model.typ"
label = "notebook-nodes"
schema-version = 1

[commands.list]
help = "List notebook entries."
kind = "query"
query = "nodes"
projection = ["id", "title"]

[commands.edit]
help = "Open a notebook entry."
kind = "argv"
argv = ["{module}/tools/edit"]
```

Run `tola notes --help`, `tola notes list --help`, or `tola query notes.nodes`.
Built-in command names are reserved; different aliases may select the same module. Help reads
declarations without evaluating a query or running its tool. Unknown manifest fields are errors.
Help, initializers and argv tools do not require unrelated query entries, slots or assets to
exist. Query execution and site construction validate those inputs when they are needed.
For `{tola} nu` commands, define arguments, types, defaults, and a description in the script's
`main` signature and comments. Root, namespace, and command help read that signature without
executing the script; Nu binds arguments when it runs. The manifest's `help` is optional and can
override the description. Other argv commands can use `help` to describe their usage.

A query entry is relative to its module, including a declared module outside the site root.
It runs with the site's source discovery and packages, preserving the site root while replacing
only the query's entry. Its label must identify exactly one metadata value. The default `json`
format accepts JSON scalars, arrays and dictionaries. Project content, paths and functions need an
explicit projection. `format = "entities"` with `schema-version = 1` instead accepts an array of
records with unique string `id`, string `label`, optional string `detail` and `url`, and an optional
native Typst `source` path. The source and external URL are distinct; the CLI reports a source's
virtual root and path. Entity queries can support editor semantics through manifest declarations:

```toml
[[semantics]]
export = "local"
parameter = { positional = 0 }
role = "reference"
query = "nodes"
```

Each role names an export of `[typst].entry` and an `entities` query. Dot-separated exports
traverse public members, such as `content.local`, never private module paths. A definition role uses
`role = "definition"`; named parameters use `parameter = { named = "address" }`.
Queries run no hooks, export no pages, and publish nothing. Missing generated inputs remain errors:
prepare them explicitly. Results identify the input mode and skipped hook policy, report declared
generated inputs as unverified, and carry diagnostics. Module input declarations are listed
regardless of their `enabled-by` condition: they describe possible producers, not executed hooks.
`resultDigest` hashes returned data; it is
not an input revision and must not be used as a precondition for editing files.

Queries print their JSON value by default. `--report` includes schema, diagnostics and input
evidence; `--pretty` indents JSON, and `--output` writes the ordinary result to a file. `-i`
browses a value in a terminal. A command's fixed `projection` is applied after selection, so
interactive previews keep the original fields. Use Nu `where` and `select` for further filtering.

An argv command runs only when invoked, from the site root, with inherited standard streams.
Its arguments receive literal `{module}`, `{project}`, and `{tola}` path substitutions; trailing user arguments
are forwarded unchanged. No shell interpolation runs. The tool receives `TOLA_PROJECT_ROOT`,
`TOLA_MODULE_ROOT`, `TOLA_MODULE_NAME`, `TOLA_CONTENT_ROOT`, `TOLA_CONFIG_PATH`, `TOLA_EXECUTABLE`, and `TOLA_INPUT_SCOPE`. The last two let a
tool call the same Tola executable with the caller's input policy; they do not sandbox the tool.
Tola forwards exit status and cancellation and restores a controlling terminal after an interactive
command. Shell job suspension with Ctrl-Z is not managed by this interface.

Use `["{tola}", "nu", "tools/new.nu"]` to run a module script with the embedded Nushell runtime.
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
let source = (image metadata-file photo.png)
image resize-file photo.png preview.webp --width 640 --op fit-width --format webp
```

`resize-file` returns metadata and requires a new destination in an existing directory.

A module can also declare its site initializer:

```toml
[init]
argv = ["{tola}", "nu", "tools/init.nu"]
```

Run `tola init notes --from path/to/module -- --title "My notes"` before a site exists.
`--alias forest` selects the new site's namespace; otherwise the manifest's name is used.
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
occupied target without overwriting existing files. An initializer is an ordinary external
program, not a sandbox: it must keep its proposed writes in `TOLA_INIT_OUTPUT_DIR`.

For another configuration, use `tola notes --config other.toml COMMAND ...`; after COMMAND,
arguments belong to that command. A declared tool never silently falls back to a global plugin.
"#;

/// A module selected by the site.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModuleDeclaration {
    #[serde(flatten)]
    pub source: ModuleSource,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, toml::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<ModuleAssetPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModuleSource {
    Local {
        path: PathBuf,
    },
    Remote {
        repository: String,
        #[serde(flatten)]
        revision: RemoteRevision,
        subdir: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteRevision {
    Tag(String),
    Branch(String),
    Rev(String),
}

impl Default for ModuleDeclaration {
    fn default() -> Self {
        Self {
            source: ModuleSource::Local {
                path: PathBuf::new(),
            },
            options: BTreeMap::new(),
            overrides: BTreeMap::new(),
            assets: None,
        }
    }
}

impl<'de> Deserialize<'de> for ModuleDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            path: Option<PathBuf>,
            repository: Option<String>,
            tag: Option<String>,
            branch: Option<String>,
            rev: Option<String>,
            subdir: Option<PathBuf>,
            #[serde(default)]
            options: BTreeMap<String, toml::Value>,
            #[serde(default)]
            overrides: BTreeMap<String, PathBuf>,
            assets: Option<ModuleAssetPolicy>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let source = match (
            fields.path,
            fields.repository,
            fields.tag,
            fields.branch,
            fields.rev,
            fields.subdir,
        ) {
            (Some(path), None, None, None, None, None) => ModuleSource::Local { path },
            (None, Some(repository), tag, branch, rev, subdir) => {
                let revision = match (tag, branch, rev) {
                    (Some(value), None, None) => RemoteRevision::Tag(value),
                    (None, Some(value), None) => RemoteRevision::Branch(value),
                    (None, None, Some(value)) => RemoteRevision::Rev(
                        gix::ObjectId::from_hex(value.as_bytes())
                            .map_err(|_| serde::de::Error::custom("rev must be a full commit ID"))?
                            .to_string(),
                    ),
                    _ => {
                        return Err(serde::de::Error::custom(
                            "choose exactly one of tag, branch, or rev",
                        ));
                    }
                };
                let subdir = subdir.unwrap_or_else(|| PathBuf::from("."));
                if repository.is_empty() || !is_relative_path(&subdir) {
                    return Err(serde::de::Error::custom(
                        "repository must not be empty and subdir must stay within the repository",
                    ));
                }
                ModuleSource::Remote {
                    repository,
                    revision,
                    subdir,
                }
            }
            _ => return Err(serde::de::Error::custom("choose either path or repository")),
        };
        Ok(Self {
            source,
            options: fields.options,
            overrides: fields.overrides,
            assets: fields.assets,
        })
    }
}

/// Project selection and replacement of one module's published assets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModuleAssetPolicy {
    pub enabled: bool,
    pub include: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub replace: BTreeMap<String, PathBuf>,
}

impl Default for ModuleAssetPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            include: None,
            exclude: Vec::new(),
            replace: BTreeMap::new(),
        }
    }
}

/// Selects the module providing the site's entry point.
#[derive(Debug, Clone, Default, Serialize, Deserialize, tola_config::Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "theme")]
pub struct ThemeConfig {
    /// Namespace of the module providing this site.
    pub module: Option<String>,
}

impl ThemeConfig {
    pub const HELP: &str = "Select one declared module whose `[site].entry` generates the site.\n\n\
```toml\n[theme]\nmodule = \"notes\"\n\n[modules.notes]\npath = \"theme\"\n```\n\n\
The project needs no copied entry program. An explicitly configured `[build].entry` conflicts\n\
with this selection. Other modules can still provide libraries, queries, and commands.\n\
Read `[modules]` for module manifests, options, paths, and public imports.";
}

/// A module-relative Typst program.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub entry: PathBuf,
}

/// A module-relative Typst schema and the value it exports.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OptionsSchema {
    pub schema: PathBuf,
    pub export: String,
}

/// A module-relative asset directory.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assets {
    pub directory: PathBuf,
}

/// A private metadata query declared by a module.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Query {
    /// Source file relative to the module root.
    pub entry: PathBuf,
    pub label: String,
    pub schema_version: u32,
    /// The shape of the metadata value returned by this query.
    #[serde(default)]
    pub format: QueryFormat,
}

/// The supported query result contracts.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QueryFormat {
    #[default]
    Json,
    Entities,
}

/// A command's statically inspectable description and action.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Command {
    #[serde(default)]
    pub help: String,
    #[serde(flatten)]
    pub action: CommandAction,
}

/// A module-owned site initializer, invoked through the same argv convention as commands.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Initializer {
    pub argv: Vec<String>,
}

/// External commands execute an explicit argv; query commands name a declared query.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    Argv {
        argv: Vec<String>,
    },
    Query {
        query: String,
        #[serde(default)]
        projection: Vec<String>,
    },
}

pub mod manifest;

/// The paths and exact bytes from which a module manifest was parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEvidence {
    root: crate::filesystem::FilesystemSourceIdentity,
    manifest: crate::filesystem::FilesystemSourceIdentity,
    digest: blake3::Hash,
    locked_source: Option<(PathBuf, String, ModuleSource)>,
}

impl ManifestEvidence {
    pub(crate) fn paths(
        &self,
    ) -> (
        &crate::filesystem::FilesystemSourceIdentity,
        &crate::filesystem::FilesystemSourceIdentity,
    ) {
        (&self.root, &self.manifest)
    }

    pub(crate) fn is_current(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> std::result::Result<bool, crate::cancellation::BuildCancelled> {
        cancellation.ensure_active()?;
        let identity = || {
            (
                crate::filesystem::FilesystemSourceIdentity::from_path(self.root.logical_path()),
                crate::filesystem::FilesystemSourceIdentity::from_path(
                    self.manifest.logical_path(),
                ),
            )
        };
        if identity() != (self.root.clone(), self.manifest.clone()) {
            return Ok(false);
        }
        let mut hasher = blake3::Hasher::new();
        match crate::filesystem::read_file_chunks(
            self.manifest.logical_path(),
            cancellation,
            |bytes| {
                hasher.update(bytes);
            },
        ) {
            Ok(()) => {}
            Err(crate::filesystem::FileReadError::Cancelled(error)) => return Err(error),
            Err(crate::filesystem::FileReadError::Io(_)) => return Ok(false),
        }
        cancellation.ensure_active()?;
        Ok(hasher.finalize() == self.digest
            && identity() == (self.root.clone(), self.manifest.clone())
            && self
                .locked_source
                .as_ref()
                .is_none_or(|(site, name, source)| {
                    locked::module_root(site, name, source).ok().as_deref()
                        == self.root.canonical_path()
                }))
    }
}

/// A module and the manifest that supplied its declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedModule {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    /// Evidence captured from the exact manifest bytes parsed for this module.
    pub manifest_evidence: Option<ManifestEvidence>,
    pub summary: String,
    pub package_name: Option<String>,
    pub version: Option<tola_typst::PackageVersion>,
    pub typst: Option<Entry>,
    pub nu: Option<Entry>,
    pub site: Option<Entry>,
    pub options_schema: Option<OptionsSchema>,
    pub assets: Option<Assets>,
    pub slots: BTreeMap<String, PathBuf>,
    pub queries: BTreeMap<String, Query>,
    pub semantics: Vec<semantics::SemanticRole>,
    pub commands: BTreeMap<String, Command>,
    pub initializer: Option<Initializer>,
    pub hooks: Vec<hooks::ModuleHook>,
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `path` is a nonempty spelling of plain path segments: no `..`, root, or prefix.
fn is_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// A module path Tola could not read, or one that leaves its module.
#[derive(Debug, thiserror::Error)]
pub enum ModulePathError {
    #[error("Tola could not read `{path}`")]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is outside module `{module}`")]
    Outside { module: String, path: String },
    #[error("`{path}` is not a file")]
    NotAFile { path: String },
    #[error("`{path}` is not a directory")]
    NotADirectory { path: String },
}

/// Why one declared module could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ModuleResolutionError {
    #[error(transparent)]
    Manifest(#[from] manifest::ManifestError),
    #[error(transparent)]
    ModulePath(#[from] ModulePathError),
    #[error("resolve entry for query {query}")]
    QueryEntry {
        query: String,
        #[source]
        source: std::io::Error,
    },
    #[error("query {query} entry escapes its module or is not a file")]
    QueryEntryOutside { query: String },
}

fn validate_module_path(
    site_root: &Path,
    module: &str,
    root: &Path,
    path: &Path,
    directory: bool,
) -> std::result::Result<PathBuf, ModulePathError> {
    let shown = crate::filesystem::display_path(&root.join(path), site_root);
    let resolved =
        root.join(path)
            .canonicalize()
            .map_err(|source| ModulePathError::Unreadable {
                path: shown.clone(),
                source,
            })?;
    if !resolved.starts_with(root) {
        return Err(ModulePathError::Outside {
            module: module.to_owned(),
            path: shown,
        });
    }
    let right_type = if directory {
        resolved.is_dir()
    } else {
        resolved.is_file()
    };
    if !right_type {
        return Err(if directory {
            ModulePathError::NotADirectory { path: shown }
        } else {
            ModulePathError::NotAFile { path: shown }
        });
    }
    Ok(resolved)
}

/// Commands and initializer rooted in one module, independent of its build inputs.
#[derive(Debug)]
pub struct ModuleCommands {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub summary: String,
    pub commands: BTreeMap<String, Command>,
    pub initializer: Option<Initializer>,
}

/// Command declarations remain available when unrelated build inputs need repair.
pub fn resolve_command_modules(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::result::Result<BTreeMap<String, ModuleCommands>, manifest::ManifestError> {
    Ok(manifest::read_selected(site_root, declarations, scope)?
        .into_iter()
        .map(|(name, loaded)| {
            (
                name.clone(),
                ModuleCommands {
                    name,
                    root: loaded.root,
                    manifest: loaded.path,
                    summary: loaded.manifest.summary,
                    commands: loaded.manifest.commands,
                    initializer: loaded.manifest.init,
                },
            )
        })
        .collect())
}

/// Resolve explicitly declared local modules with the caller's input scope.
///
/// The manifest is read only after the scope is known. Pure builds require the
/// module's physical root to stay inside the site. Relative query entries must
/// stay within their own module, including after following symlinks.
pub fn resolve_modules(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::result::Result<BTreeMap<String, ResolvedModule>, ModuleResolutionError> {
    let mut result = BTreeMap::new();
    for (name, loaded) in manifest::read_selected(site_root, declarations, scope)? {
        let root = loaded.root;
        let canonical_manifest = loaded.path;
        let evidence = loaded.evidence;
        let manifest = loaded.manifest;
        for entry in [
            manifest.typst.as_ref().map(|v| &v.entry),
            manifest.nu.as_ref().map(|v| &v.entry),
            manifest.site.as_ref().map(|v| &v.entry),
            manifest.options.as_ref().map(|v| &v.schema),
        ]
        .into_iter()
        .flatten()
        {
            validate_module_path(site_root, &name, &root, entry, false)?;
        }
        let assets_disabled = declarations
            .get(&name)
            .and_then(|declaration| declaration.assets.as_ref())
            .is_some_and(|policy| !policy.enabled);
        if let Some(assets) = &manifest.assets
            && !assets_disabled
        {
            validate_module_path(site_root, &name, &root, &assets.directory, true)?;
        }
        for path in manifest.slots.values() {
            validate_module_path(site_root, &name, &root, path, false)?;
        }
        for (query_name, query) in &manifest.queries {
            let query_key = format!("{name}.{query_name}");
            let entry = root.join(&query.entry).canonicalize().map_err(|source| {
                ModuleResolutionError::QueryEntry {
                    query: query_key.clone(),
                    source,
                }
            })?;
            if !(entry.starts_with(&root) && entry.is_file()) {
                return Err(ModuleResolutionError::QueryEntryOutside { query: query_key });
            }
        }
        result.insert(
            name.clone(),
            ResolvedModule {
                name: name.clone(),
                root,
                manifest: canonical_manifest,
                manifest_evidence: Some(evidence),
                summary: manifest.summary,
                package_name: manifest.name,
                version: manifest.version,
                typst: manifest.typst,
                nu: manifest.nu,
                site: manifest.site,
                options_schema: manifest.options,
                assets: manifest.assets,
                slots: manifest.slots,
                queries: manifest.queries,
                semantics: manifest.semantics,
                commands: manifest.commands,
                initializer: manifest.init,
                hooks: manifest.hooks,
            },
        );
    }
    Ok(result)
}

/// One imported module with its project-owned options and slot overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredModule {
    pub module: ResolvedModule,
    pub options: BTreeMap<String, toml::Value>,
    /// Slot implementations resolved from either the module or the project.
    pub slots: BTreeMap<String, PathBuf>,
}

/// Check project-owned configuration for any imported module.
pub fn configure_module(
    config: &crate::config::ResolvedSiteConfig,
    module: ResolvedModule,
) -> Result<ConfiguredModule> {
    let namespace = &module.name;
    let declaration = config
        .modules()
        .get(namespace)
        .with_context(|| format!("module `{namespace}` is not declared in `modules`"))?;
    ensure!(
        declaration.options.is_empty() || module.options_schema.is_some(),
        "module `{namespace}` has options but declares no `options.schema`"
    );
    let site_root = config
        .get_root()
        .canonicalize()
        .context("Tola could not read the site directory")?;
    let mut slots = module
        .slots
        .iter()
        .map(|(name, path)| (name.clone(), module.root.join(path)))
        .collect::<BTreeMap<_, _>>();
    for (name, path) in &declaration.overrides {
        ensure!(
            slots.contains_key(name),
            "module `{namespace}` has no slot `{name}`"
        );
        ensure!(
            is_relative_path(path),
            "`modules.{namespace}.overrides.{name}` must be a relative path inside the site"
        );
        let resolved = site_root.join(path).canonicalize().with_context(|| {
            format!("Tola could not read `modules.{namespace}.overrides.{name}`")
        })?;
        ensure!(
            resolved.starts_with(&site_root) && resolved.is_file(),
            "`modules.{namespace}.overrides.{name}` points outside the site or is not a file"
        );
        slots.insert(name.clone(), resolved);
    }
    Ok(ConfiguredModule {
        module,
        options: declaration.options.clone(),
        slots,
    })
}

/// The selected site driver and its project-owned configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub module: ResolvedModule,
    pub site_entry: PathBuf,
    pub options: BTreeMap<String, toml::Value>,
    pub slots: BTreeMap<String, PathBuf>,
}

/// Resolve the selected theme only when the caller knows its input scope.
pub fn resolve_theme(
    config: &crate::config::ResolvedSiteConfig,
    scope: InputScope,
) -> Result<Option<ResolvedTheme>> {
    let Some(namespace) = &config.theme().module else {
        return Ok(None);
    };
    let module = resolve_modules(config.get_root(), config.modules(), scope)?
        .remove(namespace)
        .expect("theme namespace was validated during configuration");
    ensure!(
        module.package_name.is_some() && module.version.is_some(),
        "the theme module `{namespace}` needs `name` and `version`"
    );
    let site = module
        .site
        .as_ref()
        .with_context(|| format!("the theme module `{namespace}` declares no `site.entry`"))?;
    let site_entry = module.root.join(&site.entry);
    let configured = configure_module(config, module)?;
    Ok(Some(ResolvedTheme {
        module: configured.module,
        site_entry,
        options: configured.options,
        slots: configured.slots,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
schema-version = 1
summary = "Forest tools"
[queries.nodes]
entry = "model.typ"
label = "forest-nodes"
schema-version = 1
[commands.list]
help = "List nodes"
kind = "query"
query = "nodes"
projection = ["id", "title"]
[commands.edit]
help = "Edit a node"
kind = "argv"
argv = ["editor", "--wait"]
"#;

    #[test]
    fn theme_paths_options_project_slot() {
        let manifest = r#"
schema-version = 1
name = "forest"
version = "2.1.3"
[typst]
entry = "lib.typ"
[site]
entry = "site.typ"
[options]
schema = "options.typ"
export = "schema"
[assets]
directory = "assets"
[slots]
footer = "footer.typ"
"#;
        let (site, _) = theme_site(manifest);
        let module = site.path().join("theme");
        for file in ["lib.typ", "site.typ", "options.typ", "footer.typ"] {
            fs::write(module.join(file), "").unwrap();
        }
        fs::create_dir(module.join("assets")).unwrap();
        fs::write(site.path().join("custom.typ"), "").unwrap();
        let source = "[modules.forest]\npath = \"theme\"\n[modules.forest.options]\nshow = false\nitems = [\"a\", \"b\"]\nnested = { count = 3 }\n[modules.forest.overrides]\nfooter = \"custom.typ\"\n[theme]\nmodule = \"forest\"\n";
        let config = crate::config::loading::resolve_site_config(
            &site.path().join("tola.toml"),
            source,
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let selected = resolve_theme(&config, InputScope::Pure).unwrap().unwrap();
        assert_eq!(selected.module.version.unwrap().to_string(), "2.1.3");
        assert_eq!(
            selected.module.typst.unwrap().entry,
            PathBuf::from("lib.typ")
        );
        assert_eq!(selected.options["show"], toml::Value::Boolean(false));
        assert_eq!(selected.options["items"].as_array().unwrap().len(), 2);
        assert_eq!(selected.options["nested"]["count"].as_integer(), Some(3));
        assert_eq!(
            selected.slots["footer"],
            site.path().join("custom.typ").canonicalize().unwrap()
        );
        assert_eq!(
            selected.site_entry,
            module.canonicalize().unwrap().join("site.typ")
        );
        let invalid = source.replace("footer = \"custom.typ\"", "missing = \"custom.typ\"");
        let config = crate::config::loading::resolve_site_config(
            &site.path().join("tola.toml"),
            &invalid,
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        assert!(resolve_theme(&config, InputScope::Online).is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("outside.typ"), "").unwrap();
            fs::remove_file(site.path().join("custom.typ")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("outside.typ"),
                site.path().join("custom.typ"),
            )
            .unwrap();
            let config = crate::config::loading::resolve_site_config(
                &site.path().join("tola.toml"),
                source,
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap()
            .into_config();
            assert!(resolve_theme(&config, InputScope::Online).is_err());
        }
    }

    #[test]
    fn theme_export_checks_identity_and_path() {
        for manifest in [
            "schema-version = 1\n[site]\nentry = \"model.typ\"\n",
            "schema-version = 1\nname = \"forest\"\nversion = \"bad\"\n[site]\nentry = \"model.typ\"\n",
            "schema-version = 1\nname = \"forest\"\nversion = \"1.0.0\"\n[site]\nentry = \"../model.typ\"\n",
        ] {
            let (site, declarations) = theme_site(manifest);
            assert!(resolve_modules(site.path(), &declarations, InputScope::Online).is_err());
        }
    }

    #[test]
    fn reads_local_module_without_executing_commands() {
        let (site, declarations) = theme_site(VALID);
        let modules = resolve_modules(site.path(), &declarations, InputScope::Pure).unwrap();
        let forest = &modules["forest"];
        assert_eq!(
            forest.root,
            site.path().join("theme").canonicalize().unwrap()
        );
        assert_eq!(forest.queries["nodes"].label, "forest-nodes");
        assert_eq!(
            forest.commands["edit"].action,
            CommandAction::Argv {
                argv: vec!["editor".into(), "--wait".into()]
            }
        );
    }

    #[test]
    fn rejects_bad_versions_and_unknown_queries() {
        for source in [
            VALID.replace("schema-version = 1", "schema-version = 2"),
            VALID.replace("query = \"nodes\"", "query = \"missing\""),
        ] {
            let (site, declarations) = theme_site(&source);
            assert!(resolve_modules(site.path(), &declarations, InputScope::Online).is_err());
        }
    }

    #[test]
    fn rejects_escape_allows_distinct_aliases() {
        let (site, mut declarations) = theme_site(&VALID.replace("model.typ", "../outside.typ"));
        assert!(resolve_modules(site.path(), &declarations, InputScope::Online).is_err());
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
            resolve_modules(site.path(), &declarations, InputScope::Online)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn rejects_unknown_command_fields() {
        let (site, declarations) = theme_site(&VALID.replace("argv = [", "typo = true\nargv = ["));
        let error = resolve_modules(site.path(), &declarations, InputScope::Online).unwrap_err();
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
        assert!(resolve_modules(site.path(), &declarations, InputScope::Online).is_err());
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
        assert!(resolve_modules(site.path(), &declarations, InputScope::Online).is_ok());
        assert!(resolve_modules(site.path(), &declarations, InputScope::Pure).is_err());
    }

    mod module_assets {
        use std::fs;
        use std::path::{Path, PathBuf};

        use crate::build::{BuildMode, BuildRequest, BuildReuse, BuildTrigger};
        use crate::config::ResolvedSiteConfig;
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession, InputScope};

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
                    "schema-version=1\nname=\"sample\"\nversion=\"1.0.0\"\n[typst]\nentry=\"lib.typ\"\n[site]\nentry=\"site.typ\"\n[assets]\ndirectory=\"assets\"\n",
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
                        "[site]\nbase-path=\"/notes/\"\n[modules.demo]\npath=\"../module\"\n[theme]\nmodule=\"demo\"\n[typst.fonts]\nsystem=false\n[build.minify]\nhtml=false\ncss=false\njavascript=false\n{extra}"
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
                fs::read_to_string(self.root.join("public/_modules/demo").join(path)).unwrap()
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
            project.config("[modules.demo.assets]\nenabled=false\ninclude=[\"scripts\"]\n[modules.demo.assets.replace]\n\"style.css\"=\"missing.css\"\n");
            project.build();
            assert!(project.html().contains("Library available"));
            assert!(!project.root.join("public/_modules/demo").exists());
        }

        #[test]
        fn selected_assets_publish_replacements() {
            let project = Project::new();
            write(&project.root, "overrides/custom.css", "project-style");
            project.config("[assets]\ncache-busting=true\n[modules.demo.assets]\ninclude=[\"style.css\",\"scripts\"]\nexclude=[\"scripts/legacy.js\"]\n[modules.demo.assets.replace]\n\"style.css\"=\"overrides/custom.css\"\n");
            project.build();
            let output = project.root.join("public/_modules/demo");
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
                "/notes/_modules/demo/style.css?h=",
                "/notes/_modules/demo/scripts/main.js?h=",
            ] {
                assert!(html.contains(text), "{text}: {html}");
            }
        }

        #[test]
        fn empty_include_rejects_bad_replacement() {
            let project = Project::new();
            write(&project.root, "custom.css", "replacement");
            project.config("[modules.demo.assets]\ninclude=[]\n");
            project.build();
            assert!(project.html().contains("Library available"));
            assert!(!project.root.join("public/_modules/demo/style.css").exists());
            for extra in [
                "[modules.demo.assets]\ninclude=[]\n[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n",
                "[modules.demo.assets]\nexclude=[\"style.css\"]\n[modules.demo.assets.replace]\n\"style.css\"=\"custom.css\"\n",
                "[modules.demo.assets.replace]\n\"missing.css\"=\"custom.css\"\n",
            ] {
                project.config(extra);
                project.build_error();
            }
        }

        #[test]
        fn stale_asset_changes_keep_outputs() {
            let project = Project::new();
            project.config(
                "[modules.demo.assets]\ninclude=[\"scripts\"]\nexclude=[\"scripts/legacy.js\"]\n",
            );
            let config = project.loaded();
            let mut session = project.session();
            session
                .build_and_write(&config, BuildRequest::default())
                .unwrap();
            write(&project.module, "assets/scripts/new.js", "new-script");
            fs::remove_file(project.module.join("assets/scripts/main.js")).unwrap();
            let paths = vec![
                project.module.join("assets/scripts/new.js"),
                project.module.join("assets/scripts/main.js"),
            ];
            rebuild(&mut session, &config, paths);
            let output = project.root.join("public/_modules/demo/scripts");
            assert!(!output.join("main.js").exists());
            assert_eq!(project.asset("scripts/new.js"), "new-script");
            assert!(!output.join("legacy.js").exists());
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
        fn excluded_invalid_name_does_not_block() {
            let project = Project::new();
            write(
                &project.module,
                "assets/scripts/bad#name.js",
                "not published",
            );
            project.config("[modules.demo.assets]\ninclude=[\"style.css\"]\n");
            project.build();
            assert!(project.root.join("public/_modules/demo/style.css").exists());
            assert!(
                !project
                    .root
                    .join("public/_modules/demo/scripts/bad#name.js")
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

        #[cfg(unix)]
        #[test]
        fn excluded_link_and_unreadable_skip_reads() {
            use std::os::unix::fs::{PermissionsExt, symlink};
            use std::sync::Arc;
            let project = Project::new();
            let scripts = project.module.join("assets/scripts");
            symlink("no-target", scripts.join("link.js")).unwrap();
            let unreadable = scripts.join("unreadable.js");
            fs::write(&unreadable, "not read").unwrap();
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
            project
                .config("[modules.demo.assets]\ninclude=[\"scripts\"]\nexclude=[\"scripts/link.js\",\"scripts/unreadable.js\",\"scripts/legacy.js\"]\n");
            let config = project.loaded();
            project.build();
            let mut checker = crate::check::SourceDiagnosticSession::with_resources(
                Arc::new(config),
                BuildResources::new().with_input_scope(InputScope::Offline),
            );
            let revision = checker
                .inspect(
                    Vec::new(),
                    &crate::cancellation::BuildCancellation::default(),
                )
                .unwrap();
            assert!(revision.compiled(), "{:?}", revision.diagnostics());
            fs::set_permissions(unreadable, fs::Permissions::from_mode(0o600)).unwrap();
        }

        #[test]
        fn moved_file_is_published() {
            let project = Project::new();
            write(&project.module, "assets/hidden/move.js", "moved script");
            project.config(
                "[modules.demo.assets]\ninclude=[\"scripts\"]\nexclude=[\"scripts/legacy.js\"]\n",
            );
            let (config, mut session) = project.develop();
            let from = project.module.join("assets/hidden/move.js");
            let to = project.module.join("assets/scripts/move.js");
            fs::rename(&from, &to).unwrap();
            rebuild(&mut session, &config, vec![from, to]);
            assert_eq!(project.asset("scripts/move.js"), "moved script");
        }

        #[test]
        fn include_change_rediscovers_files() {
            let project = Project::new();
            project.config("[modules.demo.assets]\ninclude=[\"style.css\"]\n");
            let mut session = project.session();
            session
                .build_and_write(&project.loaded(), BuildRequest::new(BuildMode::Development))
                .unwrap();
            assert!(
                !project
                    .root
                    .join("public/_modules/demo/scripts/main.js")
                    .exists()
            );
            project.config("[modules.demo.assets]\ninclude=[\"scripts\"]\n");
            session
                .build_and_write(&project.loaded(), BuildRequest::new(BuildMode::Development))
                .unwrap();
            assert_eq!(project.asset("scripts/main.js"), "main-script");
            assert!(!project.root.join("public/_modules/demo/style.css").exists());
        }

        #[test]
        fn aliases_publish_separate_selections() {
            let project = Project::new();
            project.config("[modules.other]\npath=\"../module\"\n[modules.demo.assets]\ninclude=[\"style.css\"]\n[modules.other.assets]\ninclude=[\"scripts\"]\n");
            project
                .session()
                .build_and_write(&project.loaded(), BuildRequest::new(BuildMode::Development))
                .unwrap();
            let demo = project.root.join("public/_modules/demo");
            let other = project.root.join("public/_modules/other");
            assert!(demo.join("style.css").exists());
            assert!(!demo.join("scripts/main.js").exists());
            assert!(other.join("scripts/main.js").exists());
            assert!(!other.join("style.css").exists());
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
                            .join(format!("public/_modules/{alias}/style.css"))
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
        use crate::{BuildResources, BuildSession, InputScope, modules};

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
                    r#"schema-version = 1
        name = "forester"
        version = "1.0.0"
        summary = "An independently stored notebook theme"
        [typst]
        entry = "lib.typ"
        [site]
        entry = "site.typ"
        [options]
        schema = "options.typ"
        export = "schema"
        [assets]
        directory = "assets"
        [slots]
        footer = "components/footer.typ"
        [queries.nodes]
        entry = "model.typ"
        label = "theme-acceptance-nodes"
        schema-version = 1
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
                write(
                    &module,
                    "options.typ",
                    r#"#import "@tola/schema:0.0.0" as s
        #let schema = s.schema((
          show-footer: s.optional(bool, default: true),
          labels: s.optional(s.array-of(str), default: ("Default label",)),
        ))
        "#,
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
        #import "@tola/address:0.0.0": asset-url
        #import "lib.typ": helper-marker, package-marker, source-content, module-settings
        #document("index.html", format: "html")[
          #html.elem("link", attrs: (rel: "stylesheet", href: asset-url(module-settings.asset-prefix + "/style.css")))
          #helper-marker / #package-marker
          Labels count: #module-settings.options.labels.len()
          Distribution: #module-settings.package / #module-settings.version
          #for source in all-sources() { source-content(source) }
          #if module-settings.options.show-footer { include module-settings.slots.footer }
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
          distribution: module-settings.package,
          version: module-settings.version,
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
        [theme]
        module = "forester"
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
                "/mounted/_modules/forester/style.css",
            ] {
                assert!(html.contains(expected), "{expected}: {html}");
            }
            assert!(!html.contains("WRONG MODULE SOURCE"));
            assert!(!project.root.join("site.typ").exists());
            assert!(!project.root.join("theme").exists());
            let stylesheet =
                fs::read_to_string(project.root.join("public/_modules/forester/style.css"))
                    .unwrap();
            assert!(stylesheet.contains("body") && stylesheet.contains("color"));
        }

        #[test]
        fn private_query_keeps_project_paths() {
            let project = Project::new();
            project.configure(
                r#"[[build.hooks.before-build]]
        name = "must-not-run-for-a-query"
        command = ["this-module-test-hook-must-never-be-spawned"]
        "#,
            );
            let config = Arc::new(project.config().unwrap());
            let resolved =
                modules::resolve_modules(config.get_root(), config.modules(), InputScope::Offline)
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
            assert_eq!(row["distribution"], "forester");
            assert_eq!(row["version"], "1.0.0");
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
            let resolved =
                modules::resolve_modules(config.get_root(), config.modules(), InputScope::Offline)
                    .unwrap();
            assert_eq!(resolved["forester"].root, resolved["another"].root);
            assert!(
                modules::resolve_modules(config.get_root(), config.modules(), InputScope::Pure)
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
        fn invalid_option_is_rejected() {
            let project = Project::new();
            project.configure("[modules.forester.options]\nshow-footer = 'not a boolean'\n");
            project.build_error();
            assert!(!project.root.join("public/index.html").exists());
        }

        #[test]
        fn footer_override_keeps_project_root() {
            let project = Project::new();
            write(
                &project.root,
                "overrides/footer.typ",
                "#read(\"label.txt\")",
            );
            write(
                &project.root,
                "overrides/label.txt",
                "Project footer override.",
            );
            write(
                &project.module,
                "overrides/footer.typ",
                "WRONG MODULE FOOTER",
            );
            project.configure("[modules.forester.overrides]\nfooter = 'overrides/footer.typ'\n");
            project.build().unwrap();
            let html = project.html();
            assert!(html.contains("Project footer override."), "{html}");
            assert!(!html.contains("Default module footer."), "{html}");
            assert!(!html.contains("WRONG MODULE FOOTER"), "{html}");
        }

        #[test]
        fn theme_configuration_errors_name_their_cause() {
            for (extra, needles) in [
                (
                    "[build]\nentry = 'custom.typ'\n",
                    ["entry", "theme"].as_slice(),
                ),
                (
                    "[modules.forester.overrides]\nmisspelled = 'overrides/footer.typ'\n",
                    ["misspelled"].as_slice(),
                ),
            ] {
                let project = Project::new();
                write(
                    &project.root,
                    "custom.typ",
                    "#document(\"index.html\")[User driver]",
                );
                write(&project.root, "overrides/footer.typ", "User footer");
                project.configure(extra);
                let error = project
                    .config()
                    .and_then(|config| modules::resolve_theme(&config, InputScope::Offline))
                    .expect_err("the theme configuration must be rejected");
                let message = format!("{error:#}");
                for needle in needles {
                    assert!(message.contains(needle), "{message}");
                }
            }
        }

        #[test]
        fn theme_assets_hit_collision_checks() {
            let project = Project::new();
            let entry = project.module.join("site.typ");
            let mut source = fs::read_to_string(&entry).unwrap();
            source.push_str(
                "\n#asset(\"_modules/forester/style.css\", \"Conflicting stylesheet\")\n",
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
                modules::resolve_modules(config.get_root(), config.modules(), InputScope::Offline)
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
