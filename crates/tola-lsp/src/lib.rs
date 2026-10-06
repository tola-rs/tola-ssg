//! Host-aware language services for Tola sources.
//!
//! LSP over host-supplied streams, using `tola-build` for compilation. The server
//! owns source versions, query scheduling and diagnostics. It neither starts
//! another language server nor publishes site output.
//!
//! Tola's own answers come first wherever a name has Tola's meaning. A name the site's own
//! sources establish — a declaration, an import, an alias, a name a wildcard import brings in —
//! answers from `tola-typst-syntax`'s source-local analysis, assembled host-side, which reads the
//! site's Typst files directly and needs no successful Bundle. The standard library and whatever a
//! checked world resolves still answer through `typst-ide` over that same world, so an ordinary name
//! resolves exactly as the site's build resolves it. Labels and pages answer from the compiled
//! Bundle, which knows every label the site declares; a `@key` no label has answers with the
//! entry of the bibliography call that declares it, in that call's own style.
//!
//! What one source decides on its own answers without a compilation: its formatting, folds,
//! outlines, links, colours, tokens, and selection chains, the continuation a line break applies,
//! and where an `asset(…)` argument publishes. What the site decides — the value a name holds, the
//! pages a source is served at, its diagnostics — answers from a check.
//!
//! Each tracked source version retains an immutable `tola-typst-syntax::names::SourceNames` index.
//! Source edits replace that version's index; queries borrow its source and indexed ranges together.
//!
//! A site that does not compile does not silence the file the author is editing. Every check
//! resolves the world the site's own imports, packages, and fonts come from, and a source the site
//! could not compile compiles on its own in that world: the answers then say what that file alone
//! establishes, and a check that resolved no world at all answers a hover that says why, rather
//! than something stale. Only what needs the whole site — its labels, its pages — stays empty until
//! the site compiles again. A workspace that holds no site configuration is served as the
//! documents it holds: every open document is checked on its own in that same world, and the
//! diagnostics the author reads are the file's own.
//! A source a document is built from
//! has a code lens per page it is served at, the first few named and the rest behind one lens,
//! so the author opens the preview from the file itself; a client that shows no code lens reads the
//! same pages as inlay hints.
//!
//! Standard messages and payloads use `lsp-server` and `lsp-types`. Connections own protocol state
//! and work queues, served by two bounded worker lanes. A compiler lane answers what the site's
//! check establishes — routes, lenses, workspace symbols, package sources, and the semantic queries
//! a checked world resolves — and a name lane builds the source-local name graph over every source
//! the site holds, which is what a request that must find every occurrence, references and rename,
//! is queued for. Both share one admission, cancellation, and shutdown sequence: a request a newer
//! revision superseded is answered with `ContentModified`, and the answer its job produces later is
//! discarded rather than sent; a document notification that arrives between the `initialize` reply
//! and `initialized` is dropped, because no source is tracked before the handshake ends. A file
//! change whose paths are all inside generated state — Tola's own state directory and build lock,
//! the publication and vendor workspaces, and the configured output tree — leaves the current
//! revision, and every request in flight on it, untouched. Definition, highlights, and semantic
//! tokens read the same name graph, answered in the connection without a queue.
//! Client capabilities are negotiated once per connection, so completion, hover, signature help,
//! symbols, edits, and diagnostics reach the client in a shape it declared it reads. Framing stays
//! bounded, host I/O stays cancellable, and generated source URIs cross one RFC 3986 encoding
//! boundary.
//!
//! [`serve`] accepts host callbacks for configuration loading and diagnostics.
//! Configuration loading receives the connection root and immutable unsaved
//! sources; application settings need not be serialized across crate boundaries.
//!
//! The host also supplies the `BuildResources` and cancellation token a connection runs under, so
//! the editor reads what the invocation's input scope permits: those resources select network
//! access, fonts, and reusable file inputs, and are what the package access boundary and the source
//! boundary an answer may read are derived from. Published package versions are offered only when
//! that policy allows network access, while the source-local name graph resolves packages from local
//! roots alone and fetches nothing.
//!
//! Virtual package documents use `tola-package:/namespace/name/version/path`.
//! A client resolves their immutable text with the `tola/source` request.
//! File-only clients can set `initializationOptions.packageSourceDirectory` to
//! an explicitly prepared mirror directory. Definition replies use file URIs
//! only after verifying that the existing file matches this server's embedded
//! source, so a missing or stale mirror is an error there. A query about a
//! mirrored file answers as the immutable `tola-package:` document it mirrors,
//! and a file that mirrors no builtin source answers nothing. The server never
//! writes them.

#![forbid(unsafe_code)]
// Retaining a compiled revision asks rustc to prove `tola_build::check::SourceRevision: Send`,
// which walks deeper than the default limit reaches (rust-lang/rust#159228).
#![recursion_limit = "256"]

mod analysis;
mod assets;
mod call_hierarchy;
mod capabilities;
mod code_actions;
pub mod codes;
mod compiler;
mod completion;
mod connection;
mod diagnostic;
mod files;
mod folding;
mod formatting;
mod identity;
mod lenses;
mod links;
mod lint;
mod markdown;
mod nearest;
mod on_enter;
mod packages;
mod position;
mod protocol;
mod published;
mod query;
mod renames;
mod routes;
mod selection;
mod sentence;
mod server;
mod sources;
mod symbols;
mod tokens;
mod transport;
mod uri;

pub use server::{ServedWorkspace, serve};
