# Tola end-to-end tests

Tests exercise real commands, filesystem edits, development builds, HTTP/WebSocket delivery, and browser
behavior, grouped by user surface.

- `tests/cli/` covers command behavior, including preview, issue logs, and the language-server session.
- `tests/dev/` covers startup, empty sites, source dependencies, diagnostics, hooks, and icons through a
  running development server. `tests/dev/reload/` covers browser revision handling, navigation, in-place
  document updates, and resource updates.
- `tests/site/` covers icons in complete generated pages.

## Setup and commands

The harness uses Deno, TypeScript, Playwright Test, and Chromium. From the repository root:

```sh
just e2e::setup
just e2e
```

`just e2e` builds the current workspace binary, type-checks the harness, and runs the suite with that binary's
absolute path. The Nix development shell supplies the pinned Deno version:

```sh
nix --extra-experimental-features 'nix-command flakes' develop
```

Focused runs from the repository root also build and type-check:

```sh
just e2e::run tests/dev/reload/navigation.spec.ts
just e2e::run --grep 'revision'
```

From this directory, `deno task typecheck`, `deno task list`, and `deno task test` operate directly.

Direct runs use `TOLA_E2E_BIN`, or `target/debug/tola` (`tola.exe` on Windows) when unset. Build it before
bypassing `just e2e`; a missing executable is an error. CI supplies its release binary.

Dependency installation is the workspace's: `deno install` writes `node_modules/` under the root
`nodeModulesDir` setting, and the VS Code integration launcher resolves `playwright` through
`e2e/node_modules` under Node.

The language-server case starts a separate `tola lsp` process and covers unsaved diagnostics, host-aware
completion, immutable virtual package definitions, configuration changes, publication preservation, and
shutdown. No external language server is needed.

## How Playwright runs under Deno

Playwright transforms TypeScript here because Deno exposes `node:module.registerHooks`; the asynchronous
`module.register` its loader would rather use is not implemented, so that path is a silent no-op. If
collection or transforms ever misbehave, `PW_DISABLE_TS_ESM=1` turns the transform off — with it off a
reported location column shifts from `3:5` to `3:1`, which is how the transform was confirmed present on Deno
2.8.3 and 2.9.7. Everything above was measured against the pinned `@playwright/test@1.62.0`; upstream has
declined Deno-specific support (`microsoft/playwright#33128`, closed unmerged).

## Layout

Support modules:

| Module       | Responsibility                                                                                                                                                                                                                                  |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `binary.ts`  | Select and check the test executable.                                                                                                                                                                                                           |
| `process.ts` | Provide isolated directory and binary fixtures; own command completion through pipe closure, environment isolation, and separate output capture; start long-running commands; run commands with the harness's standard arguments and deadlines. |
| `server.ts`  | Provide `sites.dev()` and `sites.preview()` fixtures, content edits, restart, failure attachments, and teardown. Readiness requires a complete serving line on stderr.                                                                          |
| `reload.ts`  | Serve controlled documents, load the embedded reload runtime, observe reload requests, and send revisions.                                                                                                                                      |
| `media.ts`   | Supply shared image fixtures.                                                                                                                                                                                                                   |
| `gate.ts`    | Hold HTTP responses open so a hook command blocks until the test releases it.                                                                                                                                                                   |
| `proxy.ts`   | Provide recorded refusing or held HTTP proxies and the environment routing a child through them.                                                                                                                                                |
| `hooks.ts`   | Render build-hook tables, write a site's hook configuration, and read a hook's run journal.                                                                                                                                                     |
| `log.ts`     | Read a command's JSONL log records, including the newest session log a running site records.                                                                                                                                                    |
| `site.ts`    | Write the smallest site a CLI command can inspect, and locate a site's content documents.                                                                                                                                                       |

Keep support code beside its test boundary. Add test directories only for cases that need them.

Import `test` from `support/process` for CLI directory/binary fixtures or `support/server` for the `sites`
factory. Sites stay owned during startup and close before their parent directory is removed. Only call
`site.close()` when testing shutdown itself.

## Choosing the test layer

Use the lowest layer that proves the contract:

- Rust inline tests: parsing, normalization, output ownership, diagnostics, and terminal rendering.
- `tests/cli/`: binary execution and command outcomes. Colors, cursor control, input, and signals require a
  real pseudo-terminal.
- `tests/dev/`: filesystem edits through development builds, revision installation, transport, or browser
  updates.
- `tests/site/`: interactions requiring a complete generated page.

## Real-process case requirements

Each case must:

1. Create a minimal site in an isolated directory and own every child process it starts.
2. Wait for readiness through a process message, HTTP response, or revision event.
3. Edit explicit paths and contents, then wait for the resulting revision or browser update. Do not use fixed
   sleeps; a filesystem event alone does not prove publication.
4. Assert the user-visible outcome, including preservation of the served revision after build failure or
   cancellation where relevant.
5. Terminate children and remove the site even when assertions fail.

Unix shutdown sends an interrupt and waits for the command and its pipes to close. Exceeding the deadline
forces cleanup and fails the test. Windows cleanup terminates the process tree; hook cancellation is skipped
there because the harness's process pipes cannot deliver the required console interrupt.

## Failure output

Playwright writes screenshots, videos, errors, traces, and separate stdout/stderr attachments to
`test-results/playwright/`; `playwright-report/` contains the HTML report. Both directories and
`node_modules/` are Git-ignored. CI retains failure output and reports for seven days. Tests do not retry
automatically; traces are retained from the failing attempt.
