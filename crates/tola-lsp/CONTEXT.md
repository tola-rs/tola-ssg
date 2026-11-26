# tola-lsp

Tola language server: JSON-RPC transport, per-client connection state, and LSP request handling. Source analysis belongs to `tola-typst-syntax`; site builds to `tola-build`; CLI wiring to `src/`.

## Modules

- `query/` — hover, completion, definition, references, signatures, labels, citations, colors, postfix, and editing over checked site state.
- `connection/` — client lifecycle, request routing, document versions, scheduling, progress, code actions, and commands.
- `server/` — host scopes and worker lanes.
- `compiler/` — per-request analysis, check, compilation, and readback.
- `analysis/` — source-local name graph across disk and open buffers.
- `diagnostic/`, `lint/`, `code_actions/` — stored findings, editor-only lints, and quick fixes.
- Top-level files: `identity.rs`, `protocol.rs`, `capabilities.rs`, `markdown.rs`, `sentence.rs`, `packages.rs`, `transport.rs`, `formatting.rs`, `renames.rs`, `tokens.rs`, `symbols.rs`, `routes.rs`, `sources.rs`.

## Invariants

- Entity query sessions are keyed by module instance and entry; one source revision shares that entry compile across labels.
- Open buffers own unsaved text; only the connection commits token history and delta bases for accepted responses.
- One site per workspace root; workspaces without `tola.toml` check documents individually.
- Renames write each physical file once and edit all matching open aliases; divergent open buffers reject name queries.
- Cancelled or superseded requests never publish results or update watchers; watched-file filtering narrows only after a successful check of the current revision.
- Edits are validated against current syntax; UTF-16 edits preserve line endings.
- Formatting reads `[formatter]` (including open config buffers) without waiting for a site check.
