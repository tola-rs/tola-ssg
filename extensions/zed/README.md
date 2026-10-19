# Tola for Zed

The Typst language (grammar, highlighting, brackets) plus the `tola lsp` you already installed. This
is a development extension, not published to the Zed registry. It provides the language itself, so
do not also install the Typst extension that ships tinymist.

## Install

Zed 1.24.2 or newer, and `tola` on `PATH`.

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
```

`zed: install dev extension` → this directory. Zed fetches the grammar pinned in `extension.toml` at
install time, so the first install needs network access.

Project `.zed/settings.json`:

```json
{
  "languages": { "Typst": { "formatter": "language_server" } },
  "lsp": {
    "tola": {
      "initialization_options": { "checkMode": "onType" },
      "settings": { "tola": { "checkMode": "onType" } }
    }
  }
}
```

## Configuration

- `lsp.tola.binary.path` runs that executable instead of the one on `PATH`; Zed then bypasses this
  extension, so `binary.arguments` must spell the whole command, `["lsp"]` included.
- `lsp.tola.binary.arguments` and `.env` replace the adapter's `["lsp"]` and add environment.
- `lsp.tola.initialization_options` are connection options such as `checkMode` and
  `packageSourceDirectory`; `lsp.tola.settings.tola` is what the server re-reads while it runs.

Without `--config` the server reads `tola.toml` at the worktree root, and serves the worktree's
documents on their own when there is none.

## Status

The wasm build is verified for `wasm32-wasip2` (three hooks plus `init-extension` exported,
`zed_extension_api 0.7.0`); the GUI acceptance in `DESIGN.md` is not run yet.
