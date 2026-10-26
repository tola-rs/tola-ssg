# Tola

Language support and live preview for [Tola](https://github.com/tola-rs/tola-ssg) sites and Typst sources. The
extension starts the Tola language service (`tola lsp`) for the site a workspace folder selects, and registers
the Typst language, snippets, and preview commands.

## Requirements

VS Code 1.97 or newer, and a `tola` executable: leave `tola.serverPath` at `tola` to use the one on `PATH`, or
point it at an absolute path.

Build the extension and Tola executable from the same checkout when using a development version.

## Settings

| Setting                 | Meaning                                                                                                                                       |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `tola.enabled`          | Enable Tola diagnostics and language features in this workspace folder.                                                                       |
| `tola.serverPath`       | Tola executable. Relative paths use the workspace folder.                                                                                     |
| `tola.configPath`       | Site configuration file, absolute or relative to the workspace folder. An explicitly selected file never falls back to another configuration. |
| `tola.packagePath`      | Directory of local Typst packages. Leave empty for Typst's default directory.                                                                 |
| `tola.packageCachePath` | Typst package cache directory. Leave empty for Typst's default cache.                                                                         |
| `tola.checkMode`        | Check after typing pauses (`onType`, default) or when saving (`onSave`). Changes apply without restarting language services.                  |
| `tola.onEnter`          | Continue lists, comments and equations when pressing Enter.                                                                                   |
| `tola.assetDirectory`   | Directory for dropped or pasted files, relative to the site root.                                                                             |
| `tola.trace.server`     | Trace the messages the client and its language server exchange.                                                                               |

## Commands

`Tola: Restart Language Services`, `Tola: Build Site`, `Tola: Start Preview Server`, `Tola: Open Preview`,
`Tola: Open Preview in Browser`, `Tola: Open Page for Current Document`, `Tola: Copy Route`,
`Tola: Reveal Site Output`, `Tola: Show Output`, `Tola: Vendor Packages`, `Tola: Go to Definition`,
`Tola: Insert Newline`.

Build and setup commands show progress in the status bar. Build, preview, and copy commands leave their
results in the editor or Output channel without success popups. Configuration errors appear in Problems. A
stopped language service offers Restart for the affected workspace; other failures offer Show Output without
restarting healthy workspaces.

Preview prompts to save unsaved package files discovered by the language service, and can open the pages
actually published from a package source. A shared package cache is used only when one running site owns the
source; Tola does not choose a site for an ambiguous path.

The preview process reports its actual address and build outcomes through `tola dev --json`.
The status bar distinguishes a failed rebuild with the previous page still available from a
site that has not published any page. A failed after-publish task leaves the published page
available and is reported separately. Opening a page remains an explicit command.

Drop or paste a file into a site's author source to choose **Publish … as site asset**. The edit copies its
bytes into `tola.assetDirectory`. If `[assets]` already publishes that source, the configuration owns
publication; otherwise the edit adds an `#asset` declaration at the configured Bundle entry. It leaves the
current content position unchanged. Save the Bundle entry before building, or let Preview prompt to save it.
When a declaration is needed and the entry is open through multiple paths, its buffers receive the same edit;
conflicting copies must be saved or closed first.

Save the site configuration before dropping files and wait for the language service to confirm the entry.
Package sources and generated files do not offer this action. File names are adjusted to portable output
names, preserving Unicode and adding a suffix when needed. An invalid asset directory is refused with an
instruction to change `tola.assetDirectory`.

## Development

Build the workspace binary with `cargo build --locked --release -p tola`, build the bundle with
`deno task build:production`, and install the packaged VSIX. A development install uses the extension id
`tola.tola-dev` with display name `Tola Dev` and keeps the source package at `tola`.
