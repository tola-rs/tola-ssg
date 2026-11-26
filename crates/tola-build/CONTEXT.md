# tola-build

Builds, validates, and publishes Tola sites without depending on the CLI. Returns structured values and diagnostics; callers handle rendering.

## Modules

- `compiler/` — Typst inputs, official compilation, exported Bundle reading, and reusable compiler caches.
- `build/` — build pipeline: hooks, producers, compilation, graph sealing, and cache reuse.
- `check.rs` — read-only source checks for the editor (no hooks, writes, or publication).
- `output/` — output paths, collision checks, manifests, output graph, and recoverable publication.
- `site/`, `site.rs` — built documents and resources, route indexes, references, and immutable revisions.
- `content/`, `metadata/` — content discovery, source identity, and Typst source metadata records.
- `filesystem/` — source fingerprints, path normalization, unsaved overlays, and atomic writes.
- `config/` — site config loading, `include` merging, origin tracking, and validated build sections.
- `hooks/` — stage batches, concurrency permits, external execution, Nu hook dispatch, and cancellation.
- `report/` — versioned JSON diagnostic report reader for external hooks/commands.
- `seo/`, `asset/`, `image/`, `icon/`, `package/`, `modules/`, `inspect/` — output producers, module integration, and inspection views.
- `observation.rs`, `cancellation.rs`, `mode.rs`, `html.rs` — file read tracking, cancellation token, build mode, and HTML escaping.

## Invariants

- Site `include` files load depth-first before their declaring file; defaults and CLI overrides apply after merging. Module manifests do not support `include`.
- Config files (including missing optional includes) are protected from site writes and checked for freshness before publication.
- Build order: `before-build` hooks -> discovery -> root Bundle compile -> output collection -> reference checks -> candidate validation -> atomic publish.
- Within a stage, each site or module instance orders its own batches; modules run in alias order; equal-order hooks share the site concurrency limit.
- Never publish a partial graph or mutate a graph after sealing.
- Exact `[assets]` `files` entries own their source before option gates. Tree globs match from the declaring root inside `source`. Module replacements substitute already selected assets.
- `before-build` reuse requires `dev = "reuse"`, nonempty `generates`, and unchanged inputs and outputs in the current session. `generate-outputs` reuse also compares the compiled site, options, action, and outputs.
- Cache reuse must match a full build's diagnostics, dependencies, and output bytes.
- Remote icon collections are parsed before committing lock entries via `tola-input`.
- Vendor copies come from the module roots selected by a completed build; incomplete vendor swaps fail before module preparation.
- Cancellation marks the superseded run and never renders as an ordinary build error.
- Keep public APIs documented, feature boundaries valid, and `forbid(unsafe_code)`.
