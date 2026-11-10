# Tola context

Tola turns one root Typst Bundle into a static site. The site program selects sources; selected sources become documents, resources, and outputs; the complete output graph is validated, then published. The official Typst compiler and exporter decide Typst semantics; Tola owns the site model around them.

Read the closest `CONTEXT.md` to the code you touch, then walk up. This file is the repository overview; subsystem files live beside their code.

| Context | Owns |
|---|---|
| [`src/CONTEXT.md`](../src/CONTEXT.md) | the `tola` application: commands, development, presentation |
| [`crates/CONTEXT.md`](../crates/CONTEXT.md) | workspace crates and their ownership |
| [`crates/tola-build/CONTEXT.md`](../crates/tola-build/CONTEXT.md) | site construction and publication |
| [`crates/tola-module/CONTEXT.md`](../crates/tola-module/CONTEXT.md) | site modules: declarations, acquisition, preparation, queries, hooks |
| [`crates/tola-input/README.md`](../crates/tola-input/README.md) | input identity, lock entries, and access limits |
| [`crates/tola-typst/CONTEXT.md`](../crates/tola-typst/CONTEXT.md) | Typst world, compilation, export, diagnostics transport |
| [`crates/tola-lsp/CONTEXT.md`](../crates/tola-lsp/CONTEXT.md) | the language service |
| [`crates/tola-nu/CONTEXT.md`](../crates/tola-nu/CONTEXT.md) | the embedded Nu runtime |

[`AGENTS.md`](../AGENTS.md) holds hard rules; `.agents/DEV-SKILL.md` holds development practice.

## Build lifecycle

1. Before-build hooks may produce declared inputs.
2. Discovery freezes this attempt's sources and configured assets.
3. Source analysis and the root Bundle converge on one consistent source view.
4. Typst, configured assets, SEO, and embedded producers enter one candidate output graph.
5. Route, ownership, conflict, and reference checks validate the complete candidate.
6. A command policy publishes the validated graph or installs it as the next development revision.
7. After-publish hooks read a temporary view of committed output.

A successful build exposes one sealed graph. Publication follows validation; no stage publishes partially converged output.

## Layout

- `src/` — the `tola` binary: CLI policy, development runtime, editor integration, terminal presentation.
- `crates/` — workspace libraries, one responsibility per crate; see [`crates/CONTEXT.md`](../crates/CONTEXT.md).
- `extensions/vscode/`, `extensions/zed/` — editor clients.
- `tools/` — Nushell scripts: `lib/` shared primitives, `release/` publication, `assets/` embedded-resource generators; `ctx.nu` reads these documents by section.
- `just/` — recipe modules; `justfile` is the entry point.
- `nix/` — flake wiring, packages, development shell, checks.
- `.agents/` — the development skill, this overview, and design notes.
- `TOLA-SKILL.md` — the site-author guide served by `tola skill`.

## Vocabulary

- **source** — an input Tola reads; a source is not necessarily a page.
- **document** — a rendered thing: page, feed, generated HTML.
- **route** — a public address; trailing slash is identity.
- **resource** — a declared asset or generated non-document output.
- **output** — a logical path in the published tree; one owner, conflicts are errors.
- **candidate** — the complete output graph before publication.
- **snapshot** — frozen input observation for one attempt.
- **revision** — an immutable installed or published site version.
- **dependency evidence** — what a build read and why reuse is valid.
- **diagnostic** — a producer record written for the site author.
- **publication** — committing a validated graph into the output tree.

## Boundaries

- Domain and build modules must not depend on `cli`/`terminal` presentation.
- Paths, URLs, routes, and output identities stay distinct types or boundaries.
- Long-running build and scan work stays cancellable; superseded work never publishes.
- Cache reuse is an optimization, never a second semantic path.
