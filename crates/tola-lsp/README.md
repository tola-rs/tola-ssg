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

Hover lists the parameter types observed across calls. Formatting preserves the document's line
ending convention and edits only the changed span. Configuration errors appear in diagnostics
without a separate warning popup.

Workspace symbols, file renames, and unread-binding checks share the site's parsed source set,
including unsaved files and hidden sources reached through imports or includes. Generated output
stays outside that set. File scans follow the requesting operation's cancellation.

Set `checkMode` to `onType` (the default) or `onSave` through initialization options or the `tola`
workspace settings section. VS Code exposes this as `tola.checkMode`, separately for each workspace
folder; changing it takes effect without restarting the server.

Package documents are `tola-package:/…`, mirrored on disk by `tola editor packages`; `@preview`
versions come from the build's downloader and cache. An explicit `--config` may name a site
outside the client's workspace.

When a configuration is prepared, `tola/sourceCheckStatus` carries one `site` value: the site root,
Bundle entry, effective package-root file URIs, permitted declared asset source files and trees,
generated directories, and versions of open configuration documents at report time.
Progress-only updates omit `site`; editors retain the last prepared value until another
configuration is reported. The VS Code client serves a default package-cache file only when one
site owns it. An asset drop uses the prepared entry and configuration versions so a changed
configuration cannot publish into the old site by accident.

File-only clients use `packageSourceDirectory` in initialization options to read prepared builtin
package mirrors. Editor setup supplies this directory for Helix, Zed, Emacs, and Sublime Text.
Mirrors remain package sources, rather than editable site buffers or workspace search results.
Package files follow the compiler's package-root precedence, including when a selected package is
missing a requested file.

Source answers belong to `tola-typst-syntax`, site semantics to `tola-build`, configuration and
pipe I/O to the application. Layout and ownership: [`CONTEXT.md`](CONTEXT.md).
