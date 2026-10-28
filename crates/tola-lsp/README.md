# tola-lsp

Tola's language server, speaking LSP over stdio. `tola lsp` runs it; the crate is also a library
with `serve`, `ServedWorkspace`, and `codes` as its public surface.

```sh
tola lsp                              # the client's workspace root
tola lsp --config site/tola.toml      # an explicit configuration
tola lsp --package-path ./packages    # extra package search roots
```

`tola editor setup` writes each editor's entry with the requests Tola serves (`only-features`);
the VS Code client is `extensions/vscode`.

## What it serves

Completion, hover, definition, references, rename, highlights, symbols, signature help, folding,
selection ranges, semantic tokens, links, colors, code actions, inlay hints, call hierarchy,
formatting, diagnostics (`onType` or `onSave`, over the real build path), and the `tola/*`
requests for routes, sources, and newlines.

Source analysis answers names, members, and imports without a successful check; the compiler lane
answers anything that needs evaluated values (`@tola/*`, labels, routes, bibliography, call
hierarchy). Both read one inference and one member projection, so a signature and its
documentation survive in either lane. The editor-only lint adds unreachable control flow,
ineffective `set`/`show`, unread bindings, and missing-font hints.

Package documents are `tola-package:/…`, mirrored on disk by `tola editor packages`; `@preview`
versions come from the build's downloader and cache. An explicit `--config` may name a site
outside the client's workspace.

Source answers belong to `tola-typst-syntax`, site semantics to `tola-build`, configuration and
pipe I/O to the application. Layout and ownership:
[`.agents/architecture.md`](../../.agents/architecture.md).
