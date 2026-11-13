# tola-nu

`tola-nu` prepares one Nushell environment for Tola: built-in virtual sources, configured site modules, image and diagnostic commands, and an LSP supervisor. The caller supplies the configuration, modules, overlays, and invocation; this crate loads no personal Nu configuration or plugins.

## Modules

- `execution.rs` — evaluates prepared Nu blocks and returns exit codes instead of terminating the host.
- `host.rs` — native getters, queries, diagnostic rendering, and report submission bound to one invocation.
- `modules.rs` — configured modules, sources, overlays, and dependency evidence.
- `resources.rs` — built-in module sources registered from embedded bytes as virtual files.
- `signatures.rs` — native command signatures for module and script help without evaluating scripts.
- `image.rs`, `palette.rs`, `highlight.rs` — image inspection and resizing, the default TUI palette, and highlighting. SVG resize uses Typst's renderer with the invocation's fonts.
- `lsp.rs` — the official `nu-lsp` supervisor, rebuilt when module inputs change.
- `error.rs` — the crate's error type.

## Embedded sources

`nu/` ships the diagnostic and image libraries and their command entries. The application supplies `nu-helpers` from `modules/nu-helpers/`. The root `tola` module exports the native getters, `query`, and `syntax-highlight`. The diagnostic and image sources import Rust commands from `tola-native`, whose name is bound while the built-in sources are parsed.

## Invariants

- Parse each named script once, evaluate the parsed block through the official engine; never rewrite module block spans.
- Real site modules keep file and overlay tracking; imports do not select independent instances.
- Getters read the invocation's supplied records. Queries and diagnostic submission call the supplied host functions.
- Nu's `PWD` stays in its engine and stack. Disable `nu_protocol::engine::UPDATE_CWD` before initialization so display hooks and environment merges never change the host process directory.
- The root `$nu` path fields stay empty: no personal config, plugins, or environment discovery.
- The LSP supervisor discards or restarts work on malformed input instead of exiting the host.
