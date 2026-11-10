# The tola application

`src/` builds the `tola` binary. It parses intent, chooses command policy, runs the development server, and renders everything a person reads. Build semantics belong to `tola-build`; Typst semantics to `tola-typst`.

## Command surface

- `cli/` parses arguments and chooses command policy. It owns no build semantics: a command assembles a build request, then hands the result to presentation.
- `cli/commands/` holds one module per command family; shared argument types live in `cli/args.rs`.
- `cli/output.rs` owns success output; `cli/log/` owns the JSONL log; `src/codes.rs` binds diagnostic codes.
- `cli/dispatch.rs` routes a parsed invocation; `cli/run.rs` owns the process lifecycle and exit status.

## Development

- `dev/` serves immutable published revisions. A rebuild produces a new revision only after complete validation; the previous revision stays served until the new one is installed.
- `dev/watch/` turns filesystem events into evidence; `dev/rebuild.rs` schedules attempts; `dev/session.rs` owns the live site and revision state.
- Watch roots use fixed prefixes from input patterns. Ordinary events use compiled selectors; each exclusion applies to its own declaration. Structural changes can refresh the watch topology.
- `dev/reload/` and `src/embed/dev/hotreload.js` deliver proven-safe browser updates; ambiguity or a revision gap forces navigation.
- `dev/hooks.rs` owns before/after-publish hook execution for development; the environment contract belongs to `tola-build::hooks`.
- `dev/events.rs` emits the optional versioned JSONL stream consumed by the editor clients.

## Presentation

- `terminal/` renders diagnostics, progress, the development view, help documents, and pagers. Libraries never write to it directly.
- `help/` composes `tola help` pages from configuration and package documentation without loading a site; `i18n/` holds the `zh-Hans` translations.
- `terminal/path.rs` layers invocation-relative display over `tola_diagnostic::display_path`, the one site-path renderer.

## Configuration and writes

- `config/` is the application's configuration adapter: CLI overrides, server listener settings, watching, diagnostic display limits.
- `writes.rs` owns checked creation and replacement of site files, including the bytes a reader saw.
- `edits.rs`, `editor/`, `tree.rs`, and `command_line.rs` own editor setup, generated-file edits, listings, and editor/pager selection.

## Boundaries

- `cli/` and `terminal/` are presentation. They must not leak into domain or build modules.
- A command's exit status and printed summary are presentation; the build outcome is a library value.
- Long-running development and scan work observes cancellation; superseded attempts never publish.
