# Crates

Each crate owns one domain and its public API. Read a crate's `CONTEXT.md` when present; otherwise use the list below and its module docs.

## Large subsystems

- [`tola-build/CONTEXT.md`](tola-build/CONTEXT.md) — site builds, validation, and publication.
- [`tola-module/CONTEXT.md`](tola-module/CONTEXT.md) — site module manifests, fetching, preparation, queries, hooks, and packing.
- [`tola-typst/CONTEXT.md`](tola-typst/CONTEXT.md) — Typst worlds, compilation, export, and diagnostics transport.
- [`tola-lsp/CONTEXT.md`](tola-lsp/CONTEXT.md) — Tola language server.
- [`tola-nu/CONTEXT.md`](tola-nu/CONTEXT.md) — embedded Nushell runtime.

## Ownership

- `tola-address` — routes, output paths, browser URLs, deployment mounts, and link references.
- `tola-fs` — Unix and Windows filesystem operations; see [`tola-fs/CONTEXT.md`](tola-fs/CONTEXT.md).
- `tola-input` — file/tree digests, `tola.lock`, `.tola` paths, glob selection, and input access scopes.
- `tola-slugify` — URL slug generation and CJK romanization.
- `tola-pronunciations` — Chinese and Japanese pronunciation tables (feature-gated per source).
- `tola-config`, `tola-config-macros` — TOML key paths, config derive macro, presence tracking, and field validation.
- `tola-diagnostic` — structured diagnostics and declared error codes.
- `tola-subprocess` — external command execution with process-tree cleanup, output caps, and cancellation.
- `tola-icons` — static SVG validation and IconifyJSON import.
- `tola-image` — image decoding, color handling, resizing, and encoding.
- `tola-webp` — pure-Rust lossy WebP encoder used by `tola-image`.
- `tola-packages` — built-in `@tola/*` Typst packages, natives, and docs.
- `tola-minify` — CSS and JavaScript minification.
- `tola-highlight` — terminal syntax highlighting.
- `tola-datalog` — in-memory stratified Datalog engine.
- `tola-typst-syntax` — I/O-free Typst source analysis and formatting.

## Rules

- Keep public APIs documented, feature boundaries valid, and `forbid(unsafe_code)`.
- Library crates return structured values and diagnostics; terminal rendering belongs to `src/terminal`.
- Domain crates never depend on the `tola` binary or CLI presentation.
