# tola-lsp

LSP server for Tola sites (`tola lsp`), also usable as a library (`serve`, `ServedWorkspace`, `codes`).

```sh
tola lsp
tola lsp --config site/tola.toml
tola lsp --package-path ./packages
```

Provides completion, hover, definition, references, rename, symbols, signature help, semantic tokens, folding, inlay hints, formatting, diagnostics (`onType` or `onSave` via `checkMode`), and `tola/*` custom requests.

Use `tola editor setup` to configure editors (VS Code client lives in `extensions/vscode`). See [`CONTEXT.md`](CONTEXT.md) for architecture and invariants.
