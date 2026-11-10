# tola-typst

`tola-typst` is the reusable Typst layer: worlds, file views, official compilation and export, retainable read evidence, and transport for native diagnostics. Site rules belong to `tola-build`; this crate knows no route, page, or configuration.

## Modules

- `world/` — `FileView` and `WorldBuilder`: explicit and observed bytes, package selections, fonts, resources.
- `compile/`, `bundle/`, `session.rs` — official compilation and export of the root Bundle, plus the per-compilation state.
- `diagnostic/` — opaque producer names and ordered evidence beside the original upstream diagnostic; meaning stays with the producer.
- `html/` — HTML URL-use rules and escaping.
- `reference_occurrence.rs` — records the references Typst's own output rules observed, for introspection queries.
- `introspection.rs` — the introspection sink a query reads.
- `spelling.rs` — name suggestions shared by diagnostics and the language service.
- `download.rs`, `extract.rs`, `svg.rs`, `prelude.rs` — feature-gated network and archive support, SVG handling, and shared prelude types.

## Invariants

- The official Typst compiler and exporter are the semantic authority; do not re-implement or approximate their meaning.
- `FileView` lifetimes are input semantics, not interchangeable performance settings: local worlds keep observations, shared worlds observe fresh files per compilation, external views stay fixed.
- Cancellation passed to `WorldBuilder::build` covers font preparation; system-font discovery and synchronous compilation remain indivisible boundaries.
- Native diagnostics transport producer identity and ordered evidence without interpreting it.
- Feature boundaries (`embed-fonts`, `network`, `scan`, `parallel`) stay valid for library consumers; keep `forbid(unsafe_code)`.
