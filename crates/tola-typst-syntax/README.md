# tola-typst-syntax

Pure, I/O-free Typst source analysis for editors and tooling: positions, scopes, dead bindings, import graphs, outlines, folds, tokens, colors, and formatting (optional `format` feature).

Formatting checks edits against the syntax tree, preserves strings, raw blocks, `@typstyle off` regions, and existing line endings. Re-exports `typst-syntax` and `typst-library` at Tola's compiler version.
