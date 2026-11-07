# tola-lsp

`tola-lsp` is the Tola language server: JSON-RPC framing, one client connection's protocol state, and projection of source and site answers into editor shapes. Source answers belong to `tola-typst-syntax`; site semantics to `tola-build`; invocation and configuration to the application.

## Modules

- `query/` — prepares a request's source, resolves names, shapes answers (hover, completion, definition, signature, actions, labels, citations, colours, postfix, editing), and reads checked site facts.
- `connection/` — one client's protocol state: lifecycle, routing, replies, document versions, scheduling, progress, queues, code actions, commands.
- `server/` — host scopes and the worker lanes.
- `compiler/` — one job per request: analysis, check, compilation, readback.
- `analysis/` — the source-local name graph over disk and open buffers.
- `diagnostic/`, `lint/`, `code_actions/` — what a client reads: stored findings, editor hints, and fix construction.
- Top-level owners: `identity.rs` (file and URI identity), `protocol.rs` (wire types), `capabilities.rs` (negotiated shapes), `markdown.rs`, `sentence.rs`, `packages.rs` (the package access a query reads), `transport.rs`, `formatting.rs`, `renames.rs`, `tokens.rs`, `symbols.rs`, `routes.rs`, `sources.rs`.

## Invariants

- Open buffers own unsaved text; the connection alone commits token history and delta bases after accepting a current response.
- One site per workspace root. A workspace with no configuration is checked as the documents it holds, one by one.
- A rename writes a shared physical file once; matching open aliases each receive a buffer edit. Divergent open buffers refuse a name query.
- Cancelled or superseded requests cannot publish an answer or update watchers; a successful check of the current revision licenses precise watched-file filtering.
- Ranges and edits are validated against current syntax; UTF-16 edits preserve the document's newline convention.
- Formatting style comes from the site's `[formatter]` configuration; no client setting carries a style of its own.
