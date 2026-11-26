# tola-typst

Typst compiler and exporter wrapper: worlds, file views, compilation/export, read tracking, and diagnostics transport. Knows nothing about site routes, pages, or config (`tola-build` owns those).

## Modules

- `world/` — `FileView` and `WorldBuilder` (files, packages, fonts, `sys.inputs`).
- `compile/`, `bundle/`, `session.rs` — official Typst compilation, Bundle export, and per-compile state.
- `diagnostic/` — attaches producer tags and source excerpts beside upstream Typst diagnostics.
- `html/` — HTML URL rules and escaping.
- `reference_occurrence.rs`, `introspection.rs` — records exported references for document queries.
- `spelling.rs` — name suggestions shared by diagnostics and LSP.
- `download.rs`, `extract.rs`, `svg.rs`, `prelude.rs` — feature-gated package downloads, extraction, SVG helpers, and prelude.

## Invariants

- The official Typst compiler and exporter decide Typst semantics; never approximate them.
- `FileView` modes define input semantics: local caches pin observations until reset, shared caches re-read per compile, external views stay fixed until replaced.
- `BundleCancellation` covers font loading, file reads, and HTTP package downloads; cancelled reads never populate a shared cache cell. System-font discovery and synchronous Typst evaluation are indivisible.
- Diagnostics transport producer identity and excerpts without reinterpreting upstream meaning.
- Keep feature boundaries (`embed-fonts`, `network`, `scan`, `parallel`) valid and `forbid(unsafe_code)`.
