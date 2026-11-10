# tola-nu

`tola-nu` prepares one Nushell environment for Tola: built-in virtual sources, configured site modules, image and diagnostic commands, CLI capture, and an LSP supervisor. The caller supplies the configuration, modules, overlays, and invocation; this crate loads no personal Nu configuration or plugins.

## Modules

- `execution.rs` — evaluates prepared Nu blocks and returns exit codes instead of terminating the host.
- `host.rs` — native getters, queries, and diagnostic submission bound to one invocation.
- `modules.rs` — configured modules, sources, overlays, and dependency evidence.
- `resources.rs` — built-in module sources registered from embedded bytes as virtual files.
- `signatures.rs` — native command signatures for module help without executing scripts.
- `cli.rs` — the `cli capture` surface that keeps original `PipelineData`.
- `image.rs`, `palette.rs`, `highlight.rs` — image inspection and resizing, the default TUI palette, and highlighting.
- `lsp.rs` — the official `nu-lsp` supervisor, rebuilt when module inputs change.
- `error.rs` — the crate's error type.

## Embedded sources

`nu/` ships `cli.nu`, `ui.nu`, `diagnostic.nu`, `image.nu`, and `image_core.nu`. The root `tola` module exports the native getters and query.

## Invariants

- Parse each named script once, evaluate the parsed block through the official engine; never rewrite module block spans.
- Real site modules keep file and overlay tracking; imports do not select independent instances.
- Getters read the invocation's supplied records. Queries and diagnostic submission call the supplied host functions.
- Nu's `PWD` stays in its engine and stack. Disable `nu_protocol::engine::UPDATE_CWD` before initialization so display hooks and environment merges never change the host process directory.
- The root `$nu` path fields stay empty: no personal config, plugins, or environment discovery.
- The LSP supervisor discards or restarts work on malformed input instead of exiting the host.
