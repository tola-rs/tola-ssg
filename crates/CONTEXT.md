# Workspace crates

Every crate owns one responsibility and its public contract. Read the crate-specific context when one exists; otherwise this table and the crate's module docs orient you.

## Large subsystems

- [`tola-build/CONTEXT.md`](tola-build/CONTEXT.md) — independently consumable site construction engine.
- [`tola-module/CONTEXT.md`](tola-module/CONTEXT.md) — site modules: declarations, acquisition, preparation, queries, hooks.
- [`tola-typst/CONTEXT.md`](tola-typst/CONTEXT.md) — reusable Typst world and official compiler/exporter.
- [`tola-lsp/CONTEXT.md`](tola-lsp/CONTEXT.md) — Tola language service.
- [`tola-nu/CONTEXT.md`](tola-nu/CONTEXT.md) — embedded Nushell runtime.

## Ownership

- `tola-address` — slugs, routes, logical output paths, site URLs, deployment mounts, browser reference rules.
- `tola-fs` — native filesystem operations; see [`tola-fs/CONTEXT.md`](tola-fs/CONTEXT.md).
- `tola-input` — input identity, lock entries, and the access limits one build attempt runs under.
- `tola-slugify` — turning text into a slug: naming rules, character policy, script-specific romanization.
- `tola-pronunciations` — pronunciation tables Tola carries, one feature per source, with provenance and licence.
- `tola-config`, `tola-config-macros` — reusable configuration protocol and derive macros.
- `tola-diagnostic` — the diagnostic record every producer emits and the declared code identifiers.
- `tola-subprocess` — trusted external command execution with process-tree containment and cancellation.
- `tola-icons` — validated SVG values and immutable icon collections.
- `tola-image` — deterministic image decoding, colour management, resampling, encoding.
- `tola-webp` — pure-Rust VP8 still-image encoder used by `tola-image`.
- `tola-packages` — bundled `@tola/*` packages, their natives, documentation, and signatures.
- `tola-minify` — deterministic CSS and JavaScript minification.
- `tola-highlight` — shared syntax highlighting.
- `tola-datalog` — immutable relation snapshots, stratification, tabled evaluation.
- `tola-typst-syntax` — immutable Typst source indexes and cross-source name graphs; no host I/O or evaluation.

## Rules

- Public crates keep documented APIs, feature boundaries, and `forbid(unsafe_code)`.
- A crate that renders nothing returns structured values and diagnostics; terminal output belongs to `src/terminal`.
- Dependencies point inward: domain and construction crates never depend on the application or presentation.
