# tola-module

Loads, fetches, validates, prepares, and packs Tola site modules (`path`, `git`, `tarball`, and `builtin`). Does not own build execution, the output graph, or publication.

## Modules

- `declaration.rs`, `arguments.rs` — site module declarations, command/query shapes, parameter selectors, and argument placeholders.
- `manifest.rs` — `tola-module.toml` parsing, default options, hooks, and pack rules.
- `inputs.rs`, `builtin.rs` — module roots, acquisition (`ModuleAcquisition`), lock updates, and embedded built-in modules.
- `remote.rs`, `checkout.rs` — Git fetching and tree checkout (pinned bytes, no filter drivers).
- `archive.rs` — bounded HTTPS/local downloads and single-module `.tar.gz`/`.tar.zst`/`.zip` extraction.
- `module_set.rs` — loaded instances, path checks, and command lookup.
- `prepared.rs` — `acquire`/`prepare` and `ModuleBundle` consumed by `tola-build`.
- `options.rs`, `option_refs.rs` — recursive option merging and `{ option = "..." }` manifest bindings.
- `action.rs`, `hooks.rs` — Nu/argv actions, stages, and per-instance hook ordering.
- `query.rs`, `assets.rs`, `diagnostics.rs`, `entities.rs`, `watch.rs` — queries, assets, diagnostics, editor entities, and watch globs.
- `check.rs`, `pack.rs`, `pack/selection.rs` — `tola module check` and `tola module pack`.

## Invariants

- Depends on `tola-fs`, `tola-input`, `tola-typst`, `tola-packages`, `tola-diagnostic`, and `tola-address`; never on `tola-build`, `tola-config`, or presentation.
- Read-only over the site; lock and vendor writes go through `tola-input`.
- Each module selects exactly one source: `path`, `git`, `tarball`, or `builtin`.
- Built-in modules unpack from the binary into `.tola/modules/builtin/<name>/<digest>/` and need no lock entry or vendor copy.
- Tarballs must contain one outer directory with `tola-module.toml` directly inside; extraction rejects links and path collisions and runs no scripts. Expanded dirs cache under `.tola/modules/tarballs/<sha256>/`.
- Git checkouts keep only the selected directory under `.tola/modules/<commit>/<sha256(subdir)>/` with pinned EOL and no Git filters so `tree_sha256` stays host-independent. Vendor dirs stay per alias.
- Remote content is verified against `tola.lock` in every mode; `--pure` never reads host caches but may expand locked site-local archives. Missing or changed lock entries fail with a diagnostic.
- `{ option = "..." }` substitutes supported manifest fields before validation (using merged options in a site, or defaults in `tola module check`).
- `prepare` runs no Typst; `acquire` handles reading and fetching.
- Hook contributions keep disabled entries and skip reasons. Each instance owns its hook names and `order`; the site owns execution and concurrency limits.
- Pack, asset trees, and `rerun-on` share `tola-input`'s glob matcher. Each `rerun-on` include opts into `.git`/`.jj` independently by naming that component.
- `tola module pack` checks patterns and target overlap before `before-pack`, strips `pack` and `hooks.before-pack` from the output manifest, and replaces the target directory atomically.
- Editor queries (`references`/`definitions`) validate `id`, `name`, `description`, `source`, and `url`; plain queries project values and paths to JSON.
- Keep public APIs documented and `forbid(unsafe_code)`.
