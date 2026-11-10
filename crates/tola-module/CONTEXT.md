# tola-module

`tola-module` owns Tola site modules: reading declarations and manifests, acquiring Git sources and lock entries through `tola-input`, preparing the contribution a build applies, and running module queries, options, hooks, and assets. It does not own the build sequence, the output graph, or publication.

## Modules

- `declaration.rs`, `arguments.rs` — TOML declarations, command and query shapes, public function parameter selectors, argument expansion.
- `manifest.rs` — `tola-module.toml`, static option defaults, grouped hooks, and publication selection.
- `inputs.rs` — module roots, acquisition, lock recording; the `ModuleAcquisition` context.
- `remote.rs`, `checkout.rs` — Git fetch and tree materialization; pinned bytes, no filters.
- `module_set.rs` — loaded module instances, path validation, and command discovery.
- `prepared.rs` — `acquire`/`prepare` and the `Contribution`/`PreparedModules` boundary consumed by `tola-build`.
- `options.rs` — shared static option values, recursive instance overrides, and boolean keys for optional assets and hooks.
- `action.rs`, `hooks.rs` — Nu and argv declarations and lifecycle stages.
- `query.rs`, `assets.rs`, `diagnostics.rs`, `entities.rs`, `watch.rs` — queries, asset declarations, diagnostics, entities, and watch inputs.
- `archive.rs` — bounded HTTPS/local archive reads and single-module tar.gz, tar.zst, and ZIP extraction.
- `check.rs`, `pack.rs`, `pack/selection.rs` — standalone module validation, release file enumeration, and publication.

## Invariants

- Depends on `tola-fs`, `tola-input`, `tola-typst`, `tola-packages`, `tola-diagnostic`, and `tola-address`; never on `tola-build`, `tola-config`, the compiler or check layers, or presentation.
- Read-only over the site. Lock and vendor writes go through `tola-input`.
- A declaration selects exactly one live directory `path`, a `git` table (`url`, optional `subdir`, one revision selector), or a `tarball` table (one HTTPS `url` or file `path`, optional `sha256`).
- Tarballs contain one outer directory with `tola-module.toml` immediately inside it. `archive.rs` bounds downloads, expands tar.gz/tar.zst/ZIP without links or path collisions, and removes that one layer. No subdirectory search or author code runs during acquisition.
- Tarball locks record file and tree digests. Expanded directories live in `.tola/modules/tarballs/<sha256>/`; downloaded files are temporary. Vendor copies use the same checked module directory.
- Git acquisition keeps only the selected module directory in `.tola/modules/<commit>/<sha256(subdir)>/`; aliases share it. A missing cache is refetched at the locked commit when network access is allowed, including under `--locked`. Vendor directories remain per alias.
- Existing remote contents are checked against the lock in every mode. New resolutions use their fetched directories rather than an old vendor copy. Pure builds can expand locked project-local archives afresh, but never trust a host cache.
- Declarations are authored TOML; manifests and lock entries are facts. A missing or changed fact is diagnosed, not guessed.
- `prepare` is Typst-free; `acquire` reads and fetches; `PreparedModules` is the module state the build applies.
- A module's `build.entry` names its optional site program; `typst.entry` and `nu.entry` remain public library entries. The site's `build.entry` selects one project file or module instance.
- Pack, asset trees, and hook inputs share the relative path and glob matcher in `tola-input`. Module patterns use the module root. Hook inputs can also name the project root and expand the instance alias.
- Pack validates include/exclude syntax and rejects targets that can contain selected files before author hooks. It selects files after hooks and verifies that the source manifest stayed current. Directory matches include descendants, excludes prune directories, and the release manifest is always included.
- Pack produces a checked directory and replaces the entire previous release. It removes author-only `pack` and `hooks.before-pack` declarations from the release manifest. Compression, archive loading, and publication to a remote service are outside this command.
- Public wrappers and wire types stay in `tola-build`; the CLI and LSP import module types from here.
- Queries with references or definitions validate `id`, `name`, `description`, `source`, and `url` records. Ordinary queries project values and native paths to JSON.
- `forbid(unsafe_code)`; public APIs stay documented.
