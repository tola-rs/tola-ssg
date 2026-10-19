# Tola

Language support and live preview for [Tola](https://github.com/tola-rs/tola-ssg) sites and Typst sources. The
extension starts the Tola language service (`tola lsp`) for the site a workspace folder selects, and registers
the Typst language, snippets, and preview commands.

## Requirements

VS Code 1.97 or newer, and a `tola` executable: leave `tola.serverPath` at `tola` to use the one on `PATH`, or
point it at an absolute path.

## Settings

| Setting                 | Meaning                                                                                                                                       |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `tola.enabled`          | Enable Tola diagnostics and language features in this workspace folder.                                                                       |
| `tola.serverPath`       | Tola executable. Relative paths use the workspace folder.                                                                                     |
| `tola.configPath`       | Site configuration file, absolute or relative to the workspace folder. An explicitly selected file never falls back to another configuration. |
| `tola.packagePath`      | Directory of local Typst packages. Leave empty for Typst's default directory.                                                                 |
| `tola.packageCachePath` | Typst package cache directory. Leave empty for Typst's default cache.                                                                         |
| `tola.onEnter`          | Continue lists, comments and equations when pressing Enter.                                                                                   |
| `tola.assetDirectory`   | Directory for dropped or pasted files, relative to the site root.                                                                             |
| `tola.trace.server`     | Trace the messages the client and its language server exchange.                                                                               |

## Commands

`Tola: Restart Language Services`, `Tola: Build Site`, `Tola: Start Preview Server`, `Tola: Open Preview`,
`Tola: Open Preview in Browser`, `Tola: Open Page for Current Document`, `Tola: Copy Route`,
`Tola: Reveal Site Output`, `Tola: Show Output`, `Tola: Vendor Packages`, `Tola: Go to Definition`,
`Tola: Insert Newline`.

## Development

Build the workspace binary with `cargo build --locked --release -p tola`, build the bundle with
`deno task build:production`, and install the packaged VSIX. A development install uses the extension id
`tola.tola-dev` with display name `Tola Dev` and keeps the source package at `tola`.
