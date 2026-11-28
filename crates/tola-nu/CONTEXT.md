# tola-nu

Embedded Nushell runtime for Tola: virtual built-in libraries, configured site modules, image/diagnostic commands, and the `nu-lsp` supervisor. Loads no personal Nu config or plugins.

## Modules

- `execution.rs` — runs prepared Nu blocks and returns exit codes without terminating the host process.
- `host.rs` — per-invocation native commands (`get-site`, `get-module`, `get-hook`, `get-assets`, `get-init`, `get-context`, `query`, `submit`, `render-diagnostics`).
- `modules.rs` — configured site modules, overlays, and dependency tracking.
- `resources.rs` — registers embedded built-in Nu sources as virtual files.
- `signatures.rs` — reads command signatures for `--help` without running scripts.
- `image.rs`, `palette.rs`, `highlight.rs` — image commands (SVG via Typst + invocation fonts), TUI palette, and highlighting.
- `lsp.rs` — `nu-lsp` supervisor, rebuilt when module inputs change.
- `error.rs` — crate error type.

## Embedded sources

`nu/` contains the built-in diagnostic and image libraries; the binary supplies `terminal` from `modules/terminal/`. The root `tola` module exports host commands, `query`, and `syntax-highlight`. Built-in sources import Rust commands from `tola-native`, bound during parsing.

## Invariants

- Parse each script once and run it through the official Nu engine without rewriting module spans.
- Site modules keep file and overlay tracking; importing another Nu library does not change the active module instance.
- Keep Nu's `PWD` inside the engine stack (`UPDATE_CWD` disabled) so scripts never mutate the host process working directory.
- Keep root `$nu` config/plugin paths empty.
- The `nu-lsp` supervisor discards or restarts on malformed input instead of exiting the host.
