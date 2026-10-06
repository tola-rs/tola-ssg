# tola-typst-syntax

Typst source analysis for editors and tooling: where an offset sits, what a source declares, how
imports bind, outlines, folds, tokens, colours, and small edits. It never compiles anything and
does no I/O; sources and import views are supplied to it.

`names::SourceNames` indexes one parsed source, and its ranges address that exact source.
`names::SourceNames::liveness` answers which local bindings no read reaches and which stores no
read observes, over the source's own control flow. `imports::NameGraph` connects the bindings the
source views publish; references, rename, highlighting, and token classification all read that one
graph.

Modules: `syntax`, `names`, `usage`, `imports`, `outline`, `sections`, `folds`, `nesting`,
`loops`, `tokens`, `colours`, `continuation`, `wraps`, `edit`, `position`, and `docs`; `format`
(optional) adds source and range formatting.

`typst-syntax` and `typst-library` are re-exported at the versions `tola-typst` uses.
