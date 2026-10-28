//! Host-aware language services for Tola sources.
//!
//! LSP over host-supplied streams, using `tola-build` for compilation. The server owns source
//! versions, query scheduling, and diagnostics; it neither starts another language server nor
//! publishes site output.
//!
//! Tola's own answers come first wherever a name has Tola's meaning: a declaration, an import, an
//! alias, or a name a wildcard import brings in answers from `tola-typst-syntax`'s source-local
//! analysis, assembled host-side. The standard library and whatever a checked world resolves answer
//! through `typst-ide` over that same world, so an ordinary name resolves exactly as the site's
//! build resolves it. Labels and pages answer from the compiled Bundle, which knows every label the
//! site declares.
//!
//! An answer about one source does not wait for the site: a site that does not compile still
//! answers for the file the author is editing, saying what that file alone establishes, and a check
//! that resolved no world answers why. Only what the whole site decides — its labels, its pages, its
//! diagnostics — stays empty until the site compiles again. A source a document is built from
//! carries a code lens per page it is served at, and a client that shows no code lens reads the
//! same pages as inlay hints. A workspace holding no site configuration is served as the documents
//! it holds.
//!
//! Standard messages and payloads use `lsp-server` and `lsp-types`. Connections own protocol state
//! and work queues, served by two bounded worker lanes: a compiler lane answering what the site's
//! check establishes, and a name lane building the source-local name graph references and rename
//! read. Both share one admission, cancellation, and shutdown sequence: a request a newer revision
//! superseded is answered with `ContentModified` and the answer its job produces later is discarded
//! rather than sent, and a document notification arriving between the `initialize` reply and
//! `initialized` is dropped, because no source is tracked before the handshake ends. A file
//! change whose paths are all inside generated state — the state directory and build lock, the
//! publication and vendor workspaces, and the configured output tree — leaves the current
//! revision, and every request in flight on it, untouched. Client
//! capabilities are negotiated once per connection, so completion, hover, signature help, symbols,
//! edits, and diagnostics reach the client in a shape it declared it reads. Framing stays bounded
//! and host I/O stays cancellable.
//!
//! [`serve`] accepts host callbacks for configuration loading and diagnostics, plus the
//! `BuildResources` and cancellation token a connection runs under: those select network access,
//! fonts, and reusable file inputs, and are what the package access boundary and the source
//! boundary an answer may read are derived from. Published package versions are offered only
//! when that policy allows network access; the source-local name graph resolves packages from
//! local roots and fetches nothing.
//!
//! Virtual package documents use `tola-package:/namespace/name/version/path`, resolved by the
//! `tola/source` request. A file-only client can set `initializationOptions.packageSourceDirectory`
//! to a prepared mirror; a definition reply names a file only after verifying it matches this
//! server's embedded source, and a missing or stale mirror keeps the `tola-package:` URI. The
//! server never writes those files.

#![forbid(unsafe_code)]
// Retaining a compiled revision asks rustc to prove `tola_build::check::SourceRevision: Send`,
// which walks deeper than the default limit reaches (rust-lang/rust#159228).
#![recursion_limit = "256"]
#![warn(missing_docs)]

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
