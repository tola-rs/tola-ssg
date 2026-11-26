#show: config(
  summary: [Attach local, Git, tarball, or built-in Tola modules under site-chosen aliases.],
  related: ("@tola/module", "[hooks]", "[options]", "[vendor]"),
)

Declare each module under `[modules.<alias>]`. Multiple aliases may point to the same module source with independent options. Every module directory contains a `tola-module.toml` manifest.

= Module sources

Choose exactly one source per alias:

- `path`: Local directory (relative to the site root, or absolute when allowed by the input scope; must stay inside the site under `--pure` and outside `build.publish-dir`).
- `git`: `{ url = "...", tag | branch | rev = "...", subdir = "..." }`. Supports `https://`, `ssh://`, and `git@host:path`. Uses your local Git credential helper or SSH configuration; credentials are never written to `tola.lock`.
- `tarball`: `{ url = "https://..." }` or `{ path = "..." }`, with optional 64-hex `sha256`. Supports `.tar.gz`, `.tar.zst`, and ZIP archives containing one top-level directory with `tola-module.toml`.
- `builtin`: Bundled Tola modules shipped with the binary (`"resize-images"`, `"tailwind"`).

Run `tola update <alias>` after changing a source or to update locked commits; run `tola vendor --refresh` when vendoring. Input flags control resolution: `--locked` keeps `tola.lock` unchanged while allowing downloads, `--offline` forbids network access, and `--pure` restricts builds to in-site sources and vendored copies.

= Entry, libraries, options, assets, and hooks

- Set `[build] entry = { module = "<alias>" }` to use a module's site entry. Import its public Typst library from `@tola-modules/<alias>:0.0.0`, and read its runtime handle with #ref("@tola/module#get-module").
- Override manifest defaults under `[modules.<alias>.options]`: nested tables merge key by key; arrays and scalars replace.
- Module static assets publish under `/_tola-modules/<alias>/` (with `site.base-path` prepended). Disable all assets for an instance with `[modules.<alias>.assets] enable = false`, or replace enabled files from the site root.
- Module hooks run in the same three build stages as site hooks (see #ref("[hooks]")). Inside module manifests, `{name}`, `{module}`, `{project}`, and `{tola}` expand in arguments and paths; `{ project = "..." }` in `rerun-on` watches site files.

= Module commands, guides, and initializers

- Modules may expose author commands (`[commands.<name>]` in `tola-module.toml`, invoked as `tola <alias> <command>`), built-in guides (`skill = "GUIDE.md"`), and site initializers (`[init]`, invoked via `tola init <dir> --from <source> --alias <alias>`).
- Run `tola module pack` and `tola module check` when preparing a module for distribution.

#example(title: [Configure a built-in module and a local theme with options])[
  ```toml
  [modules.theme]
  path = "theme"

  [modules.theme.options]
  title = "Field Notes"

  [modules.tailwind]
  builtin = "tailwind"
  ```
]
