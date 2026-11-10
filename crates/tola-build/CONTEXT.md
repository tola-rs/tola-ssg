# tola-build

`tola-build` constructs and validates a complete site independently of the `tola` application. It owns the build sequence, the candidate output graph, publication, and the evidence that makes reuse safe. It renders nothing; the caller prints.

## Modules

- `compiler/` — Typst inputs, official compilation, exported Bundle meaning, read evidence, reusable compiler resources.
- `build/` — candidate construction: hooks, producers, compile, seal; reusable producer resources and accepted caches.
- `check.rs` — read-only editor source checks: compiler resources without hooks, output writes, or publication.
- `output/` — logical output identity, conflict detection, manifests, graph construction, recoverable writes.
- `site/` and `site.rs` — realized documents and resources, route indexes, references, immutable revisions.
- `content/`, `metadata/` — content discovery, source identity, shared native Typst source dictionaries.
- `filesystem/` — source identities and sets, fingerprints, path normalization, unsaved source overrides, atomic writes.
- `config/` — parsed configuration sources and validated build sections; consumers receive resolved values.
- `hooks/` — hook child processes, their environment, bounded capture, cancellation.
- `report/` — the versioned report file an external program writes.
- `seo/`, `asset/`, `image/`, `icon/`, `package/`, `modules/`, `inspect/` — producers, package access, site modules, and read-only views that feed or describe the graph.
- `observation.rs` — input observation evidence; `cancellation.rs` — the shared token and cancelled error; `mode.rs` — build mode; `html.rs` — shared HTML escaping.

## Invariants

- Build order: before-build hooks, discovery, root Bundle convergence, output collection, reference validation, candidate validation, publication.
- One sealed, complete graph. Never publish partially converged output; never mutate the graph after sealing.
- Cache reuse preserves the same diagnostics, dependency evidence, and observable bytes as the authoritative build.
- Cancellation belongs to the superseded attempt and never renders as an ordinary failure.
- Library code returns structured values and diagnostics; terminal rendering belongs to the caller.
- Public library: keep the API documented, feature boundaries valid, and `forbid(unsafe_code)`.
