# tola-lsp

Tola's language service for sites, speaking LSP over stdio. `tola lsp` runs it; the crate is also
usable as a library, and `serve`, `ServedWorkspace`, and `codes` are its public surface.

Site sources are answered by Tola's own compiler: the site-specific values of `@tola/*` come from
the engine that compiles them. Other Typst language servers, tinymist included, cannot compile a
site's sources; standalone Typst documents keep using a rendering server such as tinymist. This
server starts no other language server, publishes no output, and does not plan the PDF-side
capabilities a rendering server carries — preview, export, layout profiling.

## Sources

A name answers according to what establishes it:

- A name the site's own sources establish — a declaration, an import, an alias, a name a wildcard
  import brings in — comes from `tola-typst-syntax`'s source-local name graph and needs no
  successful compilation.
- The standard library and whatever a checked world resolves answer through `typst-ide` over the
  same world the site's build uses.
- Labels, pages, routes, and bibliography entries come from the compiled Bundle.

In semantic queries, incomplete member or call syntax and a half-typed name are repaired only in an
in-memory query copy; metadata and the root program are still evaluated. An expression the root
never realizes may have no value: the service answers no stale output and invents no host value.
`current-document().` completes the fields observed in the containing documents; a source included
by several documents has several observed sets; import aliases and lexical shadowing stay
meaningful.

Metadata hover answers what a value is declared with. A declared shape answers as `### Declared
fields`: each field lists as its declaration in code ticks, then the presence fact and
documentation the schema writes on continuation lines. The shape comes from public
`@tola/schema.inspect`, projected along nested `source.meta.xxx` paths. Optional fields carry `?`;
unknown presence is stated explicitly; a declared default is written `= value`. A single field — one
hovered key, or one completion — answers the declaration in a `typc` fence, then the documentation.
The service reads no private schema dictionary and runs no callback or default parse to obtain
documentation. The [schema specification](../../SCHEMA.md#成功输出描述协议) defines the output
description protocol.

Schema provenance follows unchanged simple bindings, a `for` loop's single-name variable, a
`filter` or `map` result, and any inline array closure parameter to the particular `parse-sources`
call; `filter` and `map` calls read as pure. A chain whose origin is proved answers the declared
shape its schema declares whether or not the execution realized records — resolved per chain, never
site-wide; a hover leads with `### Declared fields` when the type line would be a bare `any`, and
keeps a type the world observed. A dictionary is shown through its declared fields rather than
printed whole, and [definition](#requests) jumps from a chain to the files its records were
observed in.

Documentation comments follow the grammar `tidy` 0.4 writes: a contiguous `///` block directly above
a declaration, with its shared indentation removed and a sample's relative indentation kept.
`- name (type): description` describes one parameter (the type may be omitted), `-> description`
describes the return, and every other line continues the part it follows, with a blank line
separating paragraphs. Fenced examples remain literal, and parameter types retain nested
A function's hover opens with its signature block — `let name(\n  parameter: type = default,\n) = return;`,
the shape tinymist prints — and then the summary the block carries. Each parameter's description
appears once, in that parameter's `## name` section, beside the type its declaration carries; a type
nothing declares falls back to the type the checked world observed. The signature block, the
parameter sections (`# Positional Parameters`, `# Rest Parameters`, `# Named Parameters`), and every
type spelling come from the `Signature` the `tola-packages` crate owns, so this service and
`tola help` cannot drift apart.
A hover on a parameter reads the block above the parameter; a hover on a named argument reads that parameter's
description from the callee's declaration. Named parameter completion and signature help read the
same parameter descriptions; module member completion also carries declaration documentation.
`tola.toml` keys and `@tola/*` package exports are read
the same way. An ```` ```example ```` fence renders as ```` ```typ ````; every other fence language
is passed through verbatim. Generated code — signature blocks, type lines, and the declaration of
a single-field answer — is fenced as ```` ```typc ````, Typst's own raw tag for Typst code. An empty
`///` above a function definition completes the template — one line per parameter, then a return
line — and a client without snippet support receives it as plain text.

Each completion item replaces the whole name under the cursor while inserting over the typed prefix
alone, so a client that reads insert-and-replace edits keeps the text after the cursor; the order
its lane answered in rides on every item.

A schema field's key answers the declaration its evaluated description carries, and the
documentation it writes, through the same public output description. Wrappers, aliases, and
description bindings retain that documentation.
`describe` affects documentation only; diagnostics carry the notes explicitly supplied to `issue`.

## Requests

No compilation is needed for formatting (`typstyle`, whole document or one range, merging the
editor's tab settings with the site's formatter options), folding (honouring `line_folding_only` and
comment-fold kinds), the outline (hierarchical or flat), selection ranges (an invalid position
rejects the whole request), semantic tokens (a fixed legend, relative encoding, prefix and suffix
delta), document links (import, include, and reader paths), colours, `tola/onEnter`, postfix
completion, and where an `asset(...)` argument publishes.

The site answers hover, definition, references, rename, and highlights for labels and names, the
call hierarchy (a source is the callable; pages and the files whose `include` or `import` writes its
path are its callers), workspace symbols (including sources the editor has not opened, labels, and
routes), code lenses (one per realized page — the first three named, the rest behind one chooser
lens — read as inlay hints by clients that show no lens), `tola/route` and `tola/routes`, completion
(package versions and directories, the `@preview` index, `tola.toml` keys, labels, dictionary
fields, named parameters), code actions (near-miss replacements, unused imports, organize imports,
wrapping a selection in a content block, an equation in its other form, or an element in a figure,
and narrowing a source collection to one of the sources a field was observed on), bibliography
entries behind `@key`, and the paste-back forms of a colour.

Source narrowing accepts one inline positional closure with one ordinary identifier parameter on
a direct `.filter` call. Lexical identity proves the selected chain uses that parameter, and complete
compiler observations prove an array receiver. The inserted path guard runs before the original
predicate, so records from other sources do not read fields specific to the selected source.

A definition the site's own sources establish points at the declaration. A definition on a metadata
field answers with every source file that field was observed on — one location each, ordered by
site path, capped — so a chain a single source satisfies jumps straight there and one several
sources satisfy offers the choice instead of answering nothing.

## Diagnostics

`checkMode` is `onType` (check after a pause) or `onSave`; either way one check costs the whole site.
Checks run the real build path, run no hooks, and publish no output. Diagnostics are published or
pulled in the shape the client declared, with result ids and versions, and the diagnostics an editor
echoes back with a request drive its quick fixes.

A site that does not compile does not silence the file being edited: the check resolves the world
the site's own imports, packages, and fonts come from, a source the site could not compile compiles
on its own in that world, and the answers say what that file alone establishes. A check that
resolved no world at all answers a hover that says why. What needs the whole site — its labels, its
pages — stays empty. A workspace without `tola.toml` at its root is served as the documents it
holds, each open document checked on its own. Automatic configuration lookup stays within that
root; use `--config` to select a site outside it. CLI build commands still discover parent
configurations.

A file change whose paths are all inside generated state — `.tola`, the build lock, the publication
and vendor workspaces, the configured output tree — leaves the current revision and every request in
flight on it untouched. A request a newer revision superseded is answered with `ContentModified`, and
the answer its job produces later is discarded rather than sent.

The editor-only lint adds hints the compiler cannot see: `break`, `continue`, or
`return` outside the construct that gives it meaning, a value an explicit `return` discards, a
`set` or `show` statement whose block produces no content for it to affect, a math spelling no
scope, import, or the math library defines (carrying the cause the compiler's own unknown-variable
quick fixes read), a `let` or `for` binding no read reaches, a stored value no read observes, and a
font family this environment does not carry (including the English name a CJK name is usually
installed under; a `text` the file itself binds is not read as the builtin). A hint whose range the
compiler already reported is dropped; liveness findings carry no compiler counterpart and are not
matched that way. Liveness stays conservative: parameters are never reported, a source that does
not parse cleanly answers no findings, and a `break` or `continue` in a loop header whose owner it
cannot decide withholds the source's findings. The math hint is conservative in the same way: a
spelling a visible wildcard or dynamic implicit import may supply is not called undefined. A
file-scope binding another source may select keeps its final visible binding and last stored value
live; an earlier shadowed declaration of that name is an ordinary local. Its codes are
`editor.configuration`, `editor.branch_outside_loop`,
`editor.return_outside_function`, `editor.discarded_by_return`, `editor.ineffective_set_show`,
`editor.unknown_math_variable`, `editor.unused_binding`, `editor.dead_store`, and
`editor.unknown_font`.

## Connections

Framing is Content-Length with an 8 KiB header and a 16 MiB body bound, and the envelope is
validated as JSON-RPC 2.0. Two bounded worker lanes run under one admission, cancellation, and
shutdown sequence: the compiler lane runs checks, queries, routes, lenses, file renames, symbols,
package sources, and incoming calls; the name lane builds the site's source-local name graph and
answers references and renames from it. Definition, highlights, semantic tokens, and every text-only
request are answered in the connection itself. Retained compilations and name graphs are keyed by
revision, and disk sources are cached by digest.

Reusing a name graph also checks every source reached through imports and each resolved import
target, including hidden files and package sources. File moves preserve relative imports from both
the importing file's destination and the target's destination.

Progress begins after the client accepts its creation request. Cancellation applies only to the
check owning that progress token; a late reply cannot start progress for a replacement check.

A document's identity is its normalized site path, but every answer keeps the client's own spelling:
a definition or reference location, a symbol, a diagnostic, a call hierarchy entry, and a rename
edit all name the file exactly as the request spelled it, so an answer matches the document the
client sent it for.

Client capabilities are negotiated once per connection: snippets or plain text, the documentation
markup format, hierarchical symbols, file renames, and the related-information format of diagnostics
are projected into the shapes the client declared.

## Packages

Virtual package documents use `tola-package:/namespace/name/version/path`, and the `tola/source`
request carries their text. Checks and queries never materialize these documents or write unsaved
buffers to disk.

The site's own package view needs no client configuration: `tola editor packages` publishes the mirrors
below `.tola/builtin-packages`, a file inside it answers as the immutable `tola-package:` document it
mirrors, and a definition reply uses that file URI only after the mirror is verified against the
embedded source. A file-only client that keeps its mirrors elsewhere names them with
`initializationOptions.packageSourceDirectory`. The server never writes these files.

The `@preview` index is read once per connection through the same downloader and cache a build uses;
the `network` feature and the invocation's network policy decide whether it is available, and without
it the server answers from the packages the site already has. Version completion offers the copies
below the site's package roots and the published versions of a `@preview` package; a hover on a
package the index knows adds its description and the links its manifest publishes.

## Library

`serve` takes the host's streams, `BuildResources`, a cancellation token, and two callbacks:
configuration loading receives the connection root and the immutable unsaved sources, and the
diagnostics callback receives one check's result. `named_configuration` keeps the configuration the
author named known before a check resolves one, so the editor's own documents still answer while the
site does not compile; `host_sections` hands over the host's own `tola.toml` section declarations,
so the server hard-codes no second list. `ServedWorkspace::Site` serves a workspace as the site one
root Bundle compiles; `ServedWorkspace::Documents` serves it as the documents it holds. `codes`
exports the public diagnostic identifiers.

## Usage

```sh
tola lsp                              # the current directory, finding tola.toml upward
tola lsp --config site/tola.toml      # an explicit configuration
tola lsp --package-path ./packages    # extra package search roots
```

`tola editor setup` writes the entry each editor reads (Helix, Neovim, VS Code, Zed, Sublime, and
Emacs have their own snippets), and that entry declares which requests Tola serves (`only-features`),
so a site that also runs Typst's own language server keeps working. A package root a bare
`tola lsp` would discover is left out of the entry, so a scaffolded site's settings stay short. VS
Code uses the extension in `extensions/vscode`, where each enabled workspace folder runs its own
language service.

## Boundaries

Source answers belong to `tola-typst-syntax`, site semantics to `tola-build`, and the invocation's
configuration and native pipe I/O to the application.
