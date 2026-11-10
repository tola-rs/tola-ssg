# tola-nu

`tola-nu` prepares one Nushell environment for Tola: built-in virtual sources, configured site modules, image and diagnostic commands, CLI capture, and an LSP supervisor. The caller supplies the configuration, modules, overlays, and invocation; this crate loads no personal Nu configuration or plugins.

## Modules

- `execution.rs` — evaluates prepared Nu blocks and returns exit codes instead of terminating the host.
- `modules.rs` — configured modules, sources, overlays, and dependency evidence.
- `resources.rs` — built-in module sources registered from embedded bytes as virtual files.
- `signatures.rs` — native command signatures for module help without executing scripts.
- `cli.rs` — the `cli capture` surface that keeps original `PipelineData`.
- `image.rs`, `theme.rs`, `highlight.rs` — image inspection and resizing, theme, and highlighting.
- `lsp.rs` — the official `nu-lsp` supervisor, rebuilt when module inputs change.
- `error.rs` — the crate's error type.

## Embedded sources

`nu/` ships the built-in modules: `tola.nu` (query), `cli.nu` (capture), `ui.nu`, `diagnostic.nu`, `image.nu`, `image_core.nu`. They are production source code: changes need the same ownership and test discipline as Rust.

## Invariants

- Parse each named script once, evaluate the parsed block through the official engine; never rewrite module block spans.
- Real site modules keep file and overlay tracking; imports do not select independent instances.
- The root `$nu` path fields stay empty: no personal config, plugins, or environment discovery.
- The LSP supervisor discards or restarts work on malformed input instead of exiting the host.
